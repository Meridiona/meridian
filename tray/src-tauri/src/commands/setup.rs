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

/// Response shape for the wizard's Model step poll.
#[derive(Debug, Serialize)]
pub struct MlxStatusResponse {
    /// Current server status.
    pub status: MlxStatus,
    /// Port the server listens on (7823).
    pub port: u16,
    /// Whether a resolvable Python binary was found on this machine.
    pub runtime_found: bool,
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

/// Returns `true` when screen capture appears available.
///
/// **Current proxy:** checks if a `screenpipe` process is running, since
/// screenpipe is the current capture process that holds Screen Recording
/// permission. When the B-in-process track lands this switches to
/// `CGPreflightScreenCaptureAccess()` on the tray process itself.
#[tauri::command]
#[tracing::instrument]
pub async fn check_screen_recording() -> bool {
    // Primary: live screenpipe process?
    if let Ok(out) = tokio::process::Command::new("pgrep")
        .args(["-f", "screenpipe"])
        .output()
        .await
    {
        if out.status.success() && !out.stdout.is_empty() {
            return true;
        }
    }
    // Fallback: screenpipe DB present — was running at some point and likely has
    // permission. Better than blocking the wizard on a clean install where the
    // user granted permission but restarted screenpipe since.
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&format!("{home}/.screenpipe/db.sqlite")).exists()
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
    tracing::debug!(status = ?m.status, runtime_found, "mlx: status queried");
    Ok(MlxStatusResponse {
        status: m.status.clone(),
        port: m.port,
        runtime_found,
    })
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
