// meridian — normalises screenpipe activity into structured app sessions
//
// Verifies that 005_agents.sql applies cleanly on top of 001-004 and creates
// the schema the meridian-agents Python service expects. New tables only —
// no ALTERs to existing app_sessions / active_session columns.

use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::str::FromStr;

async fn fresh_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    // FK enforcement is off by default; tests that exercise FK rejection
    // enable it explicitly (see fk_enabled_db()).
    pool
}

async fn fk_enabled_db() -> SqlitePool {
    let pool = fresh_db().await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1")
            .bind(name)
            .fetch_optional(pool)
            .await
            .unwrap();
    row.is_some()
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
    let rows: Vec<(i64, String)> = sqlx::query_as(&format!(
        "SELECT cid, name FROM pragma_table_info('{}')",
        table
    ))
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

async fn seed_agent_run(pool: &SqlitePool) -> i64 {
    let result = sqlx::query("INSERT INTO agent_runs DEFAULT VALUES")
        .execute(pool)
        .await
        .unwrap();
    result.last_insert_rowid()
}

// ---------------------------------------------------------------------------
// agent_runs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_runs_table_exists_with_all_columns() {
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
async fn agent_runs_round_trip() {
    let pool = fresh_db().await;
    let run_id = seed_agent_run(&pool).await;
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
    let row = sqlx::query(
        "SELECT status, sessions_processed, dispatches_sent FROM agent_runs WHERE id = ?1",
    )
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

// ---------------------------------------------------------------------------
// agent_cursor
// ---------------------------------------------------------------------------

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
    assert_eq!(id, 1);
    assert_eq!(last_session_id, 0);
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

// ---------------------------------------------------------------------------
// dispatch_queue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_queue_table_exists_with_all_columns() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "dispatch_queue").await);
    for col in [
        "id",
        "session_id",
        "agent_run_id",
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
    let run_id = seed_agent_run(&pool).await;
    let err = sqlx::query(
        "INSERT INTO dispatch_queue (session_id, agent_run_id, task_key, provider, payload_json)
         VALUES (?1, ?2, 'KAN-1', 'notion', '{}')",
    )
    .bind(session_id)
    .bind(run_id)
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
    let run_id = seed_agent_run(&pool).await;
    let err = sqlx::query(
        "INSERT INTO dispatch_queue
            (session_id, agent_run_id, task_key, provider, payload_json, state)
         VALUES (?1, ?2, 'KAN-1', 'jira', '{}', 'queued')",
    )
    .bind(session_id)
    .bind(run_id)
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
    let run_id = seed_agent_run(&pool).await;
    for (provider, state) in [
        ("jira", "pending"),
        ("github", "sent"),
        ("linear", "failed"),
        ("log", "skipped"),
    ] {
        sqlx::query(
            "INSERT INTO dispatch_queue
                (session_id, agent_run_id, task_key, provider, payload_json, state)
             VALUES (?1, ?2, 'KAN-1', ?3, '{}', ?4)",
        )
        .bind(session_id)
        .bind(run_id)
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
async fn dispatch_queue_rejects_missing_session_fk() {
    let pool = fk_enabled_db().await;
    let run_id = seed_agent_run(&pool).await;
    let err = sqlx::query(
        "INSERT INTO dispatch_queue (session_id, agent_run_id, task_key, provider, payload_json)
         VALUES (9999, ?1, 'KAN-1', 'jira', '{}')",
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "expected FK violation on session_id, got: {err}"
    );
}

#[tokio::test]
async fn dispatch_queue_rejects_missing_agent_run_fk() {
    let pool = fk_enabled_db().await;
    let session_id = seed_session(&pool).await;
    let err = sqlx::query(
        "INSERT INTO dispatch_queue (session_id, agent_run_id, task_key, provider, payload_json)
         VALUES (?1, 9999, 'KAN-1', 'jira', '{}')",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "expected FK violation on agent_run_id, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// session_summaries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_summaries_table_exists_with_all_columns() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "session_summaries").await);
    for col in [
        "id",
        "session_id",
        "agent_run_id",
        "summary_json",
        "generated_at",
    ] {
        assert!(
            column_exists(&pool, "session_summaries", col).await,
            "session_summaries missing column {col}"
        );
    }
}

#[tokio::test]
async fn session_summaries_session_id_is_unique() {
    let pool = fresh_db().await;
    let session_id = seed_session(&pool).await;
    let run_id = seed_agent_run(&pool).await;
    sqlx::query(
        "INSERT INTO session_summaries (session_id, agent_run_id, summary_json)
         VALUES (?1, ?2, '{\"summary\":\"first\"}')",
    )
    .bind(session_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();
    let err = sqlx::query(
        "INSERT INTO session_summaries (session_id, agent_run_id, summary_json)
         VALUES (?1, ?2, '{\"summary\":\"second\"}')",
    )
    .bind(session_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("unique"),
        "expected UNIQUE violation on session_id, got: {err}"
    );
}

#[tokio::test]
async fn session_summaries_rejects_missing_session_fk() {
    let pool = fk_enabled_db().await;
    let run_id = seed_agent_run(&pool).await;
    let err = sqlx::query(
        "INSERT INTO session_summaries (session_id, agent_run_id, summary_json)
         VALUES (9999, ?1, '{}')",
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "expected FK violation on session_id, got: {err}"
    );
}

#[tokio::test]
async fn session_summaries_round_trip() {
    let pool = fresh_db().await;
    let session_id = seed_session(&pool).await;
    let run_id = seed_agent_run(&pool).await;
    sqlx::query(
        "INSERT INTO session_summaries (session_id, agent_run_id, summary_json)
         VALUES (?1, ?2, '{\"summary\":\"vendored hermes\"}')",
    )
    .bind(session_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap();
    let row = sqlx::query(
        "SELECT summary_json, generated_at FROM session_summaries WHERE session_id = ?1",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let summary: String = row.get(0);
    let generated_at: String = row.get(1);
    assert!(summary.contains("vendored hermes"));
    assert!(!generated_at.is_empty());
}

// ---------------------------------------------------------------------------
// context_graph_nodes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_graph_nodes_table_exists_with_all_columns() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "context_graph_nodes").await);
    for col in [
        "id",
        "node_id",
        "node_type",
        "label",
        "last_seen",
        "frequency",
        "confidence_avg",
    ] {
        assert!(
            column_exists(&pool, "context_graph_nodes", col).await,
            "context_graph_nodes missing column {col}"
        );
    }
}

#[tokio::test]
async fn context_graph_nodes_rejects_unknown_node_type() {
    let pool = fresh_db().await;
    let err = sqlx::query(
        "INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen)
         VALUES ('weird', 'banana', 'Weird Node', 't')",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation on node_type, got: {err}"
    );
}

#[tokio::test]
async fn context_graph_nodes_accepts_all_known_types() {
    let pool = fresh_db().await;
    for node_type in ["project", "task", "tool", "pattern", "ticket"] {
        sqlx::query(
            "INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen)
             VALUES (?1, ?2, ?3, 't')",
        )
        .bind(format!("{node_type}_x"))
        .bind(node_type)
        .bind(format!("{node_type} label"))
        .execute(&pool)
        .await
        .unwrap();
    }
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM context_graph_nodes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 5);
}

#[tokio::test]
async fn context_graph_nodes_node_id_is_unique() {
    let pool = fresh_db().await;
    sqlx::query(
        "INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen)
         VALUES ('project_meridian', 'project', 'Meridian', 't')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let err = sqlx::query(
        "INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen)
         VALUES ('project_meridian', 'project', 'Meridian Again', 't')",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("unique"),
        "expected UNIQUE violation on node_id, got: {err}"
    );
}

#[tokio::test]
async fn context_graph_nodes_default_frequency_and_confidence() {
    let pool = fresh_db().await;
    sqlx::query(
        "INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen)
         VALUES ('tool_cargo', 'tool', 'cargo', 't')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let row = sqlx::query(
        "SELECT frequency, confidence_avg FROM context_graph_nodes WHERE node_id = 'tool_cargo'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let frequency: i64 = row.get(0);
    let confidence_avg: f64 = row.get(1);
    assert_eq!(frequency, 1);
    assert!((confidence_avg - 0.7).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// activity_context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_context_table_exists_with_all_columns() {
    let pool = fresh_db().await;
    assert!(table_exists(&pool, "activity_context").await);
    for col in [
        "id",
        "updated_at",
        "active_project",
        "jira_key",
        "inferred_task",
        "confidence",
        "trigger_jira_sync",
        "tags",
        "last_synced",
    ] {
        assert!(
            column_exists(&pool, "activity_context", col).await,
            "activity_context missing column {col}"
        );
    }
}

#[tokio::test]
async fn activity_context_seeded_with_id_one() {
    let pool = fresh_db().await;
    let row = sqlx::query("SELECT id, inferred_task, confidence FROM activity_context")
        .fetch_one(&pool)
        .await
        .unwrap();
    let id: i64 = row.get(0);
    let inferred_task: String = row.get(1);
    let confidence: f64 = row.get(2);
    assert_eq!(id, 1);
    assert_eq!(inferred_task, "");
    assert_eq!(confidence, 0.0);
}

#[tokio::test]
async fn activity_context_rejects_id_other_than_one() {
    let pool = fresh_db().await;
    let err = sqlx::query(
        "INSERT INTO activity_context (id, inferred_task, confidence) VALUES (2, '', 0)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation on id, got: {err}"
    );
}

#[tokio::test]
async fn activity_context_rejects_invalid_trigger_jira_sync() {
    let pool = fresh_db().await;
    let err = sqlx::query("UPDATE activity_context SET trigger_jira_sync = 5 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation on trigger_jira_sync, got: {err}"
    );
}

#[tokio::test]
async fn activity_context_round_trip() {
    let pool = fresh_db().await;
    sqlx::query(
        "UPDATE activity_context
         SET updated_at        = '2026-05-09T10:00:00Z',
             active_project    = 'meridian',
             jira_key          = 'KAN-86',
             inferred_task     = 'porting hermes synthesizer',
             confidence        = 0.92,
             trigger_jira_sync = 1,
             tags              = '[\"agents\",\"port\"]'
         WHERE id = 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    let row = sqlx::query(
        "SELECT active_project, jira_key, inferred_task, confidence, trigger_jira_sync
         FROM activity_context WHERE id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let project: String = row.get(0);
    let jira_key: String = row.get(1);
    let task: String = row.get(2);
    let confidence: f64 = row.get(3);
    let trigger: i64 = row.get(4);
    assert_eq!(project, "meridian");
    assert_eq!(jira_key, "KAN-86");
    assert_eq!(task, "porting hermes synthesizer");
    assert!((confidence - 0.92).abs() < 1e-9);
    assert_eq!(trigger, 1);
}

// ---------------------------------------------------------------------------
// Cross-cutting: existing tables NOT altered
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_005_does_not_add_columns_to_app_sessions() {
    let pool = fresh_db().await;
    // We never want these columns on app_sessions — they live in
    // session_summaries and would-be Rust-side categorizer respectively.
    assert!(
        !column_exists(&pool, "app_sessions", "summary_json").await,
        "app_sessions.summary_json must NOT exist (it lives in session_summaries)"
    );
    assert!(
        !column_exists(&pool, "app_sessions", "activity_kind").await,
        "app_sessions.activity_kind must NOT exist (the Rust categorizer uses 'category')"
    );
}
