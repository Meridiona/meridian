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

    let attach = match key_hex {
        Some(k) => {
            meridian_core::db_crypto::validate_key_hex(k)?;
            format!(
                "ATTACH DATABASE '{}' AS src KEY \"x'{}'\"",
                src_path.display(),
                k
            )
        }
        None => format!("ATTACH DATABASE '{}' AS src KEY ''", src_path.display()),
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

    carry_sqlite_sequence(&mut conn).await?;

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
    drop(conn);
    tmp_pool.close().await;

    Ok(report)
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
}
