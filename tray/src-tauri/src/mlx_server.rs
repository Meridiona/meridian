//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! MLX inference server — child process management (Approach A: bundled venv).
//!
//! Resolves, spawns, and health-monitors the MLX FastAPI server (`server.py`,
//! port 7823) as a child process. The tray owns the server's lifecycle; the
//! server is intentionally detached so a tray crash doesn't interrupt inflight
//! classification.
//!
//! **Resolution order:**
//! 1. `<app>.app/Contents/Resources/venv/bin/python` + `Resources/server.py`
//!    — bundled venv (Approach A, the production target).
//! 2. `MERIDIAN_SERVICES_DIR` env override — CI / integration test override.
//! 3. `~/.meridian/app/services/.venv/bin/python` — legacy bundle-install path.
//! 4. Walk from CWD up for `services/.venv/bin/python` — dev mode.
//!
//! # Who calls this
//! - [`crate::commands::setup`] — wizard status query + start button
//! - [`crate::lib`] — `reclaim_orphan` on tray startup
//!
//! # Related
//! - [`crate::commands::setup`] — the Tauri commands the wizard calls
//! - `services/agents/server.py` — the FastAPI server this module manages

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Status of the MLX inference server.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MlxStatus {
    /// No runtime found or server not reachable.
    #[default]
    Offline,
    /// Binary found; server spawned but not yet responding.
    Starting,
    /// Server is responding to health probes.
    Running,
    /// Start was attempted but the spawn failed.
    Error(String),
}

/// Tracks the lifecycle state of the managed MLX server subprocess.
#[derive(Debug, Default)]
pub struct MlxManager {
    pub port: u16,
    pub pid: Option<u32>,
    pub status: MlxStatus,
}

impl MlxManager {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            pid: None,
            status: MlxStatus::Offline,
        }
    }
}

/// Arc-wrapped Tokio-locked manager — the Tauri managed-state type.
pub type SharedMlxManager = Arc<Mutex<MlxManager>>;

/// Locate the Python binary and server script for the MLX server.
/// Returns `(python_bin, server_script)` or `None` when no runtime is found.
pub fn resolve_mlx_command() -> Option<(PathBuf, PathBuf)> {
    // 1. Bundled venv in the running .app (Approach A — production path).
    //    current_exe → Contents/MacOS/meridian-tray → parent×2 → Contents/
    if let Ok(exe) = std::env::current_exe() {
        if let Some(contents) = exe.parent().and_then(|p| p.parent()) {
            let py = contents.join("Resources/venv/bin/python");
            let srv = contents.join("Resources/server.py");
            if py.exists() && srv.exists() {
                return Some((py, srv));
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();

    // 2. Env override — set MERIDIAN_SERVICES_DIR to the services/ directory.
    if let Ok(sd) = std::env::var("MERIDIAN_SERVICES_DIR") {
        let py = PathBuf::from(&sd).join(".venv/bin/python");
        let srv = PathBuf::from(&sd).join("agents/server.py");
        if py.exists() && srv.exists() {
            return Some((py, srv));
        }
    }

    // 3. Legacy bundle-install path.
    {
        let py = PathBuf::from(format!("{home}/.meridian/app/services/.venv/bin/python"));
        let srv = PathBuf::from(format!("{home}/.meridian/app/services/agents/server.py"));
        if py.exists() && srv.exists() {
            return Some((py, srv));
        }
    }

    // 4. Dev mode: walk up from CWD.
    if let Ok(mut dir) = std::env::current_dir() {
        for _ in 0..6 {
            let py = dir.join("services/.venv/bin/python");
            let srv = dir.join("services/agents/server.py");
            if py.exists() && srv.exists() {
                return Some((py, srv));
            }
            if !dir.pop() {
                break;
            }
        }
    }

    None
}

/// Probe the server's `/health` endpoint. Returns `true` on a 2xx response
/// within 2 seconds.
pub async fn health_check(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Check whether a process with the given PID is alive using `kill -0`.
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// On tray startup, check if a server from a previous run is still alive.
/// If it is and health-probe passes, adopt it into the manager (pid + Running)
/// rather than spawning a fresh one. Removes the PID file if stale.
pub async fn reclaim_orphan(home: &str, port: u16, manager: &SharedMlxManager) {
    let pid_path = PathBuf::from(format!("{home}/.meridian/mlx-server.pid"));
    let pid_str = match tokio::fs::read_to_string(&pid_path).await {
        Ok(s) => s,
        Err(_) => return,
    };
    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            let _ = tokio::fs::remove_file(&pid_path).await;
            return;
        }
    };

    if !process_alive(pid) {
        let _ = tokio::fs::remove_file(&pid_path).await;
        return;
    }

    if health_check(port).await {
        tracing::info!(pid, "mlx: reclaimed orphaned server");
        let mut m = manager.lock().await;
        m.pid = Some(pid);
        m.status = MlxStatus::Running;
    }
    // Process alive but not yet healthy — health polling will discover it.
}

/// Spawn the MLX server as a detached child process. Writes `~/.meridian/mlx-server.pid`
/// and updates the manager status to `Starting`.
pub async fn start(port: u16, manager: &SharedMlxManager) -> Result<(), String> {
    let Some((python, server)) = resolve_mlx_command() else {
        return Err("MLX runtime not found. Bundle the venv in the app or set \
             MERIDIAN_SERVICES_DIR to your services/ directory."
            .to_string());
    };

    tracing::info!(%port, python = %python.display(), "mlx: starting server");

    let home = std::env::var("HOME").unwrap_or_default();
    let log_dir = format!("{home}/.meridian/logs");
    let _ = tokio::fs::create_dir_all(&log_dir).await;

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{log_dir}/mlx-server.log"))
        .map_err(|e| format!("open log: {e}"))?;
    let log_err = log_file.try_clone().map_err(|e| e.to_string())?;

    let mut cmd = tokio::process::Command::new(&python);
    cmd.arg(&server)
        .env("MLX_SERVER_PORT", port.to_string())
        .stdout(log_file)
        .stderr(log_err)
        // kill_on_drop(false): server outlives the handle so tray restart
        // doesn't interrupt inflight classification.
        .kill_on_drop(false);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("spawn: {e}");
            manager.lock().await.status = MlxStatus::Error(msg.clone());
            return Err(msg);
        }
    };
    let pid = child
        .id()
        .ok_or_else(|| "could not get child PID".to_string())?;

    // Reap the child when it eventually exits so it doesn't become a zombie.
    tokio::spawn(async move {
        let mut c = child;
        let _ = c.wait().await;
    });

    // Write PID marker for orphan reclaim on next tray launch.
    let _ = tokio::fs::write(format!("{home}/.meridian/mlx-server.pid"), pid.to_string()).await;

    {
        let mut m = manager.lock().await;
        m.pid = Some(pid);
        m.status = MlxStatus::Starting;
    }

    tracing::info!(pid, "mlx: server spawned");
    Ok(())
}

/// Probe the server and reconcile `MlxManager::status` with reality.
pub async fn sync_status(manager: &SharedMlxManager) {
    let port = manager.lock().await.port;
    let alive = health_check(port).await;
    let mut m = manager.lock().await;
    m.status = if alive {
        MlxStatus::Running
    } else if m.pid.is_some() {
        MlxStatus::Starting
    } else {
        MlxStatus::Offline
    };
}
