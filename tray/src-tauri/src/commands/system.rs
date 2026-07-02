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

/// Open (or focus) the in-app dashboard window — the single-page Meridian
/// Timeline UI, served from Rust commands, no browser, no Node server. Opens
/// maximized so the app appears in the dock; switches activation policy to
/// Regular to support dock icon + window activation. Replaces the old
/// `open_in_browser(ui_base())` which pointed at localhost:3939 — the Node
/// server was retired in Stage 5. Points at the app root ("") — the old
/// "today" route was retired when the dashboard folded into one page.
///
/// Always dismisses the popover first (see [`dismiss_popover`]) — a
/// window-opening action and the popover being left on screen over the
/// window it just opened are mutually exclusive states, regardless of which
/// caller (popover, tray menu, notification click) triggered this.
#[tauri::command]
pub async fn open_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    dismiss_popover(&app);
    if let Some(win) = app.get_webview_window("dashboard") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    match WebviewWindowBuilder::new(&app, "dashboard", WebviewUrl::App("".into()))
        // Empty title bar text — the in-page Toolbar already shows the
        // Meridian mark + wordmark centered at the top, so a second
        // "Meridian — Dashboard" label in the OS title bar is redundant.
        .title("")
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
/// page to allow re-running setup from the dashboard, and from the popover's
/// own "Setup…" affordance. [`crate::tray::open_wizard_window`] dismisses the
/// popover itself — a no-op when called from the dashboard settings page,
/// where the popover is already hidden — so every caller (this command, the
/// native tray menu, the first-run auto-open) gets the fix for free.
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
/// Dismisses the popover first (see [`dismiss_popover`]) — a no-op when
/// called from the setup wizard, where the popover is already hidden.
#[tauri::command]
pub async fn open_permission_pane(app: tauri::AppHandle, pane: String) -> Result<(), String> {
    dismiss_popover(&app);
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

/// Quit the whole app — same exit path as the tray menu's "Quit Meridian".
/// Invoked from the popover footer's Quit button.
#[tracing::instrument(skip(app))]
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    tracing::info!("quit_app: user requested app exit");
    app.exit(0);
}

/// Hide the popover (main) window. Called from app.js on Escape keydown, and
/// internally by [`dismiss_popover`] — see that function for why any window-
/// opening popover action goes through it instead of relying on the caller to
/// remember to hide the popover itself.
#[tauri::command]
pub fn hide_popover(app: tauri::AppHandle) {
    dismiss_popover(&app);
}

/// Hide the popover if it's visible — a no-op otherwise (safe to call
/// unconditionally regardless of caller). Every command that opens a
/// separate window on the popover's behalf (dashboard, worklogs, a System
/// Settings pane) calls this itself, server-side, instead of trusting the
/// frontend to invoke a second "now hide yourself" command after the fact:
/// two independent `invoke()` calls from JS race the IPC round-trip with no
/// ordering guarantee, so a client-side "open, then hide" pattern can and did
/// leave the popover on screen. Doing it here makes the two atomic from the
/// caller's perspective and works for every future caller (tray menu,
/// notification click, …) without needing to repeat the client-side wiring.
///
/// `pub(crate)` so the native tray-menu openers in [`crate::tray`] — a second,
/// independent set of window-opening paths that never go through the `invoke`
/// commands above — can call it too instead of leaving the popover stuck
/// behind a window opened via a right-click menu item.
#[tracing::instrument(skip(app))]
pub(crate) fn dismiss_popover(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            tracing::debug!("dismiss_popover: hiding popover");
        }
        let _ = win.hide();
    }
}
