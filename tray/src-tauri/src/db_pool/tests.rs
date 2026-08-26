//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for the tray's swappable `meridian.db` pool handle.
//!
//! Split out of `mod.rs` for the 500-line file cap, following the same
//! `{mod,tests}.rs` shape as `meridian-core/src/pm_sync_requests/` and
//! `src/intelligence/providers/jira/`.

use super::*;

async fn migrated_db(dir: &std::path::Path) -> (String, meridian_core::SqlitePool) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    let path = dir.join("db_pool_test.db");
    let uri = format!("sqlite://{}", path.display());
    // `open_existing`/`open_existing_lazy` both set `create_if_missing(false)`
    // (they assume the daemon already created the file) - a test fixture
    // needs its own connect path to create one from scratch.
    let opts = SqliteConnectOptions::from_str(&uri)
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .connect_with(opts)
        .await
        .expect("create db");
    sqlx::migrate!("../../src/migrations")
        .run(&pool)
        .await
        .expect("migrate");
    pool.close().await;
    // Reopen through the same lazy path `DbPool` itself uses, so the
    // handle under test behaves exactly like production.
    let pool = meridian_core::open_existing_lazy(&uri, None)
        .await
        .expect("reopen lazily");
    (uri, pool)
}

/// `get()` must return exactly what was passed to `new()`.
#[tokio::test]
async fn get_returns_the_pool_it_was_built_with() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, pool) = migrated_db(dir.path()).await;
    let handle = DbPool::new(Some(pool), uri, None);
    assert!(handle.get().is_some());
}

/// A handle built with no pool (e.g. the DB isn't open yet) must behave
/// exactly like the old `None` state every caller already handles.
#[tokio::test]
async fn get_is_none_when_built_empty() {
    let handle = DbPool::new(None, "sqlite://does-not-matter".to_string(), None);
    assert!(handle.get().is_none());
}

/// The exact sequence `reload_daemon` runs: close, then a read in
/// between must see `None` (nothing races the daemon's restart), then
/// reopen brings a working pool back without needing to be told the URI
/// again.
#[tokio::test]
async fn close_then_reopen_restores_a_working_pool() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, pool) = migrated_db(dir.path()).await;
    let handle = DbPool::new(Some(pool), uri, None);

    handle.close().await;
    assert!(
        handle.get().is_none(),
        "a reader between close() and reopen() must see no pool, not a stale one"
    );

    handle.reopen().await;
    let reopened = handle.get().expect("reopen must restore a pool");
    // Prove it's a genuinely live connection, not just a non-None marker.
    meridian_core::ping(&reopened)
        .await
        .expect("reopened pool must actually work");
}

/// **The regression test for the 1.91.0-staging.2 write wedge.**
///
/// A pool CLONE taken before `close()` is dead afterwards, while the handle
/// keeps working across the same close/reopen. That is the entire difference
/// between the old capture consumers (which cached
/// `Option<SqlitePool>` for the process lifetime and wrote through it every
/// ~2.5 s) and the current ones (which call `get()` per write).
///
/// Asserting on the clone is what makes this a real guard rather than a
/// tautology: it proves the snapshot pattern is *observably* broken by a
/// daemon reload, so re-introducing it anywhere cannot look harmless.
#[tokio::test]
async fn a_pool_snapshot_dies_across_a_reload_but_the_handle_survives() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, pool) = migrated_db(dir.path()).await;
    let handle = DbPool::new(Some(pool), uri, None);

    // What a long-lived consumer used to cache at startup.
    let snapshot = handle.get().expect("a pool to snapshot");
    meridian_core::ping(&snapshot)
        .await
        .expect("the snapshot works before the reload");

    // Exactly what `reload_daemon` does around every daemon restart.
    handle.close().await;
    handle.reopen().await;

    assert!(
        meridian_core::ping(&snapshot).await.is_err(),
        "a cached SqlitePool must be observably dead after close/reopen - if this \
         ever passes, the snapshot pattern looks safe and the capture wedge returns"
    );

    let fresh = handle.get().expect("the handle must still yield a pool");
    meridian_core::ping(&fresh)
        .await
        .expect("resolving the handle per use must survive the reload");
}

fn corrupt_err() -> anyhow::Error {
    anyhow::anyhow!("error returned from database: (code: 11) database disk image is malformed")
        .context("writing a PM sync request")
}

/// The self-heal: a corrupt-VIEW write failure must give the caller a working
/// pool back, with no user action.
///
/// This is what removes the relaunch. On 1.91.0-staging.2 a wedged pool stayed
/// wedged for the life of the tray process, and quitting the app was the only
/// cure - which no user could be expected to discover.
#[tokio::test]
async fn a_corrupt_write_recycles_the_pool_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, pool) = migrated_db(dir.path()).await;
    let handle = DbPool::new(Some(pool), uri, None);
    let before = handle.get().expect("a pool");

    assert!(
        handle.recover_if_corrupt(&corrupt_err()).await,
        "a corrupt write must report that recovery succeeded"
    );

    // The connections that held the broken view are gone...
    assert!(
        meridian_core::ping(&before).await.is_err(),
        "the recycled pool's old connections must be dropped, not reused"
    );
    // ...and the caller can retry against a working one.
    let after = handle.get().expect("a fresh pool after recycle");
    meridian_core::ping(&after)
        .await
        .expect("the recycled pool must actually work");
}

/// Only corruption may recycle. A locked database, a missing table or a pending
/// migration must leave the pool alone - dropping every connection on an ordinary
/// transient error would turn a blip into an outage.
#[tokio::test]
async fn an_unrelated_error_does_not_recycle_the_pool() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, pool) = migrated_db(dir.path()).await;
    let handle = DbPool::new(Some(pool), uri, None);
    let before = handle.get().expect("a pool");

    let err = anyhow::anyhow!("database is locked").context("writing a PM sync request");
    assert!(!handle.recover_if_corrupt(&err).await);

    meridian_core::ping(&before)
        .await
        .expect("an unrelated error must leave the existing pool usable");
}

/// The cooldown. Capture writes every ~2.5 s, so a fault the recycle CANNOT fix
/// (real file damage, a revoked key) would otherwise become a close/reopen storm
/// on the file the daemon is also using.
#[tokio::test]
async fn a_second_corrupt_write_inside_the_cooldown_does_not_recycle_again() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, pool) = migrated_db(dir.path()).await;
    let handle = DbPool::new(Some(pool), uri, None);

    assert!(handle.recover_if_corrupt(&corrupt_err()).await);
    let after_first = handle.get().expect("a pool");

    assert!(
        !handle.recover_if_corrupt(&corrupt_err()).await,
        "a second failure moments later must be refused, not recycled again"
    );
    meridian_core::ping(&after_first)
        .await
        .expect("the refused attempt must not have torn down the working pool");
}

/// In-memory, schema-migrated. Enough for [`raise_if_corrupt`], which only
/// inspects the error it is handed and needs a `system_notices` table to
/// write into - real corrupted bytes on disk are not required, and
/// `sqlite::memory:` cannot be corrupted anyway (see
/// `src/db/test_corrupt.rs` for fixtures that can).
async fn fresh_db() -> SqlitePool {
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("../../src/migrations")
        .run(&pool)
        .await
        .unwrap();
    pool
}

/// Whichever side of the app touches this pool must raise `db.corrupt` the
/// moment IT hits corruption, not wait for a daemon-side query to stumble
/// onto the same damage minutes later. `db::integrity::is_corrupt_error`
/// (the classifier this delegates to) is already pinned against the real
/// field-incident shape elsewhere.
#[tokio::test]
async fn raise_if_corrupt_writes_the_notice_on_a_corrupt_error() {
    let pool = fresh_db().await;
    let err = anyhow::anyhow!(
        "error returned from database: (code: 11) database disk image is malformed"
    )
    .context("current_task: fetch most recent task session");

    raise_if_corrupt(&pool, &err).await;

    let row: (String, String) =
        sqlx::query_as("SELECT severity, detail FROM system_notices WHERE notice_id = ?")
            .bind(meridian::notices::DB_CORRUPT)
            .fetch_one(&pool)
            .await
            .expect("db.corrupt notice must be written");
    assert_eq!(row.0, "error");
    assert!(
        row.1.contains("database disk image is malformed"),
        "notice detail dropped the actual cause: {}",
        row.1
    );
}

/// Every other failure (a lock, a missing table, a network blip on an
/// unrelated call) must NOT raise the corruption banner — that would train
/// the user to run `db repair` for faults it can't fix.
#[tokio::test]
async fn raise_if_corrupt_is_silent_on_unrelated_errors() {
    let pool = fresh_db().await;
    let err = anyhow::anyhow!("database is locked").context("today: fetch sessions");

    raise_if_corrupt(&pool, &err).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?")
        .bind(meridian::notices::DB_CORRUPT)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "an unrelated error must not raise db.corrupt");
}

/// `reopen` must not panic on failure, and must leave `get()` at `None`
/// rather than propagating the error - `reopen` is deliberately
/// best-effort (see its doc), the same shape as any other reason a lazy
/// pool isn't open yet. A missing/wrong-shaped file is NOT enough to
/// prove this (`open_existing_lazy` defers that check to first use, so
/// it would return `Ok` here regardless) - an invalid key is what
/// actually fails synchronously, at `validate_key_hex` inside
/// `open_existing_lazy` itself, before any connection is attempted.
#[tokio::test]
async fn reopen_failure_leaves_the_handle_empty_not_panicked() {
    let handle = DbPool::new(
        None,
        "sqlite://does-not-matter".to_string(),
        Some("not-valid-hex".to_string()),
    );
    handle.reopen().await;
    assert!(handle.get().is_none());
}
