//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The no-migration SQLite opener and the raw `active_session` row.
//!
//! These are the lowest layer of the shared data layer: a no-migration WAL opener
//! plus the `ActiveSession` row type the daemon re-exports. The [`crate::readers`]
//! build their richer dashboard views on top of a pool opened here.
//!
//! The load-bearing invariant is **schema ownership**: this pool MUST NOT run
//! migrations or alter the schema — the daemon owns that, and a second migrator
//! would race it. *Data* writes are permitted, though: the app issues the ported
//! `daily_plan` mutations (see [`crate::plan`]) through this same pool, exactly as
//! the former Node `getWriteDb` did. WAL serialises writers and `busy_timeout`
//! rides out the daemon's short write transactions.
//!
//! Re-exported at the crate root (`meridian_core::{open_existing,
//! open_existing_lazy, ping, get_active_session, ActiveSession}`) — `db` is
//! internal organization only.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use crate::db_crypto::open_pool_with_key;

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
    /// OCR samples from monitors other than the one this session's app was
    /// focused on — context, not activity (JSON array, see
    /// `src/db/screenpipe.rs::SecondaryScreenEvent` on the daemon side).
    pub secondary_screens: Option<String>,
}

/// Open an EXISTING meridian.db WITHOUT running migrations or creating the file.
///
/// For the dashboard / Tauri app, which must not own or mutate the SCHEMA — the
/// daemon owns migrations and a second migrator would race it. Opens a normal WAL
/// connection so reads stay correct alongside the daemon's writes; the app also
/// issues the ported `daily_plan` data-writes through it (see [`crate::plan`]).
///
/// `busy_timeout` matches the former Node handle's 5000ms: the daemon writes this
/// same file every poll/ETL, and SQLite's write lock is database-wide, so without
/// it a concurrent plan-write would fail with "database is locked". Harmless for
/// the readers sharing the pool — WAL readers don't take the write lock.
///
/// `key`: the SQLCipher encryption key (64 hex chars), or `None` to open an
/// unencrypted database exactly as before. The tray resolves this once at
/// startup (`tray/src-tauri/src/db_key.rs`) and passes it through here — see
/// [`crate::db_crypto`] for why the key is applied via a raw `after_connect`
/// hook rather than sqlx's `SqliteConnectOptions` pragma builder methods.
#[tracing::instrument(skip_all, fields(uri = %uri, encrypted = key.is_some()))]
pub async fn open_existing(uri: &str, key: Option<&str>) -> anyhow::Result<SqlitePool> {
    let pool = open_pool_with_key(uri, key, false, &[]).await?;
    tracing::info!(
        uri,
        encrypted = key.is_some(),
        "opened meridian.db (WAL, 5s busy_timeout)"
    );
    Ok(pool)
}

/// Same contract as [`open_existing`], but the pool is built **without
/// connecting** — the first connection is made on demand and every later
/// acquire retries, so a database that does not exist yet is recovered from
/// automatically once it appears.
///
/// This is what the tray uses at startup. It must not require `meridian.db` to
/// already exist: on a first launch the tray builds this handle seconds before
/// it installs and starts the daemon that creates the file, and an eager open
/// there yields a `None` pool that nothing ever retries — disabling every
/// DB-backed command and silently discarding every captured frame until the
/// user restarts the tray. See [`crate::db_crypto::open_pool_with_key_lazy`]
/// for the full rationale and for why the key decision is per-connection.
///
/// Because no connection is attempted here, a returned `Ok` says nothing about
/// reachability — use [`ping`] when you want that answer at a point in time.
///
/// Must be called from inside a Tokio runtime even though it never awaits — see
/// [`crate::db_crypto::open_pool_with_key_lazy`] for why building the handle
/// needs one, and why that requirement is encoded in the signature.
#[tracing::instrument(skip_all, fields(uri = %uri, encrypted = key.is_some()))]
pub async fn open_existing_lazy(uri: &str, key: Option<&str>) -> anyhow::Result<SqlitePool> {
    let pool = crate::db_crypto::open_pool_with_key_lazy(uri, key).await?;
    tracing::info!(
        uri,
        encrypted = key.is_some(),
        "prepared meridian.db pool (lazy, WAL, 5s busy_timeout)"
    );
    Ok(pool)
}

/// Acquire one connection and run `SELECT 1`, reporting whether the database is
/// reachable *right now*.
///
/// Exists for [`open_existing_lazy`]'s callers: a lazy pool cannot fail at
/// build time, so without an explicit probe a tray whose database is
/// unreachable would produce no startup signal at all — which is precisely how
/// the first-launch failure this pair replaces stayed invisible for a whole
/// session. Purely diagnostic: callers log the result and carry on, because the
/// pool recovers by itself.
#[tracing::instrument(skip_all)]
pub async fn ping(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .context("ping: meridian.db is not reachable")?;
    Ok(())
}

/// Read the single active session (the `active_session` row, id = 1), or `None`.
#[tracing::instrument(skip_all)]
pub async fn get_active_session(pool: &SqlitePool) -> anyhow::Result<Option<ActiveSession>> {
    let row = sqlx::query_as::<_, ActiveSession>(
        r#"
        SELECT id, app_name, started_at, last_seen_at,
               window_titles, audio_snippets, signals,
               min_frame_id, max_frame_id, frame_count, idle_frame_count,
               category, confidence, session_text, secondary_screens
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
