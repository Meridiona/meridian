//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray menu: its single source-of-truth builder, the menu-event dispatch,
//! and the window/browser openers the menu items trigger.
//!
//! Extracted from `lib.rs` so the app bootstrap stays a thin wiring file. The
//! builder is the ONLY place item ids/labels live, so the poll loop's
//! health-driven rebuild ([`crate::poll`]) can't drift out of sync.
//!
//! # Related
//! - [`crate::commands::system`] — the same open-actions exposed as Tauri commands.
//! - [`crate::commands::daemon`] — `toggle_daemon`, invoked by the toggle menu item.

use crate::state::{AppState, HealthStatus};
use crate::sys;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuBuilder, MenuItemBuilder},
    Manager, Runtime, WebviewUrl, WebviewWindowBuilder,
};

/// The toggle item's label for a given daemon health. Kept next to the menu
/// builder so the label and the menu never disagree.
pub(crate) fn toggle_label(health: &HealthStatus) -> &'static str {
    match health {
        HealthStatus::Healthy => "Connected ●",
        HealthStatus::Unhealthy | HealthStatus::Unknown => "Disconnected ○",
    }
}

/// Build the full tray menu. The single definition of the tray's items —
/// called from `setup()` at startup AND from the poll loop when health flips
/// ([`crate::poll`]). Only the toggle label is health-dependent; everything
/// else is constant. Adding a menu item here keeps both call sites in sync.
pub(crate) fn build_tray_menu<R: Runtime>(
    app: &tauri::AppHandle<R>,
    health: &HealthStatus,
) -> tauri::Result<Menu<R>> {
    let toggle_item = MenuItemBuilder::with_id("toggle_daemon", toggle_label(health)).build(app)?;
    // Opens the in-app dashboard window (no browser, no Node server).
    let dashboard_item = MenuItemBuilder::with_id("open_dashboard", "Open Dashboard").build(app)?;
    // First-run / re-run onboarding wizard (permissions, model, tracker auth).
    let setup_item = MenuItemBuilder::with_id("open_setup", "Setup…").build(app)?;
    let worklogs_item = MenuItemBuilder::with_id("open_worklogs", "Review Drafts").build(app)?;
    let restart_item = MenuItemBuilder::with_id("restart_daemon", "Restart Daemon").build(app)?;
    // DMG auto-update (handled by tauri-plugin-updater). A no-op toast in a
    // source/dev run; the real swap+relaunch only happens for a packaged `.app`.
    let update_item = MenuItemBuilder::with_id("check_updates", "Check for Updates…").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit Meridian").build(app)?;
    MenuBuilder::new(app)
        .items(&[
            &toggle_item,
            &dashboard_item,
            &setup_item,
            &worklogs_item,
            &restart_item,
            &update_item,
            &quit_item,
        ])
        .build()
}

/// Dispatch a tray menu click by item id. Pulls any state it needs from `app`
/// (so it stays a free function, not a closure capturing the world).
pub(crate) fn handle_menu_event(app: &tauri::AppHandle, id: &str) {
    match id {
        "open_dashboard" => open_native_dashboard(app),
        "open_setup" => open_wizard_window(app),
        // Review Drafts: open the dashboard (user navigates to Worklogs from there).
        "open_worklogs" => open_native_dashboard(app),
        "toggle_daemon" => toggle_from_menu(app),
        "restart_daemon" => restart_from_menu(),
        "check_updates" => crate::update::check_for_updates(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

/// In-app dashboard window (Today/Week from Rust). Reuse the window if it
/// already exists, else build it against the Next `today` route.
fn open_native_dashboard(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("dashboard") {
        let _ = win.show();
        let _ = win.set_focus();
    } else if let Err(e) = WebviewWindowBuilder::new(
        app,
        "dashboard",
        // Resolves against devUrl → next dev in dev, the static export in build.
        WebviewUrl::App("today".into()),
    )
    .title("Meridian — Dashboard")
    .inner_size(1100.0, 760.0)
    .decorations(true)
    .resizable(true)
    .maximizable(true)
    .minimizable(true)
    .closable(true)
    .build()
    {
        eprintln!("tray: failed to open native dashboard: {e}");
    }
}

/// Toggle the daemon from the menu: snapshot health, then spawn the async
/// `toggle_daemon` command (which also fires the pause/resume toast).
fn toggle_from_menu(app: &tauri::AppHandle) {
    if let Ok(state_guard) = app.state::<Arc<Mutex<AppState>>>().lock() {
        let is_running = state_guard.health == HealthStatus::Healthy;
        drop(state_guard);
        let app_for_notify = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::commands::toggle_daemon(app_for_notify, is_running).await;
        });
    }
}

/// Restart the daemon from the menu via `launchctl kickstart -k`.
fn restart_from_menu() {
    let uid = sys::uid_str();
    let _ = std::process::Command::new("launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("gui/{}/com.meridiona.daemon", uid),
        ])
        .spawn();
}

/// Open (or focus) the in-app onboarding wizard window. Loads the Next `/setup`
/// route; the wizard drives permissions, model status, and tracker auth entirely
/// through Tauri commands (no Node server). `pub(crate)` so `lib.rs` can call it
/// for the first-run auto-open.
pub(crate) fn open_wizard_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("setup") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    // The wizard renders the "A · Rail" shell (a 948×628 onboarding window with a
    // left step rail); size the host window to fit it with a little breathing room.
    if let Err(e) = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("setup".into()))
        .title("Meridian — Setup")
        .inner_size(980.0, 660.0)
        .resizable(false)
        .build()
    {
        eprintln!("tray: failed to open setup wizard: {e}");
    }
}
