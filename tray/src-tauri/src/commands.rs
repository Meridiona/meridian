// meridian — normalises screenpipe activity into structured app sessions
use crate::state::{AppState, StatusPayload};
use std::sync::{Arc, Mutex};
use tauri::State;
use tauri_plugin_opener::OpenerExt;

fn ui_base() -> String {
    let port = std::env::var("MERIDIAN_UI_PORT").unwrap_or_else(|_| "3939".to_string());
    format!("http://127.0.0.1:{}", port)
}

#[tauri::command]
pub fn get_status(state: State<'_, Arc<Mutex<AppState>>>) -> Result<StatusPayload, String> {
    state
        .lock()
        .map(|s| s.to_payload())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(&ui_base(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_worklogs(app: tauri::AppHandle) -> Result<(), String> {
    let url = format!("{}/worklogs", ui_base());
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    let uid = uid_str();
    let status = std::process::Command::new("launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("gui/{}/com.meridiona.daemon", uid),
        ])
        .status()
        .map_err(|e| format!("launchctl failed: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err("launchctl kickstart returned non-zero".to_string())
    }
}

#[tauri::command]
pub async fn toggle_daemon(app: tauri::AppHandle, is_running: bool) -> Result<(), String> {
    let uid = uid_str();
    let service = format!("gui/{}/com.meridiona.daemon", uid);

    let status = if is_running {
        std::process::Command::new("launchctl")
            .args(["stop", &service])
            .status()
    } else {
        std::process::Command::new("launchctl")
            .args(["start", &service])
            .status()
    }
    .map_err(|e| format!("launchctl failed: {}", e))?;

    if status.success() {
        let (title, body) = if is_running {
            ("Paused", "Meridian is paused. Click to resume.")
        } else {
            ("Resumed", "Meridian is back tracking.")
        };
        notify_user(&app, title, body);
        Ok(())
    } else {
        Err(format!(
            "launchctl {} returned non-zero",
            if is_running { "stop" } else { "start" }
        ))
    }
}

fn notify_user(app: &tauri::AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
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
