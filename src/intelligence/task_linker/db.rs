// meridian — normalises screenpipe activity into structured app sessions

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;

use super::{SessionClassification, BATCH_LIMIT};

/// Returns the max `id` in `app_sessions`, or `None` if the table is empty.
pub(super) async fn get_max_session_id(pool: &SqlitePool) -> Result<Option<i64>> {
    let row = sqlx::query_as::<_, (Option<i64>,)>("SELECT MAX(id) FROM app_sessions")
        .fetch_one(pool)
        .await
        .context("reading max app_sessions id")?;
    Ok(row.0)
}

/// Returns the `last_session_id` from `agent_cursor` (row id=1), or 0 if absent.
pub(super) async fn get_agent_cursor(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query_as::<_, (i64,)>("SELECT last_session_id FROM agent_cursor WHERE id = 1")
        .fetch_optional(pool)
        .await
        .context("reading agent_cursor")?;

    Ok(row.map(|(v,)| v).unwrap_or(0))
}

/// Advance the cursor monotonically — only updates when `session_id` is strictly
/// greater than the stored value so out-of-order writes are safe.
pub(super) async fn advance_agent_cursor(pool: &SqlitePool, session_id: i64) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE agent_cursor SET last_session_id = ?, updated_at = ? \
         WHERE id = 1 AND ? > last_session_id",
    )
    .bind(session_id)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("advancing agent_cursor to {}", session_id))?;
    Ok(())
}

/// Fetch up to `BATCH_LIMIT` sessions with id > `after_id` that have not yet
/// been classified (absent from `ticket_links`) and meet the minimum duration.
pub(super) async fn fetch_unclassified_sessions(
    pool: &SqlitePool,
    after_id: i64,
    min_duration_s: i64,
) -> Result<
    Vec<(
        i64,
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<f64>,
    )>,
> {
    sqlx::query_as::<
        _,
        (
            i64,
            String,
            i64,
            String,
            Option<String>,
            Option<String>,
            Option<f64>,
        ),
    >(
        "SELECT id, app_name, duration_s, window_titles, session_text, category, confidence
         FROM app_sessions
         WHERE id > ?
           AND duration_s > ?
           AND id NOT IN (SELECT session_id FROM ticket_links)
         ORDER BY id ASC
         LIMIT ?",
    )
    .bind(after_id)
    .bind(min_duration_s)
    .bind(BATCH_LIMIT)
    .fetch_all(pool)
    .await
    .context("fetching unclassified sessions")
}

/// Fetch all open (non-done) PM tasks.
pub(super) async fn fetch_open_pm_tasks(pool: &SqlitePool) -> Result<Vec<super::TaskPayload>> {
    sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT task_key, title,
                COALESCE(description_text, ''),
                COALESCE(status, ''),
                COALESCE(status_category, '')
         FROM pm_tasks
         WHERE LOWER(status_category) != 'done'",
    )
    .fetch_all(pool)
    .await
    .context("fetching open pm_tasks")?
    .into_iter()
    .map(
        |(task_key, title, description_text, status, status_category)| {
            Ok(super::TaskPayload {
                task_key,
                title,
                description_text,
                status,
                status_category,
            })
        },
    )
    .collect()
}

/// Write a trivial (no session text) session as `overhead/skip` without calling
/// the LLM.
pub(super) async fn write_overhead_link(pool: &SqlitePool, session_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO ticket_links \
         (session_id, task_key, provider, method, confidence, session_type, routing) \
         VALUES (?, NULL, NULL, 'prefilter_trivial', 0.0, 'overhead', 'skip')",
    )
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("writing overhead link for session {}", session_id))?;
    Ok(())
}

/// Persist a classification result from Python into `ticket_links`.
pub(super) async fn write_ticket_link(pool: &SqlitePool, r: &SessionClassification) -> Result<()> {
    let session_type = if r.task_key.is_some() {
        "task"
    } else {
        "overhead"
    };
    sqlx::query(
        "INSERT OR IGNORE INTO ticket_links \
         (session_id, task_key, provider, method, confidence, session_type, routing) \
         VALUES (?, ?, 'jira', ?, ?, ?, ?)",
    )
    .bind(r.session_id)
    .bind(&r.task_key)
    .bind(&r.method)
    .bind(r.confidence)
    .bind(session_type)
    .bind(&r.routing)
    .execute(pool)
    .await
    .with_context(|| format!("writing ticket_link for session {}", r.session_id))?;
    Ok(())
}

/// Persist multi-label dimension tags returned by Python.
pub(super) async fn write_dimensions(
    pool: &SqlitePool,
    session_id: i64,
    dims: &HashMap<String, Vec<String>>,
) -> Result<()> {
    for (dimension, values) in dims {
        for value in values {
            sqlx::query(
                "INSERT OR IGNORE INTO session_dimensions \
                 (session_id, dimension, value, confidence, source) \
                 VALUES (?, ?, ?, 0.75, 'hermes_standalone')",
            )
            .bind(session_id)
            .bind(dimension)
            .bind(value)
            .execute(pool)
            .await
            .with_context(|| {
                format!(
                    "writing dimension {}={} for session {}",
                    dimension, value, session_id
                )
            })?;
        }
    }
    Ok(())
}

/// Insert an `agent_runs` row with `status = 'running'` and return its id.
pub(super) async fn start_agent_run(pool: &SqlitePool) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let row = sqlx::query_as::<_, (i64,)>(
        "INSERT INTO agent_runs (started_at, status) VALUES (?, 'running') RETURNING id",
    )
    .bind(&now)
    .fetch_one(pool)
    .await
    .context("inserting agent_run row")?;
    Ok(row.0)
}

/// Mark an `agent_runs` row as finished.
pub(super) async fn complete_agent_run(
    pool: &SqlitePool,
    run_id: i64,
    status: &str,
    sessions: i64,
    links: i64,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE agent_runs \
         SET finished_at = ?, status = ?, sessions_processed = ?, links_written = ? \
         WHERE id = ?",
    )
    .bind(&now)
    .bind(status)
    .bind(sessions)
    .bind(links)
    .bind(run_id)
    .execute(pool)
    .await
    .with_context(|| format!("completing agent_run {}", run_id))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — DB helpers; no subprocess
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn seed_session(pool: &SqlitePool) -> i64 {
        sqlx::query(
            "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
             VALUES ('t', 0, 0, 'success')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO app_sessions (
                app_name, started_at, ended_at, duration_s,
                window_titles, audio_snippets, signals,
                min_frame_id, max_frame_id, frame_count,
                idle_frame_count, etl_run_id
             ) VALUES ('TestApp', 't', 't', 120, '[]', '[]', '{}', 1, 1, 1, 0, 1)",
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    // ── cursor ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_agent_cursor_returns_zero_on_fresh_db() {
        let pool = fresh_db().await;
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn advance_agent_cursor_is_monotonic() {
        let pool = fresh_db().await;
        advance_agent_cursor(&pool, 5).await.unwrap();
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 5);
        advance_agent_cursor(&pool, 3).await.unwrap();
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn advance_agent_cursor_advances_forward() {
        let pool = fresh_db().await;
        advance_agent_cursor(&pool, 10).await.unwrap();
        advance_agent_cursor(&pool, 20).await.unwrap();
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 20);
    }

    // ── write_overhead_link ───────────────────────────────────────────────

    #[tokio::test]
    async fn write_overhead_link_inserts_correct_row() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        write_overhead_link(&pool, session_id).await.unwrap();

        let row = sqlx::query_as::<_, (Option<String>, String, String, f64)>(
            "SELECT task_key, method, routing, confidence
             FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.0.is_none());
        assert_eq!(row.1, "prefilter_trivial");
        assert_eq!(row.2, "skip");
        assert_eq!(row.3, 0.0);
    }

    #[tokio::test]
    async fn write_overhead_link_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        write_overhead_link(&pool, session_id).await.unwrap();
        write_overhead_link(&pool, session_id).await.unwrap();
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    // ── write_ticket_link ─────────────────────────────────────────────────

    #[tokio::test]
    async fn write_ticket_link_task_match_stores_correct_row() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            task_key: Some("KAN-42".to_string()),
            confidence: 0.87,
            routing: "auto".to_string(),
            reasoning: "test".to_string(),
            method: "llm_standalone".to_string(),
            dimensions: HashMap::new(),
            elapsed_s: 0.5,
        };
        write_ticket_link(&pool, &r).await.unwrap();

        let row = sqlx::query_as::<_, (String, String, f64, String)>(
            "SELECT task_key, method, confidence, session_type
             FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "KAN-42");
        assert_eq!(row.1, "llm_standalone");
        assert!((row.2 - 0.87).abs() < 1e-9);
        assert_eq!(row.3, "task");
    }

    #[tokio::test]
    async fn write_ticket_link_overhead_when_no_task_key() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            task_key: None,
            confidence: 0.1,
            routing: "skip".to_string(),
            reasoning: "test".to_string(),
            method: "llm_standalone".to_string(),
            dimensions: HashMap::new(),
            elapsed_s: 0.2,
        };
        write_ticket_link(&pool, &r).await.unwrap();

        let row = sqlx::query_as::<_, (String,)>(
            "SELECT session_type FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "overhead");
    }

    #[tokio::test]
    async fn write_ticket_link_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            task_key: Some("KAN-1".to_string()),
            confidence: 0.9,
            routing: "auto".to_string(),
            reasoning: "test".to_string(),
            method: "llm_standalone".to_string(),
            dimensions: HashMap::new(),
            elapsed_s: 0.1,
        };
        write_ticket_link(&pool, &r).await.unwrap();
        write_ticket_link(&pool, &r).await.unwrap();
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    // ── write_dimensions ──────────────────────────────────────────────────

    #[tokio::test]
    async fn write_dimensions_inserts_all_values() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let mut dims = HashMap::new();
        dims.insert(
            "activity".to_string(),
            vec!["coding".to_string(), "reviewing".to_string()],
        );
        dims.insert("tool".to_string(), vec!["cargo".to_string()]);
        write_dimensions(&pool, session_id, &dims).await.unwrap();

        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM session_dimensions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 3);
    }

    #[tokio::test]
    async fn write_dimensions_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let mut dims = HashMap::new();
        dims.insert("activity".to_string(), vec!["coding".to_string()]);
        write_dimensions(&pool, session_id, &dims).await.unwrap();
        write_dimensions(&pool, session_id, &dims).await.unwrap();
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM session_dimensions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    // ── fetch_unclassified_sessions ───────────────────────────────────────

    #[tokio::test]
    async fn fetch_unclassified_filters_already_linked() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        sqlx::query(
            "UPDATE app_sessions SET session_text = 'hello world', duration_s = 60 WHERE id = ?",
        )
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            fetch_unclassified_sessions(&pool, 0, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        write_overhead_link(&pool, session_id).await.unwrap();
        assert_eq!(
            fetch_unclassified_sessions(&pool, 0, 10)
                .await
                .unwrap()
                .len(),
            0,
            "linked session must be excluded"
        );
    }

    #[tokio::test]
    async fn fetch_unclassified_filters_by_cursor() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        sqlx::query("UPDATE app_sessions SET session_text = 'hello', duration_s = 60 WHERE id = ?")
            .bind(session_id)
            .execute(&pool)
            .await
            .unwrap();

        let rows = fetch_unclassified_sessions(&pool, session_id, 10)
            .await
            .unwrap();
        assert!(rows.is_empty(), "cursor at session_id must exclude it");
    }

    #[tokio::test]
    async fn fetch_unclassified_filters_short_duration() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        sqlx::query("UPDATE app_sessions SET session_text = 'hello', duration_s = 5 WHERE id = ?")
            .bind(session_id)
            .execute(&pool)
            .await
            .unwrap();

        let rows = fetch_unclassified_sessions(&pool, 0, 10).await.unwrap();
        assert!(rows.is_empty(), "duration_s ≤ min must be excluded");
    }

    // ── agent_runs ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn agent_run_round_trip() {
        let pool = fresh_db().await;
        let run_id = start_agent_run(&pool).await.unwrap();
        complete_agent_run(&pool, run_id, "success", 5, 3)
            .await
            .unwrap();

        let row = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT status, sessions_processed, links_written
             FROM agent_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "success");
        assert_eq!(row.1, 5);
        assert_eq!(row.2, 3);
    }
}
