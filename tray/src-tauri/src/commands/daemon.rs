//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Daemon lifecycle + status commands.
//!
//! Controls the `com.meridiona.daemon` launchd service (restart / pause / resume)
//! and reports its liveness — both the cached tray view ([`get_status`], from
//! [`crate::state::AppState`]) and a fresh socket probe ([`get_daemon_status`],
//! the ported `/api/daemon/status`).
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
use std::time::Duration;
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

/// Force-restart the daemon via `launchctl kickstart -k`.
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    let uid = sys::uid_str();
    let status = std::process::Command::new("launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("gui/{}/com.meridiona.daemon", uid),
        ])
        .status()
        .map_err(|e| format!("launchctl failed: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err("launchctl kickstart returned non-zero".to_string())
    }
}

/// Pause (`stop`) or resume (`start`) the daemon. On success, fires a toast
/// honoring the user's notification prefs (master switch + quiet hours), the
/// same policy the outbox notifications follow.
#[tauri::command]
pub async fn toggle_daemon(app: tauri::AppHandle, is_running: bool) -> Result<(), String> {
    let uid = sys::uid_str();
    let service = format!("gui/{}/com.meridiona.daemon", uid);

    let status = if is_running {
        std::process::Command::new("launchctl")
            .args(["stop", &service])
            .status()
    } else {
        std::process::Command::new("launchctl")
            .args(["start", &service])
            .status()
    }
    .map_err(|e| format!("launchctl failed: {}", e))?;

    if status.success() {
        let (title, body) = if is_running {
            ("Paused", "Meridian is paused. Click to resume.")
        } else {
            ("Resumed", "Meridian is back tracking.")
        };
        if crate::poll::notifications_allowed("system.pause").await {
            sys::notify(&app, title, body);
        }
        Ok(())
    } else {
        Err(format!(
            "launchctl {} returned non-zero",
            if is_running { "stop" } else { "start" }
        ))
    }
}

/// Response shape matching the TS route's `{ running, pid? }`.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatusResponse {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Probe `~/.meridian/daemon.sock` with an 800 ms timeout (the ported
/// `/api/daemon/status` GET). Returns `{running: false}` on any error — no error
/// surfaces to the caller (resolve-empty contract: stale UI stays visible rather
/// than erroring on every health poll tick).
#[tauri::command]
#[tracing::instrument]
pub async fn get_daemon_status() -> Result<DaemonStatusResponse, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let sock_path = format!("{}/.meridian/daemon.sock", home);

    let result = probe_socket(&sock_path).await;
    tracing::info!(running = result.running, pid = ?result.pid, "daemon_status");
    Ok(result)
}

/// `{ ok, pid }` on a successful reload — mirrors the route's success body.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadResponse {
    pub ok: bool,
    pub pid: u32,
}

/// Reload the daemon's config by sending it SIGHUP (the ported
/// `/api/daemon/reload` POST). The daemon exits cleanly on SIGHUP and launchd
/// restarts it, picking up `settings.json` changes (OTLP config, credentials).
/// Log-level changes hot-reload in-process and don't need this. Errors when the
/// daemon isn't running (the route's 503) — we resolve its pid from the same
/// `daemon.sock` greeting [`get_daemon_status`] reads.
#[tauri::command]
#[tracing::instrument]
pub async fn reload_daemon() -> Result<ReloadResponse, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let sock_path = format!("{}/.meridian/daemon.sock", home);

    let probe = probe_socket(&sock_path).await;
    let Some(pid) = probe.pid.filter(|_| probe.running) else {
        return Err("daemon not running".to_string());
    };

    // `kill -HUP <pid>` (the route's `process.kill(pid, 'SIGHUP')`) — no libc dep.
    let status = std::process::Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .map_err(|e| format!("kill failed: {e}"))?;
    if !status.success() {
        return Err(format!("kill -HUP {pid} returned non-zero"));
    }
    tracing::info!(pid, "daemon reload (SIGHUP) sent");
    Ok(ReloadResponse { ok: true, pid })
}

async fn probe_socket(sock_path: &str) -> DaemonStatusResponse {
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    let connect = timeout(Duration::from_millis(800), UnixStream::connect(sock_path)).await;

    let mut stream = match connect {
        Ok(Ok(s)) => s,
        _ => {
            return DaemonStatusResponse {
                running: false,
                pid: None,
            }
        }
    };

    // Read until EOF or timeout, then parse the greeting JSON.
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_millis(800), stream.read_to_end(&mut buf)).await;

    if buf.is_empty() {
        return DaemonStatusResponse {
            running: false,
            pid: None,
        };
    }

    match serde_json::from_slice::<serde_json::Value>(&buf) {
        Ok(v) => {
            let pid = v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32);
            DaemonStatusResponse { running: true, pid }
        }
        Err(_) => DaemonStatusResponse {
            running: false,
            pid: None,
        },
    }
}
