//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The per-table copy engine behind [`super::repair`].
//!
//! Answers exactly one question - "how do I get the rows of ONE table across
//! from a damaged file?" - and holds every hard-won answer about doing it.
//! [`super`] owns the surrounding operation (guards, the swap, the report) and
//! [`super::rebuild`] the middle step that drives this once per table. Split
//! three ways so each file stays under the repo's 500-line ceiling.
//!
//! # Who calls this
//!
//! [`super::rebuild::build_replacement`], once per table.
//!
//! # Related
//!
//! - [`super`] - its header documents the tiers and the four traps these
//!   functions exist to survive.
//! - [`crate::db::integrity`] - the corruption/constraint classifiers that
//!   decide which tier a failure falls to.

use anyhow::{Context, Result};

use super::TableOutcome;
use crate::db::integrity::{self, is_constraint_sqlx, is_corrupt_sqlx};

/// Tables that must never be copied from the damaged source.
///
/// `_sqlx_migrations` is sqlx's own ledger, and the replacement already has a
/// correct one: [`build_replacement`] just ran the migrations against it, so
/// every row is present with a checksum matching the embedded file. Copying
/// the source's on top duplicates every version and fails the primary key -
/// which is exactly what happened the first time this ran against the real
/// database (`UNIQUE constraint failed: _sqlx_migrations.version`). Even
/// without the constraint it would be wrong: the source's checksums describe
/// whatever migration bytes that file was built with, not this build's.
const NEVER_COPY: &[&str] = &["_sqlx_migrations"];

/// Tables present in BOTH the replacement schema and the damaged source,
/// minus [`NEVER_COPY`].
///
/// Intersecting matters in both directions: a table the migrations dropped
/// still exists in an old source (nothing to copy into), and a table a *newer*
/// migration added does not exist in the source (nothing to copy from).
pub(super) async fn table_names(conn: &mut sqlx::SqliteConnection) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT m.name FROM main.sqlite_master m
          WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'
            AND EXISTS (SELECT 1 FROM src.sqlite_master s
                         WHERE s.type = 'table' AND s.name = m.name)
          ORDER BY m.name",
    )
    .fetch_all(&mut *conn)
    .await
    .context("listing tables common to both databases")?;
    Ok(rows
        .into_iter()
        .map(|(s,)| s)
        .filter(|t| !NEVER_COPY.contains(&t.as_str()))
        .collect())
}

/// Column names a table has in `schema` (`"main"` or `"src"`), in declared
/// order.
async fn columns_of(
    conn: &mut sqlx::SqliteConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info(?1, ?2)")
        .bind(table)
        .bind(schema)
        .fetch_all(&mut *conn)
        .await
        .with_context(|| format!("reading columns of {schema}.{table}"))?;
    Ok(rows.into_iter().map(|(s,)| s).collect())
}

/// The columns to actually copy: those the damaged source and the freshly
/// migrated replacement have in common, in the replacement's order.
///
/// `INSERT INTO t SELECT * FROM src.t` is positional, and that is not safe
/// here. The real database this was built for has 32 columns in
/// `app_sessions` where the current migrations produce 31, so the bulk copy
/// died with "table main.app_sessions has 31 columns but 32 values were
/// supplied" - a repair that aborts on schema drift is a repair that does not
/// run when it is most needed.
///
/// Naming the columns explicitly makes the copy survive a column added,
/// removed, or reordered on either side: a column only the replacement has
/// takes its default, and one only the source has is dropped (and logged, so
/// the loss is never silent).
async fn copy_columns(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<Vec<String>> {
    let dst = columns_of(conn, "main", table).await?;
    let src = columns_of(conn, "src", table).await?;

    let dropped: Vec<&String> = src.iter().filter(|c| !dst.contains(c)).collect();
    if !dropped.is_empty() {
        tracing::warn!(
            table = %table,
            columns = ?dropped,
            "source has columns the current schema does not - their values are not carried across"
        );
    }
    let defaulted: Vec<&String> = dst.iter().filter(|c| !src.contains(c)).collect();
    if !defaulted.is_empty() {
        tracing::info!(
            table = %table,
            columns = ?defaulted,
            "current schema has columns the source lacks - they take their defaults"
        );
    }

    Ok(dst.into_iter().filter(|c| src.contains(c)).collect())
}

/// Copies one table using the three tiers described in the module header.
pub(super) async fn copy_table(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
) -> Result<TableOutcome> {
    let q = quote_ident(table);
    let mut outcome = TableOutcome {
        table: table.to_string(),
        rows_copied: 0,
        rows_unreadable: 0,
        rows_rejected: 0,
        salvaged_row_by_row: false,
        left_empty: false,
    };

    // The replacement is NOT empty: several migrations seed a default row
    // (`etl_cursor`'s cursor, `activity_context`), so copying the source's
    // rows on top collides on the primary key - which is what happened the
    // first time this ran for real (`UNIQUE constraint failed:
    // activity_context.id`). The source's rows are the user's actual state and
    // must win, so the seeds are cleared first.
    //
    // Guarded on the source having rows at all: clearing a seed and then
    // copying nothing over it would leave the table emptier than a fresh
    // install, turning a repair into a different kind of damage.
    if source_has_rows(conn, &q).await {
        sqlx::query(&format!("DELETE FROM main.{q}"))
            .execute(&mut *conn)
            .await
            .with_context(|| format!("clearing seeded rows from {table}"))?;
    }

    // Explicit column list, never `SELECT *` — see `copy_columns`.
    let cols = copy_columns(conn, table).await?;
    if cols.is_empty() {
        tracing::warn!(table = %table, "no columns in common with the source - left empty");
        outcome.left_empty = true;
        return Ok(outcome);
    }
    let col_list = cols
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");

    // Tier 1: whole table in one statement.
    let bulk = sqlx::query(&format!(
        "INSERT INTO main.{q} ({col_list}) SELECT {col_list} FROM src.{q}"
    ))
    .execute(&mut *conn)
    .await;

    match bulk {
        Ok(res) => {
            outcome.rows_copied = res.rows_affected();
            return Ok(outcome);
        }
        Err(e) if is_corrupt_sqlx(&e) => {
            tracing::warn!(table = %table, "whole-table copy hit corruption - salvaging row by row");
        }
        // A constraint rejection is all-or-nothing at this tier too: ONE row
        // the current schema refuses kills the whole statement. Aborting the
        // rebuild over it would be absurd - the data is perfectly readable -
        // so drop to row-by-row, which isolates the offending rows and counts
        // them as rejected rather than lost. Seen for real on
        // `day_task_worklogs.day_local`, a NOT NULL that predates some rows.
        Err(e) if is_constraint_sqlx(&e) => {
            tracing::warn!(
                table = %table,
                error = %e,
                "whole-table copy hit a constraint - salvaging row by row"
            );
        }
        Err(e) => {
            return Err(e).with_context(|| format!("copying table {table}"));
        }
    }

    // A partially-applied bulk INSERT would double-count rows the row-by-row
    // pass is about to re-insert.
    sqlx::query(&format!("DELETE FROM main.{q}"))
        .execute(&mut *conn)
        .await
        .with_context(|| format!("clearing partially-copied table {table}"))?;

    // Tier 3: disposable scratch is not worth salvaging row by row.
    if integrity::DISPOSABLE_TABLES.contains(&table) {
        tracing::warn!(
            table = %table,
            "capture scratch is corrupt - leaving it empty rather than salvaging row by row"
        );
        outcome.left_empty = true;
        return Ok(outcome);
    }

    // Tier 2: row by row, skipping only what genuinely cannot be read.
    outcome.salvaged_row_by_row = true;
    let Some((lo, hi)) = rowid_bounds(conn, &q).await else {
        tracing::warn!(table = %table, "could not read the rowid range - table left empty");
        outcome.left_empty = true;
        return Ok(outcome);
    };

    for rowid in lo..=hi {
        let res = sqlx::query(&format!(
            "INSERT INTO main.{q} ({col_list}) SELECT {col_list} FROM src.{q} WHERE rowid = ?"
        ))
        .bind(rowid)
        .execute(&mut *conn)
        .await;
        match res {
            Ok(r) => outcome.rows_copied += r.rows_affected(),
            Err(e) if is_corrupt_sqlx(&e) => outcome.rows_unreadable += 1,
            // Read fine, but the current schema refused it. Not corruption -
            // see the module header.
            Err(e) if is_constraint_sqlx(&e) => outcome.rows_rejected += 1,
            // Anything else (I/O, a type mismatch) is not something to paper
            // over silently, but it must not abandon the rows already
            // salvaged either - count it and keep going.
            Err(e) => {
                tracing::warn!(table = %table, rowid, error = %e, "row skipped on an unexpected error");
                outcome.rows_rejected += 1;
            }
        }
    }
    Ok(outcome)
}

/// Whether the source table has anything in it.
///
/// `EXISTS` rather than `count(*)` so a huge table costs one seek, and a
/// failure counts as **yes**: the only way this errors is corruption, and a
/// corrupt table certainly holds rows. Answering "no" there would skip
/// clearing the seed rows and put the copy back into the primary-key collision
/// this exists to avoid.
async fn source_has_rows(conn: &mut sqlx::SqliteConnection, q: &str) -> bool {
    sqlx::query_as::<_, (i64,)>(&format!("SELECT EXISTS(SELECT 1 FROM src.{q})"))
        .fetch_one(&mut *conn)
        .await
        .map(|(n,)| n != 0)
        .unwrap_or(true)
}

/// Finds the lowest and highest rowid of a table that may be damaged.
///
/// `SELECT min(rowid), max(rowid)` is the obvious way and it is not good
/// enough: SQLite evaluates both aggregates in one pass that can cross the
/// damaged pages, so on the fixture in `db::test_corrupt` it raises
/// `SQLITE_CORRUPT` and yields nothing at all. Losing the bounds means losing
/// the whole table, because tier 2 has no range to walk - which is precisely
/// the outcome tier 2 exists to prevent.
///
/// Two independent single-row seeks avoid that: ascending stops at the first
/// leaf, descending at the last, and neither has to traverse the damage in
/// between. Each is tried separately so damage at one end still leaves the
/// other usable.
async fn rowid_bounds(conn: &mut sqlx::SqliteConnection, q: &str) -> Option<(i64, i64)> {
    async fn end(conn: &mut sqlx::SqliteConnection, q: &str, dir: &str) -> Option<i64> {
        sqlx::query_as::<_, (i64,)>(&format!(
            "SELECT rowid FROM src.{q} ORDER BY rowid {dir} LIMIT 1"
        ))
        .fetch_optional(&mut *conn)
        .await
        .ok()
        .flatten()
        .map(|(v,)| v)
    }

    if let (Some(lo), Some(hi)) = (end(conn, q, "ASC").await, end(conn, q, "DESC").await) {
        return Some((lo, hi));
    }

    // Both ends unreadable, or only one. Falling back to a synthetic range
    // would be unbounded (there is no "last possible rowid" to stop at), so
    // instead enumerate rowids directly: this reads the table b-tree WITHOUT
    // touching overflow pages, which is where large payloads - and most of the
    // damage - live. A scan that dies part-way still yields a usable prefix,
    // and the bounds derived from it are honest about covering only that much.
    let mut lo: Option<i64> = None;
    let mut hi: Option<i64> = None;
    let sql = format!("SELECT rowid FROM src.{q}");
    let mut rows = sqlx::query_as::<_, (i64,)>(&sql).fetch(conn);
    while let Ok(Some((rowid,))) = futures::TryStreamExt::try_next(&mut rows).await {
        lo.get_or_insert(rowid);
        hi = Some(rowid);
    }
    match (lo, hi) {
        (Some(lo), Some(hi)) => {
            tracing::warn!(
                lo,
                hi,
                "rowid bounds recovered only by scanning; rows past the damage are unreachable"
            );
            Some((lo, hi))
        }
        _ => None,
    }
}

/// Double-quotes an identifier so a table named with a reserved word parses.
/// Names come from `sqlite_master`, never from user input.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_corrupt::corrupt_db_fixture;
    use sqlx::Executor;

    #[test]
    fn quote_ident_escapes_embedded_quotes() {
        assert_eq!(quote_ident("app_sessions"), "\"app_sessions\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    /// The end-to-end claim this module makes: a corrupt table is salvaged
    /// row by row and keeps nearly everything, while a healthy table in the
    /// same file copies whole.
    #[tokio::test]
    async fn row_by_row_salvage_beats_losing_the_table() {
        let fx = corrupt_db_fixture().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path().join("rebuilt.db");

        // The fixture's schema is not meridian's, so exercise the copy engine
        // directly against a replacement built from the fixture's own DDL.
        let tmp_uri = format!("sqlite://{}", tmp.display());
        let pool = meridian_core::db_crypto::open_pool_with_key(&tmp_uri, None, true, &[])
            .await
            .expect("create replacement");
        pool.execute("CREATE TABLE keepers (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .await
            .expect("keepers");
        pool.execute(
            "CREATE TABLE blobs (id INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL)",
        )
        .await
        .expect("blobs");

        let mut conn = pool.acquire().await.expect("acquire");
        conn.execute(format!("ATTACH DATABASE '{}' AS src KEY ''", fx.path.display()).as_str())
            .await
            .expect("attach");

        let keepers = copy_table(&mut conn, "keepers")
            .await
            .expect("copy keepers");
        assert_eq!(keepers.rows_copied, 8);
        assert!(!keepers.salvaged_row_by_row, "healthy table takes tier 1");

        let blobs = copy_table(&mut conn, "blobs").await.expect("copy blobs");
        assert!(blobs.salvaged_row_by_row, "corrupt table must fall back");
        assert!(
            blobs.rows_unreadable > 0,
            "the damaged rows must be counted as lost, got {blobs:?}"
        );
        assert!(
            blobs.rows_copied > 0,
            "row-by-row must still recover the undamaged rows, got {blobs:?}"
        );
        assert_eq!(
            blobs.rows_copied + blobs.rows_unreadable,
            400,
            "every row must be accounted for as copied or lost: {blobs:?}"
        );

        // The whole point: tier 1 would have yielded zero.
        assert!(
            blobs.rows_copied > 300,
            "salvage should keep most of the table, got {}",
            blobs.rows_copied
        );

        drop(conn);
        pool.close().await;
    }
}
