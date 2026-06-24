//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Gap detection and classification tests.
//!
//! Covers:
//! - Gap is recorded when inter-frame distance > 300 s
//! - Gap with ≥50 % idle frames → user_idle
//! - Gap with 0 frames in window → system_sleep
//! - Session duration never includes gap time
//! - Gaps exactly at or below threshold are not recorded

mod common;

use meridian::etl::run_etl;

// ---------------------------------------------------------------------------
// Basic gap detection
// ---------------------------------------------------------------------------

/// Two frames 600 s apart with nothing in between → one system_sleep gap row.
#[tokio::test]
async fn test_gap_detection() {
    let md = common::make_meridian_db().await;

    common::insert_frames(
        &md,
        &[
            ("Terminal", "2026-01-01T10:00:00+00:00"),
            ("Terminal", "2026-01-01T10:10:00+00:00"), // 600 s gap
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

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
    assert_eq!(row.0, "system_sleep");
    assert_eq!(row.1, 600);
    assert_eq!(row.2, "2026-01-01T10:00:00+00:00");
}

// ---------------------------------------------------------------------------
// Gap classification — user_idle
// ---------------------------------------------------------------------------

/// Two processed frames 400 s apart; 3 of 4 in-gap frames have
/// capture_trigger='idle' (75 % ≥ 50 %) → gap must be user_idle.
///
/// In-gap frames have NULL app_name so they are skipped by `get_frames_since`
/// but counted by `count_frames_in_window`.
#[tokio::test]
async fn test_gap_classification_user_idle() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_trigger(
        &md,
        1,
        &[
            (Some("Terminal"), "2026-01-01T10:00:00+00:00", None), // pre-gap
            (Some("Terminal"), "2026-01-01T10:06:40+00:00", None), // post-gap (400 s)
        ],
    )
    .await;

    // In-gap NULL-app frames: 3 idle, 1 non-idle → idle*2=6 >= total(4) → user_idle.
    common::insert_frames_with_trigger(
        &md,
        3,
        &[
            (None, "2026-01-01T10:01:00+00:00", Some("idle")),
            (None, "2026-01-01T10:02:00+00:00", Some("idle")),
            (None, "2026-01-01T10:03:00+00:00", Some("idle")),
            (None, "2026-01-01T10:04:00+00:00", None),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "expected exactly one gap row");

    let kind: (String,) = sqlx::query_as("SELECT kind FROM gaps LIMIT 1")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(kind.0, "user_idle", "≥50% idle frames must yield user_idle");
}

// ---------------------------------------------------------------------------
// Gap classification — system_sleep
// ---------------------------------------------------------------------------

/// Two processed frames 400 s apart with NO frames at all in the gap window.
/// count_frames_in_window returns (0, 0) → must be system_sleep.
#[tokio::test]
async fn test_gap_classification_system_sleep() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_trigger(
        &md,
        1,
        &[
            (Some("Chrome"), "2026-01-01T12:00:00+00:00", None),
            (Some("Chrome"), "2026-01-01T12:06:40+00:00", None), // 400 s later
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    let kind: (String,) = sqlx::query_as("SELECT kind FROM gaps LIMIT 1")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        kind.0, "system_sleep",
        "zero in-gap frames must yield system_sleep"
    );
}

// ---------------------------------------------------------------------------
// Session duration excludes gap time
// ---------------------------------------------------------------------------

/// Pre-gap block (30 s), then a 400 s gap, then the same app resumes.
/// The closed session's duration_s must be 30, not 430.
#[tokio::test]
async fn test_session_duration_excludes_gap() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_trigger(
        &md,
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

    run_etl(&md).await.unwrap();

    let closed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        closed.0, 1,
        "the pre-gap block should be closed into app_sessions"
    );

    let duration: (i64,) = sqlx::query_as("SELECT duration_s FROM app_sessions LIMIT 1")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        duration.0, 30,
        "duration must be 30 s (pre-gap only), not 430 s including the gap"
    );
}

// ---------------------------------------------------------------------------
// No gap below threshold
// ---------------------------------------------------------------------------

/// Two frames 299 s apart — strictly below GAP_THRESHOLD_SECS (300).
/// No gap row must be inserted.
#[tokio::test]
async fn test_no_gap_below_threshold() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_trigger(
        &md,
        1,
        &[
            (Some("Finder"), "2026-01-01T09:00:00+00:00", None),
            (Some("Finder"), "2026-01-01T09:04:59+00:00", None), // 299 s later
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        count.0, 0,
        "a 299 s gap is below threshold — no gap row expected"
    );
}
