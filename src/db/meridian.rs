//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
    pub audio_snippets: Option<String>,
    pub signals: Option<String>,
    pub min_frame_id: i64,
    pub max_frame_id: i64,
    pub frame_count: i64,
    pub idle_frame_count: i64,
    pub category: String,
    pub confidence: f64,
    pub session_text: Option<String>,
    /// JSON array of per-frame provenance (see `etl::extractor::FrameContribution`).
    /// `None` for coding-agent sessions and rows formed before migration 045.
    pub frame_contributions: Option<String>,
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

    let migrator = sqlx::migrate!("src/migrations");
    reconcile_migration_checksums(&pool, &migrator).await?;
    migrator
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    Ok(pool)
}

/// Realign `_sqlx_migrations` checksums with the embedded migration files.
///
/// sqlx records a SHA-384 checksum of every applied migration and refuses to
/// start ("migration N was previously applied but has been modified") if a
/// migration file later differs. A comment/header-only edit to a shipped
/// migration is enough to trip this and crash-loop the daemon on the next
/// upgrade — the executable SQL is unchanged, but the bytes (hence the hash)
/// are not. Rather than freeze migration bytes forever, we reconcile: for each
/// already-applied migration whose stored checksum differs from the embedded
/// one, rewrite the stored checksum (with a warning) so `run()` proceeds.
///
/// Only applied migrations present in `_sqlx_migrations` are touched; new or
/// unapplied migrations are left for `run()` to apply normally, and a fresh DB
/// (no `_sqlx_migrations` table yet) is a no-op. The trade-off is that a genuine
/// SQL edit to a shipped migration is silently accepted rather than blocked —
/// the warning log is the audit trail, and the "never edit a shipped migration"
/// rule still stands.
async fn reconcile_migration_checksums(
    pool: &SqlitePool,
    migrator: &sqlx::migrate::Migrator,
) -> anyhow::Result<()> {
    // `_sqlx_migrations` is created by the migrator on first run; on a brand-new
    // database it does not exist yet, so there is nothing applied to reconcile.
    let table_exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master \
         WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .context("checking for _sqlx_migrations table")?;
    if table_exists == 0 {
        return Ok(());
    }

    for migration in migrator.iter() {
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(migration.version)
                .fetch_optional(pool)
                .await
                .with_context(|| {
                    format!(
                        "reading stored checksum for migration {}",
                        migration.version
                    )
                })?;

        // Not applied yet, or already aligned → nothing to do.
        let Some(stored) = stored else { continue };
        if stored.as_slice() == migration.checksum.as_ref() {
            continue;
        }

        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
            .bind(migration.checksum.as_ref())
            .bind(migration.version)
            .execute(pool)
            .await
            .with_context(|| format!("repairing checksum for migration {}", migration.version))?;

        tracing::warn!(
            version = migration.version,
            description = %migration.description,
            "migration checksum drifted from the embedded file — repaired in \
             _sqlx_migrations (expected only for comment/header-only edits to a \
             shipped migration)"
        );
    }

    Ok(())
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
#[tracing::instrument(
    skip_all,
    fields(deleted_count = tracing::field::Empty)
)]
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

    sqlx::query(
        "DELETE FROM gaps WHERE etl_run_id IN (SELECT id FROM etl_runs WHERE status = 'running')",
    )
    .execute(pool)
    .await
    .context("cleanup_incomplete_runs: delete partial gaps")?;

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

    tracing::Span::current().record("deleted_count", deleted);
    Ok(deleted)
}

/// Writes the W3C `traceparent` string for a freshly-closed `app_sessions` row.
/// Used to propagate trace context to downstream consumers (Python agents).
pub async fn write_session_traceparent(
    pool: &SqlitePool,
    session_id: i64,
    traceparent: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE app_sessions SET traceparent = ?1 WHERE id = ?2")
        .bind(traceparent)
        .bind(session_id)
        .execute(pool)
        .await
        .context("write_session_traceparent: update failed")?;
    Ok(())
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
            window_titles, audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count, idle_frame_count,
            category, confidence, session_text, frame_contributions
        ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT (id) DO UPDATE SET
            app_name            = excluded.app_name,
            started_at          = excluded.started_at,
            last_seen_at        = excluded.last_seen_at,
            window_titles       = excluded.window_titles,
            audio_snippets      = excluded.audio_snippets,
            signals             = excluded.signals,
            min_frame_id        = excluded.min_frame_id,
            max_frame_id        = excluded.max_frame_id,
            frame_count         = excluded.frame_count,
            idle_frame_count    = excluded.idle_frame_count,
            category            = excluded.category,
            confidence          = excluded.confidence,
            session_text        = excluded.session_text,
            frame_contributions = excluded.frame_contributions
        "#,
    )
    .bind(&session.app_name)
    .bind(&session.started_at)
    .bind(&session.last_seen_at)
    .bind(&session.window_titles)
    .bind(&session.audio_snippets)
    .bind(&session.signals)
    .bind(session.min_frame_id)
    .bind(session.max_frame_id)
    .bind(session.frame_count)
    .bind(session.idle_frame_count)
    .bind(&session.category)
    .bind(session.confidence)
    .bind(&session.session_text)
    .bind(&session.frame_contributions)
    .execute(pool)
    .await
    .context("upsert_active_session: upsert failed")?;

    Ok(())
}

pub async fn get_active_session(pool: &SqlitePool) -> anyhow::Result<Option<ActiveSession>> {
    let row = sqlx::query_as::<_, ActiveSession>(
        r#"
        SELECT id, app_name, started_at, last_seen_at,
               window_titles, audio_snippets, signals,
               min_frame_id, max_frame_id, frame_count, idle_frame_count,
               category, confidence, session_text, frame_contributions
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
    close_active_session_with(pool, &active, etl_run_id)
        .await
        .map(Some)
}

/// Like `close_active_session` but skips the SELECT — caller already holds
/// the `ActiveSession`.  Inserts into `app_sessions`, deletes the row, and
/// returns the new `app_sessions.id`.
pub async fn close_active_session_with(
    pool: &SqlitePool,
    active: &ActiveSession,
    etl_run_id: i64,
) -> anyhow::Result<i64> {
    let started = chrono::DateTime::parse_from_rfc3339(&active.started_at)
        .with_context(|| format!("bad started_at: {}", active.started_at))?;
    let ended = chrono::DateTime::parse_from_rfc3339(&active.last_seen_at)
        .with_context(|| format!("bad last_seen_at: {}", active.last_seen_at))?;
    let duration_s = (ended - started).num_seconds().max(0);

    let result = sqlx::query(
        r#"
        INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count,
            idle_frame_count, etl_run_id,
            category, confidence, session_text, frame_contributions
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        "#,
    )
    .bind(&active.app_name)
    .bind(&active.started_at)
    .bind(&active.last_seen_at)
    .bind(duration_s)
    .bind(&active.window_titles)
    .bind(&active.audio_snippets)
    .bind(&active.signals)
    .bind(active.min_frame_id)
    .bind(active.max_frame_id)
    .bind(active.frame_count)
    .bind(active.idle_frame_count)
    .bind(etl_run_id)
    .bind(&active.category)
    .bind(active.confidence)
    .bind(&active.session_text)
    .bind(&active.frame_contributions)
    .execute(pool)
    .await
    .context("close_active_session_with: insert into app_sessions failed")?;

    let new_id = result.last_insert_rowid();

    sqlx::query("DELETE FROM active_session WHERE id = 1")
        .execute(pool)
        .await
        .context("close_active_session_with: delete failed")?;

    Ok(new_id)
}

// ---------------------------------------------------------------------------
// Gap recording
// ---------------------------------------------------------------------------

/// Inserts a gap row for a period where the machine was sleeping or user was idle.
pub async fn insert_gap(
    pool: &SqlitePool,
    started_at: &str,
    ended_at: &str,
    duration_s: i64,
    kind: &str,
    etl_run_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO gaps (started_at, ended_at, duration_s, kind, etl_run_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(started_at)
    .bind(ended_at)
    .bind(duration_s)
    .bind(kind)
    .bind(etl_run_id)
    .execute(pool)
    .await
    .context("insert_gap failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// A header/comment-only edit to a shipped migration changes its checksum and
    /// makes sqlx refuse to start ("migration N … has been modified"). Reconcile
    /// must realign the stored checksum so the daemon boots again.
    #[tokio::test]
    async fn reconcile_repairs_drifted_checksum() {
        // max_connections(1) keeps a single shared in-memory DB for the pool.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");

        let migrator = sqlx::migrate!("src/migrations");
        migrator.run(&pool).await.expect("initial migrate");

        // Simulate a #250-style header edit: the stored checksum no longer matches
        // the embedded migration's bytes.
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
            .execute(&pool)
            .await
            .expect("corrupt stored checksum");

        // This is the crash-loop the daemon hits on upgrade.
        assert!(
            migrator.run(&pool).await.is_err(),
            "a drifted checksum must make sqlx run() fail"
        );

        // After reconcile, run() succeeds again.
        reconcile_migration_checksums(&pool, &migrator)
            .await
            .expect("reconcile");
        migrator
            .run(&pool)
            .await
            .expect("migrate after reconcile must succeed");
    }

    /// A fresh database (no `_sqlx_migrations` table yet) is a clean no-op.
    #[tokio::test]
    async fn reconcile_is_noop_on_fresh_db() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        let migrator = sqlx::migrate!("src/migrations");
        reconcile_migration_checksums(&pool, &migrator)
            .await
            .expect("reconcile on fresh db must be a no-op");
    }
}
