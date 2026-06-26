//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Setup wizard Tauri commands — first-run detection, permission probes, MLX status.
//!
//! Every interactive step of `ui/app/setup/page.tsx` calls one or more commands
//! from this module. No stubs — all commands return live state so the wizard can
//! advance only when real requirements are met.
//!
//! # Who calls this
//! Command: registered in `lib.rs`; invoked from `ui/app/setup/page.tsx`.
//!
//! # Related
//! - [`crate::mlx_server`] — the MLX child-process manager these commands expose
//! - [`crate::commands::system::open_permission_pane`] — opens System Settings panes

use crate::mlx_server::{self, MlxStatus, SharedMlxManager};
use serde::Serialize;
use tauri::Emitter;

/// Response shape for the wizard's Model step poll.
#[derive(Debug, Serialize)]
pub struct MlxStatusResponse {
    /// Current server status.
    pub status: MlxStatus,
    /// Port the server listens on (7823).
    pub port: u16,
    /// Whether a resolvable Python binary was found on this machine.
    pub runtime_found: bool,
    /// Whether the downloadable runtime is provisioned in `~/.meridian/runtime/`.
    pub runtime_installed: bool,
    /// Whether a runtime tarball URL is configured (download is possible).
    pub download_available: bool,
}

/// Returns `true` on the first launch — no `~/.meridian/onboarded` flag exists.
/// The wizard auto-opens when `true` and is skipped on subsequent launches.
#[tauri::command]
#[tracing::instrument]
pub async fn is_first_run() -> bool {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    !std::path::Path::new(&format!("{home}/.meridian/onboarded")).exists()
}

/// Write `~/.meridian/onboarded` (RFC-3339 timestamp) to mark wizard completion.
/// Future tray launches skip the auto-open. Idempotent — safe to call more than once.
#[tauri::command]
#[tracing::instrument]
pub async fn mark_setup_complete() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(format!("{home}/.meridian"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create ~/.meridian: {e}"))?;
    tokio::fs::write(dir.join("onboarded"), chrono::Local::now().to_rfc3339())
        .await
        .map_err(|e| format!("write onboarded: {e}"))?;
    tracing::info!("setup: onboarded flag written");
    Ok(())
}

/// Returns `true` when the current process (the tray) has Accessibility trust.
///
/// `AXIsProcessTrusted()` is keyed on the code-signing identity of the calling
/// process. In the B-in-process capture track the tray is the capture binary, so
/// this is the authoritative signal. For the current architecture (a11y-helper is
/// separate), this tells the wizard whether the tray itself is trusted — which is
/// also the correct target since the wizard prompts the user to add Meridian.
#[tauri::command]
#[tracing::instrument]
pub async fn check_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    return ax_is_trusted();
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
fn ax_is_trusted() -> bool {
    // ApplicationServices.framework is a system framework present on all macOS
    // versions; no additional Cargo dep required.
    #[link(name = "ApplicationServices", kind = "framework")]
    #[allow(improper_ctypes)]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    // Safety: AXIsProcessTrusted is a pure status read with no side effects or UB.
    unsafe { AXIsProcessTrusted() }
}

/// Returns `true` when the tray itself holds macOS Screen Recording permission.
///
/// Post-cutover (Gap-2 Bucket 2) capture runs in-process, so the **tray** is
/// the process that needs Screen Recording. `CGPreflightScreenCaptureAccess()`
/// reads that grant directly — no prompt, no side effects — replacing the old
/// `pgrep screenpipe` / `~/.screenpipe/db.sqlite` proxy, which misreported on
/// an in-process install (no screenpipe process or DB ever exists). The
/// wizard's *grant* action (which surfaces the system prompt via
/// `CGRequestScreenCaptureAccess`) is separate slice-5 work.
#[tauri::command]
#[tracing::instrument]
pub async fn check_screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        // Safety: preflight is a pure status read — no prompt, no side effects.
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Surface the macOS Screen Recording prompt **and register the app** so it
/// appears in System Settings → Privacy → Screen Recording, then return the
/// resulting grant state.
///
/// [`check_screen_recording`] uses `CGPreflightScreenCaptureAccess` — a pure
/// status read that never registers the app. On a fresh install this means the
/// list under Privacy → Screen Recording shows "No Items", because macOS only
/// adds an entry the *first* time the app calls `CGRequestScreenCaptureAccess`.
/// This command calls that request variant (analogous to `request_input_monitoring`)
/// so clicking the wizard's grant button both registers the app and shows the
/// system dialog in one shot.
#[tauri::command]
#[tracing::instrument]
pub async fn request_screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGRequestScreenCaptureAccess() -> bool;
        }
        // Safety: CGRequestScreenCaptureAccess shows the TCC prompt + registers the
        // app, then returns the resulting grant state — no UB.
        let granted = unsafe { CGRequestScreenCaptureAccess() };
        tracing::info!(granted, "setup: requested Screen Recording access");
        granted
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Returns `true` when the tray holds macOS **Input Monitoring** permission.
///
/// The in-process input recorder (`capture::ui_events::run_ui_event_recorder`)
/// runs a `CGEventTap` listener, which macOS gates behind Input Monitoring.
/// Without it the recorder degrades silently and `capture_ui_events` stays empty,
/// so the daemon's Option C `ended_at` refinement never fires — hence the wizard
/// must surface it as its own card. `IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)`
/// reads the grant directly (no prompt, no side effects), mirroring the
/// `CGPreflightScreenCaptureAccess` / `AXIsProcessTrusted` probes above. The
/// *grant* action is the wizard's "Open in System Settings" button
/// ([`crate::commands::system::open_permission_pane`] `"input_monitoring"`).
#[tauri::command]
#[tracing::instrument]
pub async fn check_input_monitoring() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            fn IOHIDCheckAccess(request_type: u32) -> u32;
        }
        // kIOHIDRequestTypeListenEvent = 1 (Input Monitoring);
        // kIOHIDAccessTypeGranted = 0.
        const LISTEN_EVENT: u32 = 1;
        const GRANTED: u32 = 0;
        // Safety: IOHIDCheckAccess is a pure status read — no prompt, no side effects.
        unsafe { IOHIDCheckAccess(LISTEN_EVENT) == GRANTED }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Surface the macOS Input Monitoring prompt **and register the app** so it
/// appears in the Input Monitoring list, then return the resulting grant state.
///
/// [`check_input_monitoring`] only *reads* status (`IOHIDCheckAccess`) — it never
/// registers the app, so on a fresh install the System Settings pane shows
/// "No Items" and the user has nothing to toggle. `IOHIDRequestAccess` is the
/// grant analogue of `CGRequestScreenCaptureAccess`: on first call it shows the
/// system prompt and registers the app; thereafter it's a no-op returning the
/// current state. The wizard calls this from the Input Monitoring card's button
/// (alongside opening the pane). Note `IOHIDRequestAccess` returns a `Boolean`
/// (granted/not) — unlike `IOHIDCheckAccess`, which returns the access-type enum.
#[tauri::command]
#[tracing::instrument]
pub async fn request_input_monitoring() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            fn IOHIDRequestAccess(request_type: u32) -> bool;
        }
        // kIOHIDRequestTypeListenEvent = 1 (Input Monitoring).
        const LISTEN_EVENT: u32 = 1;
        // Safety: IOHIDRequestAccess surfaces the TCC prompt + registers the app,
        // then returns a Boolean grant state — no UB.
        let granted = unsafe { IOHIDRequestAccess(LISTEN_EVENT) };
        tracing::info!(granted, "setup: requested Input Monitoring access");
        granted
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Query the current MLX server status. Polled every 3 seconds by the wizard's
/// Model step. Returns `runtime_found` alongside status so the UI can distinguish
/// "not installed" from "installed but offline".
#[tauri::command]
#[tracing::instrument(skip(mlx))]
pub async fn get_mlx_status(
    mlx: tauri::State<'_, SharedMlxManager>,
) -> Result<MlxStatusResponse, String> {
    mlx_server::sync_status(&mlx).await;
    let m = mlx.lock().await;
    let runtime_found = mlx_server::resolve_mlx_command().is_some();
    let runtime_installed = mlx_server::runtime_installed();
    let download_available = mlx_server::manifest_url().is_some();
    tracing::debug!(
        status = ?m.status,
        runtime_found,
        runtime_installed,
        download_available,
        "mlx: status queried"
    );
    Ok(MlxStatusResponse {
        status: m.status.clone(),
        port: m.port,
        runtime_found,
        runtime_installed,
        download_available,
    })
}

/// Download, verify, and provision the MLX runtime into `~/.meridian/runtime/`
/// (Approach C). Manifest-driven: skips the download when the installed version
/// already matches, and verifies the tarball's SHA-256 before extracting.
///
/// Streams progress to the frontend via the `mlx-download-progress` Tauri event
/// (payload: [`crate::mlx_server::DownloadProgress`]). On success the wizard's
/// next `get_mlx_status` poll sees `runtime_installed = true` and can start the
/// server. A checksum mismatch returns an error and installs nothing.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn download_runtime_cmd(app: tauri::AppHandle) -> Result<(), String> {
    let handle = app.clone();
    let outcome = mlx_server::download_runtime(move |p| {
        let _ = handle.emit("mlx-download-progress", p);
    })
    .await
    .inspect_err(|e| tracing::warn!(error = %e, "mlx: runtime download failed"))?;
    tracing::info!(?outcome, "mlx: runtime download finished");
    Ok(())
}

/// Eager-download the spec-aware classifier model into the HF cache so the first
/// classification doesn't pay a silent ~7 GB download mid-inference. The model id
/// is chosen server-side by `llm_selector`, so this prefetches exactly what the
/// first `load()` resolves. Streams progress via the same `mlx-download-progress`
/// event the runtime download uses (the wizard's Model step listens for both
/// phases). Requires the MLX server to be running — the wizard calls this after
/// `start_mlx_server_cmd` reports the server up. Idempotent server-side.
#[tauri::command]
#[tracing::instrument(skip(app, mlx))]
pub async fn prefetch_model_cmd(
    app: tauri::AppHandle,
    mlx: tauri::State<'_, SharedMlxManager>,
) -> Result<(), String> {
    let port = mlx.lock().await.port;
    let handle = app.clone();
    mlx_server::prefetch_model(port, move |p| {
        let _ = handle.emit("mlx-download-progress", p);
    })
    .await
    .inspect_err(|e| tracing::warn!(error = %e, "mlx: model prefetch failed"))?;
    tracing::info!("mlx: model prefetch finished");
    Ok(())
}

/// Start the MLX server if it isn't already running. The wizard's Model step
/// calls this when `get_mlx_status` reports `offline` and `runtime_found = true`.
/// Safe to call when the server is already up — health check short-circuits spawn.
#[tauri::command]
#[tracing::instrument(skip(mlx))]
pub async fn start_mlx_server_cmd(mlx: tauri::State<'_, SharedMlxManager>) -> Result<(), String> {
    let port = mlx.lock().await.port;
    if mlx_server::health_check(port).await {
        let mut m = mlx.lock().await;
        m.status = MlxStatus::Running;
        return Ok(());
    }
    mlx_server::start(port, &mlx).await
}

/// Detected hardware specs for the wizard's "Local intelligence" step. Drives the
/// real spec panel + the memory gauge (the model's resident footprint is sized
/// against `ram_gb`), so every number here must be live — never fabricated.
#[derive(Debug, Serialize, Default)]
pub struct SystemSpecs {
    /// Marketing chip name, e.g. "Apple M3 Pro" (empty if undetectable).
    pub chip: String,
    /// "macOS <version>", e.g. "macOS 15.5".
    pub macos: String,
    /// Physical CPU core count (`hw.physicalcpu`).
    pub cpu_cores: u32,
    /// GPU core count parsed from `system_profiler` (0 when unknown).
    pub gpu_cores: u32,
    /// Unified memory in whole GB (`hw.memsize`, rounded).
    pub ram_gb: u32,
    /// Free space in whole GB on the data volume (0 when undetectable).
    pub free_disk_gb: u32,
}

/// Probe this Mac's chip, core counts, unified memory, and free disk for the
/// wizard's Local-intelligence step. macOS-only — all reads are non-privileged
/// (`sysctl`, `sw_vers`, `system_profiler`, `df`); on other platforms the wizard
/// gets `SystemSpecs::default()` (all zeros) and degrades to a generic panel.
///
/// Runs the shell-outs on a blocking thread (`system_profiler` can take ~1 s) so
/// the async runtime isn't stalled during the one-time wizard probe.
#[tauri::command]
#[tracing::instrument]
pub async fn detect_system_specs() -> SystemSpecs {
    #[cfg(target_os = "macos")]
    {
        let specs = tokio::task::spawn_blocking(detect_specs_blocking)
            .await
            .unwrap_or_default();
        tracing::info!(
            chip = %specs.chip,
            cpu_cores = specs.cpu_cores,
            gpu_cores = specs.gpu_cores,
            ram_gb = specs.ram_gb,
            free_disk_gb = specs.free_disk_gb,
            "setup: system specs detected"
        );
        specs
    }
    #[cfg(not(target_os = "macos"))]
    SystemSpecs::default()
}

#[cfg(target_os = "macos")]
fn detect_specs_blocking() -> SystemSpecs {
    use std::process::Command;

    // Trim a command's stdout to a clean single-line String ("" on any failure).
    let run = |bin: &str, args: &[&str]| -> String {
        Command::new(bin)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };

    let chip = run("sysctl", &["-n", "machdep.cpu.brand_string"]);
    let macos_ver = run("sw_vers", &["-productVersion"]);
    let macos = if macos_ver.is_empty() {
        String::new()
    } else {
        format!("macOS {macos_ver}")
    };
    let cpu_cores: u32 = run("sysctl", &["-n", "hw.physicalcpu"])
        .parse()
        .unwrap_or(0);
    let mem_bytes: u64 = run("sysctl", &["-n", "hw.memsize"]).parse().unwrap_or(0);
    // hw.memsize is bytes of unified memory; divide by 1024³ (GiB) so a 16 GiB
    // Mac (17179869184 bytes) reads as 16 — dividing by 1e9 would round it to 17.
    let ram_gb = ((mem_bytes as f64) / 1_073_741_824.0).round() as u32;

    // GPU cores live in `system_profiler SPDisplaysDataType` as a
    // "Total Number of Cores: <n>" line on Apple Silicon.
    let gpu_cores = run("system_profiler", &["SPDisplaysDataType"])
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("Total Number of Cores:")
                .and_then(|n| n.trim().parse::<u32>().ok())
        })
        .unwrap_or(0);

    // Free space on the user-data volume. `df -k` prints 1 K-blocks; column 4 is
    // available. Parse the data row (the 2nd line of output).
    let free_disk_gb = run("df", &["-k", "/System/Volumes/Data"])
        .lines()
        .nth(1)
        .and_then(|row| row.split_whitespace().nth(3))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| ((kb as f64) * 1024.0 / 1e9).round() as u32)
        .unwrap_or(0);

    SystemSpecs {
        chip,
        macos,
        cpu_cores,
        gpu_cores,
        ram_gb,
        free_disk_gb,
    }
}
