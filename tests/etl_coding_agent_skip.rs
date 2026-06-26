//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Integration tests for the coding-agent terminal frame skip.
//!
//! The skip suppresses VS Code frames whose terminal tab is running a coding
//! agent (Claude Code, Codex, …) to prevent double-counting with the indexer.
//! Key invariants tested:
//!
//! - Skipped frames advance the gap-detector clock — more than 300 s of
//!   back-to-back coding-agent frames must NOT produce a spurious gap row.
//! - The surrounding VS Code session is not split by skipped frames.
//! - Normal editor frames (non-terminal or non-agent tab) are captured as usual.

mod common;

use meridian::etl::run_etl;

// ---------------------------------------------------------------------------
// No spurious gap from skipped coding-agent frames (#1 from review)
// ---------------------------------------------------------------------------

/// Layout:
///   T=0    Code / editor tab           → opens block
///   T=30…360  Code / "Terminal - claude" → skipped (coding agent)
///   T=390  Code / editor tab           → continues same block
///
/// Without the clock-advance fix, the skip from T=0 to T=360 looks like a
/// 360 s gap when the editor frame at T=390 arrives → spurious system_sleep.
/// With the fix, block_last_ts advances on every skip → gap(T=360, T=390) = 30 s
/// → no gap row, session is never split.
#[tokio::test]
async fn coding_agent_skip_does_not_produce_spurious_gap() {
    let md = common::make_meridian_db().await;

    // Editor frame at T=0
    common::insert_frames_with_window(
        &md,
        1,
        &[(
            "Code",
            "main.rs \u{2014} myproject",
            "2026-01-01T10:00:00+00:00",
        )],
    )
    .await;

    // Coding-agent terminal frames spanning 330 s (> GAP_THRESHOLD of 300 s)
    // from the editor frame above.
    let agent_frames: Vec<(&str, &str, &str)> = (1..=11)
        .map(|i| {
            // Static lifetimes not possible in closures, so we'll build timestamps below.
            let _ = i;
            ("Code", "Terminal - claude", "placeholder")
        })
        .collect();
    // Build individual inserts at T=30, 60, ..., 330 s.
    for i in 1i64..=11 {
        let ts = format!(
            "2026-01-01T10:{:02}:{:02}+00:00",
            (i * 30) / 60,
            (i * 30) % 60
        );
        sqlx::query(
            "INSERT INTO capture_frames (id, app_name, window_name, timestamp) VALUES (?, ?, ?, ?)",
        )
        .bind(1 + i)
        .bind("Code")
        .bind("Terminal - claude")
        .bind(&ts)
        .execute(&md)
        .await
        .unwrap();
    }
    let _ = agent_frames; // suppress unused warning

    // Editor frame at T=390 (30 s after last coding-agent frame at T=330)
    sqlx::query(
        "INSERT INTO capture_frames (id, app_name, window_name, timestamp) VALUES (?, ?, ?, ?)",
    )
    .bind(13i64)
    .bind("Code")
    .bind("main.rs \u{2014} myproject")
    .bind("2026-01-01T10:06:30+00:00")
    .execute(&md)
    .await
    .unwrap();

    run_etl(&md).await.unwrap();

    // No spurious gap row — the clock advanced through the skipped frames.
    let gap_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gaps")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        gap_count.0, 0,
        "no gap row expected — skip advances the clock"
    );

    // The VS Code session is open (still in active_session, not split into
    // multiple app_sessions rows).
    let session_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_sessions")
        .fetch_one(&md)
        .await
        .unwrap();
    assert_eq!(
        session_count.0, 0,
        "session should stay in active_session, not be closed into app_sessions"
    );

    let active: Option<(String,)> =
        sqlx::query_as("SELECT app_name FROM active_session WHERE id = 1")
            .fetch_optional(&md)
            .await
            .unwrap();
    assert_eq!(
        active.map(|r| r.0).as_deref(),
        Some("Code"),
        "VS Code session should be open in active_session"
    );
}

// ---------------------------------------------------------------------------
// Normal editor frames are still captured
// ---------------------------------------------------------------------------

/// Editor-only frames (no terminal tab) must pass through unaffected.
#[tokio::test]
async fn editor_frames_not_suppressed() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_window(
        &md,
        1,
        &[
            (
                "Code",
                "main.rs \u{2014} myproject",
                "2026-01-01T10:00:00+00:00",
            ),
            (
                "Code",
                "lib.rs \u{2014} myproject",
                "2026-01-01T10:00:02+00:00",
            ),
            (
                "Code",
                "main.rs \u{2014} myproject",
                "2026-01-01T10:00:04+00:00",
            ),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let active: Option<(String, i64)> =
        sqlx::query_as("SELECT app_name, frame_count FROM active_session WHERE id = 1")
            .fetch_optional(&md)
            .await
            .unwrap();
    let (app, frames) = active.expect("active_session should be set");
    assert_eq!(app, "Code");
    assert_eq!(frames, 3, "all 3 editor frames should be counted");
}

// ---------------------------------------------------------------------------
// Coding-agent frames are excluded from frame_count
// ---------------------------------------------------------------------------

/// Coding-agent terminal frames must not inflate the VS Code session's
/// frame_count — only editor frames count.
#[tokio::test]
async fn coding_agent_frames_excluded_from_frame_count() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_window(
        &md,
        1,
        &[
            (
                "Code",
                "main.rs \u{2014} myproject",
                "2026-01-01T10:00:00+00:00",
            ),
            ("Code", "Terminal - claude", "2026-01-01T10:00:02+00:00"),
            ("Code", "Terminal - claude", "2026-01-01T10:00:04+00:00"),
            (
                "Code",
                "main.rs \u{2014} myproject",
                "2026-01-01T10:00:06+00:00",
            ),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let active: Option<(i64,)> =
        sqlx::query_as("SELECT frame_count FROM active_session WHERE id = 1")
            .fetch_optional(&md)
            .await
            .unwrap();
    assert_eq!(
        active.map(|r| r.0),
        Some(2),
        "only the 2 editor frames should count — coding-agent frames are skipped"
    );
}
