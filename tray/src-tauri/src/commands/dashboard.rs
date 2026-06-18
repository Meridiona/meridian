//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Dashboard read commands — the ported `/api/*` DB reads.
//!
//! Each command is a thin wrapper that resolves request-scoped values (today /
//! now / the week window) and delegates to the matching [`meridian_core`] reader,
//! keeping the core deterministic and unit-testable. The shared `meridian.db`
//! pool is opened once at startup and handed in as Tauri managed state (`None`
//! when the DB couldn't be opened, so reads error gracefully rather than panic).
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by the dashboard via
//! `ui/lib/bridge.ts::load`.
//!
//! # Related
//! - [`crate::commands::daemon`] — daemon lifecycle + status (non-DB).
//! - [`meridian_core::readers`] — the byte-for-byte route ports these delegate to.

use tauri::State;

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

/// The dashboard's active-session view (the ported /api/active): the
/// active_session row reshaped with elapsed_s + parsed JSON columns. Resolves
/// `now` here so the core fn stays deterministic.
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

/// Full detail for one board ticket (the ported /api/plan/task). `key` is the
/// task key; resolves "today" (local) here for the deterministic due_days math.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_task_detail(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    key: String,
) -> Result<Option<meridian_core::task_detail::TaskDetail>, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let today = chrono::Local::now().date_naive();
    meridian_core::task_detail::get_task_detail(pool, &key, today)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_task_detail failed");
            e.to_string()
        })
}

/// The daily plan board (the ported /api/plan GET): the committed set + ranked
/// suggestions + the full scored board. Resolves "today" (local), "now" (epoch
/// ms + the recent-work lookback bound) here so the core scoring stays
/// deterministic/testable. `date` defaults to today when omitted/garbage (the
/// route's read-side `readDate` coercion).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_plan(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    date: Option<String>,
) -> Result<meridian_core::plan::PlanResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = read_date(date);
    let (today, now_ms, recent_since) = plan_clock();
    let available = meridian_core::plan::build_available(pool, &date, today, now_ms, &recent_since)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_plan: build_available failed");
            e.to_string()
        })?;
    meridian_core::plan::build_plan_response(pool, &date, today, available)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_plan failed");
            e.to_string()
        })
}

/// A daily-plan write (the ported /api/plan POST): one of confirm/set/add/
/// remove/reorder/skip/reopen, returning the freshly-scored plan response. The
/// whole body is one payload object (`PlanBody`) so the Tauri and browser paths
/// send one identical snake_case shape (avoids the per-arg camelCase rename).
#[tauri::command]
#[tracing::instrument(skip(pool, body), fields(action = %body.action))]
pub async fn plan_action(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: meridian_core::plan::PlanBody,
) -> Result<meridian_core::plan::PlanResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    // Writes must reject a malformed EXPLICIT date (defaulting it to today would
    // mutate the WRONG day); an absent date → today. Mirrors the route's writeDate.
    let date = match write_date(body.date.as_deref()) {
        Some(d) => d,
        None => return Err("invalid date (expected YYYY-MM-DD)".to_string()),
    };
    let (today, now_ms, recent_since) = plan_clock();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let available = meridian_core::plan::build_available(pool, &date, today, now_ms, &recent_since)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "plan_action: build_available failed");
            e.to_string()
        })?;
    meridian_core::plan::apply_plan_action(pool, &body, &date, today, &now, available)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "plan_action failed");
            e.to_string()
        })
}

/// `YYYY-MM-DD` validation shared by the plan read/write date coercions.
fn is_iso_date(d: &str) -> bool {
    let b = d.as_bytes();
    d.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Read-side date coercion (the route's `readDate`): absent/garbage → today.
fn read_date(d: Option<String>) -> String {
    match d {
        Some(d) if is_iso_date(&d) => d,
        _ => meridian_core::date::today_string(),
    }
}

/// Write-side date coercion (the route's `writeDate`): absent → today, malformed
/// explicit → `None` (the caller 400s rather than mutating the wrong day).
fn write_date(d: Option<&str>) -> Option<String> {
    match d {
        None => Some(meridian_core::date::today_string()),
        Some(d) if is_iso_date(d) => Some(d.to_string()),
        Some(_) => None,
    }
}

/// Resolve the plan's request-scoped clock: (local `today`, `now` epoch-ms,
/// local `recent_since` day = `now − RECENT_WORK_DAYS`). Mirrors the TS use of
/// `new Date()` / `Date.now()` across the scoring path.
fn plan_clock() -> (chrono::NaiveDate, i64, String) {
    let now_local = chrono::Local::now();
    let today = now_local.date_naive();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let recent_since = (now_local - chrono::Duration::days(meridian_core::plan::RECENT_WORK_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    (today, now_ms, recent_since)
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
