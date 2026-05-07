// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteConnectOptions, FromRow, SqlitePool};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Sub-document types stored as JSON columns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowTitle {
    pub title: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrSample {
    pub text: String,
    pub window: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSnippet {
    pub text: String,
    pub ts: String,
    pub speaker_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    #[serde(rename = "type")]
    pub signal_type: String,
    pub value: String,
    pub ts: String,
}

// ---------------------------------------------------------------------------
// Row structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AppSession {
    pub id: i64,
    pub app_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_s: i64,
    pub window_titles: String,
    pub ocr_samples: Option<String>,
    pub elements_samples: Option<String>,
    pub audio_snippets: Option<String>,
    pub signals: Option<String>,
    pub min_frame_id: i64,
    pub max_frame_id: i64,
    pub frame_count: i64,
    pub etl_run_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActiveSession {
    pub id: i64,
    pub app_name: String,
    pub started_at: String,
    pub last_seen_at: String,
    pub window_titles: String,
    pub ocr_samples: Option<String>,
    pub elements_samples: Option<String>,
    pub audio_snippets: Option<String>,
    pub signals: Option<String>,
    pub min_frame_id: i64,
    pub max_frame_id: i64,
    pub frame_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EtlRun {
    pub id: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub from_frame_id: i64,
    pub to_frame_id: i64,
    pub sessions_closed: i64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EtlCursor {
    pub id: i64,
    pub last_frame_id: i64,
    pub last_run_at: Option<String>,
    pub last_run_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

/// Opens (or creates) `meridian.db` at `uri`, runs embedded migrations, and
/// returns a connection pool.  `uri` must be a `sqlite://…` URI.
pub async fn setup_db(uri: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(uri)
        .with_context(|| format!("invalid SQLite URI: {uri}"))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

    let pool = SqlitePool::connect_with(opts)
        .await
        .with_context(|| format!("failed to open SQLite at {uri}"))?;

    sqlx::migrate!("src/migrations")
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    Ok(pool)
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

pub async fn get_cursor(pool: &SqlitePool) -> anyhow::Result<EtlCursor> {
    let row = sqlx::query_as::<_, EtlCursor>(
        "SELECT id, last_frame_id, last_run_at, last_run_id FROM etl_cursor WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("get_cursor: fetch failed")?;

    if let Some(cursor) = row {
        return Ok(cursor);
    }

    sqlx::query("INSERT INTO etl_cursor (id, last_frame_id) VALUES (1, 0)")
        .execute(pool)
        .await
        .context("get_cursor: insert default failed")?;

    Ok(EtlCursor {
        id: 1,
        last_frame_id: 0,
        last_run_at: None,
        last_run_id: None,
    })
}

pub async fn update_cursor(
    pool: &SqlitePool,
    last_frame_id: i64,
    run_id: i64,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO etl_cursor (id, last_frame_id, last_run_at, last_run_id)
        VALUES (1, ?1, ?2, ?3)
        ON CONFLICT (id) DO UPDATE SET
            last_frame_id = excluded.last_frame_id,
            last_run_at   = excluded.last_run_at,
            last_run_id   = excluded.last_run_id
        "#,
    )
    .bind(last_frame_id)
    .bind(now)
    .bind(run_id)
    .execute(pool)
    .await
    .context("update_cursor: upsert failed")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// ETL run lifecycle
// ---------------------------------------------------------------------------

pub async fn insert_etl_run(
    pool: &SqlitePool,
    from_frame_id: i64,
    to_frame_id: i64,
) -> anyhow::Result<i64> {
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        r#"
        INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
        VALUES (?1, ?2, ?3, 'running')
        "#,
    )
    .bind(now)
    .bind(from_frame_id)
    .bind(to_frame_id)
    .execute(pool)
    .await
    .context("insert_etl_run: insert failed")?;

    Ok(result.last_insert_rowid())
}

pub async fn complete_etl_run(
    pool: &SqlitePool,
    run_id: i64,
    sessions_closed: i64,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    let status = if error.is_some() { "failed" } else { "success" };
    sqlx::query(
        r#"
        UPDATE etl_runs
        SET completed_at    = ?1,
            sessions_closed = ?2,
            status          = ?3,
            error           = ?4
        WHERE id = ?5
        "#,
    )
    .bind(now)
    .bind(sessions_closed)
    .bind(status)
    .bind(error)
    .bind(run_id)
    .execute(pool)
    .await
    .context("complete_etl_run: update failed")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Startup cleanup
// ---------------------------------------------------------------------------

/// Finds any ETL run stuck in 'running' state (i.e., the daemon was killed mid-run),
/// removes the partial sessions it wrote, clears the active_session row, and marks
/// the run as 'aborted'.  Call this once on startup before the first ETL pass.
pub async fn cleanup_incomplete_runs(pool: &SqlitePool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"
        DELETE FROM app_sessions
        WHERE etl_run_id IN (SELECT id FROM etl_runs WHERE status = 'running')
        "#,
    )
    .execute(pool)
    .await
    .context("cleanup_incomplete_runs: delete partial sessions")?;

    let deleted = result.rows_affected();

    sqlx::query("DELETE FROM active_session")
        .execute(pool)
        .await
        .context("cleanup_incomplete_runs: clear active_session")?;

    sqlx::query(
        "UPDATE etl_runs SET status = 'aborted', completed_at = ?1 WHERE status = 'running'",
    )
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .context("cleanup_incomplete_runs: mark aborted")?;

    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Active session
// ---------------------------------------------------------------------------

pub async fn upsert_active_session(
    pool: &SqlitePool,
    session: &ActiveSession,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO active_session (
            id, app_name, started_at, last_seen_at,
            window_titles, ocr_samples, elements_samples,
            audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count
        ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT (id) DO UPDATE SET
            app_name         = excluded.app_name,
            started_at       = excluded.started_at,
            last_seen_at     = excluded.last_seen_at,
            window_titles    = excluded.window_titles,
            ocr_samples      = excluded.ocr_samples,
            elements_samples = excluded.elements_samples,
            audio_snippets   = excluded.audio_snippets,
            signals          = excluded.signals,
            min_frame_id     = excluded.min_frame_id,
            max_frame_id     = excluded.max_frame_id,
            frame_count      = excluded.frame_count
        "#,
    )
    .bind(&session.app_name)
    .bind(&session.started_at)
    .bind(&session.last_seen_at)
    .bind(&session.window_titles)
    .bind(&session.ocr_samples)
    .bind(&session.elements_samples)
    .bind(&session.audio_snippets)
    .bind(&session.signals)
    .bind(session.min_frame_id)
    .bind(session.max_frame_id)
    .bind(session.frame_count)
    .execute(pool)
    .await
    .context("upsert_active_session: upsert failed")?;

    Ok(())
}

pub async fn get_active_session(pool: &SqlitePool) -> anyhow::Result<Option<ActiveSession>> {
    let row = sqlx::query_as::<_, ActiveSession>(
        r#"
        SELECT id, app_name, started_at, last_seen_at,
               window_titles, ocr_samples, elements_samples,
               audio_snippets, signals,
               min_frame_id, max_frame_id, frame_count
        FROM active_session WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("get_active_session: fetch failed")?;

    Ok(row)
}

/// Moves the active_session row into app_sessions and deletes it.
/// Returns `Some(new_session_id)` or `None` if the table was empty.
pub async fn close_active_session(
    pool: &SqlitePool,
    etl_run_id: i64,
) -> anyhow::Result<Option<i64>> {
    let Some(active) = get_active_session(pool).await? else {
        return Ok(None);
    };

    let started = chrono::DateTime::parse_from_rfc3339(&active.started_at)
        .with_context(|| format!("bad started_at: {}", active.started_at))?;
    let ended = chrono::DateTime::parse_from_rfc3339(&active.last_seen_at)
        .with_context(|| format!("bad last_seen_at: {}", active.last_seen_at))?;
    let duration_s = (ended - started).num_seconds().max(0);

    let result = sqlx::query(
        r#"
        INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, ocr_samples, elements_samples,
            audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count,
            etl_run_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        "#,
    )
    .bind(&active.app_name)
    .bind(&active.started_at)
    .bind(&active.last_seen_at)
    .bind(duration_s)
    .bind(&active.window_titles)
    .bind(&active.ocr_samples)
    .bind(&active.elements_samples)
    .bind(&active.audio_snippets)
    .bind(&active.signals)
    .bind(active.min_frame_id)
    .bind(active.max_frame_id)
    .bind(active.frame_count)
    .bind(etl_run_id)
    .execute(pool)
    .await
    .context("close_active_session: insert into app_sessions failed")?;

    let new_id = result.last_insert_rowid();

    sqlx::query("DELETE FROM active_session WHERE id = 1")
        .execute(pool)
        .await
        .context("close_active_session: delete failed")?;

    Ok(Some(new_id))
}

/// Inserts a bare `(idle)` row directly into `app_sessions` for a gap period
/// where screenpipe produced no frames (sleep / screen-lock).
pub async fn insert_idle_session(
    pool: &SqlitePool,
    started_at: &str,
    ended_at: &str,
    min_frame_id: i64,
    max_frame_id: i64,
    etl_run_id: i64,
) -> anyhow::Result<()> {
    let started = chrono::DateTime::parse_from_rfc3339(started_at)
        .with_context(|| format!("insert_idle_session: bad started_at: {started_at}"))?;
    let ended = chrono::DateTime::parse_from_rfc3339(ended_at)
        .with_context(|| format!("insert_idle_session: bad ended_at: {ended_at}"))?;
    let duration_s = (ended - started).num_seconds().max(0);

    sqlx::query(
        r#"
        INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, ocr_samples, elements_samples,
            audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count,
            etl_run_id
        ) VALUES ('(idle)', ?1, ?2, ?3, '[]', NULL, NULL, NULL, NULL, ?4, ?5, 0, ?6)
        "#,
    )
    .bind(started_at)
    .bind(ended_at)
    .bind(duration_s)
    .bind(min_frame_id)
    .bind(max_frame_id)
    .bind(etl_run_id)
    .execute(pool)
    .await
    .context("insert_idle_session: insert failed")?;

    Ok(())
}
