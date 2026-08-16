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
//! # Why "on screen" was not enough
//!
//! Gating on the running marker alone could never close the race, and the reason
//! is structural rather than a matter of timing: [`whats_new_auto_open`] is what
//! OPENS the window the tour mounts in. At the instant it consults the gate the
//! tour has not started and cannot have — it has nowhere to run. Measured in the
//! field, the changelog opened at 15:57:19.361 and the tour raised its marker at
//! 15:57:21.467, then drew itself over the top.
//!
//! So the gate reads the ENTITLEMENT marker too ([`tour_owed`]), which is on disk
//! before any window exists. That in turn needs [`disarm_walkthrough`]: the
//! frontend's own once-ever key lives in localStorage, which no Rust code can
//! read, so without a disk-side clear the tray cannot tell an install that is
//! owed the tour from one that took it months ago.
//!
//! # Who calls this
//! [`set_walkthrough_running`] and [`disarm_walkthrough`] are commands, invoked
//! from `ui/components/tutorial/useTutorial.tsx` — the first at the two points
//! that already mirror the tour's lifetime into `engine.ts`'s module flag, the
//! second next to the localStorage write that answers the same question.
//! [`tour_owed`] is read by [`crate::poll`]'s two auto-opens.
//!
//! # Related
//! - [`crate::commands::setup`] — writes `walkthrough_armed` on a FIRST RUN
//!   only, and reads it to answer a different question: whether this install is
//!   ENTITLED to the tour, not whether it should hold a window back right now.
//!   See [`tour_owed`] for why those two reads of one file differ.

use std::path::Path;

/// Marker holding the RFC-3339 time the walkthrough started.
const RUNNING_MARKER: &str = "walkthrough_running";

/// Marker holding the RFC-3339 time the walkthrough was EARNED — written by
/// [`crate::commands::setup`] when a first run finishes the wizard, cleared by
/// [`disarm_walkthrough`] when the tour is finished or skipped.
///
/// Lives here rather than in `setup` because both of this module's questions are
/// asked of it, and `setup` only writes it.
pub(crate) const ARMED_MARKER: &str = "walkthrough_armed";

/// How long a marker is believed before it is treated as abandoned.
///
/// The tour itself is minutes, but it blocks on things that are not: an OAuth
/// round trip through a browser, picking a board, installing a provider CLI.
/// Two hours is far past any real run and far short of the next day's planner
/// auto-open, which is the thing a stuck marker would otherwise suppress.
const MAX_AGE_SECS: i64 = 2 * 60 * 60;

/// How long an *unstarted* entitlement holds the auto-opens back.
///
/// Longer than [`MAX_AGE_SECS`] because the wait it covers is the user's, not
/// the tour's: finish the wizard, close the dashboard, come back after lunch.
/// Bounded at all because of the population described on
/// `a_long_forgotten_armed_marker_stops_being_owed` — every install that
/// onboarded before this change is carrying a spent armed marker, and an
/// unbounded read of it would suppress their planner forever.
const ARMED_MAX_AGE_SECS: i64 = 24 * 60 * 60;

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
    marker_is_fresh(home, RUNNING_MARKER, MAX_AGE_SECS)
}

/// Whether an auto-open should stand down because the walkthrough either **is**
/// on screen or is **about to be**.
///
/// This is the gate [`crate::poll`]'s two auto-opens read, and the second half
/// is the load-bearing one. [`walkthrough_in_progress`] alone cannot win the
/// race it was written for, because `whats_new_auto_open` is what *opens the
/// window the tour mounts in* — so at the moment it consults the gate the tour
/// has not started and could not have. The armed marker is on disk before the
/// window exists, which is the only thing that can be checked in time.
///
/// # Why this is not `walkthrough_is_armed`
///
/// Same file, deliberately different reading. [`crate::commands::setup::walkthrough_is_armed`]
/// asks "is this install ENTITLED to the tour" and any present marker means yes,
/// with no expiry — finish the wizard, open the dashboard next week, still get
/// it. This asks "should I hold a window back right now", where believing a
/// marker forever is the dangerous answer, so it expires. Collapsing the two
/// would break whichever question got the other's rule.
pub(crate) fn tour_owed(home: &Path) -> bool {
    marker_is_fresh(home, ARMED_MARKER, ARMED_MAX_AGE_SECS) || walkthrough_in_progress(home)
}

/// Shared read for both markers: present, parseable, and younger than `max_age`.
///
/// Returns `false` for a missing, unparseable, or stale marker. Every one of
/// those is "we do not know", and the safe answer to not knowing is to let the
/// auto-opens run: a planner that opens over a tour is a bad minute, while a
/// planner that never opens again is a bad install.
fn marker_is_fresh(home: &Path, marker: &str, max_age: i64) -> bool {
    let path = home.join(".meridian").join(marker);
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
    if age > max_age {
        tracing::info!(
            marker,
            age_secs = age,
            "walkthrough marker is stale - ignoring it"
        );
        return false;
    }
    true
}

/// Clear the entitlement marker, paying off what [`tour_owed`] reports.
///
/// Split from [`set_walkthrough_running`] rather than folded into its `false`
/// arm because they mean different things and only happen to coincide today:
/// "the tour left the screen" is true of a mid-tour quit, which must KEEP the
/// entitlement, while this is called only where the frontend also writes its own
/// seen key — the two live next to each other so they cannot drift.
fn disarm(home: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(home.join(".meridian").join(ARMED_MARKER)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The tour has been finished or skipped — clear the entitlement so it is not
/// offered again, and so the auto-opens stop standing down for it.
///
/// The frontend's own once-ever marker is a localStorage key, which the tray
/// cannot read; without this command the tray has no way to tell "armed and
/// unseen" from "armed and long since taken", and [`tour_owed`] would hold both
/// auto-opens back until [`ARMED_MAX_AGE_SECS`] expired.
#[tauri::command]
#[tracing::instrument(fields(otel.status_code = tracing::field::Empty))]
pub async fn disarm_walkthrough() -> Result<(), String> {
    use anyhow::Context;

    let result = meridian_core::paths::home_dir()
        .context("resolve the home directory")
        .and_then(|home| disarm(&home).with_context(|| format!("clear the {ARMED_MARKER} marker")));

    match result {
        Ok(()) => {
            tracing::info!("walkthrough: entitlement cleared - the tour is done");
            Ok(())
        }
        Err(e) => {
            tracing::Span::current().record("otel.status_code", "ERROR");
            Err(crate::cmd_err!(
                level: error,
                e,
                "walkthrough: could not clear the entitlement"
            ))
        }
    }
}

/// Raise or clear the walkthrough marker.
///
/// Called with `true` as the tour starts and `false` as it finishes, skips, or
/// unwinds. Clearing a marker that is not there is not an error.
#[tauri::command]
#[tracing::instrument(fields(otel.status_code = tracing::field::Empty))]
pub async fn set_walkthrough_running(running: bool) -> Result<(), String> {
    write_running_marker(running).await.map_err(|e| {
        tracing::Span::current().record("otel.status_code", "ERROR");
        crate::cmd_err!(
            level: error,
            e,
            running,
            "walkthrough: could not update the running marker"
        )
    })
}

/// The filesystem half of [`set_walkthrough_running`], as `anyhow` so each step
/// carries its own context.
///
/// Split out for one reason: `Result<(), String>` has to stay at the Tauri
/// boundary (it is what crosses IPC), but a `String` built with `{e}` renders
/// only the outermost message. Keeping the work in `anyhow` and rendering once,
/// at the boundary, with `cmd_err!` is what preserves the chain — the same
/// failure this repo already hit when a corrupt-database error reached the
/// dashboard as a bare `tasks: today presence`.
///
/// Clearing a marker that is not there stays a success: the frontend calls this
/// with `false` on every unwind path, including ones where the tour never
/// started, and those are not failures.
async fn write_running_marker(running: bool) -> anyhow::Result<()> {
    use anyhow::Context;

    let dir = meridian_core::paths::meridian_dir().context("resolve the Meridian directory")?;
    let path = dir.join(RUNNING_MARKER);

    if running {
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("create {}", dir.display()))?;
        tokio::fs::write(&path, chrono::Local::now().to_rfc3339())
            .await
            .with_context(|| format!("write {}", path.display()))?;
        tracing::info!("walkthrough: started - auto-opens held back");
    } else {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => tracing::info!("walkthrough: finished - auto-opens released"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("clear {}", path.display())),
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

    fn write_armed(home: &Path, stamp: &str) {
        let dir = home.join(".meridian");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ARMED_MARKER), stamp).unwrap();
    }

    /// THE ONE THAT WOULD HAVE CAUGHT IT. `walkthrough_in_progress` alone can
    /// never win this race: `whats_new_auto_open` is the thing that OPENS the
    /// window the tour mounts in, so at the instant it consults the gate the
    /// tour has not started and cannot have. Measured gap in the field: the
    /// modal opened at 15:57:19.361 and the tour raised its running marker at
    /// 15:57:21.467, then drew itself over the top.
    ///
    /// The armed marker is what closes it — it is on disk BEFORE the window
    /// exists.
    #[test]
    fn an_armed_but_unstarted_tour_holds_the_auto_opens_back() {
        let tmp = tempfile::tempdir().unwrap();
        write_armed(tmp.path(), &chrono::Local::now().to_rfc3339());
        assert!(!walkthrough_in_progress(tmp.path()), "not started yet");
        assert!(tour_owed(tmp.path()), "but it is owed, so hold off");
    }

    #[test]
    fn a_running_tour_is_still_owed() {
        let tmp = tempfile::tempdir().unwrap();
        write_marker(tmp.path(), &chrono::Local::now().to_rfc3339());
        assert!(tour_owed(tmp.path()));
    }

    #[test]
    fn nothing_armed_and_nothing_running_owes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!tour_owed(tmp.path()));
    }

    /// Finishing (or skipping) the tour is what pays the debt, and it has to
    /// clear the marker on DISK — the frontend's own "seen" key lives in
    /// localStorage, which the tray cannot read. Without this the gate above
    /// would hold both auto-opens back forever.
    #[test]
    fn a_finished_tour_stops_being_owed() {
        let tmp = tempfile::tempdir().unwrap();
        write_armed(tmp.path(), &chrono::Local::now().to_rfc3339());
        disarm(tmp.path()).unwrap();
        assert!(!tour_owed(tmp.path()));
        // Clearing a marker that is not there is not an error — the frontend
        // fires this without waiting and may repeat it on an unwind.
        disarm(tmp.path()).unwrap();
    }

    /// The counterpart to `an_abandoned_marker_stops_being_believed`, and the
    /// reason the armed marker needs an age bound of its own.
    ///
    /// Every install that onboarded before this change has `walkthrough_armed`
    /// sitting on disk with the tour already taken — the old code never consumed
    /// it, because the once-ever guarantee was a localStorage key. Reading it as
    /// "owed" with no expiry would suppress those users' daily planner for the
    /// life of the install, silently. That is a worse bug than the race this
    /// fixes, so the marker expires and every existing install self-heals with
    /// no migration.
    #[test]
    fn a_long_forgotten_armed_marker_stops_being_owed() {
        let tmp = tempfile::tempdir().unwrap();
        let long_ago = chrono::Local::now() - chrono::Duration::seconds(ARMED_MAX_AGE_SECS + 60);
        write_armed(tmp.path(), &long_ago.to_rfc3339());
        assert!(!tour_owed(tmp.path()));
    }

    /// …but an entitlement earned minutes ago is still live. A user who finishes
    /// the wizard and lets the dashboard open on its own must not have the
    /// planner land on top of the tour it is about to start.
    #[test]
    fn a_recently_armed_tour_is_still_owed() {
        let tmp = tempfile::tempdir().unwrap();
        let recent = chrono::Local::now() - chrono::Duration::seconds(ARMED_MAX_AGE_SECS - 60);
        write_armed(tmp.path(), &recent.to_rfc3339());
        assert!(tour_owed(tmp.path()));
    }

    /// Same "we do not know, so let the auto-opens run" rule as the running
    /// marker. Note this deliberately does NOT match `walkthrough_is_armed`,
    /// which treats any present file as an entitlement — see the doc on
    /// [`tour_owed`] for why the two reads answer different questions.
    #[test]
    fn an_unparseable_armed_marker_does_not_block_the_auto_opens() {
        let tmp = tempfile::tempdir().unwrap();
        write_armed(tmp.path(), "not a timestamp");
        assert!(!tour_owed(tmp.path()));
    }
}
