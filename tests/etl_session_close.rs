// meridian — normalises screenpipe activity into structured app sessions

//! Session close and data-cap tests.
//!
//! Covers:
//! - `close_active_session_with` inserts into app_sessions and clears active_session
//! - Stale-session eviction path produces two closed rows with no extra SELECTs
//! - Audio snippet count is capped at 50 (AUDIO_SNIPPET_CAP) after a long session

mod common;

use meridian::db::meridian::{
    close_active_session_with, get_active_session, insert_etl_run, upsert_active_session,
    ActiveSession,
};
use meridian::etl::run_etl;

// ---------------------------------------------------------------------------
// close_active_session_with — happy path
// ---------------------------------------------------------------------------

/// Given a pre-fetched `ActiveSession`, `close_active_session_with` must:
/// - insert the session into `app_sessions` with the correct duration_s
/// - clear the `active_session` table
/// - return the new `app_sessions.id`
#[tokio::test]
async fn test_close_active_session_with_inserts_and_clears() {
    let md = common::make_meridian_db().await;

    let session = ActiveSession {
        id: 1,
        app_name: "Terminal".into(),
        started_at: "2026-01-01T10:00:00+00:00".into(),
        last_seen_at: "2026-01-01T10:05:00+00:00".into(),
        window_titles: "[]".into(),
        ocr_samples: None,
        elements_samples: None,
        audio_snippets: None,
        signals: None,
        min_frame_id: 1,
        max_frame_id: 10,
        frame_count: 10,
        idle_frame_count: 0,
    };

    upsert_active_session(&md, &session).await.unwrap();
    assert!(get_active_session(&md).await.unwrap().is_some());

    let etl_run_id = insert_etl_run(&md, 1, 10).await.unwrap();
    let new_id = close_active_session_with(&md, &session, etl_run_id)
        .await
        .unwrap();

    assert!(
        get_active_session(&md).await.unwrap().is_none(),
        "active_session must be cleared"
    );

    let row: (i64, String, i64) =
        sqlx::query_as("SELECT id, app_name, duration_s FROM app_sessions WHERE id = ?")
            .bind(new_id)
            .fetch_one(&md)
            .await
            .unwrap();
    assert_eq!(row.0, new_id);
    assert_eq!(row.1, "Terminal");
    assert_eq!(row.2, 300, "10:05 − 10:00 = 300 s");
}

// ---------------------------------------------------------------------------
// Stale-session eviction path
// ---------------------------------------------------------------------------

/// Simulates the stale-session eviction path inside `close_block`: calling
/// `close_active_session_with` twice (once for the stale session, once for the
/// new session) must produce exactly two rows in `app_sessions`, and
/// `active_session` must be empty afterwards.
#[tokio::test]
async fn test_close_active_session_with_stale_then_new() {
    let md = common::make_meridian_db().await;
    let etl_run_id = insert_etl_run(&md, 1, 20).await.unwrap();

    let stale = ActiveSession {
        id: 1,
        app_name: "Slack".into(),
        started_at: "2026-01-01T09:00:00+00:00".into(),
        last_seen_at: "2026-01-01T09:10:00+00:00".into(),
        window_titles: "[]".into(),
        ocr_samples: None,
        elements_samples: None,
        audio_snippets: None,
        signals: None,
        min_frame_id: 1,
        max_frame_id: 5,
        frame_count: 5,
        idle_frame_count: 0,
    };

    upsert_active_session(&md, &stale).await.unwrap();
    close_active_session_with(&md, &stale, etl_run_id)
        .await
        .unwrap();

    // active_session is now clear; close the new session without an upsert first —
    // this mirrors the production path where the caller skips the extra SELECT.
    let new_session = ActiveSession {
        id: 1,
        app_name: "Code".into(),
        started_at: "2026-01-01T09:10:00+00:00".into(),
        last_seen_at: "2026-01-01T09:20:00+00:00".into(),
        window_titles: "[]".into(),
        ocr_samples: None,
        elements_samples: None,
        audio_snippets: None,
        signals: None,
        min_frame_id: 6,
        max_frame_id: 10,
        frame_count: 5,
        idle_frame_count: 0,
    };

    close_active_session_with(&md, &new_session, etl_run_id)
        .await
        .unwrap();

    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT app_name FROM app_sessions ORDER BY started_at")
            .fetch_all(&md)
            .await
            .unwrap();
    assert_eq!(rows.len(), 2, "expected two closed sessions");
    assert_eq!(rows[0].0, "Slack");
    assert_eq!(rows[1].0, "Code");

    assert!(
        get_active_session(&md).await.unwrap().is_none(),
        "active_session must be empty after both closes"
    );
}

// ---------------------------------------------------------------------------
// Audio snippet cap
// ---------------------------------------------------------------------------

/// 60 audio transcriptions across 60 frames of the same app are ingested in
/// a single ETL run. After an app switch forces the session to close, the
/// stored `audio_snippets` JSON array must have ≤ 50 entries (AUDIO_SNIPPET_CAP).
#[tokio::test]
async fn test_audio_snippet_cap_across_runs() {
    let sp = common::make_screenpipe_db().await;
    let md = common::make_meridian_db().await;

    for i in 0i64..60 {
        let ts = format!("2026-01-01T10:{:02}:{:02}+00:00", i / 6, (i % 6) * 10);

        sqlx::query(
            "INSERT INTO frames (id, app_name, window_name, timestamp) VALUES (?, 'Code', NULL, ?)",
        )
        .bind(i + 1)
        .bind(&ts)
        .execute(&sp)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO audio_transcriptions
             (audio_chunk_id, offset_index, timestamp, transcription, device)
             VALUES (?, 0, ?, 'working on the auth module right now', 'mic')",
        )
        .bind(i + 1)
        .bind(&ts)
        .execute(&sp)
        .await
        .unwrap();
    }

    // First run: all 60 Code frames are processed, session stays open.
    run_etl(&sp, &md).await.unwrap();

    // App switch: Slack frame forces Code session to close into app_sessions.
    sqlx::query(
        "INSERT INTO frames (id, app_name, window_name, timestamp)
         VALUES (61, 'Slack', NULL, '2026-01-01T10:10:00+00:00')",
    )
    .execute(&sp)
    .await
    .unwrap();

    run_etl(&sp, &md).await.unwrap();

    let row: (Option<String>,) =
        sqlx::query_as("SELECT audio_snippets FROM app_sessions WHERE app_name = 'Code'")
            .fetch_one(&md)
            .await
            .unwrap();

    let snippets: Vec<serde_json::Value> =
        serde_json::from_str(row.0.as_deref().unwrap_or("[]")).unwrap();

    assert!(
        snippets.len() <= 50,
        "expected ≤ 50 audio snippets (AUDIO_SNIPPET_CAP), got {}",
        snippets.len()
    );
}
