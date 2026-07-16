//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! DB read/write for the rolling day-task state (`day_tasks`) and the measured
//! per-hour spans (`day_hour_spans`) — migration 058.
//!
//! `day_tasks` holds the WHOLE day's task state; the fold reads it, hands it to
//! the model, and writes the merged result back. [`replace_day_tasks`] rewrites
//! the day in one transaction — upserting every task the fold returned and
//! deleting any day row the fold no longer names — so a merge that drops a task
//! never leaves an orphan, while a surviving task keeps its original
//! `created_at`. Each task's time lives in its `segments` (migration 059); its
//! `minutes` and `hours` are derived from them, so `day_hour_spans` is no longer
//! a minute source — [`upsert_hour_span`] still records the measured per-hour span
//! (write-only, kept for a future consumer) but nothing here reads it back.
//!
//! Both are today-forward, tolerant of a pre-058/059 DB: a missing table/column
//! degrades to an empty read / a skipped write, never an error that fails the hour.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};
use tracing::Instrument;

use super::segment::{self, Segment};

/// One day-task as stored — the state the fold reads back each hour.
#[derive(Debug, Clone)]
pub struct DayTaskRow {
    pub task_id: String,
    pub title: String,
    /// Running log of absorbed items, newline-joined (the model sees it as a list).
    pub summary: String,
    /// Local hour labels this task's segments touch (`'YYYY-MM-DDTHH'`) — the
    /// idempotency key + the interim hour-block UI's coarse span. Code-derived
    /// from `segments`, not authored by the model.
    pub hours: Vec<String>,
    /// Approximate `HH:MM-HH:MM` time ranges this task was worked (migration 059).
    /// Non-contiguous segments are breaks; the timeline draws the task across them.
    pub segments: Vec<Segment>,
    /// Total minutes covered by `segments`, overlap counted once.
    pub minutes: i64,
    pub status: String,
    /// PM seam — always `None` for now.
    pub linked_ticket: Option<String>,
    /// First-seen timestamp, preserved across folds.
    pub created_at: String,
}

/// Read the current day-task state, oldest task first. A missing table (pre-058
/// DB) or read error degrades to an empty Vec — the fold then seeds fresh.
pub async fn fetch_state(pool: &SqlitePool, day_local: &str) -> Vec<DayTaskRow> {
    let res = sqlx::query(
        "SELECT task_id, title, summary, hours_json, \
                COALESCE(segments_json, '[]') AS segments_json, \
                CAST(COALESCE(minutes, 0) AS INTEGER) AS minutes, \
                COALESCE(status, 'active') AS status, linked_ticket, created_at \
         FROM day_tasks WHERE day_local = ? ORDER BY task_id",
    )
    .bind(day_local)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("worklog.tasks.read.state"))
    .await;

    match res {
        Ok(rows) => {
            let out: Vec<DayTaskRow> = rows
                .into_iter()
                .map(|r| {
                    let hours_json = r.try_get::<String, _>("hours_json").unwrap_or_default();
                    let hours =
                        serde_json::from_str::<Vec<String>>(&hours_json).unwrap_or_default();
                    let segments_json = r.try_get::<String, _>("segments_json").unwrap_or_default();
                    let segments = segment::from_json_str(&segments_json);
                    DayTaskRow {
                        task_id: r.try_get::<String, _>("task_id").unwrap_or_default(),
                        title: r.try_get::<String, _>("title").unwrap_or_default(),
                        summary: r.try_get::<String, _>("summary").unwrap_or_default(),
                        hours,
                        segments,
                        minutes: r.try_get::<i64, _>("minutes").unwrap_or(0),
                        status: r.try_get::<String, _>("status").unwrap_or_default(),
                        linked_ticket: r
                            .try_get::<Option<String>, _>("linked_ticket")
                            .ok()
                            .flatten(),
                        created_at: r.try_get::<String, _>("created_at").unwrap_or_default(),
                    }
                })
                .collect();
            tracing::debug!(rows = out.len(), "worklog: fetched day-task state");
            out
        }
        Err(e) => {
            tracing::warn!(error = %e, "worklog: day_tasks read failed — treating as empty");
            Vec::new()
        }
    }
}

/// Rewrite the day's tasks in one transaction: upsert every task in `tasks` and
/// delete any day row not in the new set. Each row carries its own `created_at`
/// (the caller preserves it across folds), and `ON CONFLICT` never overwrites it
/// — so an existing task keeps its first-seen time two ways over. `now` is the
/// UTC RFC3339 timestamp stamped on `updated_at`.
///
/// A missing table (pre-058 DB) is logged and skipped, never an error.
pub async fn replace_day_tasks(
    pool: &SqlitePool,
    day_local: &str,
    tasks: &[DayTaskRow],
    now: &str,
) -> Result<()> {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return skip_or_err(e, "begin day_tasks tx"),
    };

    for t in tasks {
        let hours_json = serde_json::to_string(&t.hours).unwrap_or_else(|_| "[]".to_string());
        let segments_json = segment::to_json_string(&t.segments);
        let res = sqlx::query(
            "INSERT INTO day_tasks \
                (day_local, task_id, title, summary, hours_json, segments_json, minutes, status, \
                 linked_ticket, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(day_local, task_id) DO UPDATE SET \
                title = excluded.title, summary = excluded.summary, \
                hours_json = excluded.hours_json, segments_json = excluded.segments_json, \
                minutes = excluded.minutes, \
                status = excluded.status, linked_ticket = excluded.linked_ticket, \
                updated_at = excluded.updated_at",
        )
        .bind(day_local)
        .bind(&t.task_id)
        .bind(&t.title)
        .bind(&t.summary)
        .bind(&hours_json)
        .bind(&segments_json)
        .bind(t.minutes)
        .bind(&t.status)
        .bind(&t.linked_ticket)
        .bind(&t.created_at)
        .bind(now)
        .execute(&mut *tx)
        .await;
        if let Err(e) = res {
            return skip_or_err(e, "upsert day_task");
        }
    }

    // Delete rows for the day the fold no longer names (a merge dropped them).
    let keep: Vec<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();
    let placeholders = if keep.is_empty() {
        "''".to_string() // NOT IN ('') deletes every row for the day
    } else {
        vec!["?"; keep.len()].join(",")
    };
    let del_sql =
        format!("DELETE FROM day_tasks WHERE day_local = ? AND task_id NOT IN ({placeholders})");
    let mut q = sqlx::query(&del_sql).bind(day_local);
    for id in &keep {
        q = q.bind(*id);
    }
    if let Err(e) = q.execute(&mut *tx).await {
        return skip_or_err(e, "prune day_tasks");
    }

    match tx.commit().await {
        Ok(()) => {
            tracing::debug!(
                day = day_local,
                tasks = tasks.len(),
                "worklog: day_tasks written"
            );
            Ok(())
        }
        Err(e) => skip_or_err(e, "commit day_tasks tx"),
    }
}

/// Record the measured active minutes for one local hour. Idempotent UPSERT —
/// re-running an hour overwrites its span, never doubles it. Pre-058-tolerant.
pub async fn upsert_hour_span(
    pool: &SqlitePool,
    day_local: &str,
    hour_label: &str,
    span_min: i64,
) -> Result<()> {
    let res = sqlx::query(
        "INSERT INTO day_hour_spans (day_local, hour_label, span_min) VALUES (?, ?, ?) \
         ON CONFLICT(day_local, hour_label) DO UPDATE SET span_min = excluded.span_min",
    )
    .bind(day_local)
    .bind(hour_label)
    .bind(span_min)
    .execute(pool)
    .instrument(tracing::debug_span!("worklog.tasks.write.span"))
    .await;
    match res {
        Ok(_) => Ok(()),
        Err(e) => skip_or_err(e, "upsert day_hour_span"),
    }
}

/// A pre-058 DB lacks these tables: log and skip rather than failing the hour.
/// Any other DB error propagates with context.
fn skip_or_err(e: sqlx::Error, ctx: &'static str) -> Result<()> {
    if let sqlx::Error::Database(db) = &e {
        let msg = db.message();
        if msg.contains("no such table") || msg.contains("no such column") {
            tracing::warn!(ctx, "worklog: day-task table absent (pre-058 DB) — skipped");
            return Ok(());
        }
    }
    Err(e).context(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    fn row(id: &str, hours: &[&str], created: &str) -> DayTaskRow {
        DayTaskRow {
            task_id: id.into(),
            title: format!("Title {id}"),
            summary: "line one\nline two".into(),
            hours: hours.iter().map(|s| s.to_string()).collect(),
            segments: vec![Segment {
                start_min: 495,
                end_min: 525,
            }],
            minutes: 30,
            status: "active".into(),
            linked_ticket: None,
            created_at: created.into(),
        }
    }

    #[tokio::test]
    async fn round_trips_state_and_hours() {
        let pool = fresh().await;
        let tasks = vec![
            row("T1", &["2026-07-15T08", "2026-07-15T09"], "t0"),
            row("T2", &["2026-07-15T11"], "t0"),
        ];
        replace_day_tasks(&pool, "2026-07-15", &tasks, "t1")
            .await
            .unwrap();

        let got = fetch_state(&pool, "2026-07-15").await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].task_id, "T1");
        assert_eq!(got[0].hours, vec!["2026-07-15T08", "2026-07-15T09"]);
        assert_eq!(got[1].task_id, "T2");
        assert_eq!(got[0].summary, "line one\nline two");
        assert_eq!(
            got[0].segments,
            vec![Segment {
                start_min: 495,
                end_min: 525
            }]
        );
    }

    #[tokio::test]
    async fn upsert_preserves_created_at_and_prunes_dropped() {
        let pool = fresh().await;
        replace_day_tasks(
            &pool,
            "2026-07-15",
            &[
                row("T1", &["2026-07-15T08"], "orig"),
                row("T2", &["2026-07-15T09"], "orig"),
            ],
            "w1",
        )
        .await
        .unwrap();

        // A merge folds T2 into T1 and drops T2. T1 keeps its original created_at.
        replace_day_tasks(
            &pool,
            "2026-07-15",
            &[row("T1", &["2026-07-15T08", "2026-07-15T09"], "IGNORED")],
            "w2",
        )
        .await
        .unwrap();

        let got = fetch_state(&pool, "2026-07-15").await;
        assert_eq!(got.len(), 1, "T2 pruned");
        assert_eq!(got[0].created_at, "orig", "created_at preserved on upsert");
        assert_eq!(got[0].hours.len(), 2);
    }

    #[tokio::test]
    async fn empty_task_set_clears_the_day() {
        let pool = fresh().await;
        replace_day_tasks(
            &pool,
            "2026-07-15",
            &[row("T1", &["2026-07-15T08"], "t0")],
            "w1",
        )
        .await
        .unwrap();
        replace_day_tasks(&pool, "2026-07-15", &[], "w2")
            .await
            .unwrap();
        assert!(fetch_state(&pool, "2026-07-15").await.is_empty());
    }

    #[tokio::test]
    async fn hour_spans_upsert_and_read() {
        let pool = fresh().await;
        upsert_hour_span(&pool, "2026-07-15", "2026-07-15T08", 40)
            .await
            .unwrap();
        upsert_hour_span(&pool, "2026-07-15", "2026-07-15T09", 55)
            .await
            .unwrap();
        // Re-running the 08 hour overwrites, never doubles.
        upsert_hour_span(&pool, "2026-07-15", "2026-07-15T08", 45)
            .await
            .unwrap();

        // day_hour_spans is write-only now (minutes come from segments); verify the
        // upsert directly rather than through a reader.
        let v: i64 = sqlx::query_scalar("SELECT span_min FROM day_hour_spans WHERE hour_label = ?")
            .bind("2026-07-15T08")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(v, 45);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_hour_spans")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }
}
