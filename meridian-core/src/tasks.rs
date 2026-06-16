//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/tasks` ported to Rust — a faithful port of `ui/app/api/tasks/route.ts`.
//!
//! Per-task time accounting: YOUR hands-on foreground time on a task + the
//! agent time that ran while you were AWAY (autonomous). Agent time alongside
//! you (supervised) is NOT added — that wall-clock is already your presence, so
//! adding it would double-count the day. Hence we need your full foreground
//! presence to split autonomous from supervised agent time. Plus board-hygiene
//! verdicts the daemon wrote into pm_task_curation (tolerating older DBs).

use crate::date::local_day_bounds;
use crate::hygiene::{parse_issues, Hygiene};
use crate::intervals::{
    intersect_seconds, merge_intervals, session_interval, union_seconds, Interval,
};
use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use sqlx::FromRow;
use std::collections::BTreeMap;
use tracing::Instrument;

#[derive(Debug, Clone, Serialize)]
pub struct TaskSummary {
    pub key: String,
    pub title: String,
    pub description: String,
    pub issue_type: String,
    pub status: String,
    pub is_terminal: bool,
    pub provider: String,
    pub url: String,
    pub epic_key: Option<String>,
    pub epic_title: Option<String>,
    pub due_date: Option<String>,
    pub start_date: Option<String>,
    pub today_s: i64,
    pub today_autonomous_s: i64,
    pub week_s: i64,
    pub session_count: i64,
    pub cats: BTreeMap<String, i64>,
    pub hygiene: Option<Hygiene>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TasksResponse {
    pub tasks: Vec<TaskSummary>,
    pub unassigned_s: i64,
}

#[derive(FromRow)]
struct TaskRow {
    task_key: String,
    title: Option<String>,
    description_text: Option<String>,
    issue_type: String,
    status_raw: Option<String>,
    is_terminal: Option<i64>,
    provider: Option<String>,
    url: Option<String>,
    parent_key: Option<String>,
    epic_title: Option<String>,
    due_date: Option<String>,
    start_date: Option<String>,
}

#[derive(FromRow, Clone)]
struct SessionRow {
    started_at: String,
    ended_at: Option<String>,
    duration_s: i64,
    claude_session_uuid: Option<String>,
    category: Option<String>,
    task_key: String,
}

struct TaskTime {
    autonomous_s: i64,
    total_s: i64,
}

/// YOUR foreground time on the task + autonomous agent time (agent intervals
/// outside your presence). Mirrors the route's `taskTime`.
fn task_time(rows: &[SessionRow], presence: &[Interval]) -> TaskTime {
    let fg: Vec<Interval> = rows
        .iter()
        .filter(|r| r.claude_session_uuid.is_none())
        .map(|r| {
            session_interval(
                &r.started_at,
                r.ended_at.as_deref().unwrap_or(""),
                r.duration_s,
                None,
            )
        })
        .collect();
    let agent: Vec<Interval> = rows
        .iter()
        .filter(|r| r.claude_session_uuid.is_some())
        .map(|r| {
            session_interval(
                &r.started_at,
                r.ended_at.as_deref().unwrap_or(""),
                r.duration_s,
                r.claude_session_uuid.as_deref(),
            )
        })
        .collect();
    let your_s = union_seconds(&fg);
    let autonomous_s = (union_seconds(&agent) - intersect_seconds(&agent, presence)).max(0);
    TaskTime {
        autonomous_s,
        total_s: your_s + autonomous_s,
    }
}

struct TodayAgg {
    dur: i64,
    autonomous_s: i64,
    sessions: i64,
    cats: BTreeMap<String, i64>,
}

#[tracing::instrument(skip(pool))]
pub async fn get_tasks(
    pool: &SqlitePool,
    today: &str,
    week_start: &str,
    now_iso: &str,
) -> anyhow::Result<TasksResponse> {
    let (today_start, today_end) = local_day_bounds(today);
    let (ws, _) = local_day_bounds(week_start);

    // All tasks.
    let task_rows: Vec<TaskRow> = sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT task_key, title, description_text, COALESCE(issue_type,'') AS issue_type,
               status_raw, is_terminal, provider, url, parent_key, epic_title, due_date, start_date
        FROM pm_tasks
        ORDER BY task_key DESC
        "#,
    )
    .fetch_all(pool)
    .instrument(tracing::debug_span!("tasks.read.pm_tasks"))
    .await
    .context("tasks: fetch pm_tasks")?;
    tracing::debug!(rows = task_rows.len(), "tasks.read.pm_tasks");

    // Board-hygiene verdicts (pm_task_curation, migration 038; ignored_codes 040).
    let hygiene_by_key = load_hygiene(pool, now_iso).await?;

    // Foreground presence (every foreground session, task or not) — raw spans.
    let presence_rows = |start: String, end: String| async move {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as::<_, (String, Option<String>)>(
            r#"
                SELECT started_at, ended_at FROM app_sessions
                WHERE started_at >= ? AND started_at < ? AND claude_session_uuid IS NULL
                "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(pool)
        .await?;
        Ok::<Vec<Interval>, sqlx::Error>(
            rows.into_iter()
                .map(|(s, e)| Interval {
                    started_at: s,
                    ended_at: e.unwrap_or_default(),
                })
                .collect(),
        )
    };
    let today_presence_raw = presence_rows(today_start.clone(), today_end.clone())
        .instrument(tracing::debug_span!("tasks.read.presence", scope = "today"))
        .await
        .context("tasks: today presence")?;
    tracing::debug!(
        rows = today_presence_raw.len(),
        scope = "today",
        "tasks.read.presence"
    );
    let today_presence = merge_intervals(&today_presence_raw);

    let week_presence_raw = presence_rows(ws.clone(), today_end.clone())
        .instrument(tracing::debug_span!("tasks.read.presence", scope = "week"))
        .await
        .context("tasks: week presence")?;
    tracing::debug!(
        rows = week_presence_raw.len(),
        scope = "week",
        "tasks.read.presence"
    );
    let week_presence = merge_intervals(&week_presence_raw);

    // Task sessions (task_session_type='task', task_key NOT NULL).
    let task_sessions = |start: String, end: String| async move {
        sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT started_at, ended_at, duration_s, claude_session_uuid, category, task_key
            FROM app_sessions
            WHERE started_at >= ? AND started_at < ?
              AND task_session_type = 'task' AND task_key IS NOT NULL
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(pool)
        .await
    };
    let today_sessions = task_sessions(today_start.clone(), today_end.clone())
        .instrument(tracing::debug_span!("tasks.read.sessions", scope = "today"))
        .await
        .context("tasks: today sessions")?;
    tracing::debug!(
        rows = today_sessions.len(),
        scope = "today",
        "tasks.read.sessions"
    );
    let week_sessions = task_sessions(ws.clone(), today_end.clone())
        .instrument(tracing::debug_span!("tasks.read.sessions", scope = "week"))
        .await
        .context("tasks: week sessions")?;
    tracing::debug!(
        rows = week_sessions.len(),
        scope = "week",
        "tasks.read.sessions"
    );

    // Group by task_key.
    let mut today_by_task_rows: BTreeMap<String, Vec<SessionRow>> = BTreeMap::new();
    for s in &today_sessions {
        today_by_task_rows
            .entry(s.task_key.clone())
            .or_default()
            .push(s.clone());
    }
    let mut week_by_task_rows: BTreeMap<String, Vec<SessionRow>> = BTreeMap::new();
    for s in &week_sessions {
        week_by_task_rows
            .entry(s.task_key.clone())
            .or_default()
            .push(s.clone());
    }

    let mut today_by_task: BTreeMap<String, TodayAgg> = BTreeMap::new();
    for (k, rows) in &today_by_task_rows {
        // Category split is the FOREGROUND share only.
        let mut cats: BTreeMap<String, i64> = BTreeMap::new();
        let mut fg_count = 0i64;
        for r in rows.iter().filter(|r| r.claude_session_uuid.is_none()) {
            fg_count += 1;
            let cat = r
                .category
                .clone()
                .unwrap_or_else(|| "idle_personal".to_string());
            *cats.entry(cat).or_insert(0) += r.duration_s;
        }
        let t = task_time(rows, &today_presence);
        today_by_task.insert(
            k.clone(),
            TodayAgg {
                dur: t.total_s,
                autonomous_s: t.autonomous_s,
                sessions: fg_count,
                cats,
            },
        );
    }

    let mut week_by_task: BTreeMap<String, i64> = BTreeMap::new();
    for (k, rows) in &week_by_task_rows {
        week_by_task.insert(k.clone(), task_time(rows, &week_presence).total_s);
    }

    // Unassigned today.
    let unassigned: (i64,) = sqlx::query_as::<_, (i64,)>(
        r#"
        SELECT COALESCE(SUM(duration_s), 0) FROM app_sessions
        WHERE started_at >= ? AND started_at < ?
          AND (task_method IS NULL OR task_session_type = 'overhead')
        "#,
    )
    .bind(&today_start)
    .bind(&today_end)
    .fetch_one(pool)
    .instrument(tracing::debug_span!("tasks.read.unassigned"))
    .await
    .context("tasks: unassigned sum")?;
    tracing::debug!(unassigned_s = unassigned.0, "tasks.read.unassigned");

    let mut tasks: Vec<TaskSummary> = task_rows
        .into_iter()
        .map(|t| {
            let k = t.task_key;
            let agg = today_by_task.get(&k);
            TaskSummary {
                title: t.title.unwrap_or_default(),
                description: t.description_text.unwrap_or_default(),
                issue_type: t.issue_type,
                status: t.status_raw.unwrap_or_default(),
                is_terminal: t.is_terminal.unwrap_or(0) != 0,
                provider: t
                    .provider
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "jira".to_string()),
                url: t.url.unwrap_or_default(),
                epic_key: t.parent_key,
                epic_title: t.epic_title,
                due_date: t.due_date,
                start_date: t.start_date,
                today_s: agg.map(|a| a.dur).unwrap_or(0),
                today_autonomous_s: agg.map(|a| a.autonomous_s).unwrap_or(0),
                week_s: week_by_task.get(&k).copied().unwrap_or(0),
                session_count: agg.map(|a| a.sessions).unwrap_or(0),
                cats: agg.map(|a| a.cats.clone()).unwrap_or_default(),
                hygiene: hygiene_by_key.get(&k).cloned(),
                key: k,
            }
        })
        .collect();
    // Descending by today_s; stable so ties keep the task_key DESC order.
    tasks.sort_by(|a, b| b.today_s.cmp(&a.today_s));

    tracing::info!(
        tasks = tasks.len(),
        unassigned_s = unassigned.0,
        "tasks computed"
    );
    Ok(TasksResponse {
        tasks,
        unassigned_s: unassigned.0,
    })
}

/// Load pm_task_curation into task_key → Hygiene, tolerating a DB that predates
/// the table (migration 038) or the ignored_codes column (040). Snoozed-to-the-
/// future rows are dropped until the snooze lapses (matches the route).
async fn load_hygiene(
    pool: &SqlitePool,
    now_iso: &str,
) -> anyhow::Result<BTreeMap<String, Hygiene>> {
    let mut out: BTreeMap<String, Hygiene> = BTreeMap::new();

    let has_curation: Option<(i64,)> = sqlx::query_as::<_, (i64,)>(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='pm_task_curation'",
    )
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("tasks.read.curation_exists"))
    .await
    .context("tasks: detect pm_task_curation")?;
    tracing::debug!(
        present = has_curation.is_some(),
        "tasks.read.curation_exists"
    );
    if has_curation.is_none() {
        return Ok(out);
    }

    let has_ignored: Option<(i64,)> = sqlx::query_as::<_, (i64,)>(
        "SELECT 1 FROM pragma_table_info('pm_task_curation') WHERE name='ignored_codes'",
    )
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("tasks.read.ignored_col"))
    .await
    .context("tasks: detect ignored_codes")?;
    let ignored_col = if has_ignored.is_some() {
        "ignored_codes"
    } else {
        "'[]' AS ignored_codes"
    };

    let sql = format!(
        "SELECT task_key, bucket, reasons_json, decision, snoozed_until, {ignored_col} \
         FROM pm_task_curation"
    );
    // (task_key, bucket, reasons_json, decision, snoozed_until, ignored_codes)
    type CurationRow = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );
    let rows: Vec<CurationRow> = sqlx::query_as(&sql)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("tasks.read.pm_task_curation"))
        .await
        .context("tasks: fetch pm_task_curation")?;
    tracing::debug!(rows = rows.len(), "tasks.read.pm_task_curation");

    for (task_key, bucket, reasons_json, decision, snoozed_until, ignored_codes) in rows {
        // Snoozed-until-future drops off until the snooze lapses.
        if let Some(s) = &snoozed_until {
            if s.as_str() > now_iso {
                continue;
            }
        }
        out.insert(
            task_key,
            Hygiene {
                bucket,
                issues: parse_issues(reasons_json.as_deref(), Some(&ignored_codes)),
                decision,
            },
        );
    }
    Ok(out)
}
