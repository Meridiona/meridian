// meridian — normalises screenpipe activity into structured app sessions
//
// Smoke tests for run_task_linking():
//   - seeds a file-based DB
//   - prefilter tests verify trivial/short sessions are handled without any LLM call
//   - integration tests (marked #[ignore]) require the persistent MLX server running on
//     MLX_SERVER_PORT (default 7823): start with
//     cd services && .venv313/bin/meridian-server --backend mlx --port 7823

mod common;

use meridian::config::{Config, LlmBackendConfig, RuntimeSettings};
use meridian::intelligence::run_task_linking;
use serial_test::serial;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// File-based DB helper (MLX server needs a real file path)
// ---------------------------------------------------------------------------

async fn make_file_db() -> (tempfile::NamedTempFile, SqlitePool, String) {
    let tmp = tempfile::Builder::new()
        .suffix(".sqlite")
        .tempfile()
        .expect("tempfile");
    let path = tmp.path().to_str().unwrap().to_string();
    let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}"))
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    (tmp, pool, path)
}

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn seed_session(pool: &SqlitePool, app: &str, duration_s: i64, session_text: &str) -> i64 {
    sqlx::query(
        "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
         VALUES (strftime('%Y-%m-%dT%H:%M:%SZ','now'), 0, 0, 'success')",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s, session_text, session_text_source,
            window_titles, audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count, idle_frame_count, etl_run_id
         ) VALUES (?, strftime('%Y-%m-%dT%H:%M:%SZ','now'),
                      strftime('%Y-%m-%dT%H:%M:%SZ','now'),
                   ?, ?, 'accessibility', '[]', '[]', '{}', 1, 1, 1, 0, 1)",
    )
    .bind(app)
    .bind(duration_s)
    .bind(session_text)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn seed_pm_task(
    pool: &SqlitePool,
    task_key: &str,
    title: &str,
    description: &str,
    status_category: &str,
) {
    sqlx::query(
        "INSERT INTO pm_tasks
            (task_key, provider, title, description_text, status_category,
             issue_type, project_key, url, updated_at)
         VALUES (?, 'jira', ?, ?, ?, 'Story', 'KAN', '', strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
    )
    .bind(task_key)
    .bind(title)
    .bind(description)
    .bind(status_category)
    .execute(pool)
    .await
    .unwrap();
}

fn make_cfg(db_path: &str) -> Config {
    Config {
        screenpipe_db: String::new(),
        meridian_db: db_path.to_string(),
        poll_interval_secs: 60,
        pm_providers: vec![],
        llm_backend: LlmBackendConfig::Disabled,
        classification_enabled: true,
        classification_timeout_s: 120,
        min_classification_duration_s: 10,
        classification_services_dir: None,
        classification_backfill: true,
        category_backfill: false,
        classification_context_window: 5,
        classifier_backend: "mlx".to_owned(),
        mlx_server_port: 7823,
        jira_update_enabled: false,
        jira_update_interval_s: 14400,
        jira_office_start_hour: 9,
        jira_office_end_hour: 17,
        runtime: RuntimeSettings::default(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full round-trip: seed session + pm_tasks → MLX server classifies → Rust writes to DB.
/// Asserts that task_method, cursor, and agent_run are all written correctly.
/// Does NOT assert a specific task_key — that is LLM output and may vary.
#[tokio::test]
#[serial]
#[ignore = "requires live MLX server — start with: cd services && .venv313/bin/meridian-server --backend mlx --port 7823"]
async fn real_classification_writes_task_and_advances_cursor() {
    let (_tmp, pool, db_path) = make_file_db().await;

    let session_id = seed_session(
        &pool,
        "Cursor",
        120,
        "feat(etl): fix gap detection in runner.rs\n\
         Editing src/etl/runner.rs — closing stale blocks when inter-frame gap \
         exceeds GAP_THRESHOLD_SECS. Also touched src/db/meridian.rs to update \
         the close_block helper. Tests in tests/task_linker_smoke.rs.",
    )
    .await;

    seed_pm_task(
        &pool,
        "KAN-42",
        "Fix gap detection across ETL run boundaries",
        "Sessions that span a system sleep are incorrectly merged. \
         Fix by checking inter-frame delta before processing each frame batch.",
        "in_progress",
    )
    .await;

    seed_pm_task(
        &pool,
        "KAN-55",
        "Add dimension tagging to classified sessions",
        "After classification, write activity/intent/tool dimensions \
         to session_dimensions for the dashboard breakdown view.",
        "in_progress",
    )
    .await;

    let cfg = make_cfg(&db_path);
    run_task_linking(&pool, &cfg).await.unwrap();

    // task classification written to app_sessions
    let row = sqlx::query(
        "SELECT task_method, task_routing, task_key, task_confidence
         FROM app_sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .expect("classification must be written to app_sessions");

    assert!(
        row.get::<Option<String>, _>(0).is_some(),
        "task_method must be set after real classification"
    );
    assert!(
        row.get::<Option<String>, _>(1).is_some(),
        "task_routing must be set"
    );
    // task_key may be None if classified as overhead — that is valid
    let confidence = row.get::<Option<f64>, _>(3).unwrap_or(0.0);
    assert!(
        confidence >= 0.0 && confidence <= 1.0,
        "confidence out of range"
    );

    // cursor advanced to this session
    let cursor: (i64,) = sqlx::query_as("SELECT last_session_id FROM agent_cursor WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        cursor.0, session_id,
        "cursor must advance to classified session"
    );

    // agent_run audit row recorded as success
    let run = sqlx::query(
        "SELECT status, sessions_processed, links_written FROM agent_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>(0), "success");
    assert_eq!(run.get::<i64, _>(1), 1);
    assert_eq!(run.get::<i64, _>(2), 1);
}

/// Running the classification cycle twice must not re-classify an already-processed session.
#[tokio::test]
#[serial]
#[ignore = "requires live MLX server — start with: cd services && .venv313/bin/meridian-server --backend mlx --port 7823"]
async fn real_classification_does_not_reprocess_classified_session() {
    let (_tmp, pool, db_path) = make_file_db().await;

    seed_session(
        &pool,
        "VSCode",
        90,
        "Working on KAN-42 — editing runner.rs gap detection logic.",
    )
    .await;

    seed_pm_task(
        &pool,
        "KAN-42",
        "Fix gap detection across ETL run boundaries",
        "Sessions that span a system sleep are incorrectly merged.",
        "in_progress",
    )
    .await;

    let cfg = make_cfg(&db_path);
    run_task_linking(&pool, &cfg).await.unwrap(); // classifies
    run_task_linking(&pool, &cfg).await.unwrap(); // should be a no-op

    let run_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    // second run still inserts an agent_run row (idle success), but classifies nothing new
    assert_eq!(run_count.0, 2, "each call writes one agent_run row");

    let classified_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM app_sessions WHERE task_method IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        classified_count.0, 1,
        "session must be classified exactly once"
    );
}

/// A session below the minimum duration threshold is not sent to the MLX server.
/// This does not require the server — verifies the prefilter without any LLM.
#[tokio::test]
async fn short_session_is_not_classified() {
    let (_tmp, pool, db_path) = make_file_db().await;

    seed_session(&pool, "Terminal", 5, "cargo build").await;

    // classification_enabled=true but duration_s=5 is below min_classification_duration_s=10
    let cfg = Config {
        screenpipe_db: String::new(),
        meridian_db: db_path,
        poll_interval_secs: 60,
        pm_providers: vec![],
        llm_backend: LlmBackendConfig::Disabled,
        classification_enabled: true,
        classification_timeout_s: 30,
        min_classification_duration_s: 10,
        classification_services_dir: None,
        classification_backfill: true,
        category_backfill: false,
        classification_context_window: 5,
        classifier_backend: "mlx".to_owned(),
        mlx_server_port: 7823,
        jira_update_enabled: false,
        jira_update_interval_s: 14400,
        jira_office_start_hour: 9,
        jira_office_end_hour: 17,
        runtime: RuntimeSettings::default(),
    };

    run_task_linking(&pool, &cfg).await.unwrap();

    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM app_sessions WHERE task_method IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count.0, 0,
        "session under min_duration must not be classified"
    );
}

/// A session with empty session_text is marked overhead/skip without calling the server.
#[tokio::test]
async fn trivial_session_is_marked_overhead_without_server() {
    let (_tmp, pool, db_path) = make_file_db().await;

    let id = seed_session(&pool, "Spotify", 60, "").await;

    let cfg = Config {
        screenpipe_db: String::new(),
        meridian_db: db_path,
        poll_interval_secs: 60,
        pm_providers: vec![],
        llm_backend: LlmBackendConfig::Disabled,
        classification_enabled: true,
        classification_timeout_s: 30,
        min_classification_duration_s: 10,
        classification_services_dir: None,
        classification_backfill: true,
        category_backfill: false,
        classification_context_window: 5,
        classifier_backend: "mlx".to_owned(),
        mlx_server_port: 7823,
        jira_update_enabled: false,
        jira_update_interval_s: 14400,
        jira_office_start_hour: 9,
        jira_office_end_hour: 17,
        runtime: RuntimeSettings::default(),
    };

    run_task_linking(&pool, &cfg).await.unwrap();

    let row = sqlx::query("SELECT task_method, task_routing FROM app_sessions WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>(0), "prefilter_trivial");
    assert_eq!(row.get::<String, _>(1), "skip");
}
