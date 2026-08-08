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
        .map_err(|e| crate::cmd_err!(e, "get_active failed"))
}

/// The Today dashboard payload, computed entirely in Rust (the ported
/// /api/today). `day` defaults to today (local) when omitted, matching
/// [`crate::commands::get_worklogs`] — the timeline's Overview panel passes
/// the currently-viewed day so Focus/Time-by-app track the selected date
/// instead of always the real "today".
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_today(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<meridian_core::today::TodayResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = day.unwrap_or_else(meridian_core::date::today_string);
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::today::get_today(pool, &date, &now)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_today failed"))
}

/// The day's inferred day-level tasks (`day_tasks`, migration 058 — no route).
/// `day` defaults to today (local) when omitted, matching [`get_today`]; the
/// timeline passes the currently-viewed day so its task spans track the selected
/// date. The worklog pipeline folds each hour into these 1-5 tasks.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_day_tasks(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<meridian_core::day_tasks::DayTasksResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = day.unwrap_or_else(meridian_core::date::today_string);
    meridian_core::day_tasks::get_day_tasks(pool, &date)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_day_tasks failed"))
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
        .map_err(|e| crate::cmd_err!(e, "get_week failed"))
}

/// A day's coding-agent totals, computed in Rust (the ported /api/coding-agents).
/// `day` defaults to today (local) when omitted, matching [`get_today`] and
/// [`get_day_tasks`].
///
/// The core reader was always day-parameterised; only this wrapper pinned it to
/// today. That mattered once the day-summary gained day navigation: agent
/// sessions are excluded from `get_today`'s `sessions`, so `TimeByApp` folds
/// these totals back in — a hard-coded today here silently under-reported a past
/// day's agent time rather than failing visibly.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_coding_agents(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<meridian_core::coding_agents::CodingAgentsResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = day.unwrap_or_else(meridian_core::date::today_string);
    meridian_core::coding_agents::get_coding_agents(pool, &date)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_coding_agents failed"))
}

/// The apps Meridian has captured recently — the picker source for Settings →
/// Capture & Privacy → "Ignore an app". Last 30 days, most-seen first, capped at
/// 60. No day param (this is a settings affordance, not a timeline view).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_recent_capture_apps(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Vec<meridian_core::capture_apps::CaptureApp>, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    meridian_core::capture_apps::recent_capture_apps(pool, 30, 60)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_recent_capture_apps failed"))
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
        .map_err(|e| crate::cmd_err!(e, "get_task_detail failed"))
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
        .map_err(|e| crate::cmd_err!(e, "get_plan: build_available failed"))?;
    meridian_core::plan::build_plan_response(pool, &date, today, available)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_plan failed"))
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
        .map_err(|e| crate::cmd_err!(e, "plan_action: build_available failed"))?;
    meridian_core::plan::apply_plan_action(pool, &body, &date, today, &now, available)
        .await
        .map_err(|e| crate::cmd_err!(e, "plan_action failed"))
}

/// One ticket the worklog panel's manual picker can retarget a draft at.
/// Serialized shape for [`meridian_core::board::BoardTask`], which is a plain
/// struct in the core (no `Serialize` — the core stays wire-agnostic).
#[derive(serde::Serialize)]
pub struct BoardTicket {
    pub task_key: String,
    pub provider: String,
    pub title: String,
    pub issue_type: String,
    pub epic_title: String,
}

/// Every open ticket on the board, for the worklog panel's "match to a different
/// ticket" picker.
///
/// The matcher itself never sees this list — it only compares against the day's
/// planned tasks ([`meridian_core::plan::load_plan_candidates`]). This is the
/// human override for when the work wasn't on the plan, or the model bound it to
/// the wrong task.
///
/// `postable_only = false`: personal tasks (`provider = 'local'`) are valid picks,
/// because a match onto one is written to its own row rather than posted (see
/// `meridian::pm_worklog::generate::post_to_local_task`). The board's open-only
/// scope is otherwise kept — Done tickets are NOT pulled in here; a focus task
/// that gets marked Done is handled where the plan lives, not on the whole board.
///
/// Deliberately uncapped. The old ">40 tickets, refuse" gate existed because a
/// long list degrades a PROMPT; a searchable list a person reads has no such
/// ceiling.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_board_tickets(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Vec<BoardTicket>, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let rows = meridian_core::board::fetch_open_board(pool, false)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_board_tickets failed"))?;
    tracing::info!(tickets = rows.len(), "serving the board ticket picker");
    Ok(rows
        .into_iter()
        .map(|t| BoardTicket {
            task_key: t.task_key,
            provider: t.provider,
            title: t.title,
            issue_type: t.issue_type,
            epic_title: t.epic_title,
        })
        .collect())
}

/// The body of a [`retarget_day_task_worklog`] call.
#[derive(Debug, serde::Deserialize)]
pub struct RetargetBody {
    pub day: String,
    pub task_id: String,
    pub task_key: String,
}

/// Point a drafted worklog at a ticket the user picked themselves.
///
/// A direct core call, not a CLI shell-out like `generate_day_task_worklog`:
/// that one shells out because tracker auth and the LLM provider live in the
/// daemon, and this touches neither — it's a DB write, so it returns instantly
/// instead of paying a process spawn to rewrite prose that was already correct.
///
/// The provider comes from `pm_tasks`, never from the caller: the frontend must
/// not be able to name the tracker a comment gets posted to.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn retarget_day_task_worklog(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: RetargetBody,
) -> Result<meridian_core::day_task_worklogs::DayTaskWorklogDraft, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let provider = meridian_core::board::provider_for_key(pool, &body.task_key)
        .await
        .map_err(|e| crate::cmd_err!(e, "retarget: provider lookup failed"))?;
    let Some(provider) = provider else {
        return Err("that ticket is not on your board - try again after a sync".to_string());
    };
    // A personal task ('local') is a valid manual pick: approve writes the update
    // onto the task's own row rather than posting a comment (there is no tracker
    // thread), via the daemon's post_to_local_task. So no refusal here - the
    // provider carried on the target tells approve which path to take.

    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    meridian_core::day_task_worklogs::retarget_draft(
        pool,
        &body.day,
        &body.task_id,
        &body.task_key,
        &provider,
        &now,
    )
    .await
    .map_err(|e| crate::cmd_err!(e, "retarget_day_task_worklog failed"))
}

/// The body of a [`set_worklog_provider`] call.
#[derive(Debug, serde::Deserialize)]
pub struct SetWorklogProviderBody {
    pub day: String,
    pub task_id: String,
    /// The tracker to create the proposed ticket on. VALIDATED, never trusted.
    pub provider: String,
}

/// Choose which tracker a proposed worklog ticket gets created on.
///
/// Only reachable for the propose branch of a draft, where the provider was picked
/// for the user as "the first configured tracker" - arbitrary for anyone on two
/// boards. A matched draft carries its provider per-target and the core call refuses
/// there.
///
/// **The provider is validated against the connected trackers before it is written.**
/// [`retarget_day_task_worklog`] states the rule this obeys: the frontend must not be
/// able to name an arbitrary tracker for a comment to be posted to. It cannot come
/// from `pm_tasks` here (a proposed ticket does not exist yet), so the check is
/// against [`crate::commands::connected_providers`] instead - the same credential
/// probe `get_integrations` shows the user. An id that isn't connected is refused
/// rather than written and left to fail later inside the approve.
#[tauri::command]
#[tracing::instrument(skip(pool), err)]
pub async fn set_worklog_provider(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: SetWorklogProviderBody,
) -> Result<meridian_core::day_task_worklogs::DayTaskWorklogDraft, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    if !crate::commands::connected_providers().contains(&body.provider.as_str()) {
        tracing::warn!(provider = %body.provider, "set_worklog_provider: unconnected tracker refused");
        return Err(format!(
            "{} isn't connected - connect it in Settings first",
            body.provider
        ));
    }

    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    meridian_core::day_task_worklogs::set_draft_provider(
        pool,
        &body.day,
        &body.task_id,
        &body.provider,
        &now,
    )
    .await
    .map_err(|e| crate::cmd_err!(e, "set_worklog_provider failed"))
}

/// The body of a [`dismiss_worklog_target`] call.
#[derive(Debug, serde::Deserialize)]
pub struct DismissTargetBody {
    pub day: String,
    pub task_id: String,
    pub task_key: String,
}

/// Drop ONE ticket from a drafted worklog's target set, keeping the rest.
///
/// The AI can match several planned tasks at once; this is the per-ticket "no, not
/// that one" — cheaper than a regenerate and it preserves the written update. Like
/// [`retarget_day_task_worklog`] it's a direct core call rather than a CLI
/// shell-out: no tracker auth, no model, just a DB write.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn dismiss_worklog_target(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: DismissTargetBody,
) -> Result<meridian_core::day_task_worklogs::DayTaskWorklogDraft, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    meridian_core::day_task_worklogs::dismiss_target(pool, &body.day, &body.task_id, &body.task_key)
        .await
        .map_err(|e| crate::cmd_err!(e, "dismiss_worklog_target failed"))
}

/// The body of a [`dismiss_day_task`] / [`restore_day_task`] call.
#[derive(Debug, serde::Deserialize)]
pub struct DayTaskCorrectionBody {
    pub day: String,
    pub task_id: String,
}

/// The body of a [`merge_day_task`] call.
#[derive(Debug, serde::Deserialize)]
pub struct MergeDayTaskBody {
    pub day: String,
    pub task_id: String,
    pub into_task_id: String,
}

/// Dismiss an inferred day-task (timeline workstream): hide it and keep it hidden
/// across the hourly re-fold. Returns the day's tasks as they now stand, so the
/// timeline column re-renders without a second round trip.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn dismiss_day_task(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: DayTaskCorrectionBody,
) -> Result<meridian_core::day_tasks::DayTasksResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::day_task_corrections::dismiss_day_task(pool, &body.day, &body.task_id, &now)
        .await
        .map_err(|e| crate::cmd_err!(e, "dismiss_day_task failed"))?;
    meridian_core::day_tasks::get_day_tasks(pool, &body.day)
        .await
        .map_err(|e| crate::cmd_err!(e, "dismiss_day_task: task-list refresh failed"))
}

/// Merge one inferred day-task into another (`task_id` folds into `into_task_id`).
/// The merged task's time and summary move onto the target and it disappears; the
/// correction persists so the fold never re-splits it. Returns the fresh task list.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn merge_day_task(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: MergeDayTaskBody,
) -> Result<meridian_core::day_tasks::DayTasksResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::day_task_corrections::merge_day_task(
        pool,
        &body.day,
        &body.task_id,
        &body.into_task_id,
        &now,
    )
    .await
    .map_err(|e| crate::cmd_err!(e, "merge_day_task failed"))?;
    meridian_core::day_tasks::get_day_tasks(pool, &body.day)
        .await
        .map_err(|e| crate::cmd_err!(e, "merge_day_task: task-list refresh failed"))
}

/// Undo a day-task dismiss (un-hide it). Returns the fresh task list.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn restore_day_task(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: DayTaskCorrectionBody,
) -> Result<meridian_core::day_tasks::DayTasksResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339();
    meridian_core::day_task_corrections::restore_day_task(pool, &body.day, &body.task_id, &now)
        .await
        .map_err(|e| crate::cmd_err!(e, "restore_day_task failed"))?;
    meridian_core::day_tasks::get_day_tasks(pool, &body.day)
        .await
        .map_err(|e| crate::cmd_err!(e, "restore_day_task: task-list refresh failed"))
}

/// The user closed the Plan modal. Restarts the plan-nudge hold-back clock:
/// re-stamps `~/.meridian/plan_auto_opened` with now, so the daemon's
/// "Plan your day" reminder fires one hour after the DISMISSAL (not the
/// auto-open) when the plan is still unconfirmed. A no-op unless the marker
/// already records today — a manual planner open/close on a day the
/// auto-open didn't fire must not suppress the nudge
/// ([`meridian_core::plan_marker::restamp_if_today`] owns that guard).
/// Fire-and-forget from the shell; never errors.
#[tauri::command]
#[tracing::instrument]
pub async fn plan_dismissed() {
    let Some(home) = meridian_core::paths::home_dir() else {
        return;
    };
    let marker = meridian_core::plan_marker::marker_path(&home.join(".meridian"));
    if meridian_core::plan_marker::restamp_if_today(&marker, &chrono::Local::now()) {
        tracing::info!("plan_dismissed: nudge hold-back clock restarted");
    }
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
        .map_err(|e| crate::cmd_err!(e, "get_tasks failed"))
}
