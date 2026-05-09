// meridian — normalises screenpipe activity into structured app sessions
//
// Verifies that 004_agents.sql applies cleanly on top of 001-003 and creates
// the schema the meridian-agents Python service expects.

use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::str::FromStr;

async fn fresh_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    pool
}

async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .unwrap();
    row.is_some()
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
    let rows: Vec<(i64, String)> =
        sqlx::query_as(&format!("SELECT cid, name FROM pragma_table_info('{}')", table))
            .fetch_all(pool)
            .await
            .unwrap();
    rows.iter().any(|(_, n)| n == column)
}

/// Inserts a fake etl_run + a fake app_session and returns the session id.
async fn seed_session(pool: &SqlitePool) -> i64 {
    sqlx::query(
        "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
         VALUES ('t', 0, 0, 'success')",
    )
    .execute(pool)
    .await
    .unwrap();
    let result = sqlx::query(
        "INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, ocr_samples, elements_samples,
            audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count,
            idle_frame_count, etl_run_id
        ) VALUES ('x', 't', 't', 0, '[]', '[]', '[]', '[]', '{}', 1, 1, 1, 0, 1)",
    )
    .execute(pool)
    .await
    .unwrap();
    result.last_insert_rowid()
}

#[tokio::test]
async fn agent_runs_table_exists() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "agent_runs").await);
    for col in [
        "id",
        "started_at",
        "finished_at",
        "status",
        "error",
        "sessions_processed",
        "summaries_written",
        "links_written",
        "dispatches_queued",
        "dispatches_sent",
    ] {
        assert!(
            column_exists(&pool, "agent_runs", col).await,
            "agent_runs missing column {col}"
        );
    }
}

#[tokio::test]
async fn agent_cursor_seeded_with_id_one() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "agent_cursor").await);
    let row = sqlx::query("SELECT id, last_session_id FROM agent_cursor")
        .fetch_one(&pool)
        .await
        .unwrap();
    let id: i64 = row.get(0);
    let last_session_id: i64 = row.get(1);
    assert_eq!(id, 1, "agent_cursor seed row must have id=1");
    assert_eq!(last_session_id, 0, "initial last_session_id must be 0");
}

#[tokio::test]
async fn agent_cursor_rejects_id_other_than_one() {
    let pool = fresh_db().await;
    let err = sqlx::query("INSERT INTO agent_cursor (id, last_session_id) VALUES (2, 0)")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK constraint violation, got: {err}"
    );
}

#[tokio::test]
async fn dispatch_queue_table_exists_with_constraints() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "dispatch_queue").await);
    for col in [
        "id",
        "session_id",
        "task_key",
        "provider",
        "payload_json",
        "state",
        "attempts",
        "last_error",
        "created_at",
        "dispatched_at",
    ] {
        assert!(
            column_exists(&pool, "dispatch_queue", col).await,
            "dispatch_queue missing column {col}"
        );
    }
}

#[tokio::test]
async fn dispatch_queue_rejects_unknown_provider() {
    let pool = fresh_db().await;
    let session_id = seed_session(&pool).await;
    let err = sqlx::query(
        "INSERT INTO dispatch_queue (session_id, task_key, provider, payload_json)
         VALUES (?1, 'KAN-1', 'notion', '{}')",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation on provider, got: {err}"
    );
}

#[tokio::test]
async fn dispatch_queue_rejects_unknown_state() {
    let pool = fresh_db().await;
    let session_id = seed_session(&pool).await;
    let err = sqlx::query(
        "INSERT INTO dispatch_queue (session_id, task_key, provider, payload_json, state)
         VALUES (?1, 'KAN-1', 'jira', '{}', 'queued')",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation on state, got: {err}"
    );
}

#[tokio::test]
async fn dispatch_queue_accepts_known_providers_and_states() {
    let pool = fresh_db().await;
    let session_id = seed_session(&pool).await;
    for (provider, state) in [
        ("jira", "pending"),
        ("github", "sent"),
        ("linear", "failed"),
        ("log", "skipped"),
    ] {
        sqlx::query(
            "INSERT INTO dispatch_queue (session_id, task_key, provider, payload_json, state)
             VALUES (?1, 'KAN-1', ?2, '{}', ?3)",
        )
        .bind(session_id)
        .bind(provider)
        .bind(state)
        .execute(&pool)
        .await
        .unwrap();
    }
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dispatch_queue")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 4);
}

#[tokio::test]
async fn agent_runs_rejects_unknown_status() {
    let pool = fresh_db().await;
    let err = sqlx::query("INSERT INTO agent_runs (status) VALUES ('queued')")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation on status, got: {err}"
    );
}

#[tokio::test]
async fn app_sessions_has_summary_json_and_activity_kind_columns() {
    let pool = fresh_db().await;
    assert!(column_exists(&pool, "app_sessions", "summary_json").await);
    assert!(column_exists(&pool, "app_sessions", "activity_kind").await);
}

#[tokio::test]
async fn app_sessions_summary_json_defaults_to_null() {
    let pool = fresh_db().await;
    let session_id = seed_session(&pool).await;
    let row = sqlx::query("SELECT summary_json, activity_kind FROM app_sessions WHERE id = ?1")
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let summary: Option<String> = row.get(0);
    let kind: Option<String> = row.get(1);
    assert!(summary.is_none(), "summary_json must default to NULL");
    assert!(kind.is_none(), "activity_kind must default to NULL");
}

#[tokio::test]
async fn agent_runs_audit_round_trip() {
    let pool = fresh_db().await;
    let result = sqlx::query("INSERT INTO agent_runs DEFAULT VALUES")
        .execute(&pool)
        .await
        .unwrap();
    let run_id = result.last_insert_rowid();
    sqlx::query(
        "UPDATE agent_runs
         SET status = 'success',
             finished_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
             sessions_processed = 3,
             summaries_written = 3,
             links_written = 2,
             dispatches_queued = 2,
             dispatches_sent = 2
         WHERE id = ?1",
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();
    let row = sqlx::query("SELECT status, sessions_processed, dispatches_sent FROM agent_runs WHERE id = ?1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let status: String = row.get(0);
    let sessions: i64 = row.get(1);
    let sent: i64 = row.get(2);
    assert_eq!(status, "success");
    assert_eq!(sessions, 3);
    assert_eq!(sent, 2);
}
