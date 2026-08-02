//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Runs a requested database repair during tray startup, before anything opens
//! `meridian.db`.
//!
//! # Why this happens at boot rather than on the button press
//!
//! Repair needs both writers quiet. The daemon stands down on its own once the
//! marker exists (`meridian::db::repair::marker`), but the tray cannot quiesce
//! *itself* mid-session in any way worth trusting: it holds a live sqlx pool
//! that commands are using, the capture engine is streaming frames into it, and
//! the rebuilt file replaces the one every existing connection is bound to. A
//! pool whose connections point at a swapped-out inode keeps happily reading
//! the file that was moved aside - silently serving stale data forever.
//!
//! Restarting sidesteps all of it. [`crate::commands::repair`] writes the
//! marker and relaunches; on the way back up this runs at the one moment when
//! the tray provably has no pool and no capture engine, and the daemon is
//! standing down. Nothing needs to be torn down because nothing has been built
//! yet.
//!
//! `update.rs`'s forced-update path already relaunches the tray this way, so
//! the restart itself is a proven move rather than a new one.
//!
//! # Who calls this
//!
//! `lib.rs`'s `setup`, immediately after the SQLCipher key is resolved and
//! immediately before `open_existing_lazy` builds the pool. That ordering is
//! the whole point - see above.
//!
//! # Related
//!
//! - [`meridian::db::repair::marker`] - the handshake that holds the daemon off.
//! - [`meridian::db::repair::repair`] - the salvage this drives.
//! - [`crate::commands::repair`] - writes the marker and triggers the relaunch.

use std::path::Path;

/// How long to wait for a mid-flight daemon to notice the marker and exit.
///
/// The marker stops the daemon *starting*, but one already running when the
/// button was pressed keeps going until it next exits. `set_running(false)`
/// asks it to stop; this is the grace period for that to land before the
/// repair either proceeds or gives up.
const DAEMON_QUIESCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Outcome, kept plain so `lib.rs` can turn it into a notice once it has a pool
/// (this runs before one exists).
pub enum Outcome {
    /// Rebuilt. Carries a user-facing summary.
    Repaired { summary: String },
    /// Attempted and failed. The original database is untouched.
    Failed { error: String },
}

/// Repairs `meridian.db` if a repair was requested, then clears the marker.
///
/// `None` when no repair is pending, which is every normal launch - this must
/// be as close to free as possible on the hot path, and it is: one `stat`.
///
/// The marker is cleared on **both** outcomes. Leaving it after a failure would
/// hold the daemon down forever over an error the user can already see, and the
/// database is untouched on failure anyway, so there is nothing to protect.
pub fn run_if_requested(db_path: &Path, key_hex: Option<&str>) -> Option<Outcome> {
    if !meridian::db::repair::marker::pending(db_path) {
        return None;
    }

    tracing::info!("a database repair was requested - running it before opening the database");
    let outcome = tauri::async_runtime::block_on(async move {
        wait_for_daemon_to_stand_down().await;
        meridian::db::repair::repair(db_path, key_hex).await
    });

    // Clear before reporting: a panic while formatting the summary must not
    // leave the marker behind and the daemon permanently down.
    if let Err(e) = meridian::db::repair::marker::clear(db_path) {
        tracing::error!(error = %e, "could not clear the repair marker - the daemon will stay down until it expires");
    }

    Some(match outcome {
        Ok(report) => {
            tracing::info!(
                rows_copied = report.rows_copied(),
                rows_unreadable = report.rows_unreadable(),
                rows_rejected = report.rows_rejected(),
                "database repair finished"
            );
            Outcome::Repaired {
                summary: summarise(&report),
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "database repair failed - the original is unchanged");
            Outcome::Failed {
                error: format!("{e:#}"),
            }
        }
    })
}

/// Polls until no daemon answers, or the grace period expires.
///
/// Proceeding anyway on timeout is deliberate. `repair` re-checks for writers
/// itself and refuses if one is there, so the worst case is a clean refusal
/// reported to the user - whereas blocking startup indefinitely on a daemon
/// that will not die would wedge the tray, which is a far worse failure than a
/// repair that has to be retried.
async fn wait_for_daemon_to_stand_down() {
    let deadline = std::time::Instant::now() + DAEMON_QUIESCE_TIMEOUT;
    let mut logged = false;
    while std::time::Instant::now() < deadline {
        if !meridian::platform::daemon_already_running().await {
            return;
        }
        if !logged {
            tracing::info!("waiting for the daemon to stand down before repairing");
            logged = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    tracing::warn!(
        timeout_secs = DAEMON_QUIESCE_TIMEOUT.as_secs(),
        "the daemon is still answering - attempting the repair anyway; it will refuse if it is genuinely still writing"
    );
}

/// One line a user can act on. Deliberately leads with what was kept, not what
/// was lost: the operation's purpose is recovery, and "1 row lost" out of
/// context reads as damage the repair caused rather than damage it survived.
fn summarise(report: &meridian::db::repair::RepairReport) -> String {
    let mut s = format!("Recovered {} records.", report.rows_copied());
    if report.rows_unreadable() > 0 {
        s.push_str(&format!(
            " {} could not be read and were lost.",
            report.rows_unreadable()
        ));
    }
    if report.tables.iter().any(|t| t.left_empty) {
        s.push_str(" Recent screen-activity data was cleared.");
    }
    s
}

#[cfg(test)]
mod tests {
    use meridian::db::repair::{RepairReport, TableOutcome};

    fn outcome(table: &str, copied: u64, unreadable: u64, empty: bool) -> TableOutcome {
        TableOutcome {
            table: table.into(),
            rows_copied: copied,
            rows_unreadable: unreadable,
            rows_rejected: 0,
            salvaged_row_by_row: false,
            left_empty: empty,
        }
    }

    /// The clean case must not mention loss at all - a scary sentence about
    /// rows lost when none were is how users learn to distrust the message.
    #[test]
    fn summary_of_a_lossless_repair_mentions_no_loss() {
        let report = RepairReport {
            tables: vec![outcome("app_sessions", 11295, 0, false)],
            ..Default::default()
        };
        let s = super::summarise(&report);
        assert!(s.contains("11295"), "{s}");
        assert!(!s.contains("lost"), "nothing was lost, so say nothing: {s}");
        assert!(!s.contains("cleared"), "{s}");
    }

    /// ...and the lossy case must say so plainly, leading with what survived.
    #[test]
    fn summary_of_a_lossy_repair_names_both_halves() {
        let report = RepairReport {
            tables: vec![
                outcome("pm_worklog_hours", 745, 1, false),
                outcome("capture_frames", 0, 0, true),
            ],
            ..Default::default()
        };
        let s = super::summarise(&report);
        assert!(
            s.starts_with("Recovered 745 records."),
            "lead with the win: {s}"
        );
        assert!(s.contains("1 could not be read"), "{s}");
        assert!(s.contains("screen-activity data was cleared"), "{s}");
    }
}
