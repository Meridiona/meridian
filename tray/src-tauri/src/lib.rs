// meridian — normalises screenpipe activity into structured app sessions
mod commands;
pub(crate) mod format;
mod poll;
mod state;

use state::AppState;
use std::sync::{Arc, Mutex};
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use tauri_plugin_positioner::{Position, WindowExt};

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

            let open_item = MenuItemBuilder::with_id("open_dashboard", "Open Dashboard").build(app)?;
            let worklogs_item = MenuItemBuilder::with_id("open_worklogs", "Review Drafts").build(app)?;
            let restart_item = MenuItemBuilder::with_id("restart_daemon", "Restart Daemon").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit Meridian Tray").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&open_item, &worklogs_item, &restart_item, &quit_item])
                .build()?;

            let tray_icon_bytes = include_bytes!("../icons/tray.png");
            let tray_icon = Image::from_bytes(tray_icon_bytes)?;

            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(tray_icon)
                .tooltip("Meridian")
                .on_tray_icon_event(|tray_handle, event| {
                    tauri_plugin_positioner::on_tray_event(tray_handle.app_handle(), &event);
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray_handle.app_handle();
                        toggle_popover(app);
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open_dashboard" => {
                        open_in_browser(app, "http://localhost:3939");
                    }
                    "open_worklogs" => {
                        open_in_browser(app, "http://localhost:3939/worklogs");
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
        ])
        .run(tauri::generate_context!())
        .expect("error running meridian tray");
}

fn toggle_popover(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.move_window(Position::TrayBottomCenter);
        let visible = window.is_visible().unwrap_or(false);
        if visible {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
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
