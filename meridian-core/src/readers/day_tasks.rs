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
    /// The linked tracker key, set once a worklog for this task is posted/linked.
    pub linked_ticket: Option<String>,
    /// The PM provider this task's worklog was **posted** to (`"jira"`, `"linear"`,
    /// …), or `None` if no worklog has been posted. Drives the "posted to {logo}"
    /// badge on the timeline card. Only set for `state = 'posted'` rows.
    pub posted_provider: Option<String>,
    /// The tracker key the posted worklog landed on (matched or created), or `None`.
    pub posted_target_key: Option<String>,
    /// Deep link to the posted ticket on the tracker, or `None`.
    pub posted_browse_url: Option<String>,
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
    posted_provider: Option<String>,
    posted_target_key: Option<String>,
    posted_browse_url: Option<String>,
}

/// Whether `name` is a table in this DB. Used to keep a JOIN off a table an
/// un-migrated DB doesn't have yet, which would otherwise fail the whole read.
async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Local hour-of-day (0-23) from a `"YYYY-MM-DDTHH"` label, or `None`.
fn label_hour(label: &str) -> Option<i64> {
    label.get(11..13)?.parse::<i64>().ok()
}

/// Read the day's tasks for `day` (local `YYYY-MM-DD`). Ordered by `task_id`.
/// A missing table / read error degrades to an empty list.
#[tracing::instrument(skip(pool))]
pub async fn get_day_tasks(pool: &SqlitePool, day: &str) -> anyhow::Result<DayTasksResponse> {
    // The "posted to {provider}" badge needs each task's posted-worklog provider.
    // That lives in day_task_worklogs (migration 060, keyed identically), so LEFT
    // JOIN it — but only when the tables exist: on a pre-060/062 DB the join would
    // error and (via the fail-soft below) blank the whole timeline. Probe first.
    let has_worklogs = table_exists(pool, "day_task_worklogs").await
        && table_exists(pool, "day_task_worklog_targets").await;

    // An update can post to several tickets (migration 062), but the badge has room
    // for one — take the first that landed, in the model's own order, so the card
    // links the strongest match. The detail panel lists them all from the draft.
    let sql = if has_worklogs {
        "SELECT dt.task_id, dt.title, dt.summary, dt.hours_json, \
                COALESCE(dt.segments_json, '[]') AS segments_json, \
                CAST(COALESCE(dt.minutes, 0) AS INTEGER) AS minutes, \
                COALESCE(dt.status, 'active') AS status, dt.linked_ticket, \
                CASE WHEN w.state = 'posted' THEN w.provider END AS posted_provider, \
                CASE WHEN w.state = 'posted' THEN ( \
                    SELECT tg.task_key FROM day_task_worklog_targets tg \
                     WHERE tg.day_local = dt.day_local AND tg.task_id = dt.task_id \
                       AND tg.posted_comment_id IS NOT NULL \
                     ORDER BY tg.position LIMIT 1) END AS posted_target_key, \
                CASE WHEN w.state = 'posted' THEN ( \
                    SELECT tg.browse_url FROM day_task_worklog_targets tg \
                     WHERE tg.day_local = dt.day_local AND tg.task_id = dt.task_id \
                       AND tg.posted_comment_id IS NOT NULL \
                     ORDER BY tg.position LIMIT 1) END AS posted_browse_url \
         FROM day_tasks dt \
         LEFT JOIN day_task_worklogs w \
                ON w.day_local = dt.day_local AND w.task_id = dt.task_id \
         WHERE dt.day_local = ? ORDER BY dt.task_id"
    } else {
        "SELECT task_id, title, summary, hours_json, \
                COALESCE(segments_json, '[]') AS segments_json, \
                CAST(COALESCE(minutes, 0) AS INTEGER) AS minutes, \
                COALESCE(status, 'active') AS status, linked_ticket, \
                NULL AS posted_provider, NULL AS posted_target_key, NULL AS posted_browse_url \
         FROM day_tasks WHERE day_local = ? ORDER BY task_id"
    };

    let rows = sqlx::query_as::<_, RawDayTask>(sql)
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
                posted_provider: r.posted_provider,
                posted_target_key: r.posted_target_key,
                posted_browse_url: r.posted_browse_url,
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
        // No day_task_worklogs table here → posted badge fields stay None.
        assert!(t.posted_provider.is_none());
    }

    #[tokio::test]
    async fn exposes_posted_provider_when_worklog_posted() {
        let pool = seeded().await;
        sqlx::query(
            "INSERT INTO day_tasks VALUES ('2026-07-15','T1','Website','s','[]','[]', \
                10, 'active', 'KAN-308', 't0', 't1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // The posted-worklog side (only the columns the join reads).
        seed_worklog_tables(&pool).await;
        sqlx::query("INSERT INTO day_task_worklogs VALUES ('2026-07-15','T1','jira','posted')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO day_task_worklog_targets VALUES \
                ('2026-07-15','T1','KAN-308',0,'c1','https://x/browse/KAN-308')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let t = &get_day_tasks(&pool, "2026-07-15").await.unwrap().tasks[0];
        assert_eq!(t.posted_provider.as_deref(), Some("jira"));
        assert_eq!(t.posted_target_key.as_deref(), Some("KAN-308"));
        assert_eq!(
            t.posted_browse_url.as_deref(),
            Some("https://x/browse/KAN-308")
        );
    }

    /// The card has room for one ticket but an update can post to several. It must
    /// show the first that LANDED, in the model's order - not an unposted target
    /// that happens to sort first, which would link a ticket with no comment on it.
    #[tokio::test]
    async fn a_multi_ticket_worklog_badges_the_first_that_posted() {
        let pool = seeded().await;
        sqlx::query(
            "INSERT INTO day_tasks VALUES ('2026-07-15','T1','Website','s','[]','[]', \
                10, 'active', NULL, 't0', 't1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        seed_worklog_tables(&pool).await;
        sqlx::query("INSERT INTO day_task_worklogs VALUES ('2026-07-15','T1','jira','posted')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO day_task_worklog_targets VALUES \
                ('2026-07-15','T1','KAN-1',0,NULL,NULL), \
                ('2026-07-15','T1','KAN-2',1,'c2','https://x/browse/KAN-2')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let t = &get_day_tasks(&pool, "2026-07-15").await.unwrap().tasks[0];
        assert_eq!(t.posted_target_key.as_deref(), Some("KAN-2"));
        assert_eq!(
            t.posted_browse_url.as_deref(),
            Some("https://x/browse/KAN-2")
        );
    }

    /// The worklog side of the join, in the shape 062 leaves it — only the columns
    /// the badge reads.
    async fn seed_worklog_tables(pool: &SqlitePool) {
        sqlx::query(
            "CREATE TABLE day_task_worklogs (day_local TEXT, task_id TEXT, provider TEXT, \
                state TEXT, PRIMARY KEY (day_local, task_id))",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE day_task_worklog_targets (day_local TEXT, task_id TEXT, \
                task_key TEXT, position INTEGER, posted_comment_id TEXT, browse_url TEXT, \
                PRIMARY KEY (day_local, task_id, task_key))",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn drafted_worklog_is_not_marked_posted() {
        let pool = seeded().await;
        sqlx::query(
            "INSERT INTO day_tasks VALUES ('2026-07-15','T1','Website','s','[]','[]', \
                10, 'active', NULL, 't0', 't1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        seed_worklog_tables(&pool).await;
        // A DRAFTED (not posted) worklog must not light up the badge.
        sqlx::query("INSERT INTO day_task_worklogs VALUES ('2026-07-15','T1','jira','drafted')")
            .execute(&pool)
            .await
            .unwrap();

        let t = &get_day_tasks(&pool, "2026-07-15").await.unwrap().tasks[0];
        assert!(t.posted_provider.is_none(), "drafted is not posted");
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
