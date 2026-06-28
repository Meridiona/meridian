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

use serde::Serialize;
use std::time::Duration;

/// Success payload — mirrors the route's `{ ok, detail }` (the CLI's stdout).
#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub ok: bool,
    pub detail: String,
}

/// Re-sync the board from the tracker (the ported /api/tasks/sync POST). Spawns
/// `meridian tasks-sync` with a 30 s timeout; returns its trimmed stdout as
/// `detail` on success, or an `Err` carrying stderr (the route's 500 body.error)
/// on timeout / spawn failure / non-zero exit.
#[tauri::command]
#[tracing::instrument]
pub async fn sync_tasks() -> Result<SyncResult, String> {
    let bin = crate::install::meridian_bin();
    // Run from ~/.meridian so dotenvy finds the canonical .env (AZURE_DEVOPS_PAT,
    // JIRA_API_TOKEN, etc. all live there). Without this the spawn inherits the
    // tray's cwd, dotenvy walks up and may find the repo .env instead — which
    // lacks provider credentials, silently dropping those providers from the sync.
    let home = std::env::var("HOME")
        .map_err(|_| "HOME env var not set — cannot locate ~/.meridian".to_string())?;
    let cwd = std::path::PathBuf::from(&home).join(".meridian");
    if !cwd.exists() {
        std::fs::create_dir_all(&cwd).map_err(|e| format!("could not create ~/.meridian: {e}"))?;
    }
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
        .output();

    let output = match tokio::time::timeout(Duration::from_secs(30), child).await {
        Err(_) => {
            tracing::warn!("tasks-sync timed out");
            return Err("tasks-sync timed out after 30s".to_string());
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
        tracing::warn!(status = ?output.status.code(), "tasks-sync non-zero");
        Err(if stderr.is_empty() {
            "tasks-sync failed".to_string()
        } else {
            stderr
        })
    }
}
