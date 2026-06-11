//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Option C tests: ui_event timestamp can refine a session's ended_at on app switch.
//!
//! Option C fires when a ui_event click/key/text is strictly after the last
//! screen frame for that app and strictly before the next app's first frame.
//! A ui_event that is at or before the last frame must be ignored.

mod common;

use meridian::etl::run_etl;

// ---------------------------------------------------------------------------
// Option C applies — ui_event is after the last frame
// ---------------------------------------------------------------------------

/// Terminal runs 10:00:00–10:01:00, Chrome starts at 10:02:00.
/// A click at 10:01:45 (after the last Terminal frame) must become ended_at.
///
/// Expected: ended_at = 10:01:45, duration_s = 105.
#[tokio::test]
async fn test_ui_event_refines_session_end() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:01:00+00:00"),
            ("Chrome", "2026-01-01T10:02:00+00:00"),
        ],
    )
    .await;

    sqlx::query(
        "INSERT INTO ui_events (id, timestamp, event_type, app_name)
         VALUES (1, '2026-01-01T10:01:45+00:00', 'click', 'Terminal')",
    )
    .execute(&sp)
    .await
    .unwrap();

    run_etl(&sp, &md).await.unwrap();

    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1, "Terminal block should be closed");

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
        "duration_s should be 105 s (10:01:45 − 10:00:00)"
    );
}

// ---------------------------------------------------------------------------
// Option C does not apply — ui_event is before the last frame
// ---------------------------------------------------------------------------

/// Terminal runs 10:00:00–10:01:30, Chrome starts at 10:02:00.
/// A click at 10:00:45 is BEFORE the last Terminal frame (10:01:30).
/// Option C must NOT fire (ui_event is not more recent than the last frame).
/// Extended Option D fires instead: ended_at is advanced to next_frame_ts
/// (Chrome's first frame at 10:02:00) to recover the inter-frame gap.
///
/// Expected: ended_at = 10:02:00, duration_s = 120.
#[tokio::test]
async fn test_ui_event_before_last_frame_ignored() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames(
        &sp,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:01:30+00:00"),
            ("Chrome", "2026-01-01T10:02:00+00:00"),
        ],
    )
    .await;

    sqlx::query(
        "INSERT INTO ui_events (id, timestamp, event_type, app_name)
         VALUES (1, '2026-01-01T10:00:45+00:00', 'click', 'Terminal')",
    )
    .execute(&sp)
    .await
    .unwrap();

    run_etl(&sp, &md).await.unwrap();

    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(closed.0, 1, "Terminal block should be closed");

    let row: (String, i64) =
        sqlx::query_as("SELECT ended_at, duration_s FROM app_sessions LIMIT 1")
            .fetch_one(&md)
            .await
            .unwrap();

    assert!(
        row.0.starts_with("2026-01-01T10:02:00"),
        "ended_at must be next_frame_ts (10:02:00) — Option C did not fire, Option D advanced it; got: {}",
        row.0
    );
    assert_eq!(
        row.1, 120,
        "duration_s should be 120 s (10:02:00 − 10:00:00)"
    );
}
