//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! OS / window action commands — open URLs and deep-link into System Settings.
//!
//! These don't touch the DB or the daemon; they just drive the OS shell on the
//! user's behalf (open the dashboard in a browser, open a privacy pane).
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; invoked from the popover/settings UI.
//!
//! # Related
//! - [`crate::sys::ui_base`] — the dashboard base URL these open.
//! - [`crate::tray`] — the tray menu also opens these targets (native window path).

use crate::sys::ui_base;
use tauri_plugin_opener::OpenerExt;

/// Open the dashboard home in the user's default browser.
#[tauri::command]
pub async fn open_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(ui_base(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open the worklog-review page in the user's default browser.
#[tauri::command]
pub async fn open_worklogs(app: tauri::AppHandle) -> Result<(), String> {
    let url = format!("{}/worklogs", ui_base());
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Deep-link straight to a macOS privacy pane in System Settings. `pane` is
/// one of the wizard's known keys; anything else is rejected so the frontend
/// can't open an arbitrary URL. We always offer this button regardless of
/// current grant state — the user may need to fix a revoked permission too.
#[tauri::command]
pub async fn open_permission_pane(app: tauri::AppHandle, pane: String) -> Result<(), String> {
    let url = match pane.as_str() {
        "screen_recording" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        other => return Err(format!("unknown permission pane: {other}")),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}
