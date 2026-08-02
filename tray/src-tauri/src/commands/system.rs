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
        // "Meridian - Dashboard" label in the OS title bar is redundant.
        .title("")
        .inner_size(1100.0, 760.0)
        .decorations(true)
        .resizable(true)
        .maximizable(true)
        .minimizable(true)
        .closable(true)
        .maximized(true)
        .zoom_hotkeys_enabled(true)
        .build()
    {
        Ok(win) => {
            // Revert to Accessory (no dock icon) when the dashboard is closed
            // so the tray-only UX is restored.
            crate::sys::revert_to_accessory_on_close(&app, &win);
            // Clicking back into an already-open dashboard while the popover
            // happens to be open on top of it wouldn't otherwise dismiss the
            // popover — see dismiss_popover_on_focus's doc comment.
            dismiss_popover_on_focus(&app, &win);
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

/// Resize the setup wizard window's client area (a no-op if the window isn't
/// open). The wizard's card is a different fixed height on the Welcome screen
/// than in the step flow (see `ui/app/setup/page.tsx`) - the window was
/// previously one static size for both, which left a big empty backdrop
/// margin around the shorter Welcome card. The frontend calls this once, when
/// leaving Welcome for step 1, to grow the window back to the step-flow size;
/// [`crate::tray::open_wizard_window`] opens it small (sized for Welcome) in
/// the first place. Also raises the min size to match, so the user can't
/// resize the window smaller than whichever card is currently showing.
#[tauri::command]
pub async fn resize_setup_window(
    app: tauri::AppHandle,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let Some(win) = app.get_webview_window("setup") else {
        return Ok(());
    };
    win.set_min_size(Some(tauri::LogicalSize::new(width, height)))
        .map_err(|e| e.to_string())?;
    win.set_size(tauri::LogicalSize::new(width, height))
        .map_err(|e| e.to_string())
}

/// Fetch-and-clear the pending dashboard navigation target (e.g. "/plan") —
/// the pull half of [`crate::deep_link`]. Called once by
/// `MeridianTimelineShell` on mount; `None` on a plain open. Infallible today,
/// but returns `Result` like its sibling commands so the bridge's error path
/// stays uniform if a failure mode appears.
#[tracing::instrument(skip(app))]
#[tauri::command]
pub async fn take_pending_deep_link(app: tauri::AppHandle) -> Result<Option<String>, String> {
    Ok(crate::deep_link::take_pending(&app))
}

/// Deep-link straight to the OS's notification/privacy settings pane. `pane`
/// is one of the wizard's known keys; anything else is rejected so the
/// frontend can't open an arbitrary URL. We always offer this button
/// regardless of current grant state — the user may need to fix a revoked
/// permission too. Dismisses the popover (see [`dismiss_popover`]) once
/// `pane` is known valid — a no-op when called from the setup wizard, where
/// the popover is already hidden. Validated before dismissing rather than
/// after: an unknown `pane` used to dismiss the popover and then return `Err`
/// with no settings pane opened, losing the popover for nothing.
///
/// Only `"notifications"` is reachable on Windows: `check_accessibility` /
/// `check_screen_recording` always report `true` there (no TCC analogue —
/// see the Cargo.toml note on `cidre`), so the wizard's grant button for
/// those two never renders, and this command never receives their pane keys.
#[tauri::command]
pub async fn open_permission_pane(app: tauri::AppHandle, pane: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let url = permission_pane_url(&pane)?;
    #[cfg(not(target_os = "windows"))]
    let url = permission_pane_url(&pane, &app.config().identifier)?;
    dismiss_popover(&app);
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Resolve a wizard pane key to its OS settings URL — the pure half of
/// [`open_permission_pane`], factored out so the mapping is unit-testable
/// without a live `AppHandle`.
///
/// Windows has no per-app deep link into the notification pane the way
/// macOS's `?id=<bundle-id>` does — `ms-settings:notifications` opens the
/// general Notifications & actions page, which lists Meridian once it has
/// delivered its first toast under its AUMID (see
/// `sys::windows_notification_setting`). It's also the only pane reachable
/// here in practice: `check_accessibility` / `check_screen_recording` always
/// report `true` on Windows (no TCC analogue — see the Cargo.toml note on
/// `cidre`), so the wizard's grant button for those two never renders.
#[cfg(target_os = "windows")]
fn permission_pane_url(pane: &str) -> Result<String, String> {
    match pane {
        "notifications" => Ok("ms-settings:notifications".to_string()),
        other => Err(format!("unknown permission pane: {other}")),
    }
}

/// macOS pane resolution — `identifier` is the app's bundle id, needed only
/// by the `"notifications"` case (see the doc comment inline below).
#[cfg(not(target_os = "windows"))]
fn permission_pane_url(pane: &str, identifier: &str) -> Result<String, String> {
    match pane {
        "screen_recording" => Ok(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
                .to_string(),
        ),
        "accessibility" => Ok(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
                .to_string(),
        ),
        "input_monitoring" => Ok(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
                .to_string(),
        ),
        // Notification authorization lives under Notifications, not Privacy &
        // Security. Anchored to our own bundle id so this opens Meridian's
        // specific notification detail pane (Allow toggle, Alert Style, …)
        // instead of just the app list — verified working via the
        // `?id=<bundle-id>` param, which the pane's own extension metadata
        // (`NotificationsSettings.appex`'s `allowsXAppleSystemPreferencesURLScheme`)
        // supports even though it's otherwise undocumented. This is the deny
        // recovery path: macOS shows the authorization dialog exactly once, so
        // after a deny the wizard can only send the user here.
        "notifications" => Ok(format!(
            "x-apple.systempreferences:com.apple.preference.notifications?id={identifier}"
        )),
        other => Err(format!("unknown permission pane: {other}")),
    }
}

/// Open an external URL with its default OS handler — browser for `http(s)`,
/// mail client for `mailto:`, dialer for `tel:`.
///
/// Tauri's webview does NOT elevate a plain `<a target="_blank">` click (or a
/// mailto:/tel: anchor) to a system open (no `WKUIDelegate`/`createWebViewWith`
/// handling is wired up) — the click is silently swallowed. This is the one
/// JS-callable path to `tauri_plugin_opener`'s `open_url`; anchors route here
/// via the global `ExternalLinks` interceptor and "Open in tracker ↗" buttons
/// via `openExternal` in `@/lib/bridge`.
///
/// Scheme-allowlisted to `http`/`https`/`mailto`/`tel` (all within
/// `opener:default`'s `allow-default-urls` scope) — task URLs come from
/// tracker API responses (Jira/Linear/GitHub/Azure DevOps/Trello), so this is
/// a system boundary: reject anything else rather than handing an arbitrary
/// scheme (`file://`, `javascript:`, …) to the OS opener.
#[tauri::command]
pub async fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !is_openable_url(&url) {
        return Err(format!(
            "refusing to open URL with disallowed scheme: {url}"
        ));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// `true` for `http://`/`https://`/`mailto:`/`tel:` URLs only (schemes are
/// case-insensitive per RFC 3986). Extracted as a pure fn purely so the
/// scheme allowlist is unit-testable without a `tauri::AppHandle`.
fn is_openable_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
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

/// Wires `win` to dismiss the popover whenever it gains focus. Call once,
/// right after building any window OTHER than the popover itself (`dashboard`,
/// `setup`, …).
///
/// `dismiss_popover` at open-time (above) only covers the moment a window is
/// FIRST opened. It does not cover the case where that window was already
/// open in the background, the popover is reopened on top of it, and the user
/// then clicks back into the already-open window: `install_click_outside_monitor`'s
/// global `NSEvent` monitor never fires for that click because
/// `addGlobalMonitorForEventsMatchingMask:` only fires for events delivered to
/// OTHER processes, not clicks landing on one of our own windows (see that
/// function's doc comment in `lib.rs`). `Focused(true)` on the other window is
/// the one signal that does fire for an in-app click, so it's the dismiss hook.
pub(crate) fn dismiss_popover_on_focus(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Focused(true) = event {
            dismiss_popover(&app_handle);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_openable_url_allows_http_https_mailto_tel() {
        assert!(is_openable_url("https://linear.app/x/issue/ENG-12"));
        assert!(is_openable_url("http://localhost:5080"));
        assert!(is_openable_url("mailto:hey@meridiona.com?subject=Bug"));
        assert!(is_openable_url("tel:+15550100"));
        // Schemes are case-insensitive (RFC 3986).
        assert!(is_openable_url("MAILTO:hey@meridiona.com"));
        assert!(is_openable_url("HTTPS://trello.com/app-key"));
    }

    #[test]
    fn is_openable_url_rejects_other_schemes() {
        assert!(!is_openable_url("file:///etc/passwd"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url(
            "x-apple.systempreferences:com.apple.preference.security"
        ));
        assert!(!is_openable_url(""));
        // Multibyte content must not panic the scheme check.
        assert!(!is_openable_url("héllo→"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_notifications_pane_opens_the_general_settings_page() {
        assert_eq!(
            permission_pane_url("notifications").unwrap(),
            "ms-settings:notifications"
        );
    }

    /// Accessibility/Screen Recording never reach this command in practice on
    /// Windows (see the doc comment on `permission_pane_url`), but the
    /// rejection must still be graceful rather than panicking if the
    /// frontend ever sends one.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_rejects_macos_only_panes() {
        assert!(permission_pane_url("accessibility").is_err());
        assert!(permission_pane_url("screen_recording").is_err());
        assert!(permission_pane_url("bogus").is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn macos_notifications_pane_is_anchored_to_the_bundle_id() {
        assert_eq!(
            permission_pane_url("notifications", "com.meridiona.meridian").unwrap(),
            "x-apple.systempreferences:com.apple.preference.notifications?id=com.meridiona.meridian"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn macos_known_panes_resolve_without_the_bundle_id() {
        assert_eq!(
            permission_pane_url("screen_recording", "com.meridiona.meridian").unwrap(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        );
        assert_eq!(
            permission_pane_url("accessibility", "com.meridiona.meridian").unwrap(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        );
        assert_eq!(
            permission_pane_url("input_monitoring", "com.meridiona.meridian").unwrap(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn macos_unknown_pane_is_rejected() {
        assert!(permission_pane_url("bogus", "com.meridiona.meridian").is_err());
    }
}
