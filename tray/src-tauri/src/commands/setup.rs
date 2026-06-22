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
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    // Safety: preflight is a pure status read — no prompt, no side effects.
    unsafe { CGPreflightScreenCaptureAccess() }
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
