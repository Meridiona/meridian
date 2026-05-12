// meridian — normalises screenpipe activity into structured app sessions

//! Shared test helpers — in-memory DB factories and frame insertion utilities.
//! Every integration test module includes this via `mod common;`.

#![allow(dead_code)]

use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// DB factories
// ---------------------------------------------------------------------------

pub async fn make_meridian_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    pool
}

pub async fn make_screenpipe_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();

    sqlx::query(
        "CREATE TABLE frames (
            id INTEGER PRIMARY KEY,
            app_name TEXT,
            window_name TEXT,
            browser_url TEXT,
            timestamp TEXT NOT NULL,
            capture_trigger TEXT,
            full_text TEXT,
            text_source TEXT
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

// ---------------------------------------------------------------------------
// Frame insertion helpers
// ---------------------------------------------------------------------------

/// Inserts frames with sequential IDs starting from 1.
/// Each entry is `(app_name, timestamp)`.
pub async fn insert_frames(pool: &SqlitePool, frames: &[(&str, &str)]) {
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

/// Inserts frames carrying `full_text` and `text_source = 'accessibility'`.
/// Each entry is `(app_name, timestamp, full_text)`.
/// `id_offset` is the ID of the first frame; subsequent frames increment by 1.
pub async fn insert_frames_with_text(
    pool: &SqlitePool,
    id_offset: i64,
    frames: &[(&str, &str, &str)],
) {
    for (i, (app, ts, text)) in frames.iter().enumerate() {
        sqlx::query(
            "INSERT INTO frames (id, app_name, window_name, timestamp, full_text, text_source)
             VALUES (?, ?, NULL, ?, ?, 'accessibility')",
        )
        .bind(id_offset + i as i64)
        .bind(app)
        .bind(ts)
        .bind(text)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Inserts frames with a configurable `id_offset` and optional `capture_trigger`.
/// Each entry is `(app_name, timestamp, capture_trigger)`.
/// `app_name` may be `None` to insert NULL — such frames are skipped by
/// `get_frames_since` but visible to `count_frames_in_window` for gap
/// classification.
pub async fn insert_frames_with_trigger(
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
