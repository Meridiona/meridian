//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

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
        audio_snippets: None,
        signals: None,
        min_frame_id: 1,
        max_frame_id: 10,
        frame_count: 10,
        idle_frame_count: 0,
        category: "idle_personal".into(),
        confidence: 0.0,
        session_text: None,
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
        audio_snippets: None,
        signals: None,
        min_frame_id: 1,
        max_frame_id: 5,
        frame_count: 5,
        idle_frame_count: 0,
        category: "communication".into(),
        confidence: 0.8,
        session_text: None,
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
        audio_snippets: None,
        signals: None,
        min_frame_id: 6,
        max_frame_id: 10,
        frame_count: 5,
        idle_frame_count: 0,
        category: "coding".into(),
        confidence: 0.7,
        session_text: None,
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

// NOTE: `test_audio_snippet_cap_across_runs` was removed in the slice-4b cutover.
// In-process capture is text-only (Audio OFF), so `get_audio_snippets` is stubbed
// empty and `app_sessions.audio_snippets` is always `[]` — the AUDIO_SNIPPET_CAP
// path is no longer reachable. The cap code remains in the extractor (harmless,
// never triggered); re-add a test here if in-process audio is ever introduced.
