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

/// The cleanup working set (the ported /api/triage GET). Resolves `now` here
/// (so the core fn stays deterministic) to hide future-snoozed tickets. No
/// dashboard consumer today — ported for parity with the daemon's cleanup
/// engine; see meridian_core::triage.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_triage(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<meridian_core::triage::TriageResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::triage::get_triage(pool, &now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_triage failed");
            e.to_string()
        })
}

/// Sentinel returned to the UI when a password is stored — the real value never
/// leaves the daemon side. Matches ui/app/api/settings/route.ts.
const PASSWORD_SENTINEL: &str = "••••••••";

/// Runtime settings for the dashboard (the ported /api/settings GET). Reads
/// settings.json via the shared meridian-core reader, then matches the route's
/// response shaping: Option::None string fields → '' (TS consumers expect
/// strings, not null), and oo_password redacted to a sentinel. Read-only —
/// the PUT (write) route is ported later.
#[tauri::command]
#[tracing::instrument]
pub async fn get_settings() -> Result<serde_json::Value, String> {
    let s = meridian_core::settings::load_runtime_settings();
    let mut v = serde_json::to_value(&s).map_err(|e| e.to_string())?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "otlp_endpoint".into(),
            serde_json::json!(s.otlp_endpoint.clone().unwrap_or_default()),
        );
        obj.insert(
            "oo_email".into(),
            serde_json::json!(s.oo_email.clone().unwrap_or_default()),
        );
        let has_pw = s.oo_password.as_deref().is_some_and(|p| !p.is_empty());
        obj.insert(
            "oo_password".into(),
            serde_json::json!(if has_pw { PASSWORD_SENTINEL } else { "" }),
        );
    }
    Ok(v)
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

/// The dashboard's active-session view (the ported /api/active): the
/// active_session row reshaped with elapsed_s + parsed JSON columns. The pool is
/// opened ONCE at startup and shared as Tauri managed state (see `lib.rs`);
/// `None` means the DB couldn't be opened. Resolves `now` here so the core fn
/// stays deterministic.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_active(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Option<meridian_core::active::ActiveView>, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::active::get_active_view(pool, &now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_active failed");
            e.to_string()
        })
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
        .map_err(|e| {
            tracing::warn!(error = %e, "get_today failed");
            e.to_string()
        })
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
        .map_err(|e| {
            tracing::warn!(error = %e, "get_week failed");
            e.to_string()
        })
}

/// Today's coding-agent totals, computed in Rust (the ported /api/coding-agents).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_coding_agents(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<meridian_core::coding_agents::CodingAgentsResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = meridian_core::date::today_string();
    meridian_core::coding_agents::get_coding_agents(pool, &date)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_coding_agents failed");
            e.to_string()
        })
}

/// A day's worklogs for review, computed in Rust (the ported /api/worklogs).
/// `day` defaults to today (local) when omitted, matching the route.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_worklogs(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<meridian_core::worklogs::WorklogsResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let day = day.unwrap_or_else(meridian_core::date::today_string);
    meridian_core::worklogs::get_worklogs(pool, &day)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_worklogs failed");
            e.to_string()
        })
}

/// Per-task time + board hygiene, computed in Rust (the ported /api/tasks).
/// Resolves today, the 7-day window start, and now here so the core fn stays
/// deterministic/testable (mirrors get_today).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_tasks(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<meridian_core::tasks::TasksResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let today = meridian_core::date::today_string();
    // Local date 6 days ago (matches the route's `Date.now() - 6 days`).
    let week_start = (chrono::Local::now() - chrono::Duration::days(6))
        .format("%Y-%m-%d")
        .to_string();
    let now_iso = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    meridian_core::tasks::get_tasks(pool, &today, &week_start, &now_iso)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_tasks failed");
            e.to_string()
        })
}
