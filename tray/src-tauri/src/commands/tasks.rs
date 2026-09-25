//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Task-board action commands — the ported `/api/tasks/sync` POST.
//!
//! Re-syncs the PM board: spawns `meridian tasks-sync`, which pulls the latest
//! tickets from the connected tracker into `pm_tasks`. Tracker auth lives in the
//! daemon, so — like every tracker write — this shells out to the CLI rather than
//! talking to the provider directly; it's a process spawn, so it lives tray-side,
//! not in meridian-core. (The per-task *read*, `get_tasks`, stays in
//! [`crate::commands::dashboard`].)
//!
//! Also home to the ON-DEMAND sync triggers that replaced the daemon's standing
//! timer. PM sync used to run on every poll tick forever, which is what put a
//! rotating-OAuth-token refresh POST in flight during a macOS dark wake: the
//! machine re-suspended mid-request, the reply was lost, and the retry landed
//! outside Atlassian's 10-minute reuse leeway, killing the grant permanently.
//! A sleeping machine has no work to do, so it should attempt no refreshes -
//! these triggers fire on things a PRESENT user did instead.
//!
//! # Why a process spawn and not a table
//! The reverted `pm_sync_requests` outbox (PRs #909/#910) coordinated the tray
//! and the daemon through `meridian.db`, and the tray's read-back of the outcome
//! is what failed in production with `SQLITE_IOERR_SHORT_READ` (522) - a short
//! read while the daemon truncated the WAL on shutdown. These triggers have no
//! outcome to read: they spawn `meridian pm-sync` and forget it. There is no
//! handshake, no new table, and no migration, so shipping them cannot damage a
//! database or leave a request stuck.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by `TasksView.tsx`'s Sync
//! button via `ui/lib/bridge.ts::mutate` (success → re-fetch; error → inline msg).
//! [`trigger_background_pm_sync`] is called by [`crate::commands::system::open_dashboard`]
//! and [`crate::tray`]'s menu opener; [`trigger_background_pm_force_sync`] by
//! [`crate::commands::integrations`] on a successful tracker connect.
//!
//! # Related
//! - [`crate::install::meridian_bin`] — the shared native-first binary resolver.
//! - [`crate::commands::parents`] — the other read-side `meridian` CLI shell-out.
//! - `meridian::intelligence::sync_lock` — the cross-process lock that stops two
//!   near-simultaneous triggers both fetching the same board.

use meridian_core::proc_ext::NoWindow;
use serde::Serialize;
use std::time::Duration;

/// How long `meridian tasks-sync` gets before the command gives up and reports
/// a timeout. Named so the log field, the user-facing message and the timer can
/// never disagree — they were three independent literals, and a `30` that drifts
/// in one place turns a support report into a wrong-duration red herring.
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Success payload — mirrors the route's `{ ok, detail }` (the CLI's stdout).
#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub ok: bool,
    pub detail: String,
}

/// Re-sync the board from the tracker (the ported /api/tasks/sync POST). Spawns
/// `meridian tasks-sync` with a `SYNC_TIMEOUT` budget; returns its trimmed stdout as
/// `detail` on success, or an `Err` carrying stderr (the route's 500 body.error)
/// on timeout / spawn failure / non-zero exit.
#[tauri::command]
#[tracing::instrument]
pub async fn sync_tasks() -> Result<SyncResult, String> {
    let output = spawn_meridian_cli("tasks-sync", SYNC_TIMEOUT).await?;

    if output.status.success() {
        let detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
        tracing::info!("tasks-sync ok");
        Ok(SyncResult { ok: true, detail })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Log the CLI's own reason, not just the exit code. `status=Some(1)` alone
        // says nothing — the failure is always explained in stderr, and dropping it
        // turned a one-line diagnosis into a manual re-run of the subcommand.
        tracing::warn!(
            status = ?output.status.code(),
            stderr = %stderr,
            "tasks-sync non-zero"
        );
        Err(if stderr.is_empty() {
            "tasks-sync failed".to_string()
        } else {
            stderr
        })
    }
}

/// Budget for a background trigger's `meridian pm-sync`.
///
/// Longer than [`SYNC_TIMEOUT`] because nobody is watching a spinner - but still
/// bounded, and bounded for a specific reason: a `pm-sync` whose HTTP request
/// straddles a system suspend does NOT time out on its own (`reqwest`'s timeout
/// is `Instant`-based, and the monotonic clock does not advance while macOS is
/// asleep), so without a ceiling here an orphan can sit holding the cross-process
/// sync lock and starve every later trigger.
const TRIGGER_TIMEOUT: Duration = Duration::from_secs(90);

/// Spawn `meridian <subcommand>` and wait for it, with `timeout` and
/// `kill_on_drop`.
///
/// Factored out of [`sync_tasks`] so the button and the background triggers can
/// never drift on the two things that are easy to get wrong and invisible when
/// wrong: the cwd (which picks the credentials, because dotenvy walks up from it)
/// and `kill_on_drop` (without which a timed-out child keeps running and can
/// still mutate the board after the caller gave up).
async fn spawn_meridian_cli(
    subcommand: &str,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let bin = crate::install::meridian_bin();
    // The cwd picks the credentials, because dotenvy walks up from it: a release
    // build lands on the canonical ~/.meridian/.env (AZURE_DEVOPS_PAT, JIRA_URL, …),
    // a dev build on the checkout's own. Never inherit the tray's cwd — under a
    // packaged .app that is inside the bundle, and dotenvy finds no .env at all.
    let cwd = crate::install::cli_cwd()?;
    // WHICH binary ran, and from where, are the two facts that make a failure here
    // legible: a stale installed CLI against a DB the dev daemon migrated ahead
    // exits non-zero with nothing but `status=Some(1)` in the log otherwise.
    tracing::debug!(subcommand, bin = %bin, cwd = %cwd.display(), "meridian cli: spawning");
    let child = tokio::process::Command::new(&bin)
        .arg(subcommand)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // On timeout below, `tokio::time::timeout` drops the output future; without
        // this the orphaned child keeps running (and can still mutate the board)
        // after the caller reports a failure.
        .kill_on_drop(true)
        .no_window()
        .output();

    match tokio::time::timeout(timeout, child).await {
        Err(_) => {
            // `kill_on_drop` reaps the child here, taking its stderr with it, so
            // this log is the ONLY record a timeout ever leaves. Emitting a bare
            // "timed out" makes the two cases that matter indistinguishable in a
            // support bundle: a genuinely slow tracker sync vs. a `meridian`
            // binary that never got past opening a corrupt meridian.db.
            tracing::warn!(
                subcommand,
                bin = %bin,
                cwd = %cwd.display(),
                timeout_s = timeout.as_secs() as f64,
                "meridian cli timed out"
            );
            Err(format!(
                "{subcommand} timed out after {}s",
                timeout.as_secs()
            ))
        }
        Ok(Err(e)) => {
            tracing::warn!(subcommand, bin = %bin, error = %e, "meridian cli spawn failed");
            Err(format!("spawn error: {e}"))
        }
        Ok(Ok(o)) => Ok(o),
    }
}

/// Fire off a GATED background sync and return immediately.
///
/// `reason` names the trigger (`"dashboard_open"`, `"token_connected"`, …) and is
/// logged, so the on-demand schedule is legible in a support bundle - "why did
/// this install sync at 09:14" has an answer.
///
/// Detached on purpose: the caller is opening a window or finishing a settings
/// save, and must not wait on the network to do it. Every outcome logs at
/// `debug`/`warn` and NOTHING is surfaced to the UI - a best-effort refresh that
/// failed is not a thing to interrupt someone with, and the next trigger retries.
///
/// Gated, not forced: these fire repeatedly within seconds (open the dashboard,
/// close it, open it again), and a forced fetch each time would recreate exactly
/// the API hammering the removed timer was doing.
pub(crate) fn trigger_background_pm_sync(reason: &'static str) {
    spawn_trigger("pm-sync", reason);
}

/// Fire off a FORCED background sync and return immediately.
///
/// For the moments where the cache is knowingly wrong and the staleness gate
/// would wrongly skip: a tracker was just connected, so `pm_sync_state` may say
/// "synced 2 minutes ago" from a previous account while `pm_tasks` holds another
/// board's tickets. Same detached, best-effort, never-surfaced discipline as
/// [`trigger_background_pm_sync`].
pub(crate) fn trigger_background_pm_force_sync(reason: &'static str) {
    spawn_trigger("tasks-sync", reason);
}

/// Shared body of the two triggers: spawn onto the runtime, log the outcome,
/// surface nothing. One place so the two can never disagree about what
/// "best-effort" means.
fn spawn_trigger(subcommand: &'static str, reason: &'static str) {
    tauri::async_runtime::spawn(async move {
        match spawn_meridian_cli(subcommand, TRIGGER_TIMEOUT).await {
            Ok(o) if o.status.success() => {
                tracing::debug!(subcommand, reason, "background pm sync finished");
            }
            Ok(o) => {
                // WARN, not ERROR: a tracker being briefly unreachable is
                // ordinary, and the next trigger retries. Carrying stderr means
                // a real fault (dead grant, bad JQL) is still diagnosable.
                tracing::warn!(
                    subcommand,
                    reason,
                    status = ?o.status.code(),
                    stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                    "background pm sync exited non-zero"
                );
            }
            Err(e) => {
                tracing::warn!(subcommand, reason, detail = %e, "background pm sync failed");
            }
        }
    });
}
