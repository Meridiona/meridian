//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! "Is the first-run walkthrough on screen right now" — a marker the frontend
//! raises while the tour runs, so the tray's auto-opens stay out of its way.
//!
//! # Why this exists
//!
//! The walkthrough drives the real product: it opens Settings, waits for the
//! user to connect a tracker, opens the planner. Meanwhile the poll loop has
//! its own opinions about which window should be in front — [`plan_auto_open`]
//! opens the Plan modal once a day, and [`whats_new_auto_open`] opens the
//! dashboard on the changelog once a version.
//!
//! Both were gated only on `~/.meridian/onboarded`, which is written when the
//! *wizard* completes — and the walkthrough is the half of onboarding that
//! runs immediately AFTER that. So the gate opened at the exact moment the tour
//! began. Within one 30 s tick the planner could take the screen away from the
//! beat the tour was waiting on, and because the tour waits on a DOM target
//! rather than a timer, it then sat on a stale instruction until the beat's
//! timeout expired — up to ten minutes, since those timeouts are sized for a
//! real OAuth round trip. Nothing failed and nothing logged; the tour simply
//! looked hung. That is the reported "got stuck".
//!
//! # Why it is a timestamp and not a flag
//!
//! A bare boolean that is only ever cleared by the frontend is a permanent
//! failure waiting to happen: quit mid-tour, or crash, and the marker survives
//! forever with nobody left to clear it. The daily planner would then never
//! auto-open again, on every subsequent day, for the rest of that install's
//! life — a far worse bug than the one this fixes, and a silent one.
//!
//! So the marker holds when the tour started, and goes stale on its own. The
//! frontend clearing it is the fast path, not the only path.
//!
//! # Who calls this
//! [`set_walkthrough_running`] is a command, invoked from
//! `ui/components/tutorial/useTutorial.tsx` at the two points that already
//! mirror the tour's lifetime into `engine.ts`'s module flag.
//! [`walkthrough_in_progress`] is read by [`crate::poll`]'s two auto-opens.
//!
//! # Related
//! - [`crate::commands::setup`] — writes `walkthrough_armed`, which answers a
//!   different question: whether this install is ENTITLED to the tour, not
//!   whether it is on screen.

use std::path::Path;

/// Marker holding the RFC-3339 time the walkthrough started.
const RUNNING_MARKER: &str = "walkthrough_running";

/// How long a marker is believed before it is treated as abandoned.
///
/// The tour itself is minutes, but it blocks on things that are not: an OAuth
/// round trip through a browser, picking a board, installing a provider CLI.
/// Two hours is far past any real run and far short of the next day's planner
/// auto-open, which is the thing a stuck marker would otherwise suppress.
const MAX_AGE_SECS: i64 = 2 * 60 * 60;

/// Whether the walkthrough is currently on screen.
///
/// `home` is the HOME directory, not `~/.meridian` — matching the sibling
/// marker readers in [`crate::commands::whats_new`].
///
/// Returns `false` for a missing, unparseable, or stale marker. Every one of
/// those is "we do not know", and the safe answer to not knowing is to let the
/// auto-opens run: a planner that opens over a tour is a bad minute, while a
/// planner that never opens again is a bad install.
pub(crate) fn walkthrough_in_progress(home: &Path) -> bool {
    let path = home.join(".meridian").join(RUNNING_MARKER);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(started) = chrono::DateTime::parse_from_rfc3339(raw.trim()) else {
        tracing::warn!(
            path = %path.display(),
            "walkthrough marker is unparseable - treating the tour as not running"
        );
        return false;
    };
    let age = (chrono::Local::now() - started.with_timezone(&chrono::Local)).num_seconds();
    // A negative age means the clock moved backwards between write and read.
    // Believe the marker rather than the clock: the tour is far more likely to
    // be running than the user is to have quit and time-travelled.
    if age > MAX_AGE_SECS {
        tracing::info!(age_secs = age, "walkthrough marker is stale - ignoring it");
        return false;
    }
    true
}

/// Raise or clear the walkthrough marker.
///
/// Called with `true` as the tour starts and `false` as it finishes, skips, or
/// unwinds. Clearing a marker that is not there is not an error.
#[tauri::command]
#[tracing::instrument]
pub async fn set_walkthrough_running(running: bool) -> Result<(), String> {
    let dir = meridian_core::paths::meridian_dir()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    let path = dir.join(RUNNING_MARKER);
    if running {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("create ~/.meridian: {e}"))?;
        tokio::fs::write(&path, chrono::Local::now().to_rfc3339())
            .await
            .map_err(|e| format!("write {RUNNING_MARKER}: {e}"))?;
        tracing::info!("walkthrough: started - auto-opens held back");
    } else {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => tracing::info!("walkthrough: finished - auto-opens released"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("clear {RUNNING_MARKER}: {e}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_marker(home: &Path, stamp: &str) {
        let dir = home.join(".meridian");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(RUNNING_MARKER), stamp).unwrap();
    }

    #[test]
    fn a_fresh_marker_holds_the_auto_opens_back() {
        let tmp = tempfile::tempdir().unwrap();
        write_marker(tmp.path(), &chrono::Local::now().to_rfc3339());
        assert!(walkthrough_in_progress(tmp.path()));
    }

    #[test]
    fn no_marker_means_the_tour_is_not_running() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!walkthrough_in_progress(tmp.path()));
    }

    /// THE ONE THAT MATTERS. A marker left behind by a crash or a mid-tour quit
    /// must not suppress the daily planner forever - that would be a worse and
    /// quieter bug than the race this whole module exists to fix.
    #[test]
    fn an_abandoned_marker_stops_being_believed() {
        let tmp = tempfile::tempdir().unwrap();
        let long_ago = chrono::Local::now() - chrono::Duration::seconds(MAX_AGE_SECS + 60);
        write_marker(tmp.path(), &long_ago.to_rfc3339());
        assert!(!walkthrough_in_progress(tmp.path()));
    }

    /// Still believed at the edge of the window - a tour genuinely blocked on a
    /// slow OAuth round trip should not be cut off early.
    #[test]
    fn a_marker_inside_the_window_is_still_believed() {
        let tmp = tempfile::tempdir().unwrap();
        let recent = chrono::Local::now() - chrono::Duration::seconds(MAX_AGE_SECS - 60);
        write_marker(tmp.path(), &recent.to_rfc3339());
        assert!(walkthrough_in_progress(tmp.path()));
    }

    /// Garbage reads as "not running" rather than blocking forever, for the
    /// same reason as the stale case.
    #[test]
    fn an_unparseable_marker_does_not_block_the_auto_opens() {
        let tmp = tempfile::tempdir().unwrap();
        write_marker(tmp.path(), "not a timestamp");
        assert!(!walkthrough_in_progress(tmp.path()));
    }
}
