//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The read side of the rolling day-task state (`day_tasks`, migration 058).
//!
//! # What this is
//! The worklog pipeline folds each hour's activity into a small running set of
//! day-level TASKS (workstreams, 1-5/day) — see
//! `src/worklog_pipeline/workstream.rs`. This reader surfaces that set for the
//! dashboard timeline, which draws each task as one block spanning the hours it
//! covers (replacing the old per-hour rows). There is no matching Next.js route —
//! this is new backend work, not a route port.
//!
//! Unlike [`crate::hour_text`], this is **not** today-gated: `day_tasks` is keyed
//! by an explicit local `day_local`, so a past day returns exactly the tasks
//! computed that day (the pipeline simply never wrote any for days it didn't run).
//! A missing table (pre-058 DB) degrades to an empty list, never an error.
//!
//! # Who calls this
//! The tray `get_day_tasks` command -> the dashboard timeline column.
//!
//! # Related
//! - [`crate::hour_text`] — the per-hour report text the fold consumes.
//! - [`crate::worklogs`] — the PM-matched worklog cards (the seam this will feed
//!   once `linked_ticket` is wired; NULL for now).

use crate::SqlitePool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::Instrument;

/// One approximate `HH:MM`-`HH:MM` local time range a task was worked in (migration
/// 059). A task can hold several non-contiguous segments — the breaks between them
/// are what let the timeline draw one workstream across a gap. Stored and surfaced
/// as clock strings; the timeline parses them when it lays out pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaySegment {
    pub start: String,
    pub end: String,
}

/// One day-level task, as the timeline renders it: a title, its running summary
/// log, its measured minutes, the local hours it spans, and its approximate time
/// segments.
#[derive(Debug, Clone, Serialize)]
pub struct DayTask {
    /// Stable within the day (`"T1"`, `"T2"`, …).
    pub id: String,
    pub title: String,
    /// Running log lines (the DB stores them newline-joined).
    pub summary: Vec<String>,
    /// Deterministic measured minutes across the day (summed segment durations).
    pub minutes: i64,
    /// Local hour labels this task is active in (`"YYYY-MM-DDTHH"`), ascending —
    /// the coarse span the interim hour-block UI still renders from.
    pub hours: Vec<String>,
    /// Approximate `HH:MM-HH:MM` time ranges this task was worked, ascending;
    /// non-contiguous entries are breaks. The precise timeline renders from these.
    pub segments: Vec<DaySegment>,
    /// Earliest local hour-of-day (0-23) this task spans; `-1` if it has none.
    pub first_hour: i64,
    /// Latest local hour-of-day (0-23) this task spans; `-1` if it has none.
    pub last_hour: i64,
    pub status: String,
    /// PM seam — always `None` for now.
    pub linked_ticket: Option<String>,
}

/// The day's inferred tasks, most-worked-first is NOT guaranteed — ordered by
/// stable id so the timeline layout is stable across polls.
#[derive(Debug, Clone, Serialize)]
pub struct DayTasksResponse {
    pub day: String,
    pub tasks: Vec<DayTask>,
}

#[derive(FromRow)]
struct RawDayTask {
    task_id: String,
    title: String,
    summary: String,
    hours_json: String,
    segments_json: String,
    minutes: i64,
    status: String,
    linked_ticket: Option<String>,
}

/// Local hour-of-day (0-23) from a `"YYYY-MM-DDTHH"` label, or `None`.
fn label_hour(label: &str) -> Option<i64> {
    label.get(11..13)?.parse::<i64>().ok()
}

/// Read the day's tasks for `day` (local `YYYY-MM-DD`). Ordered by `task_id`.
/// A missing table / read error degrades to an empty list.
#[tracing::instrument(skip(pool))]
pub async fn get_day_tasks(pool: &SqlitePool, day: &str) -> anyhow::Result<DayTasksResponse> {
    let rows = sqlx::query_as::<_, RawDayTask>(
        "SELECT task_id, title, summary, hours_json, \
                COALESCE(segments_json, '[]') AS segments_json, \
                CAST(COALESCE(minutes, 0) AS INTEGER) AS minutes, \
                COALESCE(status, 'active') AS status, linked_ticket \
         FROM day_tasks WHERE day_local = ? ORDER BY task_id",
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("day_tasks.read.day_tasks"))
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            // Missing table on a pre-058 DB is not fatal — the day simply has no
            // inferred tasks yet.
            tracing::warn!(error = %e, "day_tasks: read skipped (pre-058 DB?)");
            return Ok(DayTasksResponse {
                day: day.to_string(),
                tasks: Vec::new(),
            });
        }
    };
    tracing::debug!(rows = rows.len(), "day_tasks.read.day_tasks");

    let tasks = rows
        .into_iter()
        .map(|r| {
            let mut hours: Vec<String> = serde_json::from_str(&r.hours_json).unwrap_or_default();
            hours.sort();
            let hour_nums: Vec<i64> = hours.iter().filter_map(|h| label_hour(h)).collect();
            let first_hour = hour_nums.iter().min().copied().unwrap_or(-1);
            let last_hour = hour_nums.iter().max().copied().unwrap_or(-1);
            let segments: Vec<DaySegment> =
                serde_json::from_str(&r.segments_json).unwrap_or_default();
            let summary: Vec<String> = r
                .summary
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            DayTask {
                id: r.task_id,
                title: r.title,
                summary,
                minutes: r.minutes,
                hours,
                segments,
                first_hour,
                last_hour,
                status: r.status,
                linked_ticket: r.linked_ticket,
            }
        })
        .collect::<Vec<_>>();

    tracing::info!(day, n_tasks = tasks.len(), "day_tasks computed");
    Ok(DayTasksResponse {
        day: day.to_string(),
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn seeded() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE day_tasks (day_local TEXT, task_id TEXT, title TEXT, summary TEXT, \
                hours_json TEXT, segments_json TEXT, minutes INTEGER, status TEXT, \
                linked_ticket TEXT, created_at TEXT, updated_at TEXT, \
                PRIMARY KEY (day_local, task_id))",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn empty_when_no_rows() {
        let pool = seeded().await;
        let resp = get_day_tasks(&pool, "2026-07-15").await.unwrap();
        assert_eq!(resp.day, "2026-07-15");
        assert!(resp.tasks.is_empty());
    }

    #[tokio::test]
    async fn maps_span_and_summary() {
        let pool = seeded().await;
        sqlx::query(
            "INSERT INTO day_tasks VALUES ('2026-07-15','T1','Website', \
                'Fixed SEO\nBuilt form','[\"2026-07-15T08\",\"2026-07-15T10\"]', \
                '[{\"start\":\"08:15\",\"end\":\"08:45\"},{\"start\":\"10:00\",\"end\":\"10:45\"}]', \
                75, 'active', NULL, 't0', 't1')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let resp = get_day_tasks(&pool, "2026-07-15").await.unwrap();
        assert_eq!(resp.tasks.len(), 1);
        let t = &resp.tasks[0];
        assert_eq!(t.id, "T1");
        assert_eq!(t.first_hour, 8);
        assert_eq!(t.last_hour, 10);
        assert_eq!(t.minutes, 75);
        assert_eq!(t.summary, vec!["Fixed SEO", "Built form"]);
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[0].start, "08:15");
        assert_eq!(t.segments[1].end, "10:45");
        assert!(t.linked_ticket.is_none());
    }

    #[tokio::test]
    async fn degrades_to_empty_without_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // No day_tasks table at all (pre-058 DB).
        let resp = get_day_tasks(&pool, "2026-07-15").await.unwrap();
        assert!(resp.tasks.is_empty());
    }
}
