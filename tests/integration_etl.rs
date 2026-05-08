// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use meridian::db::meridian::{
    cleanup_incomplete_runs, get_active_session, get_cursor, insert_etl_run,
};
use meridian::etl::run_etl;
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;

/// Inserts frames where each entry is `(app_name, timestamp, capture_trigger)`.
/// `app_name` may be `None` to insert a NULL (hidden from ETL's get_frames_since
/// but still visible to count_frames_in_window for gap classification).
/// `capture_trigger` may be `None` to leave the column NULL.
/// IDs are assigned sequentially starting from `id_offset`.
async fn insert_frames_with_trigger(
    pool: &SqlitePool,
    id_offset: i64,
    frames: &[(Option<&str>, &str, Option<&str>)],
) {
    for (i, (app, ts, trigger)) in frames.iter().enumerate() {
        sqlx::query(
            "INSERT INTO frames (id, app_name, window_name, timestamp, capture_trigger)
             VALUES (?, ?, NULL, ?, ?)",
        )
        .bind(id_offset + i as i64)
        .bind(app)
        .bind(ts)
        .bind(trigger)
        .execute(pool)
        .await
        .unwrap();
    }
}

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
            timestamp TEXT NOT NULL,
            capture_trigger TEXT
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

#[tokio::test]
async fn test_gap_detection() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // Two frames separated by a 10-minute gap (600 s > GAP_THRESHOLD_SECS=300).
    // No frames exist inside the gap window, so it should be classified as
    // 'system_sleep' (total_count == 0, therefore idle condition is false).
    insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:10:00+00:00"),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    // There should be exactly one gap row recorded.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "expected exactly one gap row");

    let row: (String, i64, String) =
        sqlx::query_as("SELECT kind, duration_s, started_at FROM gaps LIMIT 1")
            .fetch_one(&md)
            .await
            .unwrap();
    assert_eq!(row.0, "system_sleep", "gap should be classified as system_sleep");
    assert_eq!(row.1, 600, "gap duration should be 600 seconds");
    assert_eq!(row.2, "2026-01-01T10:00:00+00:00", "gap started_at should match first frame ts");
}

// ---------------------------------------------------------------------------
// Gap classification — user_idle
// ---------------------------------------------------------------------------

/// Two processed frames 400 s apart create a gap.  Extra screenpipe rows with
/// NULL app_name are inserted inside the gap window so count_frames_in_window
/// can see them (it counts ALL frames regardless of app_name) without the ETL
/// runner processing them (get_frames_since filters out NULL/empty app_name).
/// 3 of 4 in-gap rows have capture_trigger='idle' (75% ≥ 50%).
/// ETL must classify the gap as 'user_idle'.
#[tokio::test]
async fn test_gap_classification_user_idle() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // Processed frames: ids 1 and 2, 400 s apart.
    // Gap window queried by count_frames_in_window:
    //   timestamp > '10:00:00' AND timestamp <= '10:06:40'
    // In-gap rows (ids 3-6): NULL app_name → skipped by get_frames_since,
    //   but counted by count_frames_in_window.  3 idle, 1 non-idle.
    //   idle * 2 = 6 >= total (4) → user_idle.
    insert_frames_with_trigger(
        &sp,
        1, // id_offset
        &[
            (Some("Terminal"), "2026-01-01T10:00:00+00:00", None), // id=1, pre-gap
            (Some("Terminal"), "2026-01-01T10:06:40+00:00", None), // id=2, post-gap (400 s later)
        ],
    )
    .await;

    // Insert the in-gap classification frames with NULL app_name.
    insert_frames_with_trigger(
        &sp,
        3, // id_offset — must not clash with ids 1-2
        &[
            (None, "2026-01-01T10:01:00+00:00", Some("idle")),
            (None, "2026-01-01T10:02:00+00:00", Some("idle")),
            (None, "2026-01-01T10:03:00+00:00", Some("idle")),
            (None, "2026-01-01T10:04:00+00:00", None), // not idle
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "expected exactly one gap row");

    let kind: (String,) = sqlx::query_as("SELECT kind FROM gaps LIMIT 1")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(kind.0, "user_idle", "gap with ≥50% idle frames must be classified as user_idle");
}

// ---------------------------------------------------------------------------
// Gap classification — system_sleep
// ---------------------------------------------------------------------------

/// Two processed frames 400 s apart with NO frames at all in the gap window.
/// count_frames_in_window returns (0, 0), so the gap must be 'system_sleep'.
#[tokio::test]
async fn test_gap_classification_system_sleep() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // Only two frames, nothing in between — no in-gap rows at all.
    insert_frames_with_trigger(
        &sp,
        1,
        &[
            (Some("Chrome"), "2026-01-01T12:00:00+00:00", None),
            (Some("Chrome"), "2026-01-01T12:06:40+00:00", None), // 400 s later
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "expected exactly one gap row");

    let kind: (String,) = sqlx::query_as("SELECT kind FROM gaps LIMIT 1")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        kind.0, "system_sleep",
        "gap with no frames in window must be classified as system_sleep"
    );
}

// ---------------------------------------------------------------------------
// Session duration excludes gap time
// ---------------------------------------------------------------------------

/// One app block before a 400 s gap, same app resumes after.
/// Pre-gap block: 10:00:00 → 10:00:30 (30 s).
/// ETL closes the pre-gap block with ended_at = last frame before the gap
/// (10:00:30), so duration_s must equal 30 — not the 430 s wall-clock span
/// that would result from including the gap.
#[tokio::test]
async fn test_session_duration_excludes_gap() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // Pre-gap block: 10:00:00 → 10:00:30  (30 s block, 4 frames)
    // 400 s gap (no in-gap frames)
    // Post-gap block: 10:07:10 onwards (stays open as active_session)
    insert_frames_with_trigger(
        &sp,
        1,
        &[
            (Some("VSCode"), "2026-01-01T10:00:00+00:00", None),
            (Some("VSCode"), "2026-01-01T10:00:10+00:00", None),
            (Some("VSCode"), "2026-01-01T10:00:20+00:00", None),
            (Some("VSCode"), "2026-01-01T10:00:30+00:00", None), // last pre-gap frame
            // 400 s gap — no frames
            (Some("VSCode"), "2026-01-01T10:07:10+00:00", None), // first post-gap
            (Some("VSCode"), "2026-01-01T10:07:20+00:00", None),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    // The pre-gap block must be closed into app_sessions.
    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1, "the pre-gap block should be closed into app_sessions");

    // duration_s must reflect only the 30 s pre-gap span.
    let duration: (i64,) = sqlx::query_as("SELECT duration_s FROM app_sessions LIMIT 1")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        duration.0, 30,
        "session duration must be pre-gap block time (30 s) and must not include the 400 s gap"
    );
}

// ---------------------------------------------------------------------------
// Option C — ui_event refines session ended_at on app switch
// ---------------------------------------------------------------------------

/// Scenario: Terminal runs from 10:00:00 to 10:01:00 (two frames), then Chrome
/// appears at 10:02:00. A ui_event click at 10:01:45 (45 s after the last Terminal
/// frame, 15 s before Chrome) should become the Terminal session's ended_at.
///
/// Expected:
///   ended_at  = "2026-01-01T10:01:45+00:00"  (ui_event time, not 10:01:00)
///   duration_s = 105                          (10:01:45 − 10:00:00)
#[tokio::test]
async fn test_ui_event_refines_session_end() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // Two Terminal frames followed by one Chrome frame (triggers app-switch close).
    insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:01:00+00:00"),
            ("Chrome", "2026-01-01T10:02:00+00:00"),
        ],
    )
    .await;

    // ui_event: click in Terminal at 10:01:45 — AFTER the last Terminal frame.
    sqlx::query(
        "INSERT INTO ui_events (id, timestamp, event_type, app_name)
         VALUES (1, '2026-01-01T10:01:45+00:00', 'click', 'Terminal')",
    )
    .execute(&sp)
    .await
    .unwrap();

    run_etl(&sp, &md).await.unwrap();

    // Terminal block must be closed; Chrome stays open.
    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1, "Terminal block should be closed into app_sessions");

    let row: (String, i64) =
        sqlx::query_as("SELECT ended_at, duration_s FROM app_sessions LIMIT 1")
            .fetch_one(&md)
            .await
            .unwrap();

    assert!(
        row.0.starts_with("2026-01-01T10:01:45"),
        "ended_at should be the ui_event timestamp (10:01:45), got: {}",
        row.0
    );
    assert_eq!(
        row.1, 105,
        "duration_s should be 105 s (10:01:45 − 10:00:00), got: {}",
        row.1
    );
}

/// Scenario: Terminal runs from 10:00:00 to 10:01:30 (two frames), then Chrome
/// appears at 10:02:00. A ui_event click at 10:00:45 is BEFORE the last Terminal
/// frame (10:01:30). Option C must NOT use it — the session end must remain at
/// the last frame timestamp.
///
/// Expected:
///   ended_at  = "2026-01-01T10:01:30+00:00"  (last frame, NOT the earlier ui_event)
///   duration_s = 90                           (10:01:30 − 10:00:00)
#[tokio::test]
async fn test_ui_event_before_last_frame_ignored() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // Two Terminal frames followed by one Chrome frame.
    insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:01:30+00:00"),
            ("Chrome", "2026-01-01T10:02:00+00:00"),
        ],
    )
    .await;

    // ui_event: click in Terminal at 10:00:45 — BEFORE the last Terminal frame.
    sqlx::query(
        "INSERT INTO ui_events (id, timestamp, event_type, app_name)
         VALUES (1, '2026-01-01T10:00:45+00:00', 'click', 'Terminal')",
    )
    .execute(&sp)
    .await
    .unwrap();

    run_etl(&sp, &md).await.unwrap();

    // Terminal block must be closed.
    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1, "Terminal block should be closed into app_sessions");

    let row: (String, i64) =
        sqlx::query_as("SELECT ended_at, duration_s FROM app_sessions LIMIT 1")
            .fetch_one(&md)
            .await
            .unwrap();

    assert!(
        row.0.starts_with("2026-01-01T10:01:30"),
        "ended_at should be the last frame timestamp (10:01:30), not the earlier ui_event; got: {}",
        row.0
    );
    assert_eq!(
        row.1, 90,
        "duration_s should be 90 s (10:01:30 − 10:00:00), got: {}",
        row.1
    );
}

// ---------------------------------------------------------------------------
// No gap below threshold
// ---------------------------------------------------------------------------

/// Two frames only 299 s apart — strictly below GAP_THRESHOLD_SECS (300).
/// No gap row should be inserted.
#[tokio::test]
async fn test_no_gap_below_threshold() {
    let sp = make_screenpipe_db().await;
    let md = make_meridian_db().await;

    // 299 s separation — strictly less than the 300 s threshold.
    insert_frames_with_trigger(
        &sp,
        1,
        &[
            (Some("Finder"), "2026-01-01T09:00:00+00:00", None),
            (Some("Finder"), "2026-01-01T09:04:59+00:00", None), // 299 s later
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "a 299 s gap is below threshold and must not produce a gap row");
}
