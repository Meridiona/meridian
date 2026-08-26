//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Producer side of the `pm_sync_requests` outbox for **short-lived CLI processes**:
//! ask the daemon to sync, rather than syncing here.
//!
//! # Why this exists
//!
//! An Atlassian OAuth refresh token is single-use and rotating: the old token dies the
//! instant the new one is issued, so a lost response leaves the grant recoverable only
//! inside a 10-minute window and permanently dead after it. A credential like that has
//! exactly one safe writer, and [`crate::intelligence::sync_requests`] makes the daemon
//! that writer.
//!
//! The tray was converted to request-instead-of-sync at the same time, but six CLI
//! subcommands the tray *spawns* were missed and kept refreshing in their own process:
//! `plan-task-create` / `plan-task-edit` / `plan-task-done`, `ticket-update`,
//! `ticket-set-status`, and `worklog-generate`. Every one is user-triggered, so none of
//! them can recreate the timer-driven "refresh POST in flight when the lid closes"
//! failure - but each is a second process able to spend the token while the daemon's
//! watcher is servicing a request. The only thing serialising them was
//! `meridian-oauth`'s advisory file lock, whose 10 s timeout is shorter than the ~26 s
//! a refresh can take and which proceeds WITHOUT the lock on timeout. This closes that.
//!
//! # Why the in-process fallback stays
//!
//! A dev checkout, CI, or a support session with the daemon stopped still needs these
//! commands to refresh the board. With no daemon running there is no second writer, so
//! syncing here is safe by the same argument. [`crate::platform::daemon_already_running`]
//! is the same probe the single-instance guard uses, so the two cannot disagree about
//! who owns the data dir.
//!
//! # Who calls this
//! - `src/main.rs`'s `tasks-sync` / `pm-sync` / `ticket-update` / `ticket-set-status` /
//!   `worklog-generate` arms.
//! - [`crate::plan_tasks`]'s `create` / `edit` / `done` post-write refresh.
//!
//! # Related
//! - [`meridian_core::pm_sync_requests`] - the request/claim/complete API.
//! - [`crate::intelligence::sync_requests`] - the daemon-side consumer that does the work.
//! - [`crate::intelligence::Trigger`] - why attendedness is tracked at all.

use std::time::Duration;

use meridian_core::pm_sync_requests::{self, SyncMode, ALL_PROVIDERS};
use sqlx::SqlitePool;

use crate::config::Config;

/// How often to re-read the request row while waiting. Matched to the daemon watcher's
/// own 2 s tick rather than being tighter - a faster poll cannot see a result sooner.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// What happened to a delegated sync.
///
/// `Pending` deliberately merges "queued, not waited for" and "waited, budget elapsed":
/// both mean the daemon owns the work and will finish it, which is the only thing a
/// caller can act on. Neither is a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delegation {
    /// The sync finished successfully. `count` is the resulting `pm_tasks` total, and is
    /// present only when the daemon reported it (the in-process path does not tally).
    Synced { count: Option<i64> },
    /// The sync was attempted and failed, or the request could not be queued. The string
    /// is already a flattened error chain, ready to print or log.
    Failed { error: String },
    /// Queued for the daemon; no outcome is known yet.
    Pending,
}

/// Ask for a sync and wait up to `budget` for the outcome.
///
/// Use this when the caller's next step *reads* `pm_tasks` and would behave differently
/// against a stale board - `plan-task-create` checking whether its new ticket has
/// mirrored, or `worklog-generate` matching sessions to tickets. Pick a budget that
/// fits inside the caller's own timeout: a `Pending` return means "carry on with what
/// is cached", never "fail".
pub async fn sync_and_wait(
    pool: &SqlitePool,
    config: &Config,
    mode: SyncMode,
    label: &str,
    budget: Duration,
) -> Delegation {
    delegate(pool, config, mode, label, Some(budget)).await
}

/// The budget for a post-write refresh (`plan-task-done`, `plan-task-edit`,
/// `ticket-update`, `ticket-set-status`).
///
/// These deliberately WAIT rather than firing and forgetting, because they were
/// synchronous before delegation and the frontend re-reads the board as soon as the CLI
/// exits - returning early would show the pre-write value for a second or two and read
/// as "my change didn't save". The old inline path already paid this latency (its own
/// HTTP call, up to ~26 s with a token refresh), so waiting is not a new cost.
pub const POST_WRITE_SYNC_BUDGET: Duration = Duration::from_secs(30);

/// Ask for a post-write refresh and wait for it, using [`POST_WRITE_SYNC_BUDGET`].
///
/// A `Pending` return is not a failure: the tracker write already landed and the daemon
/// will still finish the mirror refresh. Callers log it and carry on.
pub async fn sync_after_write(pool: &SqlitePool, config: &Config, label: &str) -> Delegation {
    delegate(
        pool,
        config,
        SyncMode::Force,
        label,
        Some(POST_WRITE_SYNC_BUDGET),
    )
    .await
}

/// Shared body: pick the owner, then act.
///
/// Only the branch lives here - both halves are separately testable, which the probe
/// makes necessary: [`crate::platform::daemon_already_running`] talks to the real
/// single-instance endpoint, so a test that called this function would pass or fail
/// depending on whether the developer happens to have a daemon running.
#[tracing::instrument(skip(pool, config), fields(mode = mode.as_str()))]
async fn delegate(
    pool: &SqlitePool,
    config: &Config,
    mode: SyncMode,
    label: &str,
    wait: Option<Duration>,
) -> Delegation {
    if crate::platform::daemon_already_running().await {
        request_and_wait(pool, mode, label, wait).await
    } else {
        sync_here(pool, config, mode, label).await
    }
}

/// The no-daemon fallback: do the sync in this process.
async fn sync_here(pool: &SqlitePool, config: &Config, mode: SyncMode, label: &str) -> Delegation {
    tracing::debug!(label, "no daemon running - syncing in-process");
    let result = match mode {
        SyncMode::Force => super::run_pm_force_sync(pool, config).await,
        SyncMode::Gated => super::run_pm_sync(pool, config).await,
    };
    match result {
        Ok(()) => Delegation::Synced { count: None },
        Err(e) => Delegation::Failed {
            error: crate::errors::chain(&e),
        },
    }
}

/// The delegated path: hand the work to the daemon, optionally waiting for its outcome.
/// `wait: None` returns [`Delegation::Pending`] the moment the request is written.
async fn request_and_wait(
    pool: &SqlitePool,
    mode: SyncMode,
    label: &str,
    wait: Option<Duration>,
) -> Delegation {
    if let Err(e) = pm_sync_requests::request(pool, ALL_PROVIDERS, mode, label).await {
        return Delegation::Failed {
            error: format!(
                "could not queue the sync request: {}",
                crate::errors::chain(&e)
            ),
        };
    }
    tracing::debug!(label, "pm sync requested - the daemon owns tracker auth");

    match wait {
        Some(budget) => wait_for_outcome(pool, label, budget).await,
        None => Delegation::Pending,
    }
}

/// Poll the request row until the daemon records an outcome, or `budget` elapses.
///
/// The row is keyed on the provider (`'*'`), so a concurrent CLI's request can reset it
/// and this can end up reading a sibling's outcome. That is fine: both asked for the same
/// thing, and "some sync just completed" is exactly what the caller needs to know. It
/// cannot read a *stale* outcome, because `request` clears `completed_at`.
async fn wait_for_outcome(pool: &SqlitePool, label: &str, budget: Duration) -> Delegation {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if std::time::Instant::now() >= deadline {
            tracing::debug!(
                label,
                budget_s = budget.as_secs(),
                "daemon did not report a sync outcome in time - continuing"
            );
            return Delegation::Pending;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        match pm_sync_requests::outcome(pool, ALL_PROVIDERS).await {
            Ok(Some(out)) => {
                return match out.error {
                    Some(error) => Delegation::Failed { error },
                    None => Delegation::Synced {
                        count: out.synced_count,
                    },
                };
            }
            Ok(None) => continue,
            Err(e) => {
                return Delegation::Failed {
                    error: format!(
                        "could not read the sync outcome: {}",
                        crate::errors::chain(&e)
                    ),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    /// With no providers configured, `run_pm_*_sync` returns early - so the fallback
    /// path must report success rather than queueing anything. If it ever starts writing
    /// a request row, a dev checkout would silently stop refreshing: with no daemon
    /// there is nothing to service it.
    #[tokio::test]
    async fn the_fallback_syncs_here_and_queues_nothing() {
        let pool = db().await;
        let cfg = Config::from_env();

        let got = sync_here(&pool, &cfg, SyncMode::Force, "test").await;

        assert_eq!(got, Delegation::Synced { count: None });
        assert!(
            pm_sync_requests::outcome(&pool, ALL_PROVIDERS)
                .await
                .unwrap()
                .is_none(),
            "the in-process path must not queue a request"
        );
    }

    /// A queued request must be *claimable* by the daemon, carrying the mode and reason
    /// the producer asked for. If the row were written in a shape `claim` cannot match,
    /// every delegated sync would silently never happen.
    #[tokio::test]
    async fn a_queued_request_is_claimable_by_the_daemon() {
        let pool = db().await;

        let got = request_and_wait(&pool, SyncMode::Force, "plan-task-done", None).await;

        assert_eq!(got, Delegation::Pending);
        let claimed = pm_sync_requests::claim(&pool, ALL_PROVIDERS)
            .await
            .unwrap()
            .expect("the daemon must be able to claim the queued request");
        assert_eq!(claimed.mode, SyncMode::Force);
        assert_eq!(claimed.reason, "plan-task-done");
    }

    /// A budget that elapses with no daemon to service the row must read as `Pending`,
    /// not `Failed`. `plan-task-create` and `worklog-generate` both continue on
    /// `Pending` and would otherwise log a phantom failure on every slow sync.
    #[tokio::test]
    async fn an_elapsed_budget_is_pending_not_failed() {
        let pool = db().await;

        let got = request_and_wait(
            &pool,
            SyncMode::Gated,
            "worklog-generate",
            Some(Duration::from_millis(600)),
        )
        .await;

        assert_eq!(got, Delegation::Pending);
    }

    /// A failure the daemon recorded must reach the caller verbatim, so `tasks-sync`
    /// exits non-zero with the real reason rather than a generic timeout.
    #[tokio::test]
    async fn a_recorded_failure_is_reported_to_the_caller() {
        let pool = db().await;
        pm_sync_requests::request(&pool, ALL_PROVIDERS, SyncMode::Force, "tasks-sync")
            .await
            .unwrap();
        pm_sync_requests::claim(&pool, ALL_PROVIDERS).await.unwrap();
        pm_sync_requests::complete(&pool, ALL_PROVIDERS, None, Some("refresh_token is invalid"))
            .await
            .unwrap();

        // Re-requesting clears the outcome, so the waiter must be the one to observe it:
        // drive the wait directly against the already-completed row.
        let got = wait_for_outcome(&pool, "tasks-sync", Duration::from_secs(5)).await;

        assert_eq!(
            got,
            Delegation::Failed {
                error: "refresh_token is invalid".to_string()
            }
        );
    }

    /// `Pending` is not a failure, and callers branch on that. Pinned because the
    /// obvious refactor - folding a timeout into `Failed` - would turn "the daemon is
    /// still working" into a user-visible error on every slow sync.
    #[test]
    fn pending_is_distinct_from_failed() {
        assert_ne!(
            Delegation::Pending,
            Delegation::Failed {
                error: "x".to_string()
            }
        );
    }
}
