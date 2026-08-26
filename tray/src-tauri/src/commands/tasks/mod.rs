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

/// Attempts [`ask_daemon_to_sync`] makes to WRITE the request row before giving up.
///
/// Sized against the thing it is riding out: a daemon reload, during which the tray's
/// pool is closed and reopened and the daemon truncates the WAL on its way out. That
/// window is on the order of a second, so 4 attempts spaced [`REQUEST_RETRY_GAP`]
/// apart covers it with margin while still failing in ~1.2 s if the fault is real.
///
/// Only retries faults that mean "not now" - a cycling pool, or a transient SQLite
/// code. A corrupt database still fails on the first attempt (after one recycle),
/// because retrying that only re-reads damaged pages.
const REQUEST_ATTEMPTS: u32 = 4;

/// Gap between [`REQUEST_ATTEMPTS`]. Shorter than the daemon watcher's 2 s tick on
/// purpose: this is waiting out a pool cycle, not waiting for a sync.
const REQUEST_RETRY_GAP: Duration = Duration::from_millis(400);

/// Shown when an outbox query fails because `meridian.db` itself is damaged. Points
/// at the banner rather than repeating the SQLite text, because
/// [`explain_outbox_failure`] has just raised that banner and it carries both the
/// full cause and a Repair button - a settings panel has neither.
const DB_DAMAGED_MESSAGE: &str =
    "Meridian's database is damaged - use the Repair Database banner on the dashboard";

/// Shown during the update window described on [`explain_outbox_failure`].
const UPDATE_IN_PROGRESS_MESSAGE: &str =
    "Meridian is still finishing an update - try again in a moment";

/// What a failed `pm_sync_requests` query means for the user, plus the side effect it
/// must trigger first.
///
/// Both halves of the handoff - the request write and the outcome read - fail for the
/// same three reasons, so they classify here instead of each growing its own ladder.
/// Callers render and log the chain themselves via [`crate::cmd_err!`] and pass the
/// result in as `rendered`, which keeps each site's log message a constant (better
/// grouping in OpenObserve than one message with the operation interpolated into it).
///
/// # Corruption must reach the banner, not a settings panel
///
/// This is the branch the whole function exists for. A staging machine's
/// `meridian.db` had real b-tree damage and these two queries were the ONLY code on
/// it to find out: `repair_boot`'s startup probe is skipped while a daemon answers,
/// the daemon latches only when its own queries reach a damaged page, and
/// `poll::refresh` covers only its four dashboard reads. So the user was shown
/// `could not queue the sync: ... database disk image is malformed` inside Settings,
/// with no banner and no Repair button - a recoverable fault presented as a failed
/// button press. [`crate::db_pool::raise_if_corrupt`] fixes that at the source; this
/// function just has to call it and then say something better than the SQL.
///
/// # Why the missing-table case is special-cased
///
/// `pm_sync_requests` arrives in migration 082, and **only the daemon runs migrations**
/// (the tray opens the file with `create_if_missing(false)` and assumes the daemon made
/// it). So during an app update there is a window - new tray already running, daemon not
/// yet restarted onto the new binary - where the table genuinely does not exist yet.
///
/// It is seconds long and self-heals the moment the daemon restarts, but a user who
/// presses "Sync now" inside it would otherwise be shown a raw SQL string:
/// `could not queue the sync: no such table: pm_sync_requests`. That reads like
/// database damage for what is in fact a normal, transient update state, and it is the
/// kind of message that produces a support ticket about a non-problem.
///
/// Matched on the message rather than a typed error because sqlx surfaces this as a
/// `Database` error whose only distinguishing feature IS its text; the match is
/// deliberately loose (table name plus "no such table") so a reworded sqlite message
/// degrades to the generic branch rather than mis-reporting something else.
/// Corruption, by contrast, is classified by `is_corrupt_error` on the real error -
/// never by string matching - so it stays correct across sqlite wordings.
async fn explain_outbox_failure(
    db: &SqlitePool,
    e: &anyhow::Error,
    rendered: &str,
    fallback: &str,
) -> String {
    crate::db_pool::raise_if_corrupt(db, e).await;

    if meridian::db::integrity::is_corrupt_error(e) {
        return DB_DAMAGED_MESSAGE.to_string();
    }
    // "no such table" (082 not applied) OR "no such column" (083 not applied). The
    // column case is not hypothetical: migration 083 added `seq`/`completed_seq`, and
    // during an update the new tray queries them before the daemon has migrated. The
    // first version of this branch only matched the table and would have shown every
    // updating user `no such column: seq` - the exact raw-SQL-in-a-settings-panel
    // failure it was written to prevent, one migration later. Any future migration
    // touching this table inherits the same window, which is why this matches the
    // schema-mismatch FAMILY rather than one message.
    // SQLite names the TABLE for a missing table (`no such table:
    // pm_sync_requests`) but only the COLUMN for a missing column (`no such column:
    // seq`) - and none of this module's `.context(...)` strings contain the literal
    // table name, so the two cases need separate needles rather than one
    // table-plus-kind check.
    //
    // Matched on the full `no such column: <name>` prefix, not on the bare column
    // name: `rendered.contains("seq")` would fire on any message containing
    // "sequence" or "consequently" and mis-report an unrelated fault as a pending
    // update, which sends the user to wait out an update that already finished.
    const SCHEMA_PENDING: [&str; 3] = [
        "no such table: pm_sync_requests",
        "no such column: seq",
        "no such column: completed_seq",
    ];
    if SCHEMA_PENDING
        .iter()
        .any(|needle| rendered.contains(needle))
    {
        return UPDATE_IN_PROGRESS_MESSAGE.to_string();
    }
    format!("{fallback}: {rendered}")
}

/// Confirm a pool is currently open, without keeping the one we looked at.
///
/// Returns the HANDLE, not a `SqlitePool`. It used to return the pool, which every
/// caller then held for the length of a 30 s poll loop - so a recycle or a daemon
/// reload part-way through left the rest of that loop querying a dead pool. Callers
/// resolve `get()` per use instead; this only answers "is there any point starting".
///
/// `None` means the pool is closed (a repair, or a recycle in progress), which is a
/// real condition rather than a bug - say so plainly instead of unwrapping.
fn require_pool(
    pool: &tauri::State<'_, crate::db_pool::DbPool>,
) -> Result<crate::db_pool::DbPool, String> {
    if pool.get().is_none() {
        return Err("the database is not open - Meridian may be repairing it".to_string());
    }
    Ok(pool.inner().clone())
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
/// Takes the `DbPool` HANDLE and resolves it per query, rather than holding one
/// `SqlitePool` for the whole 30 s wait. That matters here specifically: a daemon
/// reload or a `recover_if_corrupt` recycle part-way through the poll loop replaces
/// the pool, and a cached one would spend the rest of the budget querying a closed
/// handle and then report a timeout for a sync that had finished.
#[tracing::instrument(skip(db), fields(mode = mode.as_str()))]
async fn ask_daemon_to_sync(
    db: &crate::db_pool::DbPool,
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

    // The sequence number this request was given. Polling the outcome WITHOUT it is
    // what made "Sync now" report failure for syncs that had succeeded: the connect
    // flow writes several requests seconds apart, and the old row-level
    // `completed_at` flag could not say which one an outcome belonged to.
    //
    // Attempted twice: a first failure whose cause is a broken pool VIEW (not damaged
    // data) is recovered by `recover_if_corrupt` recycling the connections, and the
    // retry then succeeds - so the user's button works instead of them having to
    // discover that quitting and relaunching the app is the cure. Exactly two
    // attempts: the recycle either fixed it or the fault is real, and a loop here
    // would hold a user-facing command open against a database that cannot serve it.
    let mut attempt = 0;
    let seq = loop {
        attempt += 1;
        let last_attempt = attempt >= REQUEST_ATTEMPTS;

        // A CLOSED pool is a normal, momentary state, not a fault. Connecting a
        // tracker reloads the daemon, and `reload_with_pool_cycle` closes this pool
        // across the signal - so a "Sync now" pressed in that window used to return
        // "the database is not open - Meridian may be repairing it" instantly, naming
        // a repair that was not happening. Wait it out instead.
        let Some(pool) = db.get() else {
            if last_attempt {
                return Err("the database is not open - Meridian may be repairing it".to_string());
            }
            tracing::debug!(
                attempt,
                "pool is cycling - waiting to queue the sync request"
            );
            tokio::time::sleep(REQUEST_RETRY_GAP).await;
            continue;
        };

        match pm_sync_requests::request(&pool, ALL_PROVIDERS, mode, reason).await {
            Ok(seq) => break seq,
            Err(e) => {
                let rendered = crate::cmd_err!(e, "could not queue a PM sync request");

                // Transient (locked, or a short read against a WAL the daemon is
                // truncating on its way out): the write simply did not happen and
                // nothing is wrong with the data. See
                // `meridian::db::integrity::is_transient_sqlx` for the measured case.
                if !last_attempt && meridian::db::integrity::is_transient_error(&e) {
                    tracing::debug!(
                        attempt,
                        detail = %rendered,
                        "transient fault queueing the sync request - retrying"
                    );
                    tokio::time::sleep(REQUEST_RETRY_GAP).await;
                    continue;
                }

                // Raises the banner and recycles the pool when the fault is a broken
                // view; returns whether a retry is worth making.
                let recovered = db.recover_if_corrupt(&e).await;
                crate::db_pool::raise_if_corrupt(&pool, &e).await;
                if recovered && attempt == 1 {
                    tracing::info!("retrying the PM sync request on the recycled pool");
                    continue;
                }
                return Err(explain_outbox_failure(
                    &pool,
                    &e,
                    &rendered,
                    "could not queue the sync",
                )
                .await);
            }
        }
    };

    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                timeout_s = SYNC_TIMEOUT.as_secs() as i64,
                seq,
                "daemon did not report a sync outcome in time"
            );
            return Ok(None);
        }
        tokio::time::sleep(OUTCOME_POLL_INTERVAL).await;

        let Some(pool) = db.get() else {
            // A recycle or a daemon reload has the pool closed right now. Keep
            // waiting rather than failing - the request row is already written and
            // the daemon will service it.
            continue;
        };
        match pm_sync_requests::outcome(&pool, ALL_PROVIDERS, seq).await {
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
            // `cmd_err!`, never a bare `{e}`: every `pm_sync_requests` query adds its
            // own `.context(...)`, and `anyhow`'s `Display` renders ONLY the outermost
            // one. This site shipped as `could not read the sync outcome: reading the
            // PM sync outcome` on a machine whose database was corrupt - the context
            // twice over and the actual `(code: 11) database disk image is malformed`
            // nowhere, which is precisely the 1.83.2 field incident `cmd_err!` was
            // written for.
            Err(e) => {
                let rendered = crate::cmd_err!(e, "could not read the PM sync outcome");
                // A TRANSIENT fault is not an answer - it is the absence of one, and
                // the request row is already written, so the only correct response is
                // to poll again.
                //
                // This branch shipped as terminal and that was the bug. Measured on
                // 1.91.0-staging.3: connecting Jira reloads the daemon, whose shutdown
                // runs `PRAGMA wal_checkpoint(TRUNCATE)`; a "Sync now" read landing in
                // that window got `(code: 522) disk I/O error`
                // (`SQLITE_IOERR_SHORT_READ` - the WAL truncated under the reader) and
                // the user was shown a red failure. The daemon completed that very sync
                // 5 s later, with 47 poll turns still left in the budget. Same class of
                // lie as the outcome-destroying bug this loop was written to fix,
                // reached from the other side.
                if meridian::db::integrity::is_transient_error(&e) {
                    tracing::debug!(
                        detail = %rendered,
                        "transient fault reading the sync outcome - retrying"
                    );
                    continue;
                }
                // Recycle on a broken view here too, then keep waiting rather than
                // failing: the request row is written and the daemon is servicing it,
                // so a healed pool on the next poll turn still reports a real result.
                if db.recover_if_corrupt(&e).await {
                    tracing::info!("recycled the pool mid-wait - continuing to poll");
                    continue;
                }
                return Err(explain_outbox_failure(
                    &pool,
                    &e,
                    &rendered,
                    "could not read the sync outcome",
                )
                .await);
            }
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
pub(crate) fn trigger_background_pm_force_sync(db: crate::db_pool::DbPool, reason: &'static str) {
    request_sync(db, SyncMode::Force, reason);
}

/// Write a sync request, fire-and-forget.
///
/// Unlike [`sync_tasks`] this does NOT fall back to the CLI when no daemon is running.
/// These fire from window-open and connect-success paths where nothing is waiting on a
/// result, so a queued row that the next daemon start services is the right outcome -
/// spawning a process per window open is exactly the cost this replaced.
///
/// Takes the [`crate::db_pool::DbPool`] HANDLE and resolves it **inside** the spawned
/// task, not at the call site.
///
/// This parameter used to be `Option<SqlitePool>`, and the reasoning was that `None`
/// is a real state (the pool is closed while a corrupt DB is repaired) so callers
/// should pass what they already hold. The state check was right; taking a pool to do
/// it was not. Every caller is a connect-success path, and those paths **restart the
/// daemon**, which calls `DbPool::close`. A pool resolved before that point is dead by
/// the time this task runs - `integrations.rs` even had a comment explaining that it
/// grabbed the pool early "so the sync request can still be written afterwards",
/// which is precisely backwards. Resolving after the spawn means the task sees
/// whichever generation is live when it actually writes.
///
/// Best-effort by design: these fire from connect-success paths where a failure must
/// never block the thing the user asked for, and the next trigger (or their explicit
/// "Sync now") retries anyway. A corrupt database is the one exception worth
/// surfacing, so it still raises the banner.
fn request_sync(db: crate::db_pool::DbPool, mode: SyncMode, reason: &'static str) {
    tauri::async_runtime::spawn(async move {
        let Some(pool) = db.get() else {
            tracing::debug!(reason, "pm sync request skipped - database not open");
            return;
        };
        match pm_sync_requests::request(&pool, ALL_PROVIDERS, mode, reason).await {
            // Nothing waits on this one, so the sequence number is discarded.
            Ok(_seq) => tracing::debug!(reason, mode = mode.as_str(), "pm sync requested"),
            Err(e) => {
                // Full chain: `anyhow`'s `Display` would render only
                // `"writing a PM sync request"` and drop the SQLite code under it.
                tracing::debug!(
                    reason,
                    error = %meridian::errors::chain(&e),
                    "pm sync request failed"
                );
                crate::db_pool::raise_if_corrupt(&pool, &e).await;
            }
        }
    });
}

#[cfg(test)]
mod tests;
