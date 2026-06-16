//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
mod commands;
pub(crate) mod format;
mod poll;
mod state;

use state::{AppState, HealthStatus};
use std::sync::{Arc, Mutex};
use tauri::{
    image::Image,
    menu::{Menu, MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager, Runtime, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

pub fn run() {
    // Dev-only (`--features otel`): export tray spans to OpenObserve via the
    // daemon's OTLP setup, tagged service.name = meridian-tray. Held for the
    // process lifetime. Compiled out entirely (and `meridian` isn't even a dep)
    // when the feature is off — release builds stay lean.
    //
    // Must run INSIDE Tauri's Tokio runtime: the OTLP batch exporter spawns a
    // background task and panics ("no reactor running") if called before one
    // exists. `block_on` enters the global runtime so the spawn succeeds; the
    // exporter task then lives on that runtime for the process lifetime.
    #[cfg(feature = "otel")]
    let _otel_guard =
        tauri::async_runtime::block_on(async { meridian::observability::init("meridian-tray") })
            .ok();

    let app_state = Arc::new(Mutex::new(AppState::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(app_state.clone())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Open meridian.db ONCE at startup and share it with commands via
            // managed state (no migrations — the daemon owns the schema). `None`
            // if the DB can't be opened yet, so get_active errors gracefully
            // instead of crashing the tray.
            let db_path = commands::meridian_db_path();
            let db_pool = tauri::async_runtime::block_on(meridian_core::open_existing(&db_path))
                .map_err(|e| eprintln!("tray: meridian.db not opened ({db_path}): {e}"))
                .ok();
            app.manage(db_pool);

            // Single source of truth for the tray menu — `build_tray_menu` is the
            // ONLY place item ids/labels live, so the poll loop's health-driven
            // rebuild (poll.rs) can't drift out of sync and silently drop items
            // (it previously clobbered this 6-item menu with a stale 5-item one).
            // Initial health is Unknown until the first poll resolves it.
            let menu = build_tray_menu(app.handle(), &HealthStatus::Unknown)?;

            let tray_icon_bytes = include_bytes!("../icons/meridiona-mark.png");
            let tray_icon = Image::from_bytes(tray_icon_bytes)?;

            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(true)
                .icon(tray_icon)
                .tooltip("Meridian")
                .on_tray_icon_event(|tray_handle, event| {
                    tauri_plugin_positioner::on_tray_event(tray_handle.app_handle(), &event);
                })
                .on_menu_event(|app, event| {
                    let app_clone = app.clone();
                    match event.id.as_ref() {
                        "open_dashboard" => {
                            open_in_browser(app, &ui_base());
                        }
                        "open_native" => {
                            // In-app dashboard window (Today/Week from Rust). Reuse
                            // the window if it already exists, else build it.
                            if let Some(win) = app.get_webview_window("dashboard") {
                                let _ = win.show();
                                let _ = win.set_focus();
                            } else if let Err(e) = WebviewWindowBuilder::new(
                                app,
                                "dashboard",
                                // The real Next dashboard route (resolves against
                                // devUrl → next dev in dev, the static export in
                                // build). Replaces the throwaway dashboard.html.
                                WebviewUrl::App("today".into()),
                            )
                            .title("Meridian — Dashboard")
                            .inner_size(1100.0, 760.0)
                            .build()
                            {
                                eprintln!("tray: failed to open native dashboard: {e}");
                            }
                        }
                        "open_setup" => {
                            open_wizard_window(app);
                        }
                        "open_worklogs" => {
                            open_in_browser(app, &format!("{}/worklogs", ui_base()));
                        }
                        "toggle_daemon" => {
                            if let Ok(state_guard) =
                                app_clone.state::<Arc<Mutex<AppState>>>().lock()
                            {
                                let is_running =
                                    state_guard.health == crate::state::HealthStatus::Healthy;
                                drop(state_guard);
                                let app_for_notify = app_clone.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ =
                                        commands::toggle_daemon(app_for_notify, is_running).await;
                                });
                            }
                        }
                        "restart_daemon" => {
                            let uid = uid_str();
                            let _ = std::process::Command::new("launchctl")
                                .args([
                                    "kickstart",
                                    "-k",
                                    &format!("gui/{}/com.meridiona.daemon", uid),
                                ])
                                .spawn();
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .build(app)?;

            {
                let mut s = app_state.lock().unwrap();
                s.tray_id = Some(tray.id().clone());
            }

            // Hide popover on focus loss.
            let main_win = app.get_webview_window("main").unwrap();
            main_win.on_window_event({
                let win = main_win.clone();
                move |event| {
                    if let WindowEvent::Focused(false) = event {
                        let _ = win.hide();
                    }
                }
            });

            let app_handle = app.handle().clone();
            let state_clone = app_state.clone();
            tauri::async_runtime::spawn(async move {
                poll::run_poll_loop(app_handle, state_clone).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::open_dashboard,
            commands::open_worklogs,
            commands::restart_daemon,
            commands::toggle_daemon,
            commands::get_active,
            commands::get_today,
            commands::get_week,
            commands::get_coding_agents,
            commands::get_worklogs,
            commands::get_tasks,
            commands::open_permission_pane,
        ])
        .run(tauri::generate_context!())
        .expect("error running meridian tray");
}

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
/// (poll.rs::update_toggle_menu). Only the toggle label is health-dependent;
/// everything else is constant. Adding a menu item here automatically keeps
/// both call sites in sync.
pub(crate) fn build_tray_menu<R: Runtime>(
    app: &tauri::AppHandle<R>,
    health: &HealthStatus,
) -> tauri::Result<Menu<R>> {
    let toggle_item = MenuItemBuilder::with_id("toggle_daemon", toggle_label(health)).build(app)?;
    let open_item = MenuItemBuilder::with_id("open_dashboard", "Open Dashboard").build(app)?;
    // Native (in-app) dashboard — renders Today/Week from Rust commands, no
    // browser, no Node server. The fold-into-Tauri end-state.
    let native_item =
        MenuItemBuilder::with_id("open_native", "Open Dashboard (native)").build(app)?;
    // First-run / re-run onboarding wizard (permissions, model, tracker auth).
    let setup_item = MenuItemBuilder::with_id("open_setup", "Setup…").build(app)?;
    let worklogs_item = MenuItemBuilder::with_id("open_worklogs", "Review Drafts").build(app)?;
    let restart_item = MenuItemBuilder::with_id("restart_daemon", "Restart Daemon").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit Meridian Tray").build(app)?;
    MenuBuilder::new(app)
        .items(&[
            &toggle_item,
            &open_item,
            &native_item,
            &setup_item,
            &worklogs_item,
            &restart_item,
            &quit_item,
        ])
        .build()
}

fn ui_base() -> String {
    let port = std::env::var("MERIDIAN_UI_PORT").unwrap_or_else(|_| "3939".to_string());
    format!("http://127.0.0.1:{}", port)
}

fn open_in_browser(app: &tauri::AppHandle, url: &str) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_url(url, None::<&str>);
}

/// Open (or focus) the in-app onboarding wizard window. Loads the Next `/setup`
/// route (resolves against devUrl → next dev in dev, the static export in
/// build); the wizard drives permissions, model status, and tracker auth
/// entirely through Tauri commands (no Node server).
fn open_wizard_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("setup") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    if let Err(e) = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("setup".into()))
        .title("Meridian — Setup")
        .inner_size(560.0, 660.0)
        .resizable(false)
        .build()
    {
        eprintln!("tray: failed to open setup wizard: {e}");
    }
}

fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}
