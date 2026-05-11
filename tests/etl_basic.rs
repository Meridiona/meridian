// meridian — normalises screenpipe activity into structured app sessions

//! Basic ETL pipeline tests: empty DB, single-app, app-switch, cursor, cleanup.

mod common;

use meridian::db::meridian::{
    cleanup_incomplete_runs, get_active_session, get_cursor, insert_etl_run, insert_gap,
};
use meridian::etl::run_etl;

// ---------------------------------------------------------------------------
// Empty input
// ---------------------------------------------------------------------------

/// An empty screenpipe DB must leave the cursor at 0 and create no sessions.
#[tokio::test]
async fn test_empty_screenpipe() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    run_etl(&sp, &md).await.unwrap();

    let cursor = get_cursor(&md).await.unwrap();
    assert_eq!(cursor.last_frame_id, 0);

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

// ---------------------------------------------------------------------------
// Single app — no switch
// ---------------------------------------------------------------------------

/// All frames from the same app: no session is closed, one active_session row
/// is written with the correct frame count.
#[tokio::test]
async fn test_single_app_no_switch() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames(
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

    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 0, "no app switch → no closed sessions");

    let active = get_active_session(&md).await.unwrap().unwrap();
    assert_eq!(active.app_name, "Terminal");
    assert_eq!(active.frame_count, 5);
}

// ---------------------------------------------------------------------------
// App switch
// ---------------------------------------------------------------------------

/// When app_name changes the old block is closed into app_sessions.
/// The new app stays open in active_session.
#[tokio::test]
async fn test_app_switch_creates_session() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames(
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

    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1, "Terminal block should be closed");

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

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// After a run the cursor must advance to the id of the last processed frame.
#[tokio::test]
async fn test_cursor_advances_to_last_frame() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Chrome", "2026-01-01T10:00:10+00:00"),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let cursor = get_cursor(&md).await.unwrap();
    assert_eq!(cursor.last_frame_id, 2);
}

// ---------------------------------------------------------------------------
// Cleanup incomplete runs
// ---------------------------------------------------------------------------

/// `cleanup_incomplete_runs` must delete all sessions belonging to 'running'
/// ETL runs and mark those runs as 'aborted'.
#[tokio::test]
async fn test_cleanup_incomplete_runs() {
    let md = common::make_meridian_db().await;

    let run_id = insert_etl_run(&md, 0, 10).await.unwrap();
    for (app, start, end) in [
        (
            "Terminal",
            "2026-01-01T10:00:00+00:00",
            "2026-01-01T10:00:10+00:00",
        ),
        (
            "Chrome",
            "2026-01-01T10:00:11+00:00",
            "2026-01-01T10:00:20+00:00",
        ),
    ] {
        sqlx::query(
            "INSERT INTO app_sessions
             (app_name, started_at, ended_at, duration_s, window_titles,
              min_frame_id, max_frame_id, frame_count, etl_run_id)
             VALUES (?, ?, ?, 10, '[]', 1, 3, 3, ?)",
        )
        .bind(app)
        .bind(start)
        .bind(end)
        .bind(run_id)
        .execute(&md)
        .await
        .unwrap();
    }

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

    let gaps_after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(gaps_after.0, 0, "gaps for aborted run must be deleted");
}

// ---------------------------------------------------------------------------
// Cleanup also removes gaps written by aborted runs
// ---------------------------------------------------------------------------

/// An aborted run may have written gap rows before crashing. Those rows must
/// be deleted by cleanup_incomplete_runs so they are not duplicated when the
/// run restarts from the same cursor position.
#[tokio::test]
async fn test_cleanup_removes_aborted_gaps() {
    let md = common::make_meridian_db().await;

    let run_id = insert_etl_run(&md, 0, 10).await.unwrap();
    insert_gap(
        &md,
        "2026-01-01T10:00:00+00:00",
        "2026-01-01T10:10:00+00:00",
        600,
        "system_sleep",
        run_id,
    )
    .await
    .unwrap();
    insert_gap(
        &md,
        "2026-01-01T11:00:00+00:00",
        "2026-01-01T11:08:00+00:00",
        480,
        "user_idle",
        run_id,
    )
    .await
    .unwrap();

    let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(before.0, 2);

    cleanup_incomplete_runs(&md).await.unwrap();

    let after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(after.0, 0, "all gaps from the aborted run must be removed");
}
