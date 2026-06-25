//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

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

// NOTE: `make_screenpipe_db` was removed in the slice-4b cutover. The ETL now
// reads meridian's own `capture_frames` / `capture_ui_events` (created by
// `make_meridian_db`'s migrations), so tests seed those via the helpers below —
// there is no separate screenpipe-shaped source DB anymore.

// ---------------------------------------------------------------------------
// Frame insertion helpers (seed meridian's capture_* tables)
// ---------------------------------------------------------------------------

/// Inserts capture frames with sequential IDs starting from 1.
/// Each entry is `(app_name, timestamp)`. Post-4b the ETL reads meridian's
/// `capture_frames`, so `pool` is the meridian pool. Explicit ids are allowed
/// even though the column is AUTOINCREMENT (SQLite permits explicit PK inserts).
pub async fn insert_frames(pool: &SqlitePool, frames: &[(&str, &str)]) {
    for (i, (app, ts)) in frames.iter().enumerate() {
        sqlx::query(
            "INSERT INTO capture_frames (id, app_name, window_name, timestamp) VALUES (?, ?, NULL, ?)",
        )
        .bind(i as i64 + 1)
        .bind(app)
        .bind(ts)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Inserts capture frames carrying accessibility text (`text_source =
/// 'accessibility'`). Each entry is `(app_name, timestamp, text)`. Mirrors the
/// production writer: a11y text lands in `accessibility_text` (full_text NULL),
/// which the reader resolves via `COALESCE(full_text, accessibility_text)`.
/// `id_offset` is the ID of the first frame; subsequent frames increment by 1.
pub async fn insert_frames_with_text(
    pool: &SqlitePool,
    id_offset: i64,
    frames: &[(&str, &str, &str)],
) {
    for (i, (app, ts, text)) in frames.iter().enumerate() {
        sqlx::query(
            "INSERT INTO capture_frames (id, app_name, window_name, timestamp, accessibility_text, text_source)
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
        // NOTE: real in-process capture leaves capture_trigger NULL (idle
        // detection isn't wired yet — see slice 4b notes), so the user_idle vs
        // system_sleep split these tests exercise won't occur in production
        // until in-process idle detection lands. The column + reader logic stay
        // so that future idle detection works without a schema change.
        sqlx::query(
            "INSERT INTO capture_frames (id, app_name, window_name, timestamp, capture_trigger)
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
