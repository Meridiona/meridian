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
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &sp,
        1,
        &[
            (
                "Code",
                "2026-01-01T10:00:00+00:00",
                "fn main() -> Result<()> {",
            ),
            (
                "Code",
                "2026-01-01T10:00:01+00:00",
                "let result = compute_value();",
            ),
            (
                "Code",
                "2026-01-01T10:00:02+00:00",
                "println!(\"build succeeded\");",
            ),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM active_session WHERE id = 1")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.expect("active_session.session_text must be set");
    assert!(
        text.contains("fn main() -> Result<()> {"),
        "must contain frame 1 content"
    );
    assert!(
        text.contains("let result = compute_value();"),
        "must contain frame 2 content"
    );
    assert!(
        text.contains("println!(\"build succeeded\");"),
        "must contain frame 3 content"
    );
}

// ---------------------------------------------------------------------------
// Closed session
// ---------------------------------------------------------------------------

/// An app switch forces a close; session_text must appear in app_sessions.
#[tokio::test]
async fn test_session_text_populated_in_closed_session() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &sp,
        1,
        &[
            (
                "Code",
                "2026-01-01T10:00:00+00:00",
                "struct FooConfig { value: u32 }",
            ),
            (
                "Code",
                "2026-01-01T10:00:01+00:00",
                "impl FooConfig { fn new() -> Self {} }",
            ),
            (
                "Code",
                "2026-01-01T10:00:02+00:00",
                "use std::io::BufReader;",
            ),
            ("Slack", "2026-01-01T10:00:03+00:00", "Channel: #general"),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row.0.expect("closed session must have session_text");
    assert!(
        text.contains("struct FooConfig { value: u32 }"),
        "missing frame 1 content"
    );
    assert!(
        text.contains("impl FooConfig { fn new() -> Self {} }"),
        "missing frame 2 content"
    );
    assert!(
        text.contains("use std::io::BufReader;"),
        "missing frame 3 content"
    );
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

/// Five frames with identical full_text — session_text must contain each line exactly once.
#[tokio::test]
async fn test_session_text_no_duplicates_across_frames() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    // 3 frames share the same content (below chrome threshold of 4 — not filtered as chrome).
    // Deduplication must still keep each line only once.
    let repeated_text = "first unique content line for dedup test\nsecond unique content for dedup";
    common::insert_frames_with_text(
        &sp,
        1,
        &[
            ("Code", "2026-01-01T10:00:00+00:00", repeated_text),
            ("Code", "2026-01-01T10:00:01+00:00", repeated_text),
            ("Code", "2026-01-01T10:00:02+00:00", repeated_text),
            // app switch forces close
            (
                "Slack",
                "2026-01-01T10:00:03+00:00",
                "notifications panel opened",
            ),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

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

    let line_a_count = content_lines
        .iter()
        .filter(|&&l| l == "first unique content line for dedup test")
        .count();
    let line_b_count = content_lines
        .iter()
        .filter(|&&l| l == "second unique content for dedup")
        .count();
    assert_eq!(
        line_a_count, 1,
        "first content line must appear exactly once"
    );
    assert_eq!(
        line_b_count, 1,
        "second content line must appear exactly once"
    );
}

// ---------------------------------------------------------------------------
// Cross-run merge
// ---------------------------------------------------------------------------

/// Two ETL runs on the same open session — session_text must contain lines from both batches.
#[tokio::test]
async fn test_session_text_merged_across_etl_runs() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    // Batch 1: frames 1–3 (set A content)
    common::insert_frames_with_text(
        &sp,
        1,
        &[
            (
                "Code",
                "2026-01-01T10:00:00+00:00",
                "alpha batch first content line",
            ),
            (
                "Code",
                "2026-01-01T10:00:01+00:00",
                "alpha batch second content line",
            ),
            (
                "Code",
                "2026-01-01T10:00:02+00:00",
                "alpha batch third content line",
            ),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap(); // active_session built from set A

    // Batch 2: frames 4–6 (set B content, no overlap with set A)
    common::insert_frames_with_text(
        &sp,
        4,
        &[
            (
                "Code",
                "2026-01-01T10:01:00+00:00",
                "beta batch first content line",
            ),
            (
                "Code",
                "2026-01-01T10:01:01+00:00",
                "beta batch second content line",
            ),
            (
                "Code",
                "2026-01-01T10:01:02+00:00",
                "beta batch third content line",
            ),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap(); // merge into active_session

    let row: (Option<String>,) =
        sqlx::query_as("SELECT session_text FROM active_session WHERE id = 1")
            .fetch_one(&md)
            .await
            .unwrap();

    let text = row
        .0
        .expect("active_session.session_text must be set after merge");

    // Set A lines must be present
    assert!(
        text.contains("alpha batch first content line"),
        "missing set A line 1"
    );
    assert!(
        text.contains("alpha batch second content line"),
        "missing set A line 2"
    );
    assert!(
        text.contains("alpha batch third content line"),
        "missing set A line 3"
    );

    // Set B lines must be present
    assert!(
        text.contains("beta batch first content line"),
        "missing set B line 1"
    );
    assert!(
        text.contains("beta batch second content line"),
        "missing set B line 2"
    );
    assert!(
        text.contains("beta batch third content line"),
        "missing set B line 3"
    );

    // No duplicates — each line appears exactly once
    for line in &[
        "alpha batch first content line",
        "alpha batch second content line",
        "alpha batch third content line",
        "beta batch first content line",
        "beta batch second content line",
        "beta batch third content line",
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
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    // Insert 3 frames with blank full_text via raw SQL (insert_frames_with_text inserts
    // non-empty strings; use empty strings to test the filter path).
    for i in 0i64..3 {
        let ts = format!("2026-01-01T10:00:0{}+00:00", i);
        sqlx::query(
            "INSERT INTO frames (id, app_name, window_name, timestamp, full_text, text_source)
             VALUES (?, 'Code', NULL, ?, '', 'accessibility')",
        )
        .bind(i + 1)
        .bind(&ts)
        .execute(&sp)
        .await
        .unwrap();
    }

    // App switch to force close
    sqlx::query(
        "INSERT INTO frames (id, app_name, window_name, timestamp, full_text, text_source)
         VALUES (4, 'Slack', NULL, '2026-01-01T10:00:04+00:00', 'notifications', 'accessibility')",
    )
    .execute(&sp)
    .await
    .unwrap();

    run_etl(&sp, &md).await.unwrap();

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
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &sp,
        1,
        &[
            // 60s apart — above the 30s threshold
            (
                "Code",
                "2026-01-01T10:00:00+00:00",
                "early content from first frame",
            ),
            (
                "Code",
                "2026-01-01T10:01:00+00:00",
                "late content from second frame",
            ),
            // app switch forces close
            (
                "Slack",
                "2026-01-01T10:01:01+00:00",
                "new messages in channel",
            ),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

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
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    common::insert_frames_with_text(
        &sp,
        1,
        &[
            // 20s apart — below the 30s threshold
            (
                "Code",
                "2026-01-01T10:00:00+00:00",
                "first content line of session",
            ),
            (
                "Code",
                "2026-01-01T10:00:20+00:00",
                "second content line of session",
            ),
            // app switch forces close
            (
                "Slack",
                "2026-01-01T10:00:21+00:00",
                "new messages in channel",
            ),
        ],
    )
    .await;

    run_etl(&sp, &md).await.unwrap();

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
