//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The repair handshake: a marker file that makes the daemon stand down so the
//! tray can rebuild `meridian.db` without either of them writing to it.
//!
//! # Why a marker rather than stopping the daemon
//!
//! [`super::repair`] needs both writers quiet. The tray can quiesce itself, but
//! stopping the daemon is the hard half, and the obvious routes are both traps:
//!
//! - **`launchctl stop`** (what `daemon_control::set_running(false)` runs) does
//!   not keep it down. The plist sets `KeepAlive=true`, so launchd relaunches
//!   the daemon after `ThrottleInterval` - 30 s on the shipped plist. A repair
//!   takes a few seconds, so this *usually* works, which is worse than never
//!   working: it fails only under load, and the failure is a daemon writing
//!   into a file mid-rebuild.
//! - **`launchctl bootout` / a disable override** does keep it down, and is the
//!   right answer for a human at a terminal (it is what
//!   [`super::stop_daemon_hint`] suggests). Automating it is another matter:
//!   this repo has already been bitten by a disable-override wedging a plist so
//!   badly that `bootstrap` returned EIO and the agent had to be reinstalled by
//!   hand. Recovery tooling must not be able to brick the thing it is
//!   recovering.
//!
//! The tray *does* now automate a bootout, for quit and for the pause toggle
//! (`tray/src-tauri/src/daemon_lifecycle.rs`). That is not a reversal of the
//! paragraph above, and repair still does not use it, for two reasons worth
//! keeping straight:
//!
//! - What wedged the plist was `launchctl disable`, an override that persists
//!   across boots. The tray never calls it, and its bootout goes through a
//!   helper that waits for the launchd entry to clear before anything tries to
//!   bootstrap again - which is the specific sequencing that produced the EIO.
//! - More importantly, the two have different requirements. A quit or a pause
//!   needs the daemon down *until the tray comes back*, and the tray is the
//!   thing that brings it back. A repair needs it down across an unbounded
//!   number of `KeepAlive` relaunches, driven by a tray that may itself be
//!   crash-looping - which is exactly the case where "the tray will undo this"
//!   is the assumption that fails. Hence a marker the daemon honours on its own,
//!   with an expiry, rather than launchd state something has to survive to
//!   restore.
//!
//! So neither side fights launchd. The tray writes a marker; the daemon checks
//! for it before touching the database and exits cleanly if it is there. A
//! KeepAlive relaunch is then harmless - the daemon simply stands down again,
//! once every 30 s, until the marker is cleared.
//!
//! # Failing open, deliberately
//!
//! The marker's dangerous failure is not the race it prevents - it is the
//! marker outliving the process that wrote it. If the tray dies between writing
//! the marker and clearing it, a "fail closed" design leaves the daemon
//! permanently and silently dead, with nothing able to raise a notice about it
//! (the tray, which owns that surface, is the thing that died).
//!
//! That is not hypothetical here: a tray crash-loop is the incident this whole
//! feature was built in response to.
//!
//! So the marker carries a timestamp and expires ([`MAX_AGE`]). Past that, the
//! daemon logs loudly, deletes it, and boots normally. A stale marker costs one
//! noisy log line; a permanent one would cost the user their daemon.
//!
//! # Who calls this
//!
//! - [`request`] - the tray, when the user confirms a repair.
//! - [`pending`] - `src/main.rs`, before it opens the database.
//! - [`clear`] - the tray, once the repair has finished (or failed).
//!
//! # Related
//!
//! - [`super::repair`] - the operation this protects.
//! - [`super::ensure_no_writers`] - the same "nothing else is writing"
//!   invariant, enforced at the CLI boundary instead.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// How long a marker is honoured before it is treated as abandoned.
///
/// Generously wide: a repair of the 2.9 GB database this was built against took
/// under 5 s, so ten minutes only elapses if the requesting process is gone.
/// The cost of being wrong is asymmetric - too short risks cutting off a live
/// repair, too long merely delays a dead daemon's recovery by minutes.
pub const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(600);

/// File name, alongside `meridian.db` rather than at a fixed path so it follows
/// a `MERIDIAN_DB` override to wherever the database actually lives.
const MARKER_NAME: &str = "repair-requested";

/// Resolves the marker path for the database at `db_path`.
pub fn path_for(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(MARKER_NAME)
}

/// Asks the daemon to stand down. Written by the tray immediately before it
/// relaunches into the repair.
///
/// The body is an RFC3339 timestamp - the only thing [`pending`] needs, and
/// deliberately not a machine-readable format anyone might grow a schema on.
pub fn request(db_path: &Path) -> Result<()> {
    let path = path_for(db_path);
    let now = chrono::Utc::now().to_rfc3339();
    std::fs::write(&path, &now)
        .with_context(|| format!("writing the repair marker at {}", path.display()))?;
    tracing::info!("repair marker written - the daemon will stand down until it is cleared");
    Ok(())
}

/// Removes the marker. Safe to call when there is none.
pub fn clear(db_path: &Path) -> Result<()> {
    let path = path_for(db_path);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            tracing::info!("repair marker cleared - the daemon may start again");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(e).with_context(|| format!("clearing the repair marker at {}", path.display()))
        }
    }
}

/// Whether a repair is currently claimed on the database at `db_path`.
///
/// `true` means **do not open the database** - a rebuild is in progress or
/// about to be. A marker older than [`MAX_AGE`] is deleted and reported as
/// absent; see the module header on why this fails open.
pub fn pending(db_path: &Path) -> bool {
    let path = path_for(db_path);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return false; // absent, or unreadable - either way, nothing claims it
    };

    let Some(age) = age_of(&body) else {
        // Unparseable. Treat as fresh rather than ignoring it: a corrupted
        // marker written by a live tray is far likelier than a hand-crafted
        // one, and honouring it costs a delayed start, not data.
        tracing::warn!("repair marker has an unreadable timestamp - honouring it anyway");
        return true;
    };

    if age <= MAX_AGE {
        return true;
    }

    tracing::warn!(
        age_secs = age.as_secs(),
        max_age_secs = MAX_AGE.as_secs(),
        "repair marker is stale - the process that requested the repair is gone. \
         Deleting it and starting normally; if the database is still damaged the \
         integrity check will say so."
    );
    if let Err(e) = std::fs::remove_file(&path) {
        tracing::warn!(error = %e, "could not delete the stale repair marker");
    }
    false
}

/// How long ago `body`'s RFC3339 timestamp was. `None` if it does not parse.
/// A timestamp in the future yields a zero age rather than an error - a clock
/// change should not invalidate a live repair.
fn age_of(body: &str) -> Option<std::time::Duration> {
    let written = chrono::DateTime::parse_from_rfc3339(body.trim()).ok()?;
    let secs = (chrono::Utc::now() - written.with_timezone(&chrono::Utc)).num_seconds();
    Some(std::time::Duration::from_secs(secs.max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("meridian.db")
    }

    #[test]
    fn absent_marker_is_not_pending() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!pending(&db_in(&dir)));
    }

    #[test]
    fn a_fresh_marker_is_pending_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let db = db_in(&dir);
        request(&db).unwrap();
        assert!(
            pending(&db),
            "a just-written marker must hold the daemon off"
        );
        clear(&db).unwrap();
        assert!(!pending(&db), "a cleared marker must let the daemon start");
    }

    #[test]
    fn clearing_a_missing_marker_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        clear(&db_in(&dir)).expect("clear must be idempotent");
    }

    /// The failure that matters: a tray that died mid-repair must not leave the
    /// daemon permanently dead. Fails OPEN - see the module header.
    #[test]
    fn a_stale_marker_is_ignored_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let db = db_in(&dir);
        let long_ago = (chrono::Utc::now()
            - chrono::Duration::seconds(MAX_AGE.as_secs() as i64 + 60))
        .to_rfc3339();
        std::fs::write(path_for(&db), long_ago).unwrap();

        assert!(
            !pending(&db),
            "an abandoned marker must not hold the daemon off forever"
        );
        assert!(
            !path_for(&db).exists(),
            "a stale marker must be deleted so it stops costing a log line every start"
        );
    }

    /// ...but a marker that is merely old-ish is still honoured, so a slow
    /// repair is never cut off part-way.
    #[test]
    fn a_marker_just_inside_the_bound_is_still_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let db = db_in(&dir);
        let recent = (chrono::Utc::now()
            - chrono::Duration::seconds(MAX_AGE.as_secs() as i64 - 30))
        .to_rfc3339();
        std::fs::write(path_for(&db), recent).unwrap();
        assert!(pending(&db));
    }

    /// An unparseable marker is honoured rather than ignored: a garbled file
    /// from a live tray is likelier than a hand-crafted one, and the cost of
    /// honouring it is a delayed start rather than a corrupted database.
    #[test]
    fn an_unparseable_marker_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let db = db_in(&dir);
        std::fs::write(path_for(&db), "not a timestamp").unwrap();
        assert!(pending(&db));
    }

    /// A clock that jumped backwards must not read as "written in the future,
    /// therefore invalid" and cut off a live repair.
    #[test]
    fn a_future_timestamp_is_treated_as_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let db = db_in(&dir);
        let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        std::fs::write(path_for(&db), future).unwrap();
        assert!(pending(&db));
    }

    /// The marker follows the database, so a `MERIDIAN_DB` override does not
    /// leave the daemon checking one directory while the tray writes another.
    #[test]
    fn marker_sits_beside_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("nested").join("custom.db");
        assert_eq!(path_for(&db), dir.path().join("nested").join(MARKER_NAME));
    }
}
