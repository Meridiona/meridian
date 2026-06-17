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
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    meridian_core::triage::get_triage(pool, &now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_triage failed");
            e.to_string()
        })
}

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
