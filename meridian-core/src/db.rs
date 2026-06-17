//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The no-migration SQLite opener and the raw `active_session` row.
//!
//! These are the lowest layer of the shared data layer: a read-only WAL opener
//! that must NOT own or mutate the schema (the daemon owns migrations), plus the
//! `ActiveSession` row type the daemon re-exports. The [`crate::readers`] build
//! their richer dashboard views on top of a pool opened here.
//!
//! Re-exported at the crate root (`meridian_core::{open_existing,
//! get_active_session, ActiveSession}`) — `db` is internal organization only.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteConnectOptions, FromRow, SqlitePool};
use std::str::FromStr;

/// The single in-progress activity block (the `active_session` row, id = 1).
/// JSON columns are stored as raw text (`String`), so this needs no chrono/json
/// sqlx features — keeping the dependency surface minimal.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActiveSession {
    pub id: i64,
    pub app_name: String,
    pub started_at: String,
    pub last_seen_at: String,
    pub window_titles: String,
    pub audio_snippets: Option<String>,
    pub signals: Option<String>,
    pub min_frame_id: i64,
    pub max_frame_id: i64,
    pub frame_count: i64,
    pub idle_frame_count: i64,
    pub category: String,
    pub confidence: f64,
    pub session_text: Option<String>,
}

/// Open an EXISTING meridian.db WITHOUT running migrations or creating the file.
///
/// For read consumers (the dashboard / Tauri app) that must not own or mutate
/// the schema — the daemon owns migrations. A second process running the
/// migrator would race it. Opens a normal WAL connection so it reads correctly
/// alongside the daemon's writes; callers issue only SELECTs.
#[tracing::instrument(skip_all, fields(uri = %uri))]
pub async fn open_existing(uri: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(uri)
        .with_context(|| format!("invalid SQLite URI: {uri}"))?
        .create_if_missing(false)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

    let pool = SqlitePool::connect_with(opts)
        .await
        .with_context(|| format!("failed to open existing SQLite at {uri}"))?;
    tracing::info!(uri, "opened meridian.db (read-only WAL)");
    Ok(pool)
}

/// Read the single active session (the `active_session` row, id = 1), or `None`.
#[tracing::instrument(skip_all)]
pub async fn get_active_session(pool: &SqlitePool) -> anyhow::Result<Option<ActiveSession>> {
    let row = sqlx::query_as::<_, ActiveSession>(
        r#"
        SELECT id, app_name, started_at, last_seen_at,
               window_titles, audio_snippets, signals,
               min_frame_id, max_frame_id, frame_count, idle_frame_count,
               category, confidence, session_text
        FROM active_session WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("get_active_session: fetch failed")?;

    tracing::debug!(
        found = row.is_some(),
        app = row.as_ref().map(|r| r.app_name.as_str()),
        "active_session read"
    );
    Ok(row)
}
