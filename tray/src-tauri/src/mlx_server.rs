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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// URL of the `runtime-manifest.json` describing the published runtime
/// (version + tarball url + sha256 + floors — see `scripts/build-mlx-runtime.sh`).
/// `""` means "not yet published" — the wizard shows a "not available" state.
/// Override at dev time with `MERIDIAN_RUNTIME_MANIFEST_URL`.
const RUNTIME_MANIFEST_URL: &str = "";

/// The published runtime descriptor (`runtime-manifest.json`). The download path
/// fetches this FIRST, then verifies the tarball against `sha256` and compares
/// `version` to skip a redundant re-download.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeManifest {
    /// Runtime version (the services package version it was built from).
    pub version: String,
    /// Target architecture — always `aarch64` (we only ship Apple Silicon).
    pub arch: String,
    /// Minimum macOS version (e.g. `13.5`) the bundled MLX stack requires.
    pub min_macos: String,
    /// Direct download URL of the tarball.
    pub url: String,
    /// Lowercase hex SHA-256 of the tarball — the integrity gate.
    pub sha256: String,
    /// Tarball size in bytes (informational / disk preflight).
    #[serde(default)]
    pub size: u64,
}

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

/// Path of the version marker written after a successful install.
fn version_marker() -> PathBuf {
    runtime_dir().join("runtime.version")
}

/// The version of the currently-installed runtime (the marker written on the
/// last successful download), or `None` if no runtime / marker is present.
pub fn installed_version() -> Option<String> {
    if !runtime_installed() {
        return None;
    }
    std::fs::read_to_string(version_marker())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// Resolve the runtime manifest URL. `MERIDIAN_RUNTIME_MANIFEST_URL` overrides
/// the compiled-in constant (useful for testing against a locally-served
/// manifest). `None` → no runtime is published yet (wizard shows "not available").
pub fn manifest_url() -> Option<String> {
    if let Ok(url) = std::env::var("MERIDIAN_RUNTIME_MANIFEST_URL") {
        if !url.is_empty() {
            return Some(url);
        }
    }
    if !RUNTIME_MANIFEST_URL.is_empty() {
        return Some(RUNTIME_MANIFEST_URL.to_string());
    }
    None
}

/// Fetch + parse the runtime manifest from [`manifest_url`].
async fn fetch_manifest() -> Result<RuntimeManifest, String> {
    let url = manifest_url().ok_or_else(|| {
        "Runtime manifest URL not configured yet. Check back for updates.".to_string()
    })?;
    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("manifest request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("manifest fetch failed: HTTP {}", resp.status()));
    }
    resp.json::<RuntimeManifest>()
        .await
        .map_err(|e| format!("manifest parse failed: {e}"))
}

/// Whether the running macOS version is at least `required` (compares
/// major.minor numerically; trailing components are ignored). Defaults to
/// permitting the install if either version can't be parsed — we'd rather let
/// the OS surface an incompatibility than wrongly block a valid machine.
fn macos_at_least(running: &str, required: &str) -> bool {
    fn major_minor(v: &str) -> Option<(u32, u32)> {
        let mut it = v.split('.');
        let major = it.next()?.trim().parse().ok()?;
        let minor = it.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        Some((major, minor))
    }
    match (major_minor(running), major_minor(required)) {
        (Some(r), Some(req)) => r >= req,
        _ => true,
    }
}

/// The running macOS product version (e.g. `14.5`), via `sw_vers`.
async fn running_macos() -> Option<String> {
    let out = tokio::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .await
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// Compute the lowercase-hex SHA-256 of a file, reading it in 1 MiB chunks.
fn sha256_hex_of(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Outcome of [`download_runtime`].
#[derive(Debug, PartialEq)]
pub enum DownloadOutcome {
    /// The installed runtime already matches the manifest version — nothing done.
    AlreadyCurrent,
    /// A new runtime was downloaded, verified, and extracted.
    Installed,
}

/// Provision the MLX runtime into `~/.meridian/runtime/` from the published
/// manifest — the integrity-checked download path (Approach C, Step 3).
///
/// Flow: fetch the manifest → preflight (arch + macOS floor) → skip if the
/// installed version already matches → stream the tarball (reporting progress)
/// → **verify SHA-256 against the manifest** → extract only on a match → write
/// the version marker. A corrupted, truncated, or tampered download fails loudly
/// and is deleted, never extracted.
///
/// `on_progress` is called with each chunk; `total` is `0` when the server omits
/// Content-Length. Extraction uses the system `tar`.
pub async fn download_runtime<F>(on_progress: F) -> Result<DownloadOutcome, String>
where
    F: Fn(DownloadProgress) + Send + 'static,
{
    use futures_util::StreamExt;

    let manifest = fetch_manifest().await?;
    tracing::info!(version = %manifest.version, url = %manifest.url, "mlx: runtime manifest");

    // ── Preflight: don't download a runtime this machine can't run. ───────────
    if manifest.arch != std::env::consts::ARCH {
        return Err(format!(
            "runtime is for {} but this machine is {}",
            manifest.arch,
            std::env::consts::ARCH
        ));
    }
    if let Some(running) = running_macos().await {
        if !macos_at_least(&running, &manifest.min_macos) {
            return Err(format!(
                "this runtime needs macOS {} or newer (you have {running})",
                manifest.min_macos
            ));
        }
    }

    // ── Version skip: already on the published version → nothing to do. ───────
    if installed_version().as_deref() == Some(manifest.version.as_str()) {
        tracing::info!(version = %manifest.version, "mlx: runtime already current");
        on_progress(DownloadProgress {
            received: 0,
            total: 0,
            message: "Runtime already up to date.".to_string(),
        });
        return Ok(DownloadOutcome::AlreadyCurrent);
    }

    on_progress(DownloadProgress {
        received: 0,
        total: 0,
        message: "Connecting…".to_string(),
    });

    let response = reqwest::Client::new()
        .get(&manifest.url)
        .send()
        .await
        .map_err(|e| format!("download request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("download failed: HTTP {}", response.status()));
    }

    let total = response.content_length().unwrap_or(manifest.size);
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
    use tokio::io::AsyncWriteExt;
    file.flush().await.map_err(|e| format!("flush: {e}"))?;
    drop(file);

    // ── Integrity gate: verify SHA-256 BEFORE extracting anything. ────────────
    on_progress(DownloadProgress {
        received,
        total: received,
        message: "Verifying…".to_string(),
    });
    let tmp = PathBuf::from(&tmp_path);
    let actual = {
        let p = tmp.clone();
        tokio::task::spawn_blocking(move || sha256_hex_of(&p))
            .await
            .map_err(|e| format!("hash task: {e}"))?
            .map_err(|e| format!("hash read: {e}"))?
    };
    if !actual.eq_ignore_ascii_case(&manifest.sha256) {
        let _ = tokio::fs::remove_file(&tmp).await;
        tracing::error!(expected = %manifest.sha256, actual = %actual, "mlx: checksum mismatch");
        return Err(format!(
            "checksum mismatch — download corrupted or tampered \
             (expected {}, got {actual}). The runtime was not installed.",
            manifest.sha256
        ));
    }
    tracing::info!(sha256 = %actual, "mlx: tarball verified");

    // ── Extract (verified) → ~/.meridian/runtime/, then stamp the version. ────
    on_progress(DownloadProgress {
        received,
        total: received,
        message: "Extracting…".to_string(),
    });
    let runtime_dir = runtime_dir();
    tokio::fs::create_dir_all(&runtime_dir)
        .await
        .map_err(|e| format!("create runtime dir: {e}"))?;
    let out = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tmp)
        .arg("-C")
        .arg(&runtime_dir)
        .arg("--strip-components=1")
        .output()
        .await
        .map_err(|e| format!("tar spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("tar extraction failed: {stderr}"));
    }

    let _ = tokio::fs::write(version_marker(), &manifest.version).await;
    let _ = tokio::fs::remove_file(&tmp).await;

    on_progress(DownloadProgress {
        received,
        total: received,
        message: "Runtime ready.".to_string(),
    });
    tracing::info!(version = %manifest.version, dir = %runtime_dir.display(), "mlx: runtime installed");
    Ok(DownloadOutcome::Installed)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_version_floor() {
        // Meets or exceeds → ok.
        assert!(macos_at_least("13.5", "13.5"));
        assert!(macos_at_least("14.0", "13.5"));
        assert!(macos_at_least("26.5.1", "13.5")); // extra components ignored
        assert!(macos_at_least("14", "13.5")); // missing minor → treated as .0
                                               // Below the floor → blocked.
        assert!(!macos_at_least("13.4", "13.5"));
        assert!(!macos_at_least("12.7", "13.5"));
        // Unparseable → permit (let the OS surface any real incompatibility).
        assert!(macos_at_least("garbage", "13.5"));
        assert!(macos_at_least("14.0", ""));
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("abc") — the canonical NIST test vector.
        let dir = std::env::temp_dir().join(format!("mlx-sha-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("abc.txt");
        std::fs::write(&f, b"abc").unwrap();
        let got = sha256_hex_of(&f).unwrap();
        assert_eq!(
            got,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Verifies the case-insensitive compare the download path relies on.
        assert!(got.eq_ignore_ascii_case(&got.to_uppercase()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_parses_build_script_shape() {
        // Mirrors scripts/build-mlx-runtime.sh's runtime-manifest.json.
        let json = r#"{
            "version": "1.59.0",
            "arch": "aarch64",
            "python": "3.11.15",
            "min_macos": "13.5",
            "tarball": "meridian-mlx-runtime-1.59.0-aarch64.tar.gz",
            "url": "https://example.com/r.tar.gz",
            "sha256": "e24d9bdf95bcb108ef852de3c9658bb0387f70571d9360945783070e28098dbe",
            "size": 173466456
        }"#;
        let m: RuntimeManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.version, "1.59.0");
        assert_eq!(m.arch, "aarch64");
        assert_eq!(m.min_macos, "13.5");
        assert_eq!(m.size, 173466456);
        assert!(m.sha256.len() == 64);
    }
}
