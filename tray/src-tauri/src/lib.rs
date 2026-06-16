//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
mod commands;
pub(crate) mod format;
mod poll;
mod state;

use state::AppState;
use std::sync::{Arc, Mutex};
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};

pub fn run() {
    let app_state = Arc::new(Mutex::new(AppState::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(app_state.clone())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Request OS notification authorization up front. Without this,
            // `.show()` is a silent no-op on macOS until the app has prompted at
            // least once — the reason the health/pause toasts never appeared.
            {
                use tauri_plugin_notification::{NotificationExt, PermissionState};
                let notifier = app.notification();
                if !matches!(notifier.permission_state(), Ok(PermissionState::Granted)) {
                    let _ = notifier.request_permission();
                }
            }

            let toggle_item =
                MenuItemBuilder::with_id("toggle_daemon", "Connected ●").build(app)?;
            let open_item =
                MenuItemBuilder::with_id("open_dashboard", "Open Dashboard").build(app)?;
            let worklogs_item =
                MenuItemBuilder::with_id("open_worklogs", "Review Drafts").build(app)?;
            let restart_item =
                MenuItemBuilder::with_id("restart_daemon", "Restart Daemon").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit Meridian Tray").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[
                    &toggle_item,
                    &open_item,
                    &worklogs_item,
                    &restart_item,
                    &quit_item,
                ])
                .build()?;

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
        ])
        .run(tauri::generate_context!())
        .expect("error running meridian tray");
}

fn ui_base() -> String {
    let port = std::env::var("MERIDIAN_UI_PORT").unwrap_or_else(|_| "3939".to_string());
    format!("http://127.0.0.1:{}", port)
}

fn open_in_browser(app: &tauri::AppHandle, url: &str) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_url(url, None::<&str>);
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
