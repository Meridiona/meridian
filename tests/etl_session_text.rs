//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Integration tests for session_text — end-to-end verification that the
//! ordered-set union of frame full_text is built, merged, and persisted
//! correctly through the ETL pipeline.

mod common;

use meridian::etl::run_etl;

// Helper: count "[HH:MM:SS]" marker lines in a session_text string.
fn count_markers(text: &str) -> usize {
    text.lines()
        .filter(|l| {
            l.len() == 10
                && l.starts_with('[')
                && l.ends_with(']')
                && &l[3..4] == ":"
                && &l[6..7] == ":"
        })
        .count()
}

// ---------------------------------------------------------------------------
// Active session
// ---------------------------------------------------------------------------

/// Frames from a single app with distinct full_text populate active_session.session_text.
#[tokio::test]
async fn test_session_text_populated_in_active_session() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &md,
        1,
        &[
            ("Code", "2026-01-01T10:00:00+00:00", "fn main() {}"),
            ("Code", "2026-01-01T10:00:01+00:00", "let x = 1;"),
            ("Code", "2026-01-01T10:00:02+00:00", "println!(\"hi\");"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM active_session WHERE id = 1")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.expect("active_session.session_text must be set");
    assert!(
        text.contains("fn main() {}"),
        "must contain frame 1 content"
    );
    assert!(text.contains("let x = 1;"), "must contain frame 2 content");
    assert!(
        text.contains("println!(\"hi\");"),
        "must contain frame 3 content"
    );
}

// ---------------------------------------------------------------------------
// Closed session
// ---------------------------------------------------------------------------

/// An app switch forces a close; session_text must appear in app_sessions.
#[tokio::test]
async fn test_session_text_populated_in_closed_session() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &md,
        1,
        &[
            ("Code", "2026-01-01T10:00:00+00:00", "struct Foo {}"),
            (
                "Code",
                "2026-01-01T10:00:01+00:00",
                "impl Foo { fn bar() {} }",
            ),
            ("Code", "2026-01-01T10:00:02+00:00", "use std::io;"),
            ("Slack", "2026-01-01T10:00:03+00:00", "Channel: #general"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.expect("closed session must have session_text");
    assert!(text.contains("struct Foo {}"), "missing frame 1 content");
    assert!(
        text.contains("impl Foo { fn bar() {} }"),
        "missing frame 2 content"
    );
    assert!(text.contains("use std::io;"), "missing frame 3 content");
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

/// Five frames with identical full_text — session_text must contain each line exactly once.
#[tokio::test]
async fn test_session_text_no_duplicates_across_frames() {
    let md = common::make_meridian_db().await;

    let repeated_text = "Line A\nLine B";
    common::insert_frames_with_text(
        &md,
        1,
        &[
            ("Code", "2026-01-01T10:00:00+00:00", repeated_text),
            ("Code", "2026-01-01T10:00:01+00:00", repeated_text),
            ("Code", "2026-01-01T10:00:02+00:00", repeated_text),
            ("Code", "2026-01-01T10:00:03+00:00", repeated_text),
            ("Code", "2026-01-01T10:00:04+00:00", repeated_text),
            // app switch forces close
            ("Slack", "2026-01-01T10:00:05+00:00", "notifications"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.unwrap_or_default();
    let content_lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with('[') || l.len() != 10)
        .collect();

    let line_a_count = content_lines.iter().filter(|&&l| l == "Line A").count();
    let line_b_count = content_lines.iter().filter(|&&l| l == "Line B").count();
    assert_eq!(line_a_count, 1, "\"Line A\" must appear exactly once");
    assert_eq!(line_b_count, 1, "\"Line B\" must appear exactly once");
}

// ---------------------------------------------------------------------------
// Cross-run merge
// ---------------------------------------------------------------------------

/// Two ETL runs on the same open session — session_text must contain lines from both batches.
#[tokio::test]
async fn test_session_text_merged_across_etl_runs() {
    let md = common::make_meridian_db().await;

    // Batch 1: frames 1–3 (set A content)
    common::insert_frames_with_text(
        &md,
        1,
        &[
            ("Code", "2026-01-01T10:00:00+00:00", "set_a_line_one"),
            ("Code", "2026-01-01T10:00:01+00:00", "set_a_line_two"),
            ("Code", "2026-01-01T10:00:02+00:00", "set_a_line_three"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap(); // active_session built from set A

    // Batch 2: frames 4–6 (set B content, no overlap with set A)
    common::insert_frames_with_text(
        &md,
        4,
        &[
            ("Code", "2026-01-01T10:01:00+00:00", "set_b_line_one"),
            ("Code", "2026-01-01T10:01:01+00:00", "set_b_line_two"),
            ("Code", "2026-01-01T10:01:02+00:00", "set_b_line_three"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap(); // merge into active_session

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM active_session WHERE id = 1")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row
        .0
        .expect("active_session.session_text must be set after merge");

    // Set A lines must be present
    assert!(text.contains("set_a_line_one"), "missing set A line 1");
    assert!(text.contains("set_a_line_two"), "missing set A line 2");
    assert!(text.contains("set_a_line_three"), "missing set A line 3");

    // Set B lines must be present
    assert!(text.contains("set_b_line_one"), "missing set B line 1");
    assert!(text.contains("set_b_line_two"), "missing set B line 2");
    assert!(text.contains("set_b_line_three"), "missing set B line 3");

    // No duplicates — each line appears exactly once
    for line in &[
        "set_a_line_one",
        "set_a_line_two",
        "set_a_line_three",
        "set_b_line_one",
        "set_b_line_two",
        "set_b_line_three",
    ] {
        let count = text.matches(line).count();
        assert_eq!(count, 1, "'{line}' must appear exactly once in merged text");
    }
}

// ---------------------------------------------------------------------------
// Empty full_text
// ---------------------------------------------------------------------------

/// Frames with empty full_text produce no session_text content.
#[tokio::test]
async fn test_session_text_empty_when_no_full_text() {
    let md = common::make_meridian_db().await;

    // Insert 3 frames with blank full_text via raw SQL (insert_frames_with_text inserts
    // non-empty strings; use empty strings to test the filter path).
    for i in 0i64..3 {
        let ts = format!("2026-01-01T10:00:0{}+00:00", i);
        sqlx::query(
            "INSERT INTO capture_frames (id, app_name, window_name, timestamp, full_text, text_source)
             VALUES (?, 'Code', NULL, ?, '', 'accessibility')",
        )
        .bind(i + 1)
        .bind(&ts)
        .execute(&md)
        .await
        .unwrap();
    }

    // App switch to force close
    sqlx::query(
        "INSERT INTO capture_frames (id, app_name, window_name, timestamp, full_text, text_source)
         VALUES (4, 'Slack', NULL, '2026-01-01T10:00:04+00:00', 'notifications', 'accessibility')",
    )
    .execute(&md)
    .await
    .unwrap();

    run_etl(&md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.unwrap_or_default();
    // Strip markers; remaining content must be empty
    let content_lines: Vec<&str> = text.lines().filter(|l| l.len() != 10).collect();
    assert!(
        content_lines.is_empty(),
        "frames with empty full_text must produce no content lines; got: {text:?}"
    );
}

// ---------------------------------------------------------------------------
// Marker emission — large time gap
// ---------------------------------------------------------------------------

/// Two frames >30s apart with distinct content must produce exactly two markers.
#[tokio::test]
async fn test_session_text_marker_emitted_for_large_time_gap() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &md,
        1,
        &[
            // 60s apart — above the 30s threshold
            ("Code", "2026-01-01T10:00:00+00:00", "early_content"),
            ("Code", "2026-01-01T10:01:00+00:00", "late_content"),
            // app switch forces close
            ("Slack", "2026-01-01T10:01:01+00:00", "messages"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.expect("closed session must have session_text");
    let marker_count = count_markers(&text);
    assert_eq!(
        marker_count, 2,
        "60s gap must produce two markers; got {marker_count} in:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Marker suppression — small time gap
// ---------------------------------------------------------------------------

/// Two frames <30s apart with distinct content must produce exactly one marker.
#[tokio::test]
async fn test_session_text_marker_suppressed_for_small_gap() {
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &md,
        1,
        &[
            // 20s apart — below the 30s threshold
            ("Code", "2026-01-01T10:00:00+00:00", "first_line"),
            ("Code", "2026-01-01T10:00:20+00:00", "second_line"),
            // app switch forces close
            ("Slack", "2026-01-01T10:00:21+00:00", "messages"),
        ],
    )
    .await;

    run_etl(&md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.expect("closed session must have session_text");
    let marker_count = count_markers(&text);
    assert_eq!(
        marker_count, 1,
        "20s gap must suppress second marker; got {marker_count} in:\n{text}"
    );
}
