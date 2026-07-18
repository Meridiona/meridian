//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Daemon lifecycle + status commands.
//!
//! Controls the daemon service (restart / pause / resume) and reports its
//! liveness — both the cached tray view ([`get_status`], from
//! [`crate::state::AppState`]) and a fresh endpoint probe ([`get_daemon_status`],
//! the ported `/api/daemon/status`).
//!
//! The OS-specific mechanics — launchd + a Unix socket on macOS, the Task
//! Scheduler + a named pipe on Windows — live in [`super::daemon_control`]; this
//! module is the platform-agnostic command surface over them.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`. `get_daemon_status` is polled by
//! `SettingsView.tsx` during a reload via `ui/lib/bridge.ts::load`.
//!
//! # Related
//! - [`crate::sys`] — shared `uid_str` (launchctl domain) + `notify` (toast).
//! - [`crate::poll::notifications_allowed`] — quiet-hours gate for the toggle toast.
//! - [`crate::commands::pause`] — `pause_for_duration`/`pause_indefinitely`, split
//!   out of this module (CLAUDE.md's 500-line file cap).

use crate::state::{AppState, StatusPayload};
use crate::sys;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::State;

/// The cached tray status (health + active session + today totals), read from
/// the poll-loop-maintained [`AppState`]. Synchronous — just locks and snapshots.
#[tauri::command]
pub fn get_status(state: State<'_, Arc<Mutex<AppState>>>) -> Result<StatusPayload, String> {
    state
        .lock()
        .map(|s| s.to_payload())
        .map_err(|e| e.to_string())
}

/// Force-restart the daemon (macOS `launchctl kickstart -k`; Windows restarts
/// the scheduled task).
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    super::daemon_control::restart().await
}

/// Pause (stop) or resume (start) the daemon. On success, fires a toast
/// honoring the user's notification prefs (master switch + quiet hours), the
/// same policy the outbox notifications follow.
#[tauri::command]
pub async fn toggle_daemon(app: tauri::AppHandle, is_running: bool) -> Result<(), String> {
    // `is_running` is the CURRENT state, so pausing means "make it not running".
    super::daemon_control::set_running(!is_running).await?;

    let (title, body) = if is_running {
        ("Paused", "Meridian is paused. Click to resume.")
    } else {
        ("Resumed", "Meridian is back tracking.")
    };
    if crate::poll::notifications_allowed("system.pause").await {
        sys::notify(&app, title, body);
    }
    Ok(())
}

/// Response shape matching the TS route's `{ running, pid? }`.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatusResponse {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Probe the daemon's IPC endpoint with an 800 ms timeout (the ported
/// `/api/daemon/status` GET). Returns `{running: false}` on any error — no error
/// surfaces to the caller (resolve-empty contract: stale UI stays visible rather
/// than erroring on every health poll tick).
#[tauri::command]
#[tracing::instrument]
pub async fn get_daemon_status() -> Result<DaemonStatusResponse, String> {
    let probe = super::daemon_control::probe().await;
    tracing::info!(running = probe.running, pid = ?probe.pid, "daemon_status");
    Ok(DaemonStatusResponse {
        running: probe.running,
        pid: probe.pid,
    })
}

/// `{ ok, pid }` on a successful reload — mirrors the route's success body.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadResponse {
    pub ok: bool,
    pub pid: u32,
}

/// Reload the daemon's config (the ported `/api/daemon/reload` POST).
///
/// macOS sends SIGHUP: the daemon exits cleanly and launchd relaunches it,
/// picking up startup-only `settings.json` values (OTLP config, credentials).
/// Windows restarts the scheduled task to the same end — most settings are
/// re-read every poll tick regardless, so only startup-only ones need this.
/// Log-level changes hot-reload in-process and need neither.
///
/// Errors when the daemon isn't running (the route's 503).
#[tauri::command]
#[tracing::instrument]
pub async fn reload_daemon() -> Result<ReloadResponse, String> {
    let pid = super::daemon_control::reload().await?;
    tracing::info!(pid, "daemon reload requested");
    Ok(ReloadResponse { ok: true, pid })
}
