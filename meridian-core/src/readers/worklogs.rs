//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/worklogs?day=YYYY-MM-DD` ported to Rust — a faithful port of
//! `ui/app/api/worklogs/route.ts`.
//!
//! # What this is
//! A day's drafted/approved/posted worklogs for review: the editable Jira
//! comment (payload `summary`), supporting bullets/next-steps for context,
//! confidence/coverage, risk flags, and post status. Read-only — the mutations
//! live in the (not-yet-ported) `worklogs/[id]` route.
//!
//! # Who calls this
//! The tray `get_worklogs` command → the dashboard `WorklogsView` (the draft
//! review list). Note: `WorklogsView`'s approve/reject actions still POST to
//! `/api/worklogs/[id]` until that write route is ported.
//!
//! # Related
//! - [`crate::tasks`] joins the same `pm_tasks` for per-ticket time.
//! - Bullets/next-steps are parsed out of the row's `payload_json` blob below.

use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;
use std::collections::BTreeMap;
use tracing::Instrument;

/// One supporting bullet on a worklog, tagged by kind (shipped / in progress /
/// blocker / decision).
#[derive(Debug, Clone, Serialize)]
pub struct WorklogBullet {
    pub kind: String,
    pub text: String,
}

/// One reviewable worklog: the editable comment (`summary`) + context (bullets,
/// next steps, risk flags, reasoning) + post state. `edited` reflects a manual
/// edit on the row.
#[derive(Debug, Clone, Serialize)]
pub struct WorklogItem {
    pub id: i64,
    pub task_key: String,
    pub task_title: Option<String>,
    pub task_url: Option<String>,
    pub provider: String,
    pub window_start: String,
    pub state: String,
    pub confidence: f64,
    pub coverage: f64,
    pub time_spent_seconds: i64,
    pub summary: String,
    pub bullets: Vec<WorklogBullet>,
    pub next_steps: Vec<String>,
    pub risk_flags: Vec<String>,
    pub reasoning: String,
    pub posted_worklog_id: Option<String>,
    pub last_post_error: Option<String>,
    pub edited: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorklogsResponse {
    pub day: String,
    pub items: Vec<WorklogItem>,
    /// state → count. Key order differs from the route's insertion order
    /// (sorted here); consumers read by key, so this is functionally identical.
    pub counts: BTreeMap<String, i64>,
}

#[derive(FromRow)]
struct RawRow {
    id: i64,
    task_key: String,
    task_title: Option<String>,
    task_url: Option<String>,
    provider: String,
    window_start: String,
    state: String,
    confidence: Option<f64>,
    coverage: Option<f64>,
    time_spent_seconds: Option<i64>,
    payload_json: Option<String>,
    posted_worklog_id: Option<String>,
    last_post_error: Option<String>,
    edited_at: Option<String>,
}

/// payload_json bullet groups → display kind (order matches the route).
const BULLET_GROUPS: [(&str, &str); 4] = [
    ("what_shipped", "shipped"),
    ("in_progress", "in progress"),
    ("blockers", "blocker"),
    ("decisions", "decision"),
];

/// Pull `[{text}]` style bullets out of a payload array field.
fn bullets_from(payload: &Value, field: &str, kind: &str, out: &mut Vec<WorklogBullet>) {
    if let Some(arr) = payload.get(field).and_then(|v| v.as_array()) {
        for b in arr {
            if let Some(text) = b.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    out.push(WorklogBullet {
                        kind: kind.to_string(),
                        text: text.to_string(),
                    });
                }
            }
        }
    }
}

/// A `["a","b"]` string array from a payload field, else empty.
fn str_array(payload: &Value, field: &str) -> Vec<String> {
    payload
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tracing::instrument(skip(pool))]
pub async fn get_worklogs(pool: &SqlitePool, day: &str) -> anyhow::Result<WorklogsResponse> {
    let rows: Vec<RawRow> = sqlx::query_as::<_, RawRow>(
        r#"
        SELECT w.id, w.task_key, t.title AS task_title, t.url AS task_url,
               COALESCE(w.provider, 'jira') AS provider, w.window_start,
               w.state, w.confidence, w.coverage,
               w.time_spent_seconds, w.payload_json, w.posted_worklog_id,
               w.last_post_error, w.edited_at
        FROM pm_worklogs w
        LEFT JOIN pm_tasks t ON t.task_key = w.task_key
        WHERE w.day_utc = ?
        ORDER BY w.window_start, w.task_key
        "#,
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("worklogs.read.pm_worklogs"))
    .await
    .context("worklogs: fetch pm_worklogs")?;
    tracing::debug!(rows = rows.len(), "worklogs.read.pm_worklogs");

    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut items: Vec<WorklogItem> = Vec::with_capacity(rows.len());

    for r in rows {
        *counts.entry(r.state.clone()).or_insert(0) += 1;

        let payload: Value = r
            .payload_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);

        let mut bullets = Vec::new();
        for (field, kind) in BULLET_GROUPS {
            bullets_from(&payload, field, kind, &mut bullets);
        }

        // The route keeps task_url only when it's an https URL.
        let task_url = r.task_url.filter(|u| u.starts_with("https://"));

        items.push(WorklogItem {
            id: r.id,
            task_key: r.task_key,
            task_title: r.task_title,
            task_url,
            provider: r.provider,
            window_start: r.window_start,
            state: r.state,
            confidence: r.confidence.unwrap_or(0.0),
            coverage: r.coverage.unwrap_or(0.0),
            time_spent_seconds: r.time_spent_seconds.unwrap_or(0),
            summary: payload
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            bullets,
            next_steps: str_array(&payload, "next_steps"),
            risk_flags: str_array(&payload, "risk_flags"),
            reasoning: payload
                .get("reasoning")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            posted_worklog_id: r.posted_worklog_id,
            last_post_error: r.last_post_error,
            edited: r.edited_at.is_some(),
        });
    }

    tracing::info!(day, items = items.len(), "worklogs computed");
    Ok(WorklogsResponse {
        day: day.to_string(),
        items,
        counts,
    })
}
