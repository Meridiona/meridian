//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The hour ledger + readiness predicate — the heart of the hour-driven driver.
//
// `pm_worklog_hours` records, per (day, hour), whether that hour has been
// processed. The driver walks hours from local-midnight forward and processes
// each one that is READY, recording even 0-task hours as done so they are never
// re-scanned. Hours are processed independently — one not-ready hour does NOT
// block later hours (a stuck upstream row can never deadlock the day).
//
// Readiness (this module supplies the data-dependent half; the scheduler owns the
// clock half):
//   * upstream settled  — ETL has crossed the hour boundary AND no session
//     started in the hour is still *in-flight*. "In-flight" mirrors the
//     classifier's own candidate rule (`duration_s > min`, `task_method IS NULL`)
//     plus the coding-agent pipeline states, so sub-threshold blips the
//     classifier will never touch do NOT keep the hour pending. When true the
//     hour's data is complete.
//   * aging escape (scheduler) — if an hour has been over for longer than the
//     aging window, process it best-effort even if a session is still unsettled,
//     so one genuinely stuck row (e.g. a crashed summariser) can't freeze the
//     hour forever. With the in-flight predicate this is once again the rare
//     backstop it was designed to be, not the everyday path.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

/// Insert a `pending` ledger row for an hour if absent.
pub async fn ensure_hour(
    pool: &SqlitePool,
    day_utc: &str,
    hour_start: &str,
    hour_end: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO pm_worklog_hours \
            (day_utc, hour_start, hour_end, status, task_count) \
         VALUES (?, ?, ?, 'pending', 0)",
    )
    .bind(day_utc)
    .bind(hour_start)
    .bind(hour_end)
    .execute(pool)
    .await
    .context("ensure pm_worklog_hours row")?;
    Ok(())
}

/// Has this hour already been marked done?
pub async fn hour_is_done(pool: &SqlitePool, hour_start: &str) -> Result<bool> {
    let row = sqlx::query("SELECT status FROM pm_worklog_hours WHERE hour_start = ?")
        .bind(hour_start)
        .fetch_optional(pool)
        .await
        .context("read hour status")?;
    Ok(row
        .map(|r| r.get::<String, _>("status") == "done")
        .unwrap_or(false))
}

/// Mark an hour done with the number of tasks it produced worklogs for.
pub async fn mark_hour_done(pool: &SqlitePool, hour_start: &str, task_count: i64) -> Result<()> {
    sqlx::query(
        "UPDATE pm_worklog_hours \
         SET status = 'done', task_count = ?, \
             processed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
         WHERE hour_start = ?",
    )
    .bind(task_count)
    .bind(hour_start)
    .execute(pool)
    .await
    .context("mark hour done")?;
    Ok(())
}

/// True when the hour's upstream data is complete: ETL has crossed the hour
/// boundary AND no session started in the hour is still *genuinely in-flight*.
///
/// "In-flight" mirrors the classifier's own definition of work it will pick up —
/// NOT the cruder "any unclassified row" test. The classifier only ever touches
/// rows with `duration_s > min_duration_s` (see `db.rs::fetch_unclassified_sessions`),
/// so a sub-threshold blip keeps `task_session_type = NULL` forever. Waiting for
/// those NULLs to clear (the old predicate) meant every hour containing a short
/// session could never settle and could only fire via the 90-min aging escape —
/// turning the rare deadlock-breaker into the everyday path. We now only block on
/// rows the pipeline will actually advance, so an hour settles the instant its
/// real work is classified.
pub async fn upstream_settled(
    pool: &SqlitePool,
    hour_start: &str,
    hour_end: &str,
    min_duration_s: i64,
) -> Result<bool> {
    // B — ETL past the boundary: the latest activity (a sealed session or the
    // live active_session) reaches at or beyond hour_end.
    let max_ended: Option<String> = sqlx::query_scalar("SELECT MAX(ended_at) FROM app_sessions")
        .fetch_one(pool)
        .await
        .context("max ended_at")?;
    let active_started: Option<String> =
        sqlx::query_scalar("SELECT started_at FROM active_session WHERE id = 1")
            .fetch_optional(pool)
            .await
            .context("active session start")?
            .flatten();
    let etl_past = max_ended.as_deref().map(|m| m >= hour_end).unwrap_or(false)
        || active_started
            .as_deref()
            .map(|s| s >= hour_end)
            .unwrap_or(false);
    if !etl_past {
        return Ok(false);
    }

    // C — no session started in the hour is still in-flight. A row blocks only if
    // the pipeline will still advance it:
    //   * regular row the classifier will pick up:
    //       coding_agent_session_uuid IS NULL AND task_method IS NULL AND duration_s > min
    //     (exactly the classifier's candidate condition — by construction the hour
    //      settles when the classify queue for this window drains).
    //   * coding-agent row still moving through summarise:
    //       task_method IN ('coding_agent_live','pending_summariser')
    //     ('summarised' is terminal — agno workflow reads session_summary directly).
    // A sub-threshold regular blip (duration_s <= min) is ignored: the classifier
    // never touches it, so there is nothing to wait for. It also never becomes a
    // `task` row, so excluding it loses no worklog content.
    let in_flight: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_sessions \
         WHERE started_at >= ? AND started_at < ? \
           AND task_session_type IS NULL \
           AND ( \
                 (coding_agent_session_uuid IS NULL \
                    AND task_method IS NULL \
                    AND duration_s > ?) \
              OR (coding_agent_session_uuid IS NOT NULL \
                    AND task_method IN \
                        ('coding_agent_live', 'pending_summariser')) \
           )",
    )
    .bind(hour_start)
    .bind(hour_end)
    .bind(min_duration_s)
    .fetch_one(pool)
    .await
    .context("count in-flight sessions in hour")?;

    Ok(in_flight == 0)
}

/// Distinct task keys with classified `task` sessions started in this hour —
/// the 1, 2, 3… tasks the driver writes a worklog for.
pub async fn tasks_in_hour(
    pool: &SqlitePool,
    hour_start: &str,
    hour_end: &str,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT task_key FROM app_sessions \
         WHERE started_at >= ? AND started_at < ? \
           AND task_key IS NOT NULL \
           AND COALESCE(task_session_type, '') = 'task' \
         ORDER BY task_key ASC",
    )
    .bind(hour_start)
    .bind(hour_end)
    .fetch_all(pool)
    .await
    .context("distinct task keys in hour")?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("task_key"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    const HS: &str = "2026-05-30T05:00:00+00:00";
    const HE: &str = "2026-05-30T06:00:00+00:00";
    const IN_HOUR: &str = "2026-05-30T05:30:00+00:00";
    const PAST_BOUNDARY: &str = "2026-05-30T06:30:00+00:00"; // ended_at >= HE → ETL past
    const MIN_DUR: i64 = 10;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_session(
        pool: &SqlitePool,
        started_at: &str,
        ended_at: &str,
        duration_s: i64,
        claude_uuid: Option<&str>,
        task_method: Option<&str>,
        task_session_type: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status) \
             VALUES ('t', 0, 0, 'success')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO app_sessions ( \
                app_name, started_at, ended_at, duration_s, \
                window_titles, audio_snippets, signals, \
                min_frame_id, max_frame_id, frame_count, idle_frame_count, etl_run_id, \
                coding_agent_session_uuid, task_method, task_session_type \
             ) VALUES ('App', ?, ?, ?, '[]', '[]', '{}', 1, 1, 1, 0, \
                       (SELECT MAX(id) FROM etl_runs), ?, ?, ?)",
        )
        .bind(started_at)
        .bind(ended_at)
        .bind(duration_s)
        .bind(claude_uuid)
        .bind(task_method)
        .bind(task_session_type)
        .execute(pool)
        .await
        .unwrap();
    }

    /// A regular row the classifier will still pick up keeps the hour pending.
    #[tokio::test]
    async fn in_flight_regular_blocks() {
        let pool = fresh_db().await;
        insert_session(&pool, IN_HOUR, PAST_BOUNDARY, 120, None, None, None).await;
        assert!(!upstream_settled(&pool, HS, HE, MIN_DUR).await.unwrap());
    }

    /// THE FIX: a sub-threshold blip the classifier never touches must NOT block.
    #[tokio::test]
    async fn sub_threshold_blip_does_not_block() {
        let pool = fresh_db().await;
        // duration 5 <= MIN_DUR(10): never classified, task_session_type stays NULL
        // forever. The old predicate would wait for it (→ 90-min aging). The new
        // one ignores it, so the hour settles immediately.
        insert_session(&pool, IN_HOUR, PAST_BOUNDARY, 5, None, None, None).await;
        assert!(upstream_settled(&pool, HS, HE, MIN_DUR).await.unwrap());
    }

    /// A classified regular row is settled.
    #[tokio::test]
    async fn classified_regular_settles() {
        let pool = fresh_db().await;
        insert_session(
            &pool,
            IN_HOUR,
            PAST_BOUNDARY,
            120,
            None,
            Some("mlx"),
            Some("task"),
        )
        .await;
        assert!(upstream_settled(&pool, HS, HE, MIN_DUR).await.unwrap());
    }

    /// A coding-agent row still moving through the pipeline blocks the hour.
    #[tokio::test]
    async fn coding_pending_blocks() {
        let pool = fresh_db().await;
        insert_session(
            &pool,
            IN_HOUR,
            PAST_BOUNDARY,
            120,
            Some("uuid-1"),
            Some("pending_summariser"),
            None,
        )
        .await;
        assert!(!upstream_settled(&pool, HS, HE, MIN_DUR).await.unwrap());
    }

    /// A coding-agent row that reached the terminal `mlx_direct` is settled even
    /// before its task_session_type is read (the task_method terminal path).
    #[tokio::test]
    async fn coding_terminal_settles() {
        let pool = fresh_db().await;
        insert_session(
            &pool,
            IN_HOUR,
            PAST_BOUNDARY,
            120,
            Some("uuid-1"),
            Some("mlx_direct"),
            None,
        )
        .await;
        assert!(upstream_settled(&pool, HS, HE, MIN_DUR).await.unwrap());
    }

    /// Even with all sessions classified, an hour the ETL has not yet crossed is
    /// not settled (condition B).
    #[tokio::test]
    async fn not_settled_before_etl_boundary() {
        let pool = fresh_db().await;
        // Classified row, but it ended at 05:45 — before HE(06:00). MAX(ended_at)
        // < HE, no active_session → ETL has not crossed the boundary.
        insert_session(
            &pool,
            IN_HOUR,
            "2026-05-30T05:45:00+00:00",
            120,
            None,
            Some("mlx"),
            Some("task"),
        )
        .await;
        assert!(!upstream_settled(&pool, HS, HE, MIN_DUR).await.unwrap());
    }
}
