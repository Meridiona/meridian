//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! OS / window action commands — open native windows and deep-link into System Settings.
//!
//! These don't touch the DB or the daemon; they drive the OS shell or open
//! in-app Tauri windows on the user's behalf.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; invoked from the popover (`app.js`)
//! and the dashboard UI.
//!
//! # Related
//! - [`crate::tray`] — the tray menu also opens these targets (same native window path).

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;

/// Open (or focus) the in-app dashboard window (Today/Week from Rust commands,
/// no browser, no Node server). Opens maximized so the app appears in the dock;
/// switches activation policy to Regular to support dock icon + window activation.
/// Replaces the old `open_in_browser(ui_base())` which pointed at localhost:3939
/// — the Node server was retired in Stage 5.
#[tauri::command]
pub async fn open_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("dashboard") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    match WebviewWindowBuilder::new(&app, "dashboard", WebviewUrl::App("today".into()))
        .title("Meridian — Dashboard")
        .inner_size(1100.0, 760.0)
        .decorations(true)
        .resizable(true)
        .maximizable(true)
        .minimizable(true)
        .closable(true)
        .maximized(true)
        .build()
    {
        Ok(win) => {
            // Revert to Accessory (no dock icon) when the dashboard is closed
            // so the tray-only UX is restored.
            let app_handle = app.clone();
            win.on_window_event(move |event| {
                if let WindowEvent::Destroyed = event {
                    #[cfg(target_os = "macos")]
                    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            });
            Ok(())
        }
        Err(e) => {
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            Err(e.to_string())
        }
    }
}

/// Open (or focus) the in-app dashboard window and navigate to the Worklogs
/// view. The user arrives on Today; the dashboard nav takes them to Worklogs.
/// Replaces the old `open_in_browser(worklogs_url)` — the Node server is gone.
#[tauri::command]
pub async fn open_worklogs(app: tauri::AppHandle) -> Result<(), String> {
    // Reuse the dashboard window; the user navigates to Worklogs from there.
    open_dashboard(app).await
}

/// Open (or focus) the in-app onboarding setup wizard window. Loads the Next
/// `/setup` route; the wizard drives permissions, model status, and tracker
/// auth entirely through Tauri commands (no Node server). Called from settings
/// page to allow re-running setup from the dashboard.
#[tracing::instrument(skip(app))]
#[tauri::command]
pub async fn open_setup(app: tauri::AppHandle) -> Result<(), String> {
    crate::tray::open_wizard_window(&app);
    Ok(())
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
        "input_monitoring" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        other => return Err(format!("unknown permission pane: {other}")),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open an arbitrary `http(s)` URL in the user's default system browser.
///
/// Tauri's webview does NOT elevate a plain `<a target="_blank">` click to a
/// system-browser open (no `WKUIDelegate`/`createWebViewWith` handling is
/// wired up) — the click is silently swallowed. This is the one JS-callable
/// path to `tauri_plugin_opener`'s `open_url`; every "Open in tracker ↗" link
/// across the dashboard must route through it via `openExternal` in
/// `@/lib/bridge` rather than relying on the anchor's native navigation.
///
/// Scheme-restricted to `http`/`https` — task URLs come from tracker API
/// responses (Jira/Linear/GitHub/Azure DevOps/Trello), so this is a system
/// boundary: reject anything that isn't a normal web link rather than handing
/// an arbitrary scheme (`file://`, `javascript:`, …) to the OS opener.
#[tauri::command]
pub async fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !is_http_url(&url) {
        return Err(format!("refusing to open non-http(s) URL: {url}"));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// `true` for `http://`/`https://` URLs only. Extracted as a pure fn purely so
/// the scheme allowlist is unit-testable without a `tauri::AppHandle`.
fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Quit the whole app — same exit path as the tray menu's "Quit Meridian".
/// Invoked from the popover footer's Quit button.
#[tracing::instrument(skip(app))]
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    tracing::info!("quit_app: user requested app exit");
    app.exit(0);
}

/// Hide the popover (main) window. Called from app.js on Escape keydown.
/// The popover runs as a non-activating NSPanel on macOS so Focused(false)
/// never fires — Escape is the keyboard dismiss path.
#[tauri::command]
pub fn hide_popover(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_http_url_allows_http_and_https() {
        assert!(is_http_url("https://linear.app/x/issue/ENG-12"));
        assert!(is_http_url("http://localhost:5080"));
    }

    #[test]
    fn is_http_url_rejects_other_schemes() {
        assert!(!is_http_url("file:///etc/passwd"));
        assert!(!is_http_url("javascript:alert(1)"));
        assert!(!is_http_url(
            "x-apple.systempreferences:com.apple.preference.security"
        ));
        assert!(!is_http_url(""));
    }
}
