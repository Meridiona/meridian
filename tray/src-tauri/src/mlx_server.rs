//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! MLX inference server — child process management (Approach C: download-and-provision).
//!
//! The tray ships pure-Rust + small. On first run the user clicks "Download" in
//! the wizard and this module fetches a pre-built `meridian-mlx-runtime` tarball
//! (CPython + venv + `server.py`) into `~/.meridian/runtime/`. After that, the
//! tray manages the server as a child process on every launch — identical to how
//! openhuman manages `ollama serve`.
//!
//! **Resolution order:**
//! 1. `~/.meridian/runtime/bin/python` + `runtime/server.py` — downloaded runtime (Approach C target).
//! 2. `MERIDIAN_SERVICES_DIR` env override — CI / integration test override.
//! 3. `~/.meridian/app/services/.venv/bin/python` — legacy bundle-install path.
//! 4. Walk from CWD up for `services/.venv/bin/python` — dev mode.
//!
//! # Who calls this
//! - [`crate::commands::setup`] — wizard status / download / start commands
//! - [`crate::lib`] — `reclaim_orphan` on tray startup
//!
//! # Related
//! - [`crate::commands::setup`] — the Tauri commands the wizard calls
//! - `services/agents/server.py` — the FastAPI server this module manages

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// URL of the pre-built `meridian-mlx-runtime` tarball (CPython + venv + server.py).
/// `""` means "not yet published" — the wizard shows a "not available" state.
/// Override at dev time with `MERIDIAN_RUNTIME_URL` env var.
const RUNTIME_TARBALL_URL: &str = "";

/// Download state emitted as progress events to the wizard.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    /// Bytes received so far.
    pub received: u64,
    /// Total bytes expected (`0` when the server omits Content-Length).
    pub total: u64,
    /// Human-readable status line shown under the progress bar.
    pub message: String,
}

/// Status of the MLX inference server.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MlxStatus {
    /// Runtime not present; download required.
    #[default]
    Offline,
    /// Runtime present; server spawned but not yet responding.
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

/// Returns the path where the downloaded runtime lives.
pub fn runtime_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.meridian/runtime"))
}

/// Returns `true` when the downloaded runtime has been provisioned.
pub fn runtime_installed() -> bool {
    let dir = runtime_dir();
    dir.join("bin/python").exists() && dir.join("server.py").exists()
}

/// Locate the Python binary and server script for the MLX server.
/// Returns `(python_bin, server_script)` or `None` when no runtime is found.
pub fn resolve_mlx_command() -> Option<(PathBuf, PathBuf)> {
    // 1. Downloaded runtime — the production path for Approach C.
    {
        let dir = runtime_dir();
        let py = dir.join("bin/python");
        let srv = dir.join("server.py");
        if py.exists() && srv.exists() {
            return Some((py, srv));
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

/// Resolve the runtime tarball URL. `MERIDIAN_RUNTIME_URL` env var overrides
/// the compiled-in constant (useful for testing with a locally-served tarball).
pub fn runtime_url() -> Option<String> {
    if let Ok(url) = std::env::var("MERIDIAN_RUNTIME_URL") {
        if !url.is_empty() {
            return Some(url);
        }
    }
    if !RUNTIME_TARBALL_URL.is_empty() {
        return Some(RUNTIME_TARBALL_URL.to_string());
    }
    None
}

/// Download the runtime tarball and extract it to `~/.meridian/runtime/`.
///
/// Streams download progress by calling `on_progress` with each chunk. When
/// the server omits Content-Length, `total` in `DownloadProgress` is `0`.
/// Extraction uses the system `tar` binary (always present on macOS).
pub async fn download_runtime<F>(on_progress: F) -> Result<(), String>
where
    F: Fn(DownloadProgress) + Send + 'static,
{
    use futures_util::StreamExt;

    let url = runtime_url().ok_or_else(|| {
        "Runtime tarball URL not yet configured. Check back for updates.".to_string()
    })?;

    tracing::info!(url, "mlx: downloading runtime");
    on_progress(DownloadProgress {
        received: 0,
        total: 0,
        message: "Connecting…".to_string(),
    });

    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("download request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("download failed: HTTP {}", response.status()));
    }

    let total = response.content_length().unwrap_or(0);
    let home = std::env::var("HOME").unwrap_or_default();
    let tmp_path = format!("{home}/.meridian/runtime.tar.gz");

    let _ = tokio::fs::create_dir_all(format!("{home}/.meridian")).await;

    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| format!("create temp file: {e}"))?;

    let mut stream = response.bytes_stream();
    let mut received: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        use tokio::io::AsyncWriteExt;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write error: {e}"))?;
        received += chunk.len() as u64;
        on_progress(DownloadProgress {
            received,
            total,
            message: if total > 0 {
                format!(
                    "Downloading… {:.0}%",
                    received as f64 / total as f64 * 100.0
                )
            } else {
                format!("Downloading… {} MB", received / 1_048_576)
            },
        });
    }

    on_progress(DownloadProgress {
        received,
        total: received,
        message: "Extracting…".to_string(),
    });

    // Extract tarball → ~/.meridian/runtime/
    let runtime_dir = format!("{home}/.meridian/runtime");
    tokio::fs::create_dir_all(&runtime_dir)
        .await
        .map_err(|e| format!("create runtime dir: {e}"))?;

    let out = tokio::process::Command::new("tar")
        .args([
            "-xzf",
            &tmp_path,
            "-C",
            &runtime_dir,
            "--strip-components=1",
        ])
        .output()
        .await
        .map_err(|e| format!("tar spawn: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("tar extraction failed: {stderr}"));
    }

    // Clean up the temp tarball.
    let _ = tokio::fs::remove_file(&tmp_path).await;

    on_progress(DownloadProgress {
        received,
        total: received,
        message: "Runtime ready.".to_string(),
    });

    tracing::info!(%runtime_dir, "mlx: runtime extracted");
    Ok(())
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
/// rather than spawning a duplicate. Removes the PID file if stale.
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
        return Err(
            "MLX runtime not found. Download the runtime from the wizard's Model step or \
             set MERIDIAN_SERVICES_DIR to your services/ directory."
                .to_string(),
        );
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

    // Launch the way the layout demands. The dev layout
    // (services/agents/server.py) MUST run as `python -m agents.server` from the
    // services dir, or the top-level `from agents import …` fails with
    // ModuleNotFoundError. A downloaded runtime that bundles the `agents` package
    // runs the same way from its root; a flat runtime ships a self-contained
    // server.py run directly. The port goes via `--port` — server.py's `main()`
    // parses `--port` and does NOT read MLX_SERVER_PORT (we still set the env for
    // any code path that wants it).
    let parent = server
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let module_args = || {
        vec![
            "-m".to_string(),
            "agents.server".to_string(),
            "--port".to_string(),
            port.to_string(),
        ]
    };
    let (work_dir, launch_args): (PathBuf, Vec<String>) =
        if parent.file_name().map(|n| n == "agents").unwrap_or(false) {
            // dev: .../services/agents/server.py → run from services/ with -m
            (
                parent.parent().unwrap_or(parent.as_path()).to_path_buf(),
                module_args(),
            )
        } else if parent.join("agents").is_dir() {
            // downloaded runtime that bundles the agents package at its root
            (parent.clone(), module_args())
        } else {
            // flat runtime: a self-contained server.py at the runtime root
            let file = server
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "server.py".to_string());
            (
                parent.clone(),
                vec![file, "--port".to_string(), port.to_string()],
            )
        };

    tracing::info!(work_dir = %work_dir.display(), args = ?launch_args, "mlx: launch resolved");

    let mut cmd = tokio::process::Command::new(&python);
    cmd.current_dir(&work_dir)
        .args(&launch_args)
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

/// Whether *something* is accepting TCP connections on the port. A wedged server
/// (socket bound, worker dead) still completes the TCP handshake even though
/// `/health` times out, so this distinguishes "port held by someone" from "free"
/// — letting [`supervise`] avoid spawning a duplicate that would just EADDRINUSE.
async fn port_listening(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(300),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Terminate a process by PID (SIGTERM) — used to clear our own wedged server.
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .output();
}

/// The result of one [`supervise`] cycle — drives the poll loop's logging and
/// bounded-restart bookkeeping.
#[derive(Debug, PartialEq)]
pub enum SuperviseOutcome {
    /// Server answered `/health`.
    Healthy,
    /// No runtime resolvable — nothing to supervise (the wizard download owns this).
    NoRuntime,
    /// Port is held by a process the tray doesn't manage (e.g. a hand-started dev
    /// server, possibly wedged); not spawning a doomed duplicate.
    PortHeldForeign,
    /// Killed our own wedged server; a restart follows on the next cycle.
    KilledWedged,
    /// Spawned a fresh server.
    Restarted,
    /// A restart was attempted but the spawn failed.
    RestartFailed(String),
}

/// One supervision cycle: keep the MLX server alive.
///
/// - Healthy → mark `Running`.
/// - Down, no runtime → `Offline` (nothing to do; the wizard provisions it).
/// - Down, our managed pid alive-but-wedged → kill it (restart next cycle).
/// - Down, port held by a foreign process → report, don't fight it.
/// - Down, port free, runtime available → (re)start.
///
/// The caller ([`crate::poll`]) bounds how many times it invokes this after a
/// failure, so a server that refuses to come up doesn't spawn-storm.
pub async fn supervise(manager: &SharedMlxManager) -> SuperviseOutcome {
    let port = manager.lock().await.port;

    if health_check(port).await {
        manager.lock().await.status = MlxStatus::Running;
        return SuperviseOutcome::Healthy;
    }

    if resolve_mlx_command().is_none() {
        let mut m = manager.lock().await;
        m.status = MlxStatus::Offline;
        m.pid = None;
        return SuperviseOutcome::NoRuntime;
    }

    // Down, but a runtime exists. Work out *why* the port isn't serving.
    let our_pid = manager.lock().await.pid;
    if let Some(pid) = our_pid {
        if process_alive(pid) {
            // Our server is alive but not answering — wedged. Kill it so the next
            // cycle starts clean (avoids racing a still-bound socket → EADDRINUSE).
            tracing::warn!(
                pid,
                "mlx: managed server wedged (alive, not serving) — killing"
            );
            kill_pid(pid);
            let home = std::env::var("HOME").unwrap_or_default();
            let _ = tokio::fs::remove_file(format!("{home}/.meridian/mlx-server.pid")).await;
            let mut m = manager.lock().await;
            m.pid = None;
            m.status = MlxStatus::Starting;
            return SuperviseOutcome::KilledWedged;
        }
    }

    // No live managed process. If the port is still held, it's a foreign owner
    // (e.g. a hand-started dev server) — don't spawn a duplicate that EADDRINUSEs.
    if port_listening(port).await {
        tracing::warn!(
            port,
            "mlx: port held by an unmanaged process — not restarting"
        );
        return SuperviseOutcome::PortHeldForeign;
    }

    // Stale pid recorded but the process is gone — clear it, then start fresh.
    if our_pid.is_some() {
        manager.lock().await.pid = None;
    }
    match start(port, manager).await {
        Ok(()) => SuperviseOutcome::Restarted,
        Err(e) => SuperviseOutcome::RestartFailed(e),
    }
}
