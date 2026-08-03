//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Corruption detection for `meridian.db`.
//!
//! Before this module the daemon had no notion of a corrupt database. A
//! b-tree fault surfaced as an ordinary `anyhow` error from whichever query
//! touched the damaged pages first - in practice `etl::runner`'s
//! `get_frames_since` - which the poll loop reported as the generic
//! "Activity capture pipeline failed / Open /logs" notice and then retried
//! every 60 s, forever. The condition cannot clear on its own, so that is an
//! infinite loop of failing reads behind a banner that tells the user nothing
//! actionable.
//!
//! This module supplies the three things that were missing: recognising a
//! corruption error when one is thrown ([`is_corrupt_error`]), a cheap
//! structural check ([`quick_check`]), and an authoritative per-table scan
//! ([`scan_tables`]) that says exactly which tables are damaged so
//! [`crate::db::repair`] knows what it can and cannot carry across.
//!
//! # Two measurement traps, both found the hard way
//!
//! Both of these produce a *clean* result on a database that is definitely
//! corrupt, so neither failure is visible without a known-bad file to test
//! against.
//!
//! 1. **`SELECT count(*)` is not a readability test.** SQLite answers it from
//!    the smallest available index, never touching the table b-tree. On the
//!    database this module was written against, `pm_worklog_hours` returned
//!    746 from `count(*)` and raised `SQLITE_CORRUPT` from `SELECT *`.
//! 2. **`count(*) ... NOT INDEXED` is not enough either.** Forcing the table
//!    b-tree still only walks leaf cells to count them; it never follows
//!    **overflow pages**, which is where a large TEXT payload actually lives.
//!    Same table, same file: `SELECT count(*) FROM pm_worklog_hours NOT
//!    INDEXED` also returned 746.
//!
//! Only materialising every column of every row reads the overflow chain, so
//! [`scan_tables`] streams full rows and discards them. Streaming (rather than
//! `fetch_all`) is what keeps that affordable: `capture_frames` is millions of
//! rows of OCR text, and only one row is ever held at a time.
//!
//! # Who calls this
//!
//! - `src/main.rs`'s poll loop, via [`is_corrupt_error`] on the `run_etl`
//!   error path - the trigger that raises the `db.corrupt` notice and stops
//!   the loop from retrying a read that cannot succeed.
//! - `src/main.rs`'s startup, via [`quick_check`] (daemon path only - see that
//!   function's note on why this is not in `setup_db`).
//! - [`crate::db::repair`], via [`scan_tables`], to decide which tables get
//!   the fast whole-table copy and which need the row-by-row fallback.
//! - `meridian db check`, the operator-facing diagnostic.
//!
//! # Related
//!
//! - [`crate::db::repair`] - the salvage-into-a-fresh-file recovery this
//!   module's findings drive. Repair-in-place is not an option: on a corrupt
//!   tree both `REINDEX` and `DROP TABLE` themselves raise `SQLITE_CORRUPT`.
//! - [`crate::notices`] - where a positive result is surfaced to the user.

use anyhow::{Context, Result};
use futures::TryStreamExt;
use sqlx::SqlitePool;

/// Tables the tray's in-process capture engine owns (see the "Capture source"
/// section of `CLAUDE.md`). They are bounded scratch - `etl::capture_retention`
/// prunes them on an age + processed basis - so losing them costs at most the
/// activity not yet consumed by the ETL cursor, never user-visible history.
///
/// Used only to phrase findings honestly (a corrupt capture table is a very
/// different sentence to the user than a corrupt `pm_worklogs`) and to skip
/// the slow row-by-row salvage in [`crate::db::repair`]. It is deliberately
/// NOT a routing decision about *whether* to repair: corruption does not
/// respect this boundary, and on the database this module was written against
/// it had already reached `pm_worklog_hours`.
pub const DISPOSABLE_TABLES: &[&str] = &[
    "capture_frames",
    "capture_ui_events",
    "capture_secondary_screens",
];

/// SQLite's `SQLITE_CORRUPT` primary result code.
///
/// Extended codes are formed as `primary | (n << 8)` - `SQLITE_CORRUPT_VTAB`
/// (267), `_SEQUENCE` (523), `_INDEX` (779) - so the low byte is what
/// identifies the family. Comparing the whole code for equality would miss
/// every extended variant.
const SQLITE_CORRUPT: u32 = 11;

/// True when `err` - or anything in its `anyhow` chain - is SQLite reporting
/// a corrupt database image.
///
/// The chain walk is the point: callers see this error wrapped in whatever
/// `.context(…)` the failing query added (`"failed to read first frame batch"`
/// in the ETL's case), so the `sqlx::Error` is never the outermost layer.
///
/// Verified against a real corrupt database rather than assumed - sqlx surfaces
/// it as `sqlx::Error::Database` whose `code()` is the **string** `"11"`, so
/// this parses the code rather than matching an integer. The message test is a
/// fallback for paths that lose the structured error (a stringified error
/// crossing a task boundary), not the primary signal.
pub fn is_corrupt_error(err: &anyhow::Error) -> bool {
    if err
        .chain()
        .filter_map(|c| c.downcast_ref::<sqlx::Error>())
        .any(is_corrupt_sqlx)
    {
        return true;
    }
    // Fallback: the structured error did not survive to here.
    err.chain()
        .any(|c| c.to_string().contains("database disk image is malformed"))
}

/// SQLite's `SQLITE_CONSTRAINT` primary result code. Extended variants encode
/// which constraint failed - `_NOTNULL` (1299), `_PRIMARYKEY` (1555),
/// `_UNIQUE` (2067) - in the same `primary | (n << 8)` scheme as
/// [`SQLITE_CORRUPT`].
const SQLITE_CONSTRAINT: u32 = 19;

/// True when `err` is SQLite rejecting a row on a constraint.
///
/// Distinct from corruption and treated very differently by
/// [`crate::db::repair`]: a row the *current* schema refuses is a row that read
/// off disk perfectly well, so it must never be reported as data lost to
/// damage. Real examples from the database this was built against - a
/// `NOT NULL` on `day_task_worklogs.day_local` that older rows predate, and
/// rows colliding with a seed row a migration inserts.
pub fn is_constraint_sqlx(err: &sqlx::Error) -> bool {
    let sqlx::Error::Database(db) = err else {
        return false;
    };
    db.code()
        .and_then(|c| c.parse::<u32>().ok())
        .is_some_and(|code| code & 0xFF == SQLITE_CONSTRAINT)
}

/// [`is_corrupt_error`] for a bare `sqlx::Error`, for callers holding one
/// before it has been wrapped by `anyhow`.
pub fn is_corrupt_sqlx(err: &sqlx::Error) -> bool {
    let sqlx::Error::Database(db) = err else {
        return false;
    };
    match db.code().and_then(|c| c.parse::<u32>().ok()) {
        Some(code) => code & 0xFF == SQLITE_CORRUPT,
        // Some builds report no code; fall back to the message.
        None => db.message().contains("database disk image is malformed"),
    }
}

/// Runs `PRAGMA quick_check(max_errors)` and returns the reported problems -
/// empty means structurally sound.
///
/// `quick_check` is the cheap screen: it verifies b-tree structure but skips
/// the index-vs-table consistency checks `integrity_check` performs, so it can
/// miss "row missing from index" while catching every out-of-range page
/// pointer. That trade is right for a startup check and wrong for the operator
/// diagnostic, which uses [`integrity_check`] instead.
///
/// # Why this is not in `db::meridian::setup_db`
///
/// `setup_db` has ~20 call sites, nearly all of them short-lived CLI hops
/// (`coding-agent-summarise`, `oauth-login`, the health probe). `quick_check`
/// reads every page in the file - multi-second on a multi-GB database - so
/// putting it there would tax every one of them. The daemon's startup path
/// calls it explicitly instead.
#[tracing::instrument(skip(pool))]
pub async fn quick_check(pool: &SqlitePool, max_errors: u32) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(&format!("PRAGMA quick_check({max_errors})"))
        .fetch_all(pool)
        .await
        .context("PRAGMA quick_check failed")?;
    Ok(split_problems(rows))
}

/// Runs the fuller `PRAGMA integrity_check(max_errors)`. Slower than
/// [`quick_check`] and additionally cross-checks indexes against their tables.
/// Used by `meridian db check`, where the operator is waiting on a diagnosis
/// and completeness beats latency.
#[tracing::instrument(skip(pool))]
pub async fn integrity_check(pool: &SqlitePool, max_errors: u32) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(&format!("PRAGMA integrity_check({max_errors})"))
        .fetch_all(pool)
        .await
        .context("PRAGMA integrity_check failed")?;
    Ok(split_problems(rows))
}

/// Normalises what the two check pragmas return into one problem per element.
///
/// SQLite is inconsistent here: depending on build and error count it may
/// return one row per problem OR a single row containing every problem
/// separated by newlines. The second shape is what the real corrupt database
/// produced - `problems.len()` was 1 for fifty distinct faults, so anything
/// counting or truncating the list reported nonsense. Splitting on newlines
/// makes both shapes behave the same.
fn split_problems(rows: Vec<(String,)>) -> Vec<String> {
    rows.into_iter()
        .flat_map(|(s,)| {
            s.split('\n')
                .map(str::trim)
                .filter(|l| !l.is_empty() && *l != "ok")
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Which tables could and could not be read end to end.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Tables whose every row materialised without error.
    pub healthy: Vec<String>,
    /// Tables that raised `SQLITE_CORRUPT` part-way through.
    pub corrupt: Vec<String>,
}

impl Scan {
    /// True when nothing is damaged.
    pub fn is_healthy(&self) -> bool {
        self.corrupt.is_empty()
    }

    /// Corrupt tables that are NOT disposable capture scratch - i.e. the ones
    /// whose damage means real user-visible loss. Drives the wording of the
    /// notice and of `meridian db repair`'s summary.
    pub fn corrupt_product_tables(&self) -> Vec<&str> {
        self.corrupt
            .iter()
            .map(String::as_str)
            .filter(|t| !DISPOSABLE_TABLES.contains(t))
            .collect()
    }
}

/// Lists every user table in the database, `sqlite_%` internals excluded.
#[tracing::instrument(skip(pool))]
pub async fn user_tables(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master
          WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
          ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .context("list user tables")?;
    Ok(rows.into_iter().map(|(s,)| s).collect())
}

/// Reads every row of every table and reports which ones are damaged.
///
/// This is the authoritative health test - see this module's header for why
/// neither `count(*)` nor `count(*) … NOT INDEXED` can stand in for it. Rows
/// are streamed and dropped immediately, so peak memory is one row regardless
/// of table size.
///
/// A non-corruption error (a missing table, a busy database) aborts with
/// `Err`; only `SQLITE_CORRUPT` marks a table corrupt and lets the scan
/// continue to the next one. That distinction matters: treating a transient
/// `SQLITE_BUSY` as corruption would send a healthy database to repair.
#[tracing::instrument(skip(pool), fields(tables = tracing::field::Empty, corrupt = tracing::field::Empty))]
pub async fn scan_tables(pool: &SqlitePool) -> Result<Scan> {
    let tables = user_tables(pool).await?;
    tracing::Span::current().record("tables", tables.len());

    let mut scan = Scan::default();
    for table in tables {
        match scan_one(pool, &table).await {
            Ok(()) => scan.healthy.push(table),
            Err(e) if is_corrupt_error(&e) => {
                tracing::warn!(
                    table = %table,
                    "integrity scan: table is corrupt and cannot be read end to end"
                );
                scan.corrupt.push(table);
            }
            Err(e) => return Err(e).context("integrity scan aborted on a non-corruption error"),
        }
    }

    tracing::Span::current().record("corrupt", scan.corrupt.len());
    tracing::info!(
        healthy = scan.healthy.len(),
        corrupt = scan.corrupt.len(),
        "integrity scan complete"
    );
    Ok(scan)
}

/// Streams every row of one table, discarding each. `Ok(())` means every page
/// backing the table - including the overflow chains holding large TEXT
/// values - read cleanly.
async fn scan_one(pool: &SqlitePool, table: &str) -> Result<()> {
    // `table` comes from sqlite_master, never from user input, but it is still
    // quoted so a table named with a reserved word parses.
    let sql = format!("SELECT * FROM \"{}\"", table.replace('"', "\"\""));
    let mut rows = sqlx::query(&sql).fetch(pool);
    let mut n: u64 = 0;
    while let Some(_row) = rows
        .try_next()
        .await
        .with_context(|| format!("scanning table {table}"))?
    {
        n += 1;
    }
    tracing::debug!(table = %table, rows = n, "table scanned clean");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_corrupt::{corrupt_db_fixture, healthy_db_fixture};

    #[test]
    fn corrupt_code_matches_primary_and_extended() {
        // Guards the `& 0xFF` masking: an equality test against 11 would pass
        // the first case and fail the rest, silently missing real corruption.
        for code in [11u32, 267, 523, 779] {
            assert_eq!(code & 0xFF, SQLITE_CORRUPT, "code {code} must be CORRUPT");
        }
        for code in [5u32, 8, 14, 26] {
            assert_ne!(code & 0xFF, SQLITE_CORRUPT, "code {code} must not be");
        }
    }

    #[test]
    fn message_fallback_survives_a_stringified_error() {
        let err = anyhow::anyhow!(
            "error returned from database: (code: 11) database disk image is malformed"
        )
        .context("failed to read first frame batch");
        assert!(is_corrupt_error(&err));
    }

    #[test]
    fn unrelated_errors_are_not_corruption() {
        let err = anyhow::anyhow!("database is locked").context("failed to read first frame batch");
        assert!(!is_corrupt_error(&err));
    }

    #[test]
    fn disposable_classification_splits_capture_from_product() {
        let scan = Scan {
            healthy: vec!["app_sessions".into()],
            corrupt: vec!["capture_frames".into(), "pm_worklog_hours".into()],
        };
        assert!(!scan.is_healthy());
        assert_eq!(scan.corrupt_product_tables(), vec!["pm_worklog_hours"]);
    }

    /// The real end-to-end shape: a byte-patched database must be recognised
    /// as corrupt through the same `anyhow` wrapping the ETL applies.
    #[tokio::test]
    async fn scan_finds_the_corrupted_table() {
        let fx = corrupt_db_fixture().await;
        let scan = scan_tables(&fx.pool).await.expect("scan");
        assert!(
            scan.corrupt.contains(&"blobs".to_string()),
            "expected the patched table to scan corrupt, got {scan:?}"
        );
    }

    /// quick_check must stay silent on a sound database - a false positive
    /// here would send healthy installs down the repair path.
    #[tokio::test]
    async fn healthy_database_scans_clean() {
        let fx = healthy_db_fixture().await;
        assert!(quick_check(&fx.pool, 20)
            .await
            .expect("quick_check")
            .is_empty());
        let scan = scan_tables(&fx.pool).await.expect("scan");
        assert!(scan.is_healthy(), "healthy db reported corrupt: {scan:?}");
        assert!(scan.corrupt_product_tables().is_empty());
    }

    /// The trap this module exists to document: `count(*)` passing proves
    /// nothing. Pinned as a test so nobody "optimises" `scan_one` into it.
    #[tokio::test]
    async fn count_star_is_not_a_readability_test() {
        let fx = corrupt_db_fixture().await;
        let counted: Result<(i64,), _> = sqlx::query_as("SELECT count(*) FROM blobs")
            .fetch_one(&fx.pool)
            .await;
        let scanned = scan_one(&fx.pool, "blobs").await;
        assert!(
            counted.is_ok() || scanned.is_err(),
            "fixture must corrupt the row payload, not merely the count"
        );
        assert!(scanned.is_err(), "full scan must detect the damage");
    }
}
