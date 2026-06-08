// meridian — normalises screenpipe activity into structured app sessions
use crate::state::{AppState, StatusPayload};
use std::sync::{Arc, Mutex};
use tauri::State;
use tauri_plugin_opener::OpenerExt;

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
        .open_url("http://localhost:3939", None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_worklogs(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url("http://localhost:3939/worklogs", None::<&str>)
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

fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}
