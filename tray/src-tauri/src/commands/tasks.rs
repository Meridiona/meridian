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
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by `TasksView.tsx`'s Sync
//! button via `ui/lib/bridge.ts::mutate` (success → re-fetch; error → inline msg).
//!
//! # Related
//! - [`crate::install::meridian_bin`] — the shared native-first binary resolver.
//! - [`crate::commands::parents`] — the other read-side `meridian` CLI shell-out.

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
/// `meridian tasks-sync` with a [`SYNC_TIMEOUT`] budget; returns its trimmed stdout as
/// `detail` on success, or an `Err` carrying stderr (the route's 500 body.error)
/// on timeout / spawn failure / non-zero exit.
#[tauri::command]
#[tracing::instrument]
pub async fn sync_tasks() -> Result<SyncResult, String> {
    let bin = crate::install::meridian_bin();
    // The cwd picks the credentials, because dotenvy walks up from it: a release
    // build lands on the canonical ~/.meridian/.env (AZURE_DEVOPS_PAT, JIRA_URL, …),
    // a dev build on the checkout's own. Never inherit the tray's cwd — under a
    // packaged .app that is inside the bundle, and dotenvy finds no .env at all.
    let cwd = crate::install::cli_cwd()?;
    // WHICH binary ran, and from where, are the two facts that make a failure here
    // legible: a stale installed CLI against a DB the dev daemon migrated ahead
    // exits non-zero with nothing but `status=Some(1)` in the log otherwise.
    tracing::debug!(bin = %bin, cwd = %cwd.display(), "tasks-sync: spawning");
    let child = tokio::process::Command::new(&bin)
        .arg("tasks-sync")
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // On timeout below, `tokio::time::timeout` drops the output future; without
        // this the orphaned `meridian tasks-sync` keeps running (and can still mutate
        // the board) after the UI reports a failure. The deleted /api/tasks/sync route
        // called child.kill() on its 30s timer — kill_on_drop preserves that contract.
        .kill_on_drop(true)
        .no_window()
        .output();

    let output = match tokio::time::timeout(SYNC_TIMEOUT, child).await {
        Err(_) => {
            // `kill_on_drop` reaps the child here, taking its stderr with it, so
            // this log is the ONLY record a timeout ever leaves. Emitting a bare
            // "tasks-sync timed out" (as it did) makes the two cases that matter
            // indistinguishable in a support bundle: a genuinely slow tracker
            // sync vs. a `meridian` binary that never got past opening a corrupt
            // meridian.db. WHICH binary and WHICH cwd is what separates them —
            // the same two facts the spawn/non-zero arms below already log.
            tracing::warn!(
                bin = %bin,
                cwd = %cwd.display(),
                timeout_s = SYNC_TIMEOUT.as_secs(),
                "tasks-sync timed out"
            );
            return Err(format!(
                "tasks-sync timed out after {}s",
                SYNC_TIMEOUT.as_secs()
            ));
        }
        Ok(Err(e)) => {
            tracing::warn!(bin = %bin, error = %e, "tasks-sync spawn failed");
            return Err(format!("spawn error: {e}"));
        }
        Ok(Ok(o)) => o,
    };

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
            bin = %bin,
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
