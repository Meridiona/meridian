//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Building the replacement database: fresh file, canonical schema, then every
//! table copied across.
//!
//! [`super`] owns the operation the user asked for (guards, the atomic swap,
//! the report); this owns the middle step that produces the file that swap
//! installs; [`super::copy`] owns one table at a time. Split three ways so
//! each file stays under the repo's 500-line ceiling.
//!
//! # Who calls this
//!
//! [`super::repair`], exactly once.
//!
//! # Related
//!
//! - [`super`] - its header explains why a rebuild is the only available
//!   repair, and the four traps this step has to survive.
//! - [`super::copy`] - the per-table copy engine invoked from here.

use anyhow::{Context, Result};
use sqlx::Executor;
use std::path::Path;

use super::copy::{copy_table, table_names};
use super::RepairReport;

/// Writes the replacement database at `tmp_path` from the (damaged) source.
/// Does not touch `db_path`.
pub(super) async fn build_replacement(
    src_path: &Path,
    tmp_path: &Path,
    key_hex: Option<&str>,
) -> Result<RepairReport> {
    let tmp_uri = format!("sqlite://{}", tmp_path.display());

    // A brand-new file, so `auto_vacuum=INCREMENTAL` actually takes effect
    // here — SQLite only honours it before the first table is created, which
    // is why an established database is stuck on `NONE` and why
    // `etl::capture_retention`'s incremental_vacuum has been a documented
    // no-op. A repaired database gets working vacuuming as a side effect.
    let tmp_pool = meridian_core::db_crypto::open_pool_with_key(
        &tmp_uri,
        key_hex,
        true,
        &[("auto_vacuum", "INCREMENTAL")],
    )
    .await
    .context("creating the replacement database")?;

    // The schema comes from the migrations, not from the damaged file's
    // `sqlite_master`. Copying the old DDL would faithfully reproduce whatever
    // state that file was in; running the migrations produces the schema this
    // build expects, and leaves `_sqlx_migrations` consistent with it.
    sqlx::migrate!("src/migrations")
        .run(&tmp_pool)
        .await
        .context("building the replacement schema")?;

    // Everything below runs on ONE connection: `ATTACH` is per-connection
    // state, so a pooled query could otherwise land on a connection that has
    // never seen `src`.
    let mut conn = tmp_pool
        .acquire()
        .await
        .context("acquiring the rebuild connection")?;

    // Foreign keys OFF for the duration of the copy.
    //
    // sqlx turns them ON by default, and tables are copied in name order, so a
    // child lands before its parent and every one of its rows is rejected:
    // `pm_worklog_feedback` (which REFERENCES `pm_worklogs`) lost all 20 rows
    // this way on the first real run - silently, counted as "schema drift".
    //
    // Ordering the copy by dependency instead would be fragile and would still
    // break on a cycle. Enforcement is simply the wrong thing here: this is a
    // restore of an already-consistent dataset, not acceptance of new writes.
    // `foreign_key_check` below re-verifies the result, so nothing is taken on
    // trust.
    conn.execute("PRAGMA foreign_keys = OFF")
        .await
        .context("disabling foreign keys for the rebuild")?;

    // `ATTACH` takes no bind parameter for the filename, so the path goes in
    // as a SQL literal. Double any embedded single quote - a home directory
    // such as `/Users/o'brien/.meridian` would otherwise break the statement,
    // and recovery is the last resort, so it must not fail on a legal path.
    let src_literal = src_path.display().to_string().replace('\'', "''");
    let attach = match key_hex {
        Some(k) => {
            meridian_core::db_crypto::validate_key_hex(k)?;
            format!("ATTACH DATABASE '{src_literal}' AS src KEY \"x'{k}'\"")
        }
        None => format!("ATTACH DATABASE '{src_literal}' AS src KEY ''"),
    };
    conn.execute(attach.as_str())
        .await
        .context("attaching the damaged database")?;

    let tables = table_names(&mut conn).await?;
    let mut report = RepairReport::default();
    for table in tables {
        let outcome = copy_table(&mut conn, &table).await?;
        tracing::info!(
            table = %outcome.table,
            rows_copied = outcome.rows_copied,
            rows_unreadable = outcome.rows_unreadable,
            rows_rejected = outcome.rows_rejected,
            salvaged_row_by_row = outcome.salvaged_row_by_row,
            left_empty = outcome.left_empty,
            "table rebuilt"
        );
        report.tables.push(outcome);
    }

    // Aborting the whole repair here would throw away a salvage that already
    // succeeded for every table, over a failure that is deterministic - a
    // retry would hit the exact same damaged `sqlite_sequence` page and fail
    // identically, so the user could never repair the database despite every
    // row having been readable. Record it instead and let the CLI say so.
    if let Err(e) = carry_sqlite_sequence(&mut conn).await {
        tracing::error!(
            error = %e,
            "could not carry sqlite_sequence into the replacement - AUTOINCREMENT ids may be reused"
        );
        report.sequence_carried = false;
    }

    // Enforcement was off for the copy, so verify rather than assume. A
    // dangling reference here means the source was already inconsistent, or a
    // parent row was one of the unreadable ones — worth surfacing, but never
    // worth failing a repair that has just recovered everything else.
    match sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *conn)
        .await
    {
        Ok(rows) if rows.is_empty() => {
            tracing::info!("rebuilt database passes foreign_key_check");
        }
        Ok(rows) => tracing::warn!(
            violations = rows.len(),
            "rebuilt database has dangling references - a parent row was probably unreadable"
        ),
        Err(e) => tracing::warn!(error = %e, "foreign_key_check could not run"),
    }

    conn.execute("DETACH DATABASE src")
        .await
        .context("detaching the damaged database")?;

    // Close this connection EXPLICITLY rather than dropping it.
    //
    // `drop` hands a pooled connection back to the pool from a spawned task,
    // and `pool.close()` does not reliably wait for that hand-back to land - so
    // the pool reports closed while one connection is still open on the file.
    // On Unix that shows up as a `-wal` outliving the rebuild (caught by
    // [`ensure_checkpointed`]); on Windows the surviving handle makes the swap's
    // `rename(tmp, path)` fail with os error 32, which is exactly how this was
    // found - a CI failure that looked like an antivirus race and was not.
    //
    // `close()` awaits the real `sqlite3_close`, so afterwards the file is
    // provably released and the WAL provably checkpointed away.
    conn.close()
        .await
        .context("closing the rebuild connection")?;
    tmp_pool.close().await;

    ensure_checkpointed(tmp_path)?;
    Ok(report)
}

/// Refuses to hand back a replacement that still has a write-ahead log.
///
/// The pool runs in WAL mode, so rows land in `<tmp>-wal` and only reach the
/// database file at a checkpoint. `close()` checkpoints and deletes both
/// sidecars - verified, not assumed (`sidecars_are_gone_after_the_rebuild`) -
/// but the swap that follows renames **only the main file**. If a `-wal` ever
/// survived, the repair would install a database missing whatever the log still
/// held, in the one operation whose entire purpose is not losing rows, and it
/// would look like success.
///
/// So this is a cheap assertion on an invariant that currently holds, not a
/// workaround for one that doesn't. Failing here is safe: nothing has been
/// swapped, and [`super::repair`] deletes the scratch file and reports the
/// original as unchanged.
fn ensure_checkpointed(tmp_path: &Path) -> Result<()> {
    let wal = std::path::PathBuf::from(format!("{}-wal", tmp_path.display()));
    // A missing sidecar is the only case that actually means "no WAL held" -
    // collapsing every stat error into that (the previous `unwrap_or(0)`) let
    // a permissions problem or another transient I/O error masquerade as a
    // clean checkpoint on the one check standing between a corrupt swap and a
    // sound one. Anything other than "not found" fails loud instead.
    let held = match std::fs::metadata(&wal) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => {
            return Err(e).with_context(|| {
                format!("could not check for a write-ahead log at {}", wal.display())
            })
        }
    };
    if held > 0 {
        anyhow::bail!(
            "the rebuilt database still has an un-checkpointed write-ahead log \
             ({held} bytes) - swapping it in would silently drop the rows it holds"
        );
    }
    Ok(())
}

/// Carries `sqlite_sequence` high-water marks across.
///
/// Load-bearing, and silently catastrophic if skipped - see the module header.
/// Only ever raises a value: the copy above already advanced the sequence for
/// tables that copied cleanly, and lowering one would hand out ids that are
/// already in use.
async fn carry_sqlite_sequence(conn: &mut sqlx::SqliteConnection) -> Result<()> {
    // `sqlite_sequence` only exists once an AUTOINCREMENT table has been
    // created, which the migrations guarantee for the real schema but not for
    // an arbitrary source.
    let has_seq: Option<(String,)> =
        sqlx::query_as("SELECT name FROM src.sqlite_master WHERE name = 'sqlite_sequence'")
            .fetch_optional(&mut *conn)
            .await
            .context("checking for sqlite_sequence")?;
    if has_seq.is_none() {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO main.sqlite_sequence (name, seq)
         SELECT s.name, s.seq FROM src.sqlite_sequence s
          WHERE s.name NOT IN (SELECT name FROM main.sqlite_sequence)",
    )
    .execute(&mut *conn)
    .await
    .context("carrying new sqlite_sequence rows")?;

    let updated = sqlx::query(
        "UPDATE main.sqlite_sequence AS m
            SET seq = (SELECT s.seq FROM src.sqlite_sequence s WHERE s.name = m.name)
          WHERE EXISTS (SELECT 1 FROM src.sqlite_sequence s
                         WHERE s.name = m.name AND s.seq > m.seq)",
    )
    .execute(&mut *conn)
    .await
    .context("raising sqlite_sequence high-water marks")?;

    tracing::info!(
        raised = updated.rows_affected(),
        "carried sqlite_sequence high-water marks into the replacement"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_corrupt::corrupt_db_fixture;

    /// `sqlite_sequence` must never come out lower than it went in.
    #[tokio::test]
    async fn sqlite_sequence_high_water_mark_is_carried() {
        let fx = corrupt_db_fixture().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path().join("rebuilt.db");
        let tmp_uri = format!("sqlite://{}", tmp.display());
        let pool = meridian_core::db_crypto::open_pool_with_key(&tmp_uri, None, true, &[])
            .await
            .expect("create");
        pool.execute(
            "CREATE TABLE blobs (id INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL)",
        )
        .await
        .expect("blobs");
        // One row, so main's own sequence is far below the source's 400.
        pool.execute("INSERT INTO blobs (payload) VALUES ('seed')")
            .await
            .expect("seed");

        let mut conn = pool.acquire().await.expect("acquire");
        conn.execute(format!("ATTACH DATABASE '{}' AS src KEY ''", fx.path.display()).as_str())
            .await
            .expect("attach");
        carry_sqlite_sequence(&mut conn).await.expect("carry");

        let (seq,): (i64,) =
            sqlx::query_as("SELECT seq FROM main.sqlite_sequence WHERE name='blobs'")
                .fetch_one(&mut *conn)
                .await
                .expect("read seq");
        let (src_seq,): (i64,) =
            sqlx::query_as("SELECT seq FROM src.sqlite_sequence WHERE name='blobs'")
                .fetch_one(&mut *conn)
                .await
                .expect("read source seq");
        assert_eq!(
            seq, src_seq,
            "the replacement must inherit the source's high-water mark, not its own"
        );
        assert!(src_seq > 1, "fixture must leave a mark above main's own");

        drop(conn);
        pool.close().await;
    }

    /// Pins the invariant [`ensure_checkpointed`] guards: a closed pool leaves
    /// no write-ahead log behind, so the swap's single-file rename carries every
    /// row. If sqlx ever stopped checkpointing on close, this fails here rather
    /// than as unexplained missing rows after a repair.
    #[tokio::test]
    async fn sidecars_are_gone_after_the_rebuild() {
        let fx = corrupt_db_fixture().await;
        fx.pool.close().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path().join("rebuilt.db");

        build_replacement(&fx.path, &tmp, None)
            .await
            .expect("rebuild");

        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{}", tmp.display(), suffix));
            assert!(
                !sidecar.exists(),
                "{} survived the rebuild - the swap renames only the main file, \
                 so its contents would be silently lost",
                sidecar.display()
            );
        }
        ensure_checkpointed(&tmp).expect("a checkpointed rebuild must pass the guard");
    }

    /// ...and the guard actually refuses when the log is there, rather than
    /// being an assertion that can only ever pass.
    #[test]
    fn a_surviving_write_ahead_log_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path().join("rebuilt.db");
        std::fs::write(&tmp, b"db").expect("db");
        std::fs::write(format!("{}-wal", tmp.display()), b"unflushed rows").expect("wal");

        let err = ensure_checkpointed(&tmp).expect_err("a non-empty WAL must stop the swap");
        assert!(
            err.to_string().contains("write-ahead log"),
            "the error must name what went wrong: {err}"
        );
    }
}
