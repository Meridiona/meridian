//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
        .open_url(ui_base(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_worklogs(app: tauri::AppHandle) -> Result<(), String> {
    let url = format!("{}/worklogs", ui_base());
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())
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
        other => return Err(format!("unknown permission pane: {other}")),
    };
    app.opener()
        .open_url(url, None::<&str>)
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
    let _ = app.notification().builder().title(title).body(body).show();
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

/// Resolve meridian.db: `MERIDIAN_DB` env, else `~/.meridian/meridian.db`.
/// (Production should reuse `meridian::config` for full .env + `~` handling.)
pub(crate) fn meridian_db_path() -> String {
    if let Ok(p) = std::env::var("MERIDIAN_DB") {
        return p;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/.meridian/meridian.db", home)
}

/// Read the live active session from meridian.db via the daemon's own query
/// layer. The pool is opened ONCE at startup and shared as Tauri managed state
/// (see `lib.rs`); `None` means the DB couldn't be opened at startup. This is
/// the template every ported dashboard read route follows.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_active(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Option<meridian_core::ActiveSession>, String> {
    match pool.inner() {
        Some(pool) => meridian_core::get_active_session(pool)
            .await
            .map_err(|e| e.to_string()),
        None => Err("meridian.db is not open yet".to_string()),
    }
}

/// The Today dashboard payload, computed entirely in Rust (the ported
/// /api/today). Resolves "today" (local) + "now" here so the core fn stays
/// deterministic/testable.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_today(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<meridian_core::today::TodayResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = meridian_core::date::today_string();
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::today::get_today(pool, &date, &now)
        .await
        .map_err(|e| e.to_string())
}

/// The 7-day Week summary, computed in Rust (the ported /api/week).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_week(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<meridian_core::week::WeekResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::week::get_week(pool, &now)
        .await
        .map_err(|e| e.to_string())
}
