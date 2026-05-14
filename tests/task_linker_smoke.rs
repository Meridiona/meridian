// meridian — normalises screenpipe activity into structured app sessions
//
// Smoke tests for run_task_linking():
//   - disabled path (no DB writes)
//   - no pending sessions (idle no-op)
//   - trivial sessions (empty/whitespace session_text → overhead/skip without Python)
//   - short-duration sessions (below min threshold → not processed)
//   - pre-linked sessions (already in ticket_links → not reprocessed)
//   - stub Python subprocess → task link + dimensions + cursor advance

mod common;

use meridian::config::{Config, LlmBackendConfig};
use meridian::intelligence::run_task_linking;
use sqlx::Row;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn disabled_cfg() -> Config {
    make_cfg(false, None)
}

fn cfg_no_python() -> Config {
    make_cfg(true, Some("/nonexistent/path".to_string()))
}

fn make_cfg(enabled: bool, services_dir: Option<String>) -> Config {
    // Tests explicitly opt-in to backfill so they can seed sessions and observe
    // classification results. The real daemon defaults to backfill=false.
    make_cfg_backfill(enabled, services_dir, true)
}

fn make_cfg_backfill(enabled: bool, services_dir: Option<String>, backfill: bool) -> Config {
    Config {
        screenpipe_db: String::new(),
        meridian_db: String::new(),
        poll_interval_secs: 60,
        pm_providers: vec![],
        llm_backend: LlmBackendConfig::Disabled,
        classification_enabled: enabled,
        classification_timeout_s: 30,
        min_classification_duration_s: 10,
        classification_services_dir: services_dir,
        classification_backfill: backfill,
        jira_update_enabled: false,
        jira_update_interval_s: 14400,
        jira_office_start_hour: 9,
        jira_office_end_hour: 17,
    }
}

async fn seed_session(
    pool: &sqlx::SqlitePool,
    app: &str,
    duration_s: i64,
    session_text: Option<&str>,
) -> i64 {
    sqlx::query(
        "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
         VALUES ('t', 0, 0, 'success')",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s, session_text,
            window_titles, audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count, idle_frame_count, etl_run_id
         ) VALUES (?, 't', 't', ?, ?, '[]', '[]', '{}', 1, 1, 1, 0, 1)",
    )
    .bind(app)
    .bind(duration_s)
    .bind(session_text)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

/// Creates a temp services dir containing a stub `agents/run_task_linker.py`
/// that reads JSON from stdin and echoes back one `auto`-routed result per session.
fn stub_services_dir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let agents = dir.path().join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("__init__.py"), "").unwrap();
    std::fs::write(
        agents.join("run_task_linker.py"),
        r#"import sys, json
payload = json.loads(sys.stdin.read())
results = []
for s in payload.get("sessions", []):
    results.append({
        "session_id": s["id"],
        "task_key": "KAN-99",
        "confidence": 0.92,
        "routing": "auto",
        "reasoning": "stub",
        "method": "llm_standalone",
        "dimensions": {"activity": ["coding"], "tool": ["cargo"]},
        "elapsed_s": 0.01,
    })
sys.stdout.write(json.dumps({"results": results}))
sys.stdout.write("\n")
"#,
    )
    .unwrap();
    dir
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn classification_disabled_returns_ok_and_writes_nothing() {
    let pool = common::make_meridian_db().await;
    seed_session(&pool, "Xcode", 120, Some("working on feature")).await;

    run_task_linking(&pool, &disabled_cfg()).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ticket_links")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);

    let run_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(run_count.0, 0, "disabled path must not write agent_runs");
}

#[tokio::test]
async fn no_sessions_pending_is_idle_success() {
    let pool = common::make_meridian_db().await;
    // No sessions at all
    run_task_linking(&pool, &cfg_no_python()).await.unwrap();

    let row = sqlx::query(
        "SELECT status, sessions_processed, links_written FROM agent_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "success");
    assert_eq!(row.get::<i64, _>(1), 0);
    assert_eq!(row.get::<i64, _>(2), 0);
}

#[tokio::test]
async fn trivial_sessions_overhead_skip_without_python() {
    let pool = common::make_meridian_db().await;
    let id1 = seed_session(&pool, "Slack", 60, None).await;
    let id2 = seed_session(&pool, "Spotify", 90, Some("   ")).await; // whitespace-only

    run_task_linking(&pool, &cfg_no_python()).await.unwrap();

    for session_id in [id1, id2] {
        let row = sqlx::query(
            "SELECT method, routing, task_key FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|_| panic!("no ticket_link for session {session_id}"));

        assert_eq!(row.get::<String, _>(0), "prefilter_trivial");
        assert_eq!(row.get::<String, _>(1), "skip");
        assert!(row.get::<Option<String>, _>(2).is_none());
    }

    // Cursor must advance past both
    let cursor: (i64,) =
        sqlx::query_as("SELECT last_session_id FROM agent_cursor WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cursor.0, id2);
}

#[tokio::test]
async fn short_duration_sessions_are_not_processed() {
    let pool = common::make_meridian_db().await;
    // duration_s = 5, min is 10
    seed_session(&pool, "Terminal", 5, Some("cargo build")).await;

    run_task_linking(&pool, &cfg_no_python()).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ticket_links")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn pre_linked_sessions_are_not_reprocessed() {
    let pool = common::make_meridian_db().await;
    let session_id = seed_session(&pool, "Xcode", 120, Some("building meridian")).await;

    sqlx::query(
        "INSERT INTO ticket_links
             (session_id, task_key, provider, method, confidence, session_type, routing)
         VALUES (?, 'KAN-1', 'jira', 'manual', 1.0, 'task', 'auto')",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap();

    run_task_linking(&pool, &cfg_no_python()).await.unwrap();

    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1, "pre-linked session must not get a duplicate link");
}

#[tokio::test]
async fn mixed_batch_trivial_and_short_handled_correctly() {
    let pool = common::make_meridian_db().await;
    let trivial_id = seed_session(&pool, "Music", 60, None).await;
    let short_id = seed_session(&pool, "App", 3, Some("some text")).await;

    run_task_linking(&pool, &cfg_no_python()).await.unwrap();

    // Trivial: linked as overhead/skip
    let trivial_row = sqlx::query(
        "SELECT method FROM ticket_links WHERE session_id = ?",
    )
    .bind(trivial_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(trivial_row.get::<String, _>(0), "prefilter_trivial");

    // Short: not linked at all
    let short_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
            .bind(short_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(short_count.0, 0);
}

/// Full pipeline smoke test: stub Python returns a task match; verify ticket_links,
/// session_dimensions, cursor, and agent_runs are all written correctly.
#[tokio::test]
async fn stub_python_writes_task_link_dimensions_and_cursor() {
    if !python3_available() {
        eprintln!("python3 not in PATH — skipping stub_python test");
        return;
    }

    let pool = common::make_meridian_db().await;
    let session_id =
        seed_session(&pool, "Xcode", 120, Some("implementing hermes bridge")).await;

    let stub_dir = stub_services_dir();
    let cfg = make_cfg_backfill(
        true,
        Some(stub_dir.path().to_str().unwrap().to_string()),
        true,
    );

    run_task_linking(&pool, &cfg).await.unwrap();

    // ticket_links row
    let link = sqlx::query(
        "SELECT task_key, method, session_type, routing, confidence
         FROM ticket_links WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .expect("ticket_link must be written");
    assert_eq!(link.get::<String, _>(0), "KAN-99");
    assert_eq!(link.get::<String, _>(1), "llm_standalone");
    assert_eq!(link.get::<String, _>(2), "task");
    assert_eq!(link.get::<String, _>(3), "auto");
    assert!((link.get::<f64, _>(4) - 0.92).abs() < 1e-9);

    // dimensions: activity=coding, tool=cargo (2 rows)
    let dim_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM session_dimensions WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dim_count.0, 2);

    let activity: (String,) = sqlx::query_as(
        "SELECT value FROM session_dimensions WHERE session_id = ? AND dimension = 'activity'",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activity.0, "coding");

    // cursor advanced to this session
    let cursor: (i64,) =
        sqlx::query_as("SELECT last_session_id FROM agent_cursor WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cursor.0, session_id);

    // agent_run audit row is success
    let run = sqlx::query(
        "SELECT status, sessions_processed, links_written
         FROM agent_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>(0), "success");
    assert_eq!(run.get::<i64, _>(1), 1);
    assert_eq!(run.get::<i64, _>(2), 1);
}

/// Second run on already-processed sessions is a no-op (cursor has advanced).
#[tokio::test]
async fn second_run_is_idle_when_cursor_is_current() {
    if !python3_available() {
        eprintln!("python3 not in PATH — skipping second_run test");
        return;
    }

    let pool = common::make_meridian_db().await;
    let session_id =
        seed_session(&pool, "VSCode", 120, Some("adding unit tests")).await;

    let stub_dir = stub_services_dir();
    let cfg = make_cfg_backfill(
        true,
        Some(stub_dir.path().to_str().unwrap().to_string()),
        true,
    );

    run_task_linking(&pool, &cfg).await.unwrap(); // first run classifies
    run_task_linking(&pool, &cfg).await.unwrap(); // second run: nothing new

    let link_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(link_count.0, 1, "session must not be linked twice");

    let run_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(run_count.0, 2, "each call creates one agent_run row");
}

// ---------------------------------------------------------------------------
// No-backfill tests (default daemon behavior)
// ---------------------------------------------------------------------------

/// On first run with backfill disabled, cursor jumps to current max and
/// no historical sessions are classified.
#[tokio::test]
async fn no_backfill_skips_existing_sessions_on_first_run() {
    let pool = common::make_meridian_db().await;
    // Seed sessions that look classifiable
    seed_session(&pool, "Xcode", 120, Some("old work")).await;
    seed_session(&pool, "VSCode", 90, Some("more old work")).await;

    let cfg = make_cfg_backfill(true, Some("/nonexistent/path".to_string()), false);
    run_task_linking(&pool, &cfg).await.unwrap();

    // Nothing classified — cursor jumped past everything
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ticket_links")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "existing sessions must not be classified when backfill=false");

    // Cursor must have advanced to the max session id
    let cursor: (i64,) =
        sqlx::query_as("SELECT last_session_id FROM agent_cursor WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let max_id: (i64,) = sqlx::query_as("SELECT MAX(id) FROM app_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cursor.0, max_id.0);
}

/// After the initial cursor advance, sessions added afterwards ARE classified.
#[tokio::test]
async fn no_backfill_classifies_new_sessions_after_first_run() {
    if !python3_available() {
        eprintln!("python3 not in PATH — skipping no_backfill_new_sessions test");
        return;
    }

    let pool = common::make_meridian_db().await;
    // Pre-existing session — must NOT be classified
    seed_session(&pool, "Slack", 60, Some("old stuff")).await;

    let stub_dir = stub_services_dir();
    let cfg = make_cfg_backfill(
        true,
        Some(stub_dir.path().to_str().unwrap().to_string()),
        false,
    );

    // First run: cursor jumps to max, nothing classified
    run_task_linking(&pool, &cfg).await.unwrap();
    let count_after_first: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ticket_links")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_after_first.0, 0);

    // New session arrives after the cursor advance
    let new_id = seed_session(&pool, "Xcode", 120, Some("new work after cursor")).await;

    // Second run: only the new session is classified
    run_task_linking(&pool, &cfg).await.unwrap();

    let count_new: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
            .bind(new_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count_new.0, 1, "new session added after first run must be classified");
}
