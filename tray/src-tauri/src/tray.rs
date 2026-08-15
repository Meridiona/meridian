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
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
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
    // Opens the in-app uninstall wizard — the safe way to tear Meridian down
    // (stops every launchd agent, offers to remove data plus any legacy
    // runtime/models, and points at System Settings for the permission grants
    // deleting the app never revokes). Separated from the rest of the menu
    // since it's a destructive action, same grouping convention as Quit.
    let uninstall_item =
        MenuItemBuilder::with_id("open_uninstall", "Uninstall Meridian…").build(app)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit Meridian").build(app)?;
    MenuBuilder::new(app)
        .items(&[
            &toggle_item,
            &dashboard_item,
            &setup_item,
            &worklogs_item,
            &restart_item,
            &update_item,
            &separator,
            &uninstall_item,
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
        "open_uninstall" => open_uninstall_window(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

/// In-app dashboard window (Today/Week from Rust). Reuse the window if it
/// already exists, else build it against the Next `today` route. Opens
/// maximized so the app appears in the dock; switches activation policy to
/// Regular to support dock icon + window activation.
///
/// Dismisses the popover first (see [`crate::commands::system::dismiss_popover`])
/// — this is the native tray-menu path (right-click → Open Dashboard / Review
/// Drafts), independent of the popover's own `invoke('open_dashboard')` button,
/// so it needs the same fix to avoid leaving the popover stuck on screen.
pub(crate) fn open_native_dashboard(app: &tauri::AppHandle) {
    // SETUP COMES FIRST ON A FRESH INSTALL.
    //
    // The wizard auto-opens 800 ms after launch, but nothing stopped the user
    // reaching the dashboard before or instead of it — the tray menu's "Open
    // Dashboard" and "Review Drafts" are both live from the first second, and
    // the tray icon is the most obvious thing on screen while the wizard is
    // still coming up.
    //
    // What they got was the real timeline against a database with no capture
    // permissions granted, no AI provider, and no tracker: an empty product that
    // looks broken rather than unconfigured, on the one screen that decides
    // whether they keep it.
    //
    // Redirect rather than refuse. The wizard IS what they wanted — a way into
    // the app — so open it; a disabled menu item or a silent no-op would just
    // read as the app being broken instead.
    if !crate::onboarding_complete() {
        tracing::info!("dashboard requested before onboarding finished; opening the wizard");
        open_wizard_window(app);
        return;
    }
    crate::commands::system::dismiss_popover(app);
    if let Some(win) = app.get_webview_window("dashboard") {
        let _ = win.show();
        let _ = win.set_focus();
    } else {
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        // The dashboard is now a single page (Meridian Timeline one-pager) —
        // the old "today" route was retired in the timeline migration.
        match WebviewWindowBuilder::new(app, "dashboard", WebviewUrl::App("".into()))
            .title("Meridian - Dashboard")
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
                // Fills the screen, then enters native full-screen. The
                // builder's `.maximized(true)` above is a silent no-op on
                // macOS — see `sys::open_full_screen` for the tao/AppKit
                // early-return behind that, and for the resize hazard the
                // full-screen style mask carries.
                crate::sys::open_full_screen(&win);
                // Revert to Accessory (no dock icon) when the dashboard is closed
                // so the tray-only UX is restored.
                crate::sys::revert_to_accessory_on_close(app, &win);
            }
            Err(e) => {
                eprintln!("tray: failed to open native dashboard: {e}");
                #[cfg(target_os = "macos")]
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
        }
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
            let db_pool = app_for_notify.state::<Option<meridian_core::SqlitePool>>();
            let _ =
                crate::commands::toggle_daemon(app_for_notify.clone(), is_running, db_pool).await;
        });
    }
}

/// Restart the daemon from the menu.
///
/// Routes through [`crate::commands::daemon_control::restart`] (launchd on
/// macOS, the scheduled task on Windows) rather than shelling `launchctl`
/// directly — the direct call would silently do nothing on Windows. Spawned
/// onto the async runtime because the menu handler is synchronous; the result
/// is best-effort, matching the previous fire-and-forget behaviour.
fn restart_from_menu() {
    tauri::async_runtime::spawn(async {
        if let Err(e) = crate::commands::daemon_control::restart().await {
            tracing::warn!(error = %e, "menu restart_daemon failed");
        }
    });
}

/// Open (or focus) the in-app onboarding wizard window. Loads the Next `/setup`
/// route; the wizard drives permissions, model status, and tracker auth entirely
/// through Tauri commands (no Node server). `pub(crate)` so `lib.rs` can call it
/// for the first-run auto-open.
///
/// Dismisses the popover first (see [`crate::commands::system::dismiss_popover`])
/// — the single implementation shared by the popover's "Setup…" button, the
/// native tray-menu "Setup…" item, and the first-run auto-open, so every
/// caller gets the fix without repeating the call itself.
pub(crate) fn open_wizard_window(app: &tauri::AppHandle) {
    crate::commands::system::dismiss_popover(app);
    if let Some(win) = app.get_webview_window("setup") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    // The wizard's first screen (Welcome) renders a shorter 948×520 card than
    // the step flow's 948×628 (`ui/app/setup/page.tsx`) — open the window
    // sized for Welcome so it doesn't start with a big empty backdrop margin;
    // the frontend calls `resize_setup_window` to grow it back once the user
    // leaves Welcome for step 1 (`onBegin` in page.tsx). Resizable (with a
    // floor at whichever card is currently showing) so the user can still grow
    // it or enter macOS full-screen — the card stays centred and the backdrop
    // just widens, so the layout never breaks.
    let builder = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("setup".into()))
        .title("Meridian - Setup")
        .inner_size(1000.0, 572.0)
        .min_inner_size(1000.0, 572.0)
        .resizable(true)
        .zoom_hotkeys_enabled(true);
    // Transparent title bar so the webview fills the *whole* window and the
    // centred card gets equal backdrop margins on all four sides. With a
    // normal (opaque) title bar the bar sits above the webview, so the top
    // gap reads larger than the sides/bottom. The size above is chosen so the
    // 948×520 Welcome card keeps ~26px margins all round — enough clearance
    // for the overlaid traffic lights + title to sit in the top backdrop, not
    // on the card.
    //
    // Applied as a separate statement rather than inline in the chain because
    // both `title_bar_style` and `tauri::TitleBarStyle` are macOS-only in Tauri
    // — a `#[cfg]` on the method call alone would still leave the unresolvable
    // type in a Windows build. Off macOS the window keeps its standard title
    // bar; since `inner_size` sizes the CLIENT area, the webview is still
    // 1000×572 and the card still centres within it — only the outer window
    // grows by the title-bar height. No layout change, just a taller frame.
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Transparent);
    match builder.build() {
        Ok(win) => {
            // Opt the window into native full-screen so the green traffic-light
            // shows the enter-full-screen arrows, not the plain zoom (+) glyph.
            // make_fullscreenable touches the raw NSWindow directly (no Tauri-safe
            // wrapper around it), so it must run on the main thread — most callers
            // of open_wizard_window are already on it (tray menu clicks, dock
            // reopen are native AppKit callbacks), but the first-launch auto-open
            // task (lib.rs) calls in from a tokio worker thread after its 800 ms
            // sleep, and AppKit hard-aborts the process (SIGTRAP) if an NSWindow is
            // touched off the main thread. run_on_main_thread queues the call
            // rather than blocking, so it's safe from any caller thread.
            #[cfg(target_os = "macos")]
            {
                let fs_win = win.clone();
                if let Err(e) = app.run_on_main_thread(move || make_fullscreenable(&fs_win)) {
                    tracing::warn!(error = %e, "failed to dispatch make_fullscreenable to main thread");
                }
            }
            // Same gap as the dashboard window — see dismiss_popover_on_focus's
            // doc comment (clicking back into an already-open setup window
            // wouldn't otherwise dismiss a popover reopened on top of it).
            crate::commands::system::dismiss_popover_on_focus(app, &win);
        }
        Err(e) => eprintln!("tray: failed to open setup wizard: {e}"),
    }
}

/// macOS: opt a window into native full-screen. Tauri leaves
/// `NSWindowCollectionBehaviorFullScreenPrimary` off even for a resizable window,
/// so the green traffic-light button falls back to zoom (`+`) instead of the
/// enter-full-screen arrows. OR the flag onto the raw `NSWindow` to fix it.
#[cfg(target_os = "macos")]
fn make_fullscreenable(win: &tauri::WebviewWindow) {
    use objc2::{msg_send, runtime::AnyObject};

    let ptr = match win.ns_window() {
        Ok(p) if !p.is_null() => p as *mut AnyObject,
        _ => {
            tracing::warn!(label = %win.label(), "make_fullscreenable: ns_window unavailable");
            return;
        }
    };
    // NSWindowCollectionBehaviorFullScreenPrimary = 1 << 7.
    const FULLSCREEN_PRIMARY: usize = 1 << 7;
    // Safety: caller (open_wizard_window) must run this on the main thread —
    // AppKit aborts the process if an NSWindow is touched off it. Standard
    // NSWindow selectors, no ownership transfer.
    unsafe {
        let ns = &*ptr;
        let current: usize = msg_send![ns, collectionBehavior];
        let _: () = msg_send![ns, setCollectionBehavior: current | FULLSCREEN_PRIMARY];
        tracing::info!(
            label = %win.label(),
            behavior = current | FULLSCREEN_PRIMARY,
            "make_fullscreenable: enabled native full-screen"
        );
    }
}

/// Open (or focus) the in-app uninstall wizard window. Loads the Next
/// `/uninstall` route; the wizard drives the plan/execute flow entirely
/// through Tauri commands (`commands::uninstall`), same as the setup wizard.
///
/// Dismisses the popover first (see
/// [`crate::commands::system::dismiss_popover`]) — the native tray-menu
/// "Uninstall Meridian…" item is currently this window's only opener, but it
/// follows the same convention as every other opener in this file so a future
/// caller (a Settings-page "Uninstall…" button) gets the fix for free.
pub(crate) fn open_uninstall_window(app: &tauri::AppHandle) {
    crate::commands::system::dismiss_popover(app);
    if let Some(win) = app.get_webview_window("uninstall") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    if let Err(e) = WebviewWindowBuilder::new(app, "uninstall", WebviewUrl::App("uninstall".into()))
        .title("Meridian - Uninstall")
        .inner_size(720.0, 620.0)
        .resizable(false)
        .zoom_hotkeys_enabled(true)
        .build()
    {
        eprintln!("tray: failed to open uninstall wizard: {e}");
    }
}
