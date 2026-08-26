//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Single-owner PM sync: the daemon-side consumer of the `pm_sync_requests` outbox.
//!
//! # Why this task exists
//!
//! An Atlassian OAuth refresh token is single-use and rotating: the old token dies
//! the instant the new one is issued, so a lost response leaves the grant
//! recoverable only inside a 10-minute window and permanently dead after it. A
//! credential like that has exactly one safe writer.
//!
//! It used to have many. The tray refreshed in-process, the daemon refreshed on its
//! poll loop, and the tray spawned fresh `meridian pm-sync` / `tasks-sync` processes
//! that each refreshed too. The advisory file lock meant to serialise them could not:
//! its 10 s timeout is shorter than the ~26 s a refresh can take, and on timeout the
//! code proceeded WITHOUT the lock. Two processes could spend the same token, and
//! only Atlassian's grace window kept that from corrupting state.
//!
//! This watcher makes the daemon the sole consumer, so single-ownership is a property
//! of the architecture rather than of lock discipline. Every other would-be writer -
//! the tray, and the `tasks-sync` / `pm-sync` / `plan-task-*` / `ticket-update` /
//! `ticket-set-status` / `worklog-generate` CLIs - now writes a request row instead
//! (see [`crate::intelligence::sync_delegate`]). The only remaining in-process sync is
//! the fallback taken when no daemon is running at all, where there is no second
//! writer to race.
//!
//! # Why a separate task instead of the main poll loop
//!
//! The poll loop ticks on `POLL_INTERVAL_SECS` (60 s by default). Making a user who
//! pressed "Sync now" wait up to a minute for the sync to even *begin* would be a
//! visible regression against the old shell-out, which started immediately. So this
//! runs its own short cadence ([`WATCH_INTERVAL`]) - a single indexed read against a
//! local SQLite file, negligible next to the ETL work sharing the process.
//!
//! It is deliberately NOT a timer that syncs on its own: it only ever acts on a row
//! a producer wrote, so every refresh still traces back to a human action. That is
//! the property that stops a refresh POST being in flight when a laptop lid closes.
//!
//! # Who calls this
//! - [`run_watcher`] is spawned once by `src/main.rs` at daemon startup.
//! - Producers write rows via [`meridian_core::pm_sync_requests::request`].
//!
//! # Related
//! - [`meridian_core::pm_sync_requests`] - the request/claim/complete API.
//! - [`crate::intelligence::run_pm_sync`] / [`crate::intelligence::run_pm_force_sync`]
//!   - the work this dispatches to.
//! - [`crate::intelligence::Trigger`] - why attendedness is tracked at all.

use std::time::Duration;

use meridian_core::pm_sync_requests::{self, SyncMode, ALL_PROVIDERS};
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::config::Config;

/// How often to look for a pending request. Short enough that "Sync now" feels
/// immediate, and cheap enough to be irrelevant: one indexed read of a single-row
/// table on a local file.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Drain PM sync requests for the life of the process.
///
/// Releases claims stranded by a previous daemon exit once at startup (see
/// [`pm_sync_requests::reset_stale_claims`]), then loops.
///
/// Never propagates an error: a failure to read the table, or a failing sync, must
/// not take down the daemon or stop future requests being serviced. Outcomes are
/// recorded on the row so a producer can surface them.
///
/// Returns when `shutdown_rx` goes true. The shutdown check is on the SLEEP, not
/// mid-sync, so a sync already in flight is allowed to finish and record its
/// outcome rather than being cut off with the row left claimed. A hard kill during a
/// sync is still possible, and [`pm_sync_requests::reset_stale_claims`] cleans that
/// up on the next boot.
#[tracing::instrument(skip(pool, shutdown_rx))]
pub async fn run_watcher(pool: SqlitePool, mut shutdown_rx: watch::Receiver<bool>) {
    if let Err(e) = pm_sync_requests::reset_stale_claims(&pool).await {
        // Non-fatal: the table may not exist yet on a very old DB mid-migration.
        // A stranded claim self-heals as soon as any producer writes a new request
        // (which resets `claimed_at`), so this is a latency issue, not a dead end.
        tracing::warn!(
            error = %crate::errors::chain(&e),
            "could not release stale PM sync claims - a pending request may wait for the next producer"
        );
    }

    loop {
        tokio::select! {
            _ = tokio::time::sleep(WATCH_INTERVAL) => service_once(&pool).await,
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!("PM sync request watcher stopping");
                    return;
                }
            }
        }
    }
}

/// Claim and service at most one pending request. Split from the loop so the
/// decision logic is reachable without spawning a task.
async fn service_once(pool: &SqlitePool) {
    // READ first, and claim only if there is something to claim.
    //
    // `claim` is an UPDATE, and SQLite opens a write transaction to evaluate one even
    // when it matches nothing - so calling it unconditionally on this 2 s tick put
    // ~43,000 write transactions a day on an idle machine's database, and made every
    // daemon kill far likelier to land mid-write. See
    // `pm_sync_requests::has_pending` for the full reasoning. The read is advisory;
    // `claim` below is still what decides, so exclusivity is unchanged.
    match pm_sync_requests::has_pending(pool, ALL_PROVIDERS).await {
        Ok(false) => return,
        Ok(true) => {}
        Err(e) => {
            // Logged at debug, not warn: on a fresh install this fires every 2 s
            // until migration 082 has run, and a warn-level line every 2 s would
            // bury real problems (and, being WARN+, would egress to central
            // observability on every packaged install).
            tracing::debug!(
                error = %crate::errors::chain(&e),
                "could not read PM sync requests"
            );
            return;
        }
    }

    let req = match pm_sync_requests::claim(pool, ALL_PROVIDERS).await {
        Ok(Some(req)) => req,
        // Lost the race to another claimant between the read and here - fine, and
        // the reason `has_pending` is documented as advisory.
        Ok(None) => return,
        Err(e) => {
            tracing::debug!(
                error = %crate::errors::chain(&e),
                "could not claim a PM sync request"
            );
            return;
        }
    };

    // Config is read here rather than captured at spawn time so a settings change
    // (a newly connected tracker, an edited JQL) is picked up without a restart.
    // Cheap because this only runs when there is actually work.
    let cfg = Config::from_env();

    tracing::info!(
        mode = req.mode.as_str(),
        reason = %req.reason,
        seq = req.seq,
        "servicing PM sync request"
    );

    // Both arms are ATTENDED: a row exists only because a producer wrote it in
    // response to a user action, which is precisely the condition that makes
    // spending the rotating refresh token safe. There is deliberately no unattended
    // path through this watcher - the clock-driven worklog sweep calls
    // `run_pm_sync_unattended` directly instead of requesting.
    let result = match req.mode {
        SyncMode::Force => crate::intelligence::run_pm_force_sync(pool, &cfg).await,
        SyncMode::Gated => crate::intelligence::run_pm_sync(pool, &cfg).await,
    };

    let (count, error) = match result {
        Ok(()) => {
            // The count is read back from `pm_tasks` rather than threaded out of the
            // sync: `run_pm_sync` fans out over every provider and a per-provider
            // tally would have to pick one to report. The board total is what "Sync
            // now" actually wants to show.
            let n = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pm_tasks")
                .fetch_one(pool)
                .await
                .ok();
            tracing::info!(task_count = ?n, "PM sync request completed");
            (n, None)
        }
        Err(e) => {
            let detail = crate::errors::chain(&e);
            tracing::warn!(error = %detail, "PM sync request failed");
            (None, Some(detail))
        }
    };

    if let Err(e) =
        pm_sync_requests::complete(pool, &req.provider, req.seq, count, error.as_deref()).await
    {
        tracing::warn!(
            error = %crate::errors::chain(&e),
            "could not record the PM sync outcome - the producer will keep waiting"
        );
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

    /// An empty table must be a silent no-op. This runs every 2 s forever, so any
    /// noise or error here would be a permanent log flood.
    #[tokio::test]
    async fn service_once_is_quiet_with_no_requests() {
        let pool = db().await;
        service_once(&pool).await;
        assert!(pm_sync_requests::outcome(&pool, ALL_PROVIDERS, 1)
            .await
            .unwrap()
            .is_none());
    }

    /// With no PM providers configured, `run_pm_sync` returns `Ok(())` early, so the
    /// request must still be marked complete rather than left pending forever - a
    /// producer polling for the outcome would otherwise hang.
    #[tokio::test]
    async fn a_request_is_completed_even_with_no_providers_configured() {
        let pool = db().await;
        pm_sync_requests::request(&pool, ALL_PROVIDERS, SyncMode::Gated, "test")
            .await
            .unwrap();

        service_once(&pool).await;

        let out = pm_sync_requests::outcome(&pool, ALL_PROVIDERS, 1)
            .await
            .unwrap()
            .expect("the request must be completed, not left pending");
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
    }

    /// The migration must actually create the table the watcher depends on.
    #[tokio::test]
    async fn migration_creates_the_requests_table() {
        let pool = db().await;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
                            WHERE type = 'table' AND name = 'pm_sync_requests')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(exists, 1);
    }
}
