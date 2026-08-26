//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Task-board action commands — the ported `/api/tasks/sync` POST, plus the
//! on-demand sync triggers every other tray command fires into.
//!
//! There is no background poller anymore: syncing `pm_tasks` from the connected
//! tracker happens only at genuine on-demand moments — the manual "Sync now"
//! button ([`sync_tasks`], force), connecting a tracker
//! ([`trigger_background_pm_force_sync`], force, fire-and-forget), plan mount and
//! the match-to-ticket picker ([`request_gated_sync_tasks`], gated, waits for
//! result).
//!
//! # The tray does not sync, it ASKS
//!
//! Every function here writes a row to `pm_sync_requests` and the **daemon** does the
//! work. The tray never holds a tracker credential, and it no longer spawns a
//! `meridian` process to borrow one either.
//!
//! That is not tidiness, it is the fix for a production incident. An Atlassian OAuth
//! refresh token is single-use and rotating: the old token dies the instant the new
//! one is issued, so a lost response leaves the grant recoverable only inside a
//! 10-minute window and permanently dead after it. The credential therefore has
//! exactly one safe writer — and it used to have several (daemon, tray in-process,
//! plus a fresh CLI process per trigger). The advisory file lock meant to serialise
//! them could not: its 10 s timeout is shorter than the ~26 s a refresh can take, and
//! on timeout the code proceeded WITHOUT the lock. Only Atlassian's grace window kept
//! that from corrupting state.
//!
//! Requesting instead of doing also removed a per-trigger process spawn: opening the
//! dashboard used to start a whole `meridian pm-sync` process — process init, DB
//! open, config load — which then usually discovered the staleness gate and did
//! nothing. It is now one coalescing UPSERT.
//!
//! (The per-task *read*, `get_tasks`, stays in [`crate::commands::dashboard`], and
//! deliberately never triggers a sync itself — it's polled every 30-60s while a panel
//! is mounted, and wiring a sync to that would silently rebuild the background-timer
//! problem this replaces.)
//!
//! # Who calls this
//! [`sync_tasks`] and [`request_gated_sync_tasks`] are registered in `lib.rs`'s
//! `invoke_handler!`. `sync_tasks` (force) backs every "Sync now" — the Tasks panel,
//! the planner's Refresh chip, and the per-provider button in Settings — via
//! `ui/lib/taskSync.ts::syncTasks`. `request_gated_sync_tasks` backs
//! `taskSync.ts::requestGatedTaskSync`, called by `PlanView` on mount and
//! `WorklogTicketPicker` on open.
//!
//! [`trigger_background_pm_force_sync`] is a plain Rust fn (not a Tauri command),
//! called from `commands::integrations`'s connect-success paths.
//!
//! Nothing calls a gated sync from a window opener any more: `open_dashboard` and
//! `tray::open_native_dashboard` used to, and no longer do.
//!
//! # Related
//! - [`meridian_core::pm_sync_requests`] — the request/claim/complete API.
//! - `src/intelligence/sync_requests.rs` — the daemon-side consumer.
//! - `src/intelligence/sync_delegate.rs` — the producer side for the `meridian` CLI
//!   subcommands the tray spawns (`tasks-sync`, `pm-sync`, `plan-task-*`,
//!   `ticket-update`, `ticket-set-status`, `worklog-generate`), which delegate to the
//!   same outbox when a daemon is running, for the same reason.

use meridian_core::pm_sync_requests::{self, SyncMode, ALL_PROVIDERS};
use meridian_core::SqlitePool;
use serde::Serialize;
use std::time::Duration;

/// How long [`ask_daemon_to_sync`] waits for the daemon to report an outcome, and the
/// budget for the no-daemon CLI fallback. Named so the log field, the user-facing
/// message and the timer can never disagree — they were three independent literals,
/// and a `30` that drifts in one place turns a support report into a wrong-duration
/// red herring.
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Success payload — mirrors the route's `{ ok, detail }` (the CLI's stdout).
#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub ok: bool,
    pub detail: String,
}

/// How often [`ask_daemon_to_sync`] re-reads the request row while waiting. The
/// daemon's watcher ticks every 2 s, so this is matched to that rather than being
/// tighter — a faster poll would just burn reads without seeing a result any sooner.
const OUTCOME_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Resolve the tray's DB handle, or an error string suitable for returning straight
/// to the frontend. `None` means the pool is closed (a repair or a corrupt DB), which
/// is a real condition rather than a bug — say so plainly instead of unwrapping.
fn require_pool(
    pool: &tauri::State<'_, crate::db_pool::DbPool>,
) -> Result<meridian_core::SqlitePool, String> {
    pool.get()
        .ok_or_else(|| "the database is not open - Meridian may be repairing it".to_string())
}

/// Re-sync the board from the tracker (the ported /api/tasks/sync POST) — always
/// forces a fetch, bypassing the per-provider staleness gate, because the user
/// explicitly asked for fresh data right now.
///
/// Asks the daemon rather than syncing here. The tray must never hold the tracker
/// credential: the Jira refresh token is single-use and rotating, so two processes
/// spending it can permanently kill the grant, and the daemon is the designated sole
/// owner (see [`meridian_core::pm_sync_requests`]). This writes a `force` request,
/// then polls the row for the outcome so the button can still report a real result.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn sync_tasks(
    pool: tauri::State<'_, crate::db_pool::DbPool>,
) -> Result<SyncResult, String> {
    let db = require_pool(&pool)?;
    match ask_daemon_to_sync(&db, SyncMode::Force, "sync_now", "tasks-sync").await? {
        Some(result) => Ok(result),
        // The user pressed a button and is watching a spinner, so a timeout has to
        // SAY something — but not "failed": the request is queued and the daemon will
        // service it. This is the one caller that reports the wait as an error.
        None => Err(format!(
            "the sync is still running after {}s - it will finish in the background",
            SYNC_TIMEOUT.as_secs()
        )),
    }
}

/// Frontend-facing: request a **gated** sync and wait for the outcome.
///
/// # Only two screens should call this
///
/// The board is rendered in many places, but only two of them make a *decision from
/// the whole task list*, and a stale board there is not a cosmetic lag:
///
/// - **The daily plan**, where the user picks the day's tickets. A ticket assigned an
///   hour ago is simply ABSENT from the list they can pick from, so it cannot enter
///   the plan at all. (`PlanView`, on mount.)
/// - **The retarget / match-to-existing-ticket picker**, which lists every open
///   ticket by definition. (`WorklogTicketPicker`, on open.)
///
/// Everything else showing the board is display and self-corrects on the next real
/// trigger. In particular **worklog drafting does NOT need this**: matching reads the
/// day's *plan* as its candidate pool, not the board (see
/// `src/pm_worklog/generate.rs::fetch_plan_candidates` — "Not the board"), so a
/// fresher board cannot widen the candidate set by even one ticket.
///
/// # Gated, and never on a poll
///
/// `Gated` rather than `Force` because these fire on mount/open, which recurs: the
/// daemon applies the per-provider staleness window, so a re-open inside it costs one
/// coalescing UPSERT instead of a tracker call. `sync_tasks` is the forced variant and
/// belongs to buttons the user pressed.
///
/// **Never wire this to a polled read.** `PlanView` re-loads every 30 s and
/// `TasksPanel` every 60 s; attaching a sync to those would rebuild the
/// background-timer problem this whole design removed, relocated into the read path.
/// Mount and click are one-shot; a poll is not.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn request_gated_sync_tasks(
    pool: tauri::State<'_, crate::db_pool::DbPool>,
) -> Result<SyncResult, String> {
    let db = require_pool(&pool)?;
    // A timeout here is NOT an error, unlike [`sync_tasks`]. Nothing is watching a
    // spinner: the screen already rendered from cache and only wanted fresher rows.
    // Returning `Err` would make the frontend log a failure for a sync that is simply
    // still going, which is noise in the telemetry spool rather than information.
    Ok(
        ask_daemon_to_sync(&db, SyncMode::Gated, "plan_or_picker", "pm-sync")
            .await?
            .unwrap_or(SyncResult {
                ok: false,
                detail: "still running in the background".to_string(),
            }),
    )
}

/// Ask the daemon to sync and wait up to [`SYNC_TIMEOUT`] for its outcome.
///
/// `Ok(Some(_))` the daemon reported a result; `Ok(None)` the budget elapsed with the
/// request still queued (not a failure - the daemon will service it); `Err` the sync
/// itself failed, or the outbox could not be read.
///
/// Shared by [`sync_tasks`] and [`request_gated_sync_tasks`], which differ only in
/// their mode, their tag, their no-daemon fallback subcommand, and what they make of a
/// timeout. Those four params are the entire difference; everything else - the daemon
/// probe, the fallback, the request write, the poll loop - was duplicated line for line
/// between them, which is how the two drifted into having different log messages and
/// only one of them being `#[instrument]`ed.
///
/// `fallback_cli` is the `meridian` subcommand that performs this same sync in its own
/// process. It is used only when no daemon owns the data dir, where there is no second
/// writer to race for the rotating credential - the tray still never holds a token
/// itself. Without it, a queued row would sit unserviced and the user would watch a
/// spinner time out with the daemon stopped.
#[tracing::instrument(skip(db), fields(mode = mode.as_str()))]
async fn ask_daemon_to_sync(
    db: &SqlitePool,
    mode: SyncMode,
    reason: &'static str,
    fallback_cli: &'static str,
) -> Result<Option<SyncResult>, String> {
    if !crate::commands::daemon_control::status().await.running {
        tracing::debug!(
            fallback_cli,
            "no daemon - delegating to the CLI's own fallback"
        );
        let out =
            crate::commands::cli_exec::run_meridian(&[fallback_cli], SYNC_TIMEOUT, fallback_cli)
                .await?;
        return Ok(Some(SyncResult {
            ok: true,
            detail: out.trim().to_string(),
        }));
    }

    pm_sync_requests::request(db, ALL_PROVIDERS, mode, reason)
        .await
        .map_err(|e| format!("could not queue the sync: {e}"))?;

    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                timeout_s = SYNC_TIMEOUT.as_secs() as i64,
                "daemon did not report a sync outcome in time"
            );
            return Ok(None);
        }
        tokio::time::sleep(OUTCOME_POLL_INTERVAL).await;

        match pm_sync_requests::outcome(db, ALL_PROVIDERS).await {
            Ok(Some(out)) => {
                if let Some(err) = out.error {
                    tracing::warn!(error = %err, "daemon reported a sync failure");
                    return Err(err);
                }
                let detail = match out.synced_count {
                    Some(n) => format!("synced {n} task(s)"),
                    None => "synced".to_string(),
                };
                tracing::debug!(detail = %detail, "sync ok");
                return Ok(Some(SyncResult { ok: true, detail }));
            }
            Ok(None) => continue,
            Err(e) => return Err(format!("could not read the sync outcome: {e}")),
        }
    }
}

/// Fire-and-forget: ask the daemon to **force** a fetch (bypassing the staleness
/// gate) — for a moment where there is no stale cache to protect, e.g. a tracker was
/// just connected and the board should populate immediately rather than wait for the
/// next on-demand trigger.
///
/// There is deliberately no `trigger_background_pm_sync` (gated) sibling any more. It
/// existed for the dashboard window openers, and those were removed: a window opening
/// is not evidence anyone is about to make a decision from the whole board. The two
/// screens that genuinely are — the daily plan and the retarget ticket picker — ask
/// for themselves through [`request_gated_sync_tasks`]. See that command's doc.
pub(crate) fn trigger_background_pm_force_sync(db: Option<SqlitePool>, reason: &'static str) {
    request_sync(db, SyncMode::Force, reason);
}

/// Write a sync request, fire-and-forget.
///
/// Unlike [`sync_tasks`] this does NOT fall back to the CLI when no daemon is running.
/// These fire from window-open and connect-success paths where nothing is waiting on a
/// result, so a queued row that the next daemon start services is the right outcome -
/// spawning a process per window open is exactly the cost this replaced.
///
/// Takes `Option<SqlitePool>` (i.e. `DbPool::get()`) rather than a pool or an
/// `AppHandle`, because `None` is a real state and not an error: the pool is closed
/// while a corrupt DB is being repaired. Callers pass what they already hold, which
/// is a `DbPool` in the integration paths and app state in the window paths.
///
/// Best-effort by design: these fire from window-open and connect-success paths
/// where a failure must never block the thing the user asked for, and the next
/// trigger (or their explicit "Sync now") retries anyway.
fn request_sync(db: Option<SqlitePool>, mode: SyncMode, reason: &'static str) {
    let Some(db) = db else {
        tracing::debug!(reason, "pm sync request skipped - database not open");
        return;
    };
    tauri::async_runtime::spawn(async move {
        match pm_sync_requests::request(&db, ALL_PROVIDERS, mode, reason).await {
            Ok(()) => tracing::debug!(reason, mode = mode.as_str(), "pm sync requested"),
            Err(e) => tracing::debug!(reason, error = %e, "pm sync request failed"),
        }
    });
}
