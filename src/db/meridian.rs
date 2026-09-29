//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

// ActiveSession + the read helpers now live in meridian-core (shared with the
// Tauri dashboard, single source of truth). Re-exported so existing daemon code
// keeps using `crate::db::meridian::{ActiveSession, open_existing, get_active_session}`.
pub use meridian_core::{get_active_session, open_existing, ActiveSession};

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
///
/// Reads the SQLCipher encryption key straight from the process env
/// (`MERIDIAN_DB_KEY`) rather than taking it as a parameter — this function
/// has ~20 call sites across `main.rs`/`plan_tasks/cli.rs`, all of which run
/// after `main()`'s unconditional `dotenvy::dotenv_override()` (see
/// `src/main.rs`), which already populates this env var exactly the same way
/// it populates `MERIDIAN_DB` itself. Threading a new parameter through every
/// call site would be pure churn for no added safety. `None` (key absent)
/// opens the database unencrypted, unchanged from before this feature existed
/// — this is the normal case for dev/source/Bare installs; only a Canonical
/// (packaged) install's tray provisions a key (see
/// `tray/src-tauri/src/db_key.rs`).
pub async fn setup_db(uri: &str) -> anyhow::Result<SqlitePool> {
    let key = std::env::var("MERIDIAN_DB_KEY").ok();
    // Lets `crate::etl::capture_retention`'s periodic `incremental_vacuum`
    // actually reclaim disk space freed by its capture_* table deletes. Only
    // takes effect on a brand-new (table-less) database file — SQLite requires
    // a full `VACUUM` to convert an already-populated database from the
    // default `auto_vacuum = NONE`, which we deliberately never run
    // automatically on a live daemon (see that module's doc comment). On an
    // existing database this pragma is simply a documented no-op.
    //
    // Since the Bucket-2 cutover the tray also writes capture_frames/
    // capture_ui_events into THIS db concurrently with the daemon's ETL writes
    // (app_sessions, cursor) — the shared `busy_timeout=5000` inside
    // `open_pool_with_key` (matching the tray's `open_existing`) is what keeps
    // that from surfacing as an immediate SQLITE_BUSY.
    let pool = meridian_core::db_crypto::open_pool_with_key(
        uri,
        key.as_deref(),
        true,
        &[("auto_vacuum", "INCREMENTAL")],
    )
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

/// The migration version in `err` that this binary does not carry, when the
/// database has been migrated **ahead** of it — otherwise `None`.
///
/// # Why this is worth classifying
///
/// [`setup_db`] fails for several unrelated reasons and they were all reported
/// with one sentence naming three of them: a wrong or absent encryption key, a
/// locked file, corruption. This is a fourth, it is not on that list, and it is
/// the only one where the database is **perfectly healthy** — the binary is
/// simply older than the schema. Sending an operator at the database when the
/// answer is the binary costs hours; it cost several here.
///
/// It is also the only one that never resolves on its own. A daemon under
/// launchd `KeepAlive` retries forever: measured 2026-09-01, a v1.86.0 daemon
/// carrying 79 migrations against a database at 83 failed **13,306 times over
/// eight days** (~2,800/day), each attempt opening the database and running
/// migrations against it.
///
/// # What counts
///
/// - [`sqlx::migrate::MigrateError::VersionMissing`] — `_sqlx_migrations` has a row for a
///   migration this build does not carry. This is the observed shape.
/// - [`sqlx::migrate::MigrateError::VersionTooNew`] — the same condition as sqlx reports it
///   when it can name the newest applied version.
///
/// Deliberately NOT [`sqlx::migrate::MigrateError::VersionMismatch`] (a checksum drift, which
/// [`reconcile_migration_checksums`] repairs) or [`sqlx::migrate::MigrateError::Dirty`] (a
/// half-applied migration, which is a genuine repair case). Both are recoverable
/// in place; neither means the binary is out of date.
///
/// `MigrateError` is `#[non_exhaustive]`, so an unrecognised variant falls
/// through to `None` and keeps the general message. Failing to classify costs a
/// vaguer log line; misclassifying would tell someone to update a daemon that is
/// already current.
pub fn schema_ahead_of_binary(err: &anyhow::Error) -> Option<i64> {
    use sqlx::migrate::MigrateError;
    err.chain()
        .filter_map(|c| c.downcast_ref::<MigrateError>())
        .find_map(|m| match m {
            MigrateError::VersionMissing(v) => Some(*v),
            MigrateError::VersionTooNew(v, _) => Some(*v),
            _ => None,
        })
}

/// Forces the WAL fully into the main database file and truncates it.
///
/// Called from `main.rs`'s shutdown sequence, right before closing the pool.
/// `pool.close()` alone does not checkpoint: SQLite only auto-checkpoints when
/// the LAST connection to the file closes, and the tray holds its own
/// independent, long-lived pool on the same file for its entire process
/// lifetime (`tray/src-tauri/src/lib.rs`'s `app.manage(db_pool)`) — so from
/// this pool's point of view there is never a "last connection". Without this,
/// every daemon restart (a crash, or `reload_daemon`'s SIGHUP, which exits and
/// relies on launchd/the tray to relaunch it) hands a WAL in whatever
/// half-written state it happened to be in to a brand-new process, while the
/// tray's already-open connection keeps its stale view of the file across that
/// boundary. A clean TRUNCATE checkpoint here gives every restart a
/// well-defined, empty-WAL starting point instead.
///
/// Best-effort by design — callers should log and continue on error rather
/// than fail shutdown over it.
pub async fn checkpoint_wal(pool: &SqlitePool) -> anyhow::Result<()> {
    // `PRAGMA wal_checkpoint(TRUNCATE)` answers with a ROW — `(busy, log,
    // checkpointed)` — not merely a status. `busy = 1` means another connection
    // held the file and the checkpoint did NOT truncate anything. `.execute()`
    // discards that row, so the call reported success on a WAL it had left
    // exactly as it found it.
    //
    // That is not a remote possibility here, it is the expected contention: the
    // doc above explains this function exists BECAUSE the tray keeps its own
    // long-lived pool on this same file, and that pool is precisely the reader
    // that makes `busy` non-zero. Silently succeeding is the worst of the
    // outcomes — shutdown logs a clean checkpoint and still hands the next
    // daemon generation the half-written WAL this exists to prevent.
    let (busy, _log, _checkpointed): (i64, i64, i64) =
        sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(pool)
            .await
            .context("WAL checkpoint failed")?;
    if busy != 0 {
        anyhow::bail!(
            "WAL checkpoint could not truncate - another connection held the file. \
             The next daemon generation starts on a non-empty WAL."
        );
    }
    Ok(())
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
            category, confidence, session_text, secondary_screens
        ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT (id) DO UPDATE SET
            app_name         = excluded.app_name,
            started_at       = excluded.started_at,
            last_seen_at     = excluded.last_seen_at,
            window_titles    = excluded.window_titles,
            audio_snippets   = excluded.audio_snippets,
            signals          = excluded.signals,
            min_frame_id     = excluded.min_frame_id,
            max_frame_id     = excluded.max_frame_id,
            frame_count      = excluded.frame_count,
            idle_frame_count = excluded.idle_frame_count,
            category         = excluded.category,
            confidence       = excluded.confidence,
            session_text     = excluded.session_text,
            secondary_screens = excluded.secondary_screens
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
    .bind(&session.secondary_screens)
    .execute(pool)
    .await
    .context("upsert_active_session: upsert failed")?;

    Ok(())
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
            category, confidence, session_text, secondary_screens
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
    .bind(&active.secondary_screens)
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

/// Returns `true` if any `tracking_paused` or `schedule_paused` gap overlaps
/// the window `[from_ts, to_ts)`. Used by the ETL to skip inserting a
/// `user_idle` or `system_sleep` gap for an interval the tray already recorded
/// as a deliberate tracking pause.
pub async fn pause_gap_exists_in_window(
    pool: &SqlitePool,
    from_ts: &str,
    to_ts: &str,
) -> anyhow::Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM gaps
         WHERE kind IN ('tracking_paused', 'schedule_paused')
           AND started_at <= ?1
           AND ended_at   >= ?2",
    )
    .bind(from_ts)
    .bind(to_ts)
    .fetch_one(pool)
    .await
    .context("pause_gap_exists_in_window failed")?;
    Ok(count > 0)
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

    /// A daemon OLDER than the database must say so, in the error a human reads.
    ///
    /// # The condition
    /// Two daemon builds share one `meridian.db` and the newer one migrates the
    /// schema forward. The older binary then finds rows in `_sqlx_migrations`
    /// for migrations it does not carry, and sqlx correctly refuses to run —
    /// `MigrateError::VersionMissing`. It is correct to refuse: an old binary
    /// against a new schema would write rows the current code cannot read.
    ///
    /// # Why this is a test and not a comment
    /// Measured on a dev machine 2026-09-01: a v1.86.0 daemon (79 migrations)
    /// against a database at 83 crash-looped **13,306 times over eight days**
    /// under launchd `KeepAlive`, at ~2,800/day. Every one of those attempts
    /// opened the database and tried to migrate it. The only diagnostic it left
    /// was `error=failed to run migrations` — the outer `.context` with no
    /// cause — beside a message offering three explanations
    /// (`wrong/absent encryption key, a locked file, or corruption`), none of
    /// which was the real one. That sent the investigation at the database
    /// rather than at the binary.
    ///
    /// So this pins the property that actually matters: **the version number
    /// survives into `errors::chain`**. Without it there is nothing in
    /// telemetry that distinguishes this from corruption, and it is not
    /// reproducible on demand — it needs two binaries and a shared database.
    #[tokio::test]
    async fn an_applied_migration_the_binary_lacks_names_itself_in_the_error() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        let migrator = sqlx::migrate!("src/migrations");
        migrator.run(&pool).await.expect("initial migrate");

        // Stand in for "a newer daemon applied migration 999999 here". The
        // version is deliberately far above any real one so this never collides
        // with a migration added later.
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, installed_on, success, checksum, execution_time) \
             VALUES (999999, 'from a newer daemon', CURRENT_TIMESTAMP, 1, X'00', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert phantom applied migration");

        // Reconcile must NOT paper over this: it realigns checksums for
        // migrations this binary HAS, and has nothing to say about one it does
        // not. Silently deleting the row would let an old daemon run against a
        // schema it cannot understand, which is the failure sqlx is preventing.
        reconcile_migration_checksums(&pool, &migrator)
            .await
            .expect("reconcile must tolerate an unknown applied migration");

        let err = migrator
            .run(&pool)
            .await
            .context("failed to run migrations")
            .expect_err("sqlx must refuse to run against a schema ahead of this binary");
        let rendered = crate::errors::chain(&err);

        assert!(
            rendered.contains("999999"),
            "the offending migration version must reach the log - without it the \
             operator cannot tell this apart from corruption. got: {rendered}"
        );
        assert!(
            rendered.contains("failed to run migrations"),
            "the outer context must survive too. got: {rendered}"
        );

        // ...and it must be CLASSIFIED, not just rendered. The log site picks a
        // different message on this, so a `None` here means an operator is told
        // to look at their encryption key and their disk for a healthy database.
        assert_eq!(
            schema_ahead_of_binary(&err),
            Some(999_999),
            "an applied-but-unknown migration must classify as schema-ahead, and \
             must name the version"
        );
    }

    /// The classifier must stay narrow: only "this binary is out of date".
    ///
    /// A checksum drift is repaired in place by [`reconcile_migration_checksums`]
    /// and a plain open failure has nothing to do with versions. Reporting either
    /// as "your daemon is older than the database" would send someone to update a
    /// binary that is already current, which is worse than the vague message it
    /// replaced.
    #[test]
    fn only_an_out_of_date_binary_classifies_as_schema_ahead() {
        use sqlx::migrate::MigrateError;

        for (label, err) in [
            (
                "a drifted checksum",
                anyhow::Error::new(MigrateError::VersionMismatch(7)),
            ),
            (
                "a half-applied migration",
                anyhow::Error::new(MigrateError::Dirty(7)),
            ),
            (
                "an unrelated open failure",
                anyhow::anyhow!("file is not a database"),
            ),
        ] {
            assert_eq!(
                schema_ahead_of_binary(&err.context("failed to run migrations")),
                None,
                "{label} must NOT classify as an out-of-date binary"
            );
        }

        // And the positive case still classifies through a context layer, which
        // is how it always arrives from `setup_db`.
        assert_eq!(
            schema_ahead_of_binary(
                &anyhow::Error::new(MigrateError::VersionMissing(84))
                    .context("failed to run migrations")
            ),
            Some(84),
        );
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

    /// `checkpoint_wal` must actually move committed data out of the `-wal`
    /// sidecar and truncate it — the entire point of calling it before
    /// shutdown. `:memory:` can't exercise this (no `-wal` file exists), so
    /// this uses a real temp-dir-backed database in WAL mode, same technique
    /// `test_corrupt.rs` uses for byte-level fixtures.
    #[tokio::test]
    async fn checkpoint_wal_truncates_the_sidecar_file() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
        use std::str::FromStr;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("checkpoint_test.db");
        let wal_path = dir.path().join("checkpoint_test.db-wal");

        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        // Single connection: a second one competing for the same WAL would
        // make the size assertions below flaky for reasons unrelated to what
        // this test is pinning.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("open WAL-mode db");

        sqlx::query("CREATE TABLE t (v BLOB)")
            .execute(&pool)
            .await
            .expect("create table");
        // A large-ish payload so the write actually lands in the WAL rather
        // than being trivially small enough to round to zero either way.
        sqlx::query("INSERT INTO t (v) VALUES (zeroblob(65536))")
            .execute(&pool)
            .await
            .expect("insert");

        let wal_len_before = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
        assert!(
            wal_len_before > 0,
            "fixture is wrong: nothing landed in the WAL before checkpointing"
        );

        checkpoint_wal(&pool).await.expect("checkpoint_wal");

        let wal_len_after = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            wal_len_after, 0,
            "TRUNCATE checkpoint must leave an empty WAL, got {wal_len_after} bytes"
        );
    }

    /// The failure this used to report as SUCCESS.
    ///
    /// `PRAGMA wal_checkpoint(TRUNCATE)` answers with a row whose first column
    /// is `busy`; `.execute()` discarded it, so a checkpoint blocked by another
    /// connection returned `Ok(())` having truncated nothing. The daemon then
    /// logged a clean shutdown and handed the next generation the very WAL this
    /// call exists to clear.
    ///
    /// The second connection here is not a contrivance - it is the tray's own
    /// long-lived pool in miniature, which `checkpoint_wal`'s doc names as the
    /// reason this function has to exist at all.
    #[tokio::test]
    async fn checkpoint_wal_reports_a_busy_file_instead_of_claiming_success() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
        use sqlx::Connection;
        use std::str::FromStr;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("busy_checkpoint.db");
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts.clone())
            .await
            .expect("open WAL-mode db");
        sqlx::query("CREATE TABLE t (v BLOB)")
            .execute(&pool)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO t (v) VALUES (zeroblob(65536))")
            .execute(&pool)
            .await
            .expect("insert");

        // A separate connection holding an OPEN read transaction, which is what
        // stops a TRUNCATE checkpoint from resetting the WAL.
        let mut reader = sqlx::SqliteConnection::connect_with(&opts)
            .await
            .expect("second connection");
        sqlx::query("BEGIN DEFERRED")
            .execute(&mut reader)
            .await
            .expect("begin");
        sqlx::query("SELECT count(*) FROM t")
            .fetch_all(&mut reader)
            .await
            .expect("read inside the transaction pins the WAL");

        let err = checkpoint_wal(&pool)
            .await
            .expect_err("a checkpoint blocked by a live reader must not report success");
        assert!(
            err.to_string().contains("could not truncate"),
            "the error must say the checkpoint did not happen, got: {err}"
        );

        let _ = sqlx::query("COMMIT").execute(&mut reader).await;
    }
}
