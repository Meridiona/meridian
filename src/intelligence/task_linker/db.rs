// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use super::BATCH_LIMIT;

/// Count sessions that have not yet been classified and are above the cursor.
/// Used for stuck-state visibility logging when the subprocess is failing.
pub(super) async fn count_pending_sessions(
    pool: &SqlitePool,
    after_id: i64,
    min_duration_s: i64,
) -> Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM app_sessions
         WHERE id > ? AND duration_s > ? AND task_method IS NULL",
    )
    .bind(after_id)
    .bind(min_duration_s)
    .fetch_one(pool)
    .await
    .context("counting pending unclassified sessions")?;
    Ok(row.0)
}

/// Fetch up to `limit` sealed coding-agent rows that have been summarised and
/// are awaiting classification (`task_method = 'pending_classifier'`). This is a
/// NON-cursor queue: coding-agent rows have low ids the cursor has long passed,
/// so they are classified by summarised-state, not id order. Oldest-ended first.
pub(super) async fn fetch_pending_classifier_sessions(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM app_sessions
         WHERE claude_session_uuid IS NOT NULL
           AND session_summary IS NOT NULL
           AND task_method = 'pending_classifier'
         ORDER BY ended_at ASC
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetching pending_classifier coding-agent sessions")?;
    Ok(ids)
}

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

/// Fetch up to `BATCH_LIMIT` sessions with id > `after_id` that have not yet
/// been classified (task_method IS NULL) and meet the minimum duration.
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
        String,
        String,
        Option<String>,
        Option<f64>,
        String,
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
            String,
            String,
            Option<String>,
            Option<f64>,
            String,
        ),
    >(
        "SELECT id, app_name, duration_s, window_titles, session_text,
                started_at, ended_at, category, confidence,
                COALESCE(session_text_source, 'unknown')
         FROM app_sessions
         WHERE id > ?
           AND duration_s > ?
           AND task_method IS NULL
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

/// Fetch sessions in the explicit id range `[from_id, to_id]` (inclusive) that
/// meet the minimum duration. No cursor check; no `ticket_links` exclusion —
/// the caller (backfill) may intentionally re-classify already-linked sessions.
pub(super) async fn fetch_sessions_in_range(
    pool: &SqlitePool,
    from_id: i64,
    to_id: Option<i64>,
    min_duration_s: i64,
) -> Result<
    Vec<(
        i64,
        String,
        i64,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<f64>,
        String,
    )>,
> {
    match to_id {
        Some(to) => sqlx::query_as::<
            _,
            (
                i64,
                String,
                i64,
                String,
                Option<String>,
                String,
                String,
                Option<String>,
                Option<f64>,
                String,
            ),
        >(
            "SELECT id, app_name, duration_s, window_titles, session_text,
                    started_at, ended_at, category, confidence,
                    COALESCE(session_text_source, 'unknown')
             FROM app_sessions
             WHERE id >= ? AND id <= ? AND duration_s > ?
             ORDER BY id ASC",
        )
        .bind(from_id)
        .bind(to)
        .bind(min_duration_s)
        .fetch_all(pool)
        .await
        .context("fetching sessions in id range"),

        None => sqlx::query_as::<
            _,
            (
                i64,
                String,
                i64,
                String,
                Option<String>,
                String,
                String,
                Option<String>,
                Option<f64>,
                String,
            ),
        >(
            "SELECT id, app_name, duration_s, window_titles, session_text,
                    started_at, ended_at, category, confidence,
                    COALESCE(session_text_source, 'unknown')
             FROM app_sessions
             WHERE id >= ? AND duration_s > ?
             ORDER BY id ASC",
        )
        .bind(from_id)
        .bind(min_duration_s)
        .fetch_all(pool)
        .await
        .context("fetching sessions in id range"),
    }
}

// ---------------------------------------------------------------------------
// Tests — DB helpers; no subprocess
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::task_linker::db_write::{
        advance_agent_cursor, complete_agent_run, start_agent_run, update_coding_agent_task,
        update_session_overhead, update_session_task, write_dimensions,
    };
    use crate::intelligence::task_linker::SessionClassification;
    use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
    use std::collections::HashMap;
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

    /// Insert a sealed, summarised coding-agent row awaiting classification.
    async fn seed_pending_classifier(pool: &SqlitePool, uuid: &str, summary: &str) -> i64 {
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
                min_frame_id, max_frame_id, frame_count, idle_frame_count, etl_run_id,
                claude_session_uuid, segment_started_at, sealed_at,
                session_summary, task_method
             ) VALUES ('Claude Code',
                '2026-05-20T08:00:00.000000+00:00', '2026-05-20T08:30:00.000000+00:00', 300,
                '[]', '[]', '{}', 0, 0, 4, 0, 1,
                ?, '2026-05-20T08:00:00.000000+00:00', '2026-05-20T09:00:00.000000+00:00',
                ?, 'pending_classifier')",
        )
        .bind(uuid)
        .bind(summary)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    fn classification(session_id: i64) -> SessionClassification {
        SessionClassification {
            session_id,
            category: "coding".into(),
            category_confidence: 0.9,
            category_explanation: "VS Code, cargo build in terminal".into(),
            task_key: Some("KAN-1".into()),
            confidence: 0.9,
            routing: "queue".into(),
            session_type: "task".into(),
            reasoning: "did the work".into(),
            method: "mlx_direct".into(),
            dimensions: HashMap::new(),
            session_summary: "CLASSIFIER SUMMARY — must NOT overwrite".into(),
            elapsed_s: 1.0,
        }
    }

    #[tokio::test]
    async fn pending_classifier_queue_then_classify_preserves_summary() {
        let pool = fresh_db().await;
        let id = seed_pending_classifier(&pool, "u1", "GOOD SUMMARISER SUMMARY").await;

        // The non-cursor queue picks it up (independent of agent_cursor).
        assert_eq!(
            fetch_pending_classifier_sessions(&pool, 8).await.unwrap(),
            vec![id]
        );

        // Classify it — task fields land, session_summary is PRESERVED.
        update_coding_agent_task(&pool, &classification(id))
            .await
            .unwrap();
        let (method, task_key, summary): (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT task_method, task_key, session_summary FROM app_sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(method, "mlx_direct"); // terminal — leaves the queue
        assert_eq!(task_key.as_deref(), Some("KAN-1"));
        assert_eq!(
            summary.as_deref(),
            Some("GOOD SUMMARISER SUMMARY"),
            "summary must survive classify"
        );

        // Drained.
        assert!(fetch_pending_classifier_sessions(&pool, 8)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn pending_classifier_excludes_unsummarised_rows() {
        let pool = fresh_db().await;
        // A coding row still awaiting summary (pending_summariser) → not in this queue.
        sqlx::query(
            "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status) VALUES ('t',0,0,'success')",
        ).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO app_sessions (
                app_name, started_at, ended_at, duration_s, window_titles, audio_snippets, signals,
                min_frame_id, max_frame_id, frame_count, idle_frame_count, etl_run_id,
                claude_session_uuid, segment_started_at, sealed_at, task_method
             ) VALUES ('Claude Code','2026-05-20T08:00:00.000000+00:00','2026-05-20T08:30:00.000000+00:00',
                300,'[]','[]','{}',0,0,4,0,1,'u2','2026-05-20T08:00:00.000000+00:00',
                '2026-05-20T09:00:00.000000+00:00','pending_summariser')",
        ).execute(&pool).await.unwrap();
        assert!(fetch_pending_classifier_sessions(&pool, 8)
            .await
            .unwrap()
            .is_empty());
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

    // ── update_session_overhead ───────────────────────────────────────────

    #[tokio::test]
    async fn update_session_overhead_sets_correct_fields() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        update_session_overhead(&pool, session_id).await.unwrap();

        let row = sqlx::query_as::<_, (Option<String>, String, String, f64)>(
            "SELECT task_key, task_method, task_routing, task_confidence
             FROM app_sessions WHERE id = ?",
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
    async fn update_session_overhead_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        update_session_overhead(&pool, session_id).await.unwrap();
        update_session_overhead(&pool, session_id).await.unwrap();
        let row =
            sqlx::query_as::<_, (String,)>("SELECT task_method FROM app_sessions WHERE id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "prefilter_trivial");
    }

    // ── update_session_task ───────────────────────────────────────────────

    #[tokio::test]
    async fn update_session_task_stores_correct_fields() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            category: "coding".into(),
            category_confidence: 0.9,
            category_explanation: "VS Code, cargo build in terminal".into(),
            task_key: Some("KAN-42".to_string()),
            confidence: 0.87,
            routing: "auto".to_string(),
            session_type: "task".to_string(),
            reasoning: "test".to_string(),
            method: "hermes_aiagent".to_string(),
            dimensions: HashMap::new(),
            session_summary: String::new(),
            elapsed_s: 0.5,
        };
        update_session_task(&pool, &r).await.unwrap();

        let row = sqlx::query_as::<_, (String, String, f64, String, String, String, f64, String)>(
            "SELECT task_key, task_method, task_confidence, task_session_type,
                    category, category_method, confidence, category_explanation
             FROM app_sessions WHERE id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "KAN-42");
        assert_eq!(row.1, "hermes_aiagent");
        assert!((row.2 - 0.87).abs() < 1e-9);
        assert_eq!(row.3, "task");
        // Category is now produced by the MLX classifier (replaces the FM settler):
        // update_session_task must persist it, its confidence + explanation, and
        // stamp category_method='mlx'.
        assert_eq!(row.4, "coding");
        assert_eq!(row.5, "mlx");
        assert!((row.6 - 0.9).abs() < 1e-9);
        assert_eq!(row.7, "VS Code, cargo build in terminal");
    }

    #[tokio::test]
    async fn update_session_task_overhead_when_no_task_key() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            category: "coding".into(),
            category_confidence: 0.9,
            category_explanation: "VS Code, cargo build in terminal".into(),
            task_key: None,
            confidence: 0.1,
            routing: "skip".to_string(),
            session_type: "overhead".to_string(),
            reasoning: "test".to_string(),
            method: "hermes_aiagent".to_string(),
            dimensions: HashMap::new(),
            session_summary: String::new(),
            elapsed_s: 0.2,
        };
        update_session_task(&pool, &r).await.unwrap();

        let row = sqlx::query_as::<_, (String,)>(
            "SELECT task_session_type FROM app_sessions WHERE id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "overhead");
    }

    #[tokio::test]
    async fn update_session_task_overwrites_on_second_call() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            category: "coding".into(),
            category_confidence: 0.9,
            category_explanation: "VS Code, cargo build in terminal".into(),
            task_key: Some("KAN-1".to_string()),
            confidence: 0.9,
            routing: "auto".to_string(),
            session_type: "task".to_string(),
            reasoning: "test".to_string(),
            method: "hermes_aiagent".to_string(),
            dimensions: HashMap::new(),
            session_summary: String::new(),
            elapsed_s: 0.1,
        };
        update_session_task(&pool, &r).await.unwrap();
        update_session_task(&pool, &r).await.unwrap();
        let row = sqlx::query_as::<_, (String,)>("SELECT task_key FROM app_sessions WHERE id = ?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, "KAN-1");
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

        update_session_overhead(&pool, session_id).await.unwrap();
        assert_eq!(
            fetch_unclassified_sessions(&pool, 0, 10)
                .await
                .unwrap()
                .len(),
            0,
            "classified session must be excluded"
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
