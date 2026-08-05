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
//! - [`crate::sys`] — shared `uid_str` (launchctl domain).
//! - [`meridian::notices`] — the fault-bus the toggle notice routes through (id
//!   `tray.daemon_paused`, event_key `system.pause`, same toggle as
//!   [`crate::commands::pause`]'s capture-pause notice).
//! - [`crate::commands::pause`] — `pause_for_duration`/`pause_indefinitely`, split
//!   out of this module (CLAUDE.md's 500-line file cap).

use crate::state::{AppState, StatusPayload};
use meridian_core::SqlitePool;
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

/// Pause (stop) or resume (start) the daemon. On success, raises/clears a
/// `tray.daemon_paused` notice — same `system.pause` event_key as
/// [`crate::commands::pause`]'s capture-pause notice, so quiet-hours/master-
/// switch policy applies identically.
///
/// Start/stop goes through [`super::daemon_control`] (launchd on macOS, the
/// scheduled task on Windows) rather than `launchctl` directly.
#[tauri::command]
pub async fn toggle_daemon(
    _app: tauri::AppHandle,
    is_running: bool,
    db_pool: State<'_, Option<SqlitePool>>,
) -> Result<(), String> {
    let pool = db_pool.inner().clone();

    // `is_running` is the CURRENT state, so pausing means "make it not running".
    super::daemon_control::set_running(!is_running).await?;

    if let Some(p) = pool.as_ref() {
        let result = if is_running {
            meridian::notices::raise_typed(
                p,
                meridian::notices::Notice {
                    id: "tray.daemon_paused",
                    severity: "info",
                    title: "Paused",
                    detail: "Meridian is paused. Click to resume.",
                    remedy: None,
                    event_key: "system.pause",
                    deep_link: None,
                },
            )
            .await
        } else {
            meridian::notices::clear_typed(p, "tray.daemon_paused", "system.pause").await
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, is_running, "daemon toggle notice write failed");
        }
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

/// Probe the daemon with the process-alive second opinion (the ported
/// `/api/daemon/status` GET). Returns `{running: false}` on any error — no error
/// surfaces to the caller (resolve-empty contract: stale UI stays visible rather
/// than erroring on every health poll tick).
///
/// [`daemon_control::status`], NOT the bare probe: this drives the dashboard's
/// current-hour PAUSED badge (polled every 30 s), which flapped against a
/// healthy daemon whenever the tray's own load starved the 800 ms probe — see
/// `status`'s doc.
#[tauri::command]
#[tracing::instrument]
pub async fn get_daemon_status() -> Result<DaemonStatusResponse, String> {
    let probe = super::daemon_control::status().await;
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
