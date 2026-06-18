//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/plan/task` GET ported to Rust — full detail for one board ticket.
//!
//! A faithful port of `ui/app/api/plan/task/route.ts`: one `pm_tasks` row reshaped
//! for the plan page's task dialog (the FULL description + acceptance criteria,
//! which the list payload only excerpts), with `due_days` computed in local days.
//!
//! # Who calls this
//! - Command: `get_task_detail` (registered in the tray's `lib.rs`)
//! - Frontend: `ui/components/plan/TaskDialog.tsx` via `ui/lib/bridge.ts::load`.
//!
//! # Related
//! - [`crate::tasks`] — the per-task list this dialog drills into.

use crate::SqlitePool;
use chrono::NaiveDate;
use serde::Serialize;
use sqlx::FromRow;
use tracing::Instrument;

/// Full board-ticket detail (field names match the TS `TaskDetail` interface).
#[derive(Debug, Clone, Serialize)]
pub struct TaskDetail {
    pub key: String,
    pub title: String,
    pub provider: String,
    pub url: String,
    pub status: String,
    pub is_terminal: bool,
    pub issue_type: String,
    pub epic: Option<String>,
    pub priority: Option<String>,
    pub story_points: Option<String>,
    pub due_date: Option<String>,
    pub due_days: Option<i64>,
    pub start_date: Option<String>,
    pub description: String,
    pub acceptance_criteria: Option<String>,
}

#[derive(FromRow)]
struct TaskRow {
    task_key: String,
    title: Option<String>,
    provider: Option<String>,
    url: Option<String>,
    status_raw: Option<String>,
    is_terminal: Option<i64>,
    issue_type: Option<String>,
    epic_title: Option<String>,
    parent_key: Option<String>,
    priority: Option<String>,
    story_points: Option<String>,
    due_date: Option<String>,
    start_date: Option<String>,
    description_text: Option<String>,
    acceptance_criteria: Option<String>,
}

/// One blank-to-`None` trimmed string (mirrors the route's `(x)?.trim() || null`).
fn trimmed(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Fetch full detail for `key`, or `None` if the ticket isn't on the board
/// (the route's 404). `today` is the local date for the `due_days` math.
#[tracing::instrument(skip(pool))]
pub async fn get_task_detail(
    pool: &SqlitePool,
    key: &str,
    today: NaiveDate,
) -> anyhow::Result<Option<TaskDetail>> {
    let row: Option<TaskRow> = sqlx::query_as::<_, TaskRow>(
        r#"SELECT task_key, title, provider, url,
                  status_raw, is_terminal, issue_type, epic_title, parent_key,
                  priority, story_points, due_date, start_date,
                  description_text, acceptance_criteria
           FROM pm_tasks WHERE task_key = ?"#,
    )
    .bind(key)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("task_detail.read.pm_tasks"))
    .await?;
    tracing::debug!(found = row.is_some(), "task_detail.read.pm_tasks");

    Ok(row.map(|r| {
        // epic: epic_title (trimmed) → parent_key (trimmed) → None.
        let epic = trimmed(r.epic_title).or_else(|| trimmed(r.parent_key));
        TaskDetail {
            title: r
                .title
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| r.task_key.clone()),
            provider: r
                .provider
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "jira".to_string()),
            url: r.url.unwrap_or_default(),
            status: r.status_raw.unwrap_or_default(),
            is_terminal: r.is_terminal.unwrap_or(0) != 0,
            issue_type: r.issue_type.unwrap_or_default(),
            epic,
            priority: trimmed(r.priority),
            story_points: trimmed(r.story_points),
            due_days: crate::date::due_days_from(r.due_date.as_deref(), today),
            due_date: r.due_date,
            start_date: r.start_date,
            description: r.description_text.unwrap_or_default(),
            acceptance_criteria: trimmed(r.acceptance_criteria),
            key: r.task_key,
        }
    }))
}
