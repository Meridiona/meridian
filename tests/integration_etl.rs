// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use meridian::db::meridian::{
    cleanup_incomplete_runs, get_active_session, get_cursor, insert_etl_run,
};
use meridian::etl::run_etl;
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;

async fn make_meridian_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    pool
}

async fn make_screenpipe_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::query(
        "CREATE TABLE frames (
            id INTEGER PRIMARY KEY,
            app_name TEXT,
            window_name TEXT,
            timestamp TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TABLE ocr_text (id INTEGER PRIMARY KEY, frame_id INTEGER, text TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE elements (
            id INTEGER PRIMARY KEY,
            frame_id INTEGER,
            text TEXT,
            role TEXT,
            source TEXT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE audio_transcriptions (
            id INTEGER PRIMARY KEY,
            audio_chunk_id INTEGER NOT NULL DEFAULT 0,
            offset_index INTEGER NOT NULL DEFAULT 0,
            timestamp TEXT NOT NULL,
            transcription TEXT NOT NULL,
            device TEXT NOT NULL DEFAULT '',
            is_input_device BOOLEAN NOT NULL DEFAULT 1,
            speaker_id INTEGER,
            transcription_engine TEXT NOT NULL DEFAULT 'Whisper'
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE ui_events (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            session_id TEXT,
            relative_ms INTEGER NOT NULL DEFAULT 0,
            event_type TEXT NOT NULL,
            text_content TEXT,
            app_name TEXT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn insert_frames(pool: &SqlitePool, frames: &[(&str, &str)]) {
    for (i, (app, ts)) in frames.iter().enumerate() {
        sqlx::query(
            "INSERT INTO frames (id, app_name, window_name, timestamp) VALUES (?, ?, NULL, ?)",
        )
        .bind(i as i64 + 1)
        .bind(app)
        .bind(ts)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn test_empty_screenpipe() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    run_etl(&sp, &md).await.unwrap();

    let cursor = get_cursor(&md).await.unwrap();
    assert_eq!(cursor.last_frame_id, 0);

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn test_single_app_no_switch() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:00:10+00:00"),
            ("Terminal", "2026-01-01T10:00:20+00:00"),
            ("Terminal", "2026-01-01T10:00:30+00:00"),
            ("Terminal", "2026-01-01T10:00:40+00:00"),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    // No app switch → 0 closed sessions, 1 open active_session
    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 0);

    let active = get_active_session(&md).await.unwrap();
    assert!(active.is_some());
    let active = active.unwrap();
    assert_eq!(active.app_name, "Terminal");
    assert_eq!(active.frame_count, 5);
}

#[tokio::test]
async fn test_app_switch_creates_session() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:00:10+00:00"),
            ("Terminal", "2026-01-01T10:00:20+00:00"),
            ("Chrome", "2026-01-01T10:00:30+00:00"),
            ("Chrome", "2026-01-01T10:00:40+00:00"),
            ("Chrome", "2026-01-01T10:00:50+00:00"),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    // Terminal block closed, Chrome stays open
    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1);

    let row: (String, i64) =
        sqlx::query_as("SELECT app_name, frame_count FROM app_sessions LIMIT 1")
            .fetch_one(&md)
            .await
            .unwrap();
    assert_eq!(row.0, "Terminal");
    assert_eq!(row.1, 3);

    let active = get_active_session(&md).await.unwrap().unwrap();
    assert_eq!(active.app_name, "Chrome");
    assert_eq!(active.frame_count, 3);
}

#[tokio::test]
async fn test_cursor_advances_to_last_frame() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Chrome", "2026-01-01T10:00:10+00:00"),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let cursor = get_cursor(&md).await.unwrap();
    assert_eq!(cursor.last_frame_id, 2); // last inserted frame id
}

#[tokio::test]
async fn test_cleanup_incomplete_runs() {
    let md = make_meridian_db().await;

    // Simulate a stale run: insert a run with status='running' and two sessions for it
    let run_id = insert_etl_run(&md, 0, 10).await.unwrap();
    sqlx::query(
        "INSERT INTO app_sessions
         (app_name, started_at, ended_at, duration_s, window_titles,
          min_frame_id, max_frame_id, frame_count, etl_run_id)
         VALUES ('Terminal', '2026-01-01T10:00:00+00:00', '2026-01-01T10:00:10+00:00',
                 10, '[]', 1, 3, 3, ?)",
    )
    .bind(run_id)
    .execute(&md)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO app_sessions
         (app_name, started_at, ended_at, duration_s, window_titles,
          min_frame_id, max_frame_id, frame_count, etl_run_id)
         VALUES ('Chrome', '2026-01-01T10:00:11+00:00', '2026-01-01T10:00:20+00:00',
                 9, '[]', 4, 6, 3, ?)",
    )
    .bind(run_id)
    .execute(&md)
    .await
    .unwrap();

    let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(before.0, 2);

    let deleted = cleanup_incomplete_runs(&md).await.unwrap();
    assert_eq!(deleted, 2);

    let after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(after.0, 0);

    let status: (String,) = sqlx::query_as("SELECT status FROM etl_runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(status.0, "aborted");
}
