//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/daemon/status` GET ported to Rust — lightweight Unix-socket liveness probe.
//!
//! Connects to `~/.meridian/daemon.sock`, waits up to 800 ms for the daemon to
//! send its JSON greeting (`{"pid": N}`), and resolves `{running, pid}`. Any
//! failure (no socket, timeout, bad JSON) resolves `{running: false}` — same
//! silent-resolve contract as the TS route.
//!
//! # Who calls this
//! - Command: `get_daemon_status` (registered in `lib.rs`)
//! - Frontend: `ui/app/api/daemon/status/route.ts` (browser fallback until export cutover),
//!   then `ui/lib/bridge.ts::load('/api/daemon/status', 'get_daemon_status')`.
//!
//! # Related
//! - [`crate::commands::restart_daemon`] — kicks launchctl to restart the daemon
//! - [`crate::commands::toggle_daemon`] — starts/stops via launchctl

use serde::Serialize;
use std::time::Duration;

/// Response shape matching the TS route's `{ running, pid? }`.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatusResponse {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Probe `~/.meridian/daemon.sock` with an 800 ms timeout (mirrors the TS route).
///
/// Returns `{running: false}` on any error — no error is surfaced to the caller
/// (matches the route's resolve-empty contract: stale UI stays visible rather than
/// showing an error toast on every health poll tick).
#[tauri::command]
#[tracing::instrument]
pub async fn get_daemon_status() -> Result<DaemonStatusResponse, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let sock_path = format!("{}/.meridian/daemon.sock", home);

    let result = probe_socket(&sock_path).await;
    tracing::info!(running = result.running, pid = ?result.pid, "daemon_status");
    Ok(result)
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
