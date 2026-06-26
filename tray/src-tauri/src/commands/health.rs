//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/health` GET ported to Rust — fast local health check for the status banner.
//!
//! Three checks run in parallel (FS + log scan + socket probe), matching the TS route.
//! The launchctl fallback for a11y trust also mirrors the route.
//!
//! No in-module cache — the tray's poll loop controls cadence (`do_health` every 60 s),
//! and the Tauri command is on-demand. The TS route's 15 s stale-while-revalidate was
//! only needed because multiple SSE clients hit the same Next.js server concurrently.
//!
//! # Who calls this
//! - Command: `get_health` (registered in `lib.rs`)
//! - Internal: [`crate::poll::refresh_health`] calls [`check_health`] directly,
//!   bypassing the HTTP round-trip.
//! - Frontend: `ui/components/HealthBanner.tsx` uses `/api/health/stream` (SSE) —
//!   that stream will be replaced with a Tauri event in the SSE migration phase.
//!
//! # Related
//! - [`crate::commands::daemon`] — deeper socket probe (reads daemon PID)
//! - [`crate::poll`] — schedules the tray's periodic health refresh

use serde::Serialize;
use std::time::Duration;

/// Response shape matching the TS route's `HealthStatus`.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a11y_helper_trusted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Run all three health checks in parallel and return the combined result.
/// Called by both `get_health` (Tauri command) and `poll::refresh_health` (internal).
pub async fn check_health() -> HealthResponse {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    // When the `capture` feature is enabled, a11y runs in-process inside the
    // tray (screenpipe-screen crate). No separate a11y-helper binary is needed
    // or expected — skip the trust check so the banner never fires falsely.
    #[cfg(feature = "capture")]
    let (db, daemon) = tokio::join!(check_database(), check_daemon_running(&home));
    #[cfg(feature = "capture")]
    let trusted: Option<bool> = None;

    #[cfg(not(feature = "capture"))]
    let (db, a11y, daemon) = tokio::join!(
        check_database(),
        check_a11y_trusted(&home),
        check_daemon_running(&home),
    );
    #[cfg(not(feature = "capture"))]
    let trusted = match a11y {
        Some(v) => Some(v),
        None => launchctl_a11y_trusted().await,
    };

    let error = if !db.0 { db.1 } else { None };

    HealthResponse {
        database_ready: Some(db.0),
        error,
        a11y_helper_trusted: trusted,
        daemon_running: daemon,
    }
}

/// Check whether the meridian DB is readable.
///
/// Resolves the path through [`crate::install::meridian_db_path`] — the same
/// `MERIDIAN_DB` / `~/.meridian/.env` / default chain the daemon and the tray's
/// own DB pool use — rather than re-deriving it inline. (The old inline lookup
/// read a non-existent `MERIDIAN_DB_PATH` var and the hardcoded default, so it
/// reported "not found" on any installed system with a custom `MERIDIAN_DB`.)
async fn check_database() -> (bool, Option<String>) {
    let db = crate::install::meridian_db_path();
    match tokio::fs::metadata(&db).await {
        Ok(_) => (true, None),
        Err(_) => (
            false,
            Some(
                "Database not found — start the daemon: \
                 launchctl load ~/Library/LaunchAgents/com.meridiona.daemon.plist"
                    .to_string(),
            ),
        ),
    }
}

/// Walk the last 200 lines of `~/.meridian/logs/a11y-helper.log` for a trust
/// entry. Returns `None` when the log is absent or has no trust line yet.
#[cfg(not(feature = "capture"))]
async fn check_a11y_trusted(home: &str) -> Option<bool> {
    let log_path = format!("{}/.meridian/logs/a11y-helper.log", home);
    let content = tokio::fs::read_to_string(&log_path).await.ok()?;
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    let start = lines.len().saturating_sub(200);
    for line in lines[start..].iter().rev() {
        if line.contains("trusted: true") || line.contains("[trusted]") {
            return Some(true);
        }
        if line.contains("trusted: false") || line.contains("[untrusted]") {
            return Some(false);
        }
    }
    None
}

/// Probe `~/.meridian/daemon.sock` with a 500 ms connect timeout.
/// Returns `None` on unexpected errors, `Some(false)` on ENOENT/ECONNREFUSED.
async fn check_daemon_running(home: &str) -> Option<bool> {
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    let sock = format!("{}/.meridian/daemon.sock", home);
    match timeout(Duration::from_millis(500), UnixStream::connect(&sock)).await {
        Ok(Ok(_)) => Some(true),
        Ok(Err(e)) => {
            use std::io::ErrorKind;
            match e.kind() {
                ErrorKind::NotFound | ErrorKind::ConnectionRefused => Some(false),
                _ => None,
            }
        }
        Err(_) => Some(false), // timeout → not reachable
    }
}

/// Fallback: ask `launchctl print` for the a11y-helper trust state.
/// Only called when the log scan is inconclusive (returns `None`).
#[cfg(not(feature = "capture"))]
async fn launchctl_a11y_trusted() -> Option<bool> {
    let uid = crate::sys::uid_str();
    let out = tokio::process::Command::new("launchctl")
        .args(["print", &format!("gui/{}/com.meridiona.a11y-helper", uid)])
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.is_empty() {
        return None;
    }
    if stdout.contains("a11y_trusted = 1") || stdout.contains("trusted") {
        Some(true)
    } else if stdout.contains("a11y_trusted = 0") {
        Some(false)
    } else {
        None
    }
}

/// The health check command (the ported `/api/health` GET).
///
/// Runs all three checks in parallel and returns the combined result.
/// Errors resolve to an empty response (matches the route's silent-resolve contract).
#[tauri::command]
#[tracing::instrument]
pub async fn get_health() -> Result<HealthResponse, String> {
    let result = check_health().await;
    tracing::info!(
        db = ?result.database_ready,
        daemon = ?result.daemon_running,
        a11y = ?result.a11y_helper_trusted,
        "health checked"
    );
    Ok(result)
}
