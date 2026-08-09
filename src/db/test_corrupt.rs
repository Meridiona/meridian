//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Test-only fixtures that produce a genuinely corrupt SQLite file.
//!
//! [`crate::db::integrity`] and [`crate::db::repair`] both exist to handle a
//! damaged database, and neither can be tested without one. The crate's other
//! DB tests use `sqlite::memory:` (see `tests/integration_etl.rs`), which is
//! useless here: there is no file to damage, so the only way to exercise these
//! paths is to write a real database to disk, close it, patch bytes into it,
//! and reopen.
//!
//! # How the damage is made
//!
//! The `blobs` table is filled with rows large enough to spill onto **overflow
//! pages**, then a run of pages in the middle of the file is overwritten with
//! `0xFF`. That byte is not a valid b-tree page type, so SQLite raises
//! `SQLITE_CORRUPT` the moment it reads one.
//!
//! Two details are load-bearing:
//!
//! - **Overflow, not just leaf cells.** The damage has to land in row payload,
//!   because that is the failure `integrity::scan_tables` exists to catch -
//!   `count(*)` walks leaf cells only and reports a corrupt table as fine (see
//!   that module's header). Small rows would give a fixture that the very bug
//!   being guarded against would still pass.
//! - **`journal_mode = DELETE`, not WAL.** In WAL mode committed pages may
//!   still be sitting in the `-wal` sidecar, so patching the main file would
//!   damage bytes SQLite then reads straight over from the log - producing a
//!   fixture that is intermittently healthy.
//!
//! Page 1 is never touched: it holds the file header and the `sqlite_master`
//! root, and a database whose schema cannot be read is a different failure
//! from the one under test.
//!
//! # Who calls this
//!
//! [`crate::db::integrity`]'s and [`crate::db::repair`]'s test modules.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::str::FromStr;

/// SQLite's default page size, and the one these fixtures pin explicitly so
/// the byte offsets below are predictable.
const PAGE_SIZE: u64 = 4096;

/// First page the patcher is allowed to touch. Page 1 (offset 0) is the file
/// header + `sqlite_master` root; pages 2-9 are left alone so the schema and
/// the small `keepers` table stay readable.
const FIRST_DAMAGED_PAGE: u64 = 40;

/// How many consecutive pages to destroy.
const DAMAGED_PAGES: u64 = 12;

/// An open pool over a temp-dir database. The `TempDir` is held so the
/// directory outlives the pool - dropping it early would delete the file out
/// from under an open connection.
pub struct Fixture {
    pub pool: SqlitePool,
    pub path: PathBuf,
    _dir: tempfile::TempDir,
}

/// Builds a populated, structurally sound database.
///
/// `keepers` is small and lives early in the file; `blobs` is large enough to
/// reach the pages [`corrupt_db_fixture`] destroys. Both are plain user tables
/// so `integrity::user_tables` finds them.
pub async fn healthy_db_fixture() -> Fixture {
    let (pool, path, dir) = build(false).await;
    Fixture {
        pool,
        path,
        _dir: dir,
    }
}

/// The same database, with a run of pages overwritten so `blobs` cannot be
/// read end to end while `keepers` still can.
pub async fn corrupt_db_fixture() -> Fixture {
    let (pool, path, dir) = build(true).await;
    Fixture {
        pool,
        path,
        _dir: dir,
    }
}

async fn build(damage: bool) -> (SqlitePool, PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fixture.db");

    {
        let pool = open(&path).await;
        sqlx::query("PRAGMA page_size = 4096")
            .execute(&pool)
            .await
            .expect("page_size");
        // SQLite ignores `page_size` once the database header exists (which
        // `open` above already created), unless a `VACUUM` follows - so this
        // PRAGMA can silently no-op and leave the file on whatever the build's
        // default happens to be. Read the effective value back rather than
        // assume it took, so a mismatch fails loudly instead of quietly
        // moving the byte offsets `FIRST_DAMAGED_PAGE`/`DAMAGED_PAGES` assume.
        let (effective,): (i64,) = sqlx::query_as("PRAGMA page_size")
            .fetch_one(&pool)
            .await
            .expect("read page_size");
        assert_eq!(
            effective as u64, PAGE_SIZE,
            "the fixture's byte offsets assume a {PAGE_SIZE}-byte page"
        );
        sqlx::query("CREATE TABLE keepers (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .expect("create keepers");
        sqlx::query(
            "CREATE TABLE blobs (id INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create blobs");

        for i in 0..8 {
            sqlx::query("INSERT INTO keepers (id, name) VALUES (?, ?)")
                .bind(i)
                .bind(format!("keeper-{i}"))
                .execute(&pool)
                .await
                .expect("insert keeper");
        }

        // ~8 KB each: comfortably past the point where SQLite spills the value
        // onto overflow pages, which is the whole point of the fixture.
        let payload = "x".repeat(8 * 1024);
        for i in 0..400 {
            sqlx::query("INSERT INTO blobs (id, payload) VALUES (?, ?)")
                .bind(i)
                .bind(&payload)
                .execute(&pool)
                .await
                .expect("insert blob");
        }

        // Close deterministically: a plain drop only *queues* the close (sqlx
        // runs SQLite on a worker thread), and patching a file whose handle is
        // still open races the final flush.
        pool.close().await;
    }

    if damage {
        patch(&path);
    }

    let pool = open(&path).await;
    (pool, path, dir)
}

async fn open(path: &std::path::Path) -> SqlitePool {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .expect("uri")
        .create_if_missing(true)
        // See the module header: WAL would leave committed pages outside the
        // main file and make the patch below unreliable.
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open fixture db")
}

/// Overwrites [`DAMAGED_PAGES`] pages with `0xFF` starting at
/// [`FIRST_DAMAGED_PAGE`]. Panics if the file never grew that far - a silently
/// under-sized fixture would produce a test that passes because nothing was
/// damaged at all.
fn patch(path: &std::path::Path) {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open fixture for patching");
    let len = f.metadata().expect("metadata").len();
    let start = (FIRST_DAMAGED_PAGE - 1) * PAGE_SIZE;
    let end = start + DAMAGED_PAGES * PAGE_SIZE;
    assert!(
        len >= end,
        "fixture is only {len} bytes; needs >= {end} for the patch to land inside the file"
    );
    f.seek(SeekFrom::Start(start)).expect("seek");
    f.write_all(&vec![0xFFu8; (DAMAGED_PAGES * PAGE_SIZE) as usize])
        .expect("write garbage");
    f.sync_all().expect("sync");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture is only useful if it actually breaks the thing under test.
    /// Without this, a fixture that silently stopped corrupting anything would
    /// turn every consumer's test green for the wrong reason.
    #[tokio::test]
    async fn corrupt_fixture_really_is_corrupt() {
        let fx = corrupt_db_fixture().await;
        let res = sqlx::query("SELECT * FROM blobs").fetch_all(&fx.pool).await;
        let err = res.err().expect("blobs must fail to read");
        assert!(
            crate::db::integrity::is_corrupt_sqlx(&err),
            "expected SQLITE_CORRUPT, got: {err}"
        );
    }

    /// ...and only that thing: the scan must be able to tell damaged tables
    /// from sound ones, which needs a survivor in the same file.
    #[tokio::test]
    async fn keepers_survives_the_patch() {
        let fx = corrupt_db_fixture().await;
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM keepers ORDER BY id")
            .fetch_all(&fx.pool)
            .await
            .expect("keepers must still read");
        assert_eq!(rows.len(), 8);
    }

    #[tokio::test]
    async fn healthy_fixture_reads_clean() {
        let fx = healthy_db_fixture().await;
        let rows: Vec<(i64,)> = sqlx::query_as("SELECT id FROM blobs")
            .fetch_all(&fx.pool)
            .await
            .expect("blobs must read");
        assert_eq!(rows.len(), 400);
    }
}
