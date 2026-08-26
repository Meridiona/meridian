//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Unit tests for [`super`] — split out only to keep both files under the
//! repo's 500-line cap, following the same `{mod,tests}.rs` shape as
//! `meridian-core/src/pm_sync_requests/` and
//! `src/intelligence/providers/jira/`.

use super::*;
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

/// Render an error the way the real call sites do, so the tests exercise the same
/// `rendered` string production sees rather than a hand-written one.
fn rendered(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

async fn corrupt_notices(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?")
        .bind(meridian::notices::DB_CORRUPT)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The update window: `pm_sync_requests` does not exist yet because the daemon
/// has not applied migration 082. The user must see a transient-update message,
/// never a raw SQL string that reads like database damage.
#[tokio::test]
async fn a_missing_requests_table_reads_as_a_pending_update() {
    let pool = fresh_db().await;
    let e =
        anyhow::anyhow!("error returned from database: (code: 1) no such table: pm_sync_requests");

    let msg = explain_outbox_failure(&pool, &e, &rendered(&e), "could not queue the sync").await;

    assert_eq!(msg, UPDATE_IN_PROGRESS_MESSAGE);
    assert!(!msg.contains("no such table"), "must not leak SQL: {msg}");
    assert_eq!(
        corrupt_notices(&pool).await,
        0,
        "a pending migration is not corruption"
    );
}

/// The regression this function was rewritten for. A corrupt database must (a) get
/// the `db.corrupt` banner raised, which is the only surface carrying a Repair
/// button, and (b) send the user there rather than printing SQLite's wording into
/// a settings panel that can do nothing about it.
#[tokio::test]
async fn corruption_raises_the_banner_and_points_at_it() {
    let pool = fresh_db().await;
    let e = anyhow::anyhow!(
        "error returned from database: (code: 11) database disk image is malformed"
    )
    .context("reading the PM sync outcome");

    let msg =
        explain_outbox_failure(&pool, &e, &rendered(&e), "could not read the sync outcome").await;

    assert_eq!(msg, DB_DAMAGED_MESSAGE);
    assert_eq!(
        corrupt_notices(&pool).await,
        1,
        "corruption found by an outbox query must raise the same banner the daemon raises"
    );
    assert!(
        !msg.contains("malformed"),
        "the banner carries the cause; the panel should not repeat it: {msg}"
    );
}

/// The banner's detail must carry the real cause even though the panel message
/// does not - otherwise the diagnosis is lost exactly like the bare-`{e}` bug that
/// hid this incident in the first place.
#[tokio::test]
async fn the_banner_keeps_the_full_cause_chain() {
    let pool = fresh_db().await;
    let e = anyhow::anyhow!(
        "error returned from database: (code: 11) database disk image is malformed"
    )
    .context("reading the PM sync outcome");

    explain_outbox_failure(&pool, &e, &rendered(&e), "could not read the sync outcome").await;

    let detail: String =
        sqlx::query_scalar("SELECT detail FROM system_notices WHERE notice_id = ?")
            .bind(meridian::notices::DB_CORRUPT)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        detail.contains("database disk image is malformed"),
        "banner dropped the cause: {detail}"
    );
    assert!(
        detail.contains("reading the PM sync outcome"),
        "banner dropped the context: {detail}"
    );
}

/// Any OTHER failure keeps its detail. Collapsing every error into a friendly
/// message would hide a real fault behind "try again in a moment", which never
/// resolves. It must also NOT raise the corruption banner - that would train the
/// user to run `db repair` for faults it cannot fix.
#[tokio::test]
async fn other_failures_keep_their_detail() {
    let pool = fresh_db().await;
    let e = anyhow::anyhow!("database is locked");

    let msg = explain_outbox_failure(&pool, &e, &rendered(&e), "could not queue the sync").await;

    assert!(
        msg.contains("database is locked"),
        "detail was dropped: {msg}"
    );
    assert!(msg.starts_with("could not queue the sync"), "{msg}");
    assert_eq!(corrupt_notices(&pool).await, 0, "a lock is not corruption");
}

/// A missing table that is NOT ours is somebody else's problem and must not be
/// reported as a pending update - that would send the user to wait out an update
/// that is already finished while the real fault goes unnamed.
#[tokio::test]
async fn a_different_missing_table_is_not_reported_as_an_update() {
    let pool = fresh_db().await;
    let e = anyhow::anyhow!("no such table: pm_tasks");

    let msg = explain_outbox_failure(&pool, &e, &rendered(&e), "could not queue the sync").await;

    assert!(msg.contains("pm_tasks"), "detail was dropped: {msg}");
    assert!(!msg.contains("finishing an update"), "misattributed: {msg}");
}

/// The two call sites differ only in their fallback phrasing, and that phrasing is
/// what the user reads when nothing more specific applies. Pinned so a refactor
/// cannot silently make the outcome-read failure claim the write failed.
#[tokio::test]
async fn the_fallback_names_the_operation_that_actually_failed() {
    let pool = fresh_db().await;
    let e = anyhow::anyhow!("disk I/O error");

    let read =
        explain_outbox_failure(&pool, &e, &rendered(&e), "could not read the sync outcome").await;
    let write = explain_outbox_failure(&pool, &e, &rendered(&e), "could not queue the sync").await;

    assert!(
        read.starts_with("could not read the sync outcome"),
        "{read}"
    );
    assert!(write.starts_with("could not queue the sync"), "{write}");
}

/// The whole point of the 1.91.0-staging.3 fix, pinned where it can regress.
///
/// `ask_daemon_to_sync` has two SQLite call sites - the request write and the
/// outcome read - and both used to treat any `Err` as terminal. A daemon reload
/// truncates the WAL on its way out, so a read landing in that window returned
/// `(code: 522) disk I/O error` and the user was shown a red failure for a sync
/// the daemon went on to complete seconds later, with most of the 30 s budget
/// unspent.
///
/// A source scan rather than a behavioural test because provoking a short read
/// needs two real processes racing a `wal_checkpoint(TRUNCATE)` on a real file -
/// the one thing an in-memory pool cannot reproduce. Same idiom, and the same
/// reason, as `ui/__tests__/no-native-dialogs.test.ts`.
#[test]
fn transient_faults_are_retried_before_any_terminal_failure() {
    let src = include_str!("mod.rs");

    let guards: Vec<_> = src
        .match_indices("is_transient_error")
        .map(|(i, _)| i)
        .collect();
    let terminals: Vec<_> = src
        .match_indices("explain_outbox_failure(")
        .map(|(i, _)| i)
        // Skip the definition itself; only the call sites matter.
        .filter(|i| !src[..*i].ends_with("async fn "))
        .collect();

    assert_eq!(
        guards.len(),
        2,
        "both SQLite call sites in ask_daemon_to_sync must classify transient faults"
    );
    for terminal in &terminals {
        assert!(
            guards.iter().any(|g| g < terminal),
            "a terminal failure at byte {terminal} is reachable with no transient check before it"
        );
    }
}

/// The retry budget has to outlast the thing it exists to ride out. A daemon
/// reload closes the tray's pool, signals, and lets the daemon checkpoint and
/// exit - on the order of a second. A budget shorter than that would turn this
/// fix into a coin flip.
#[test]
fn the_request_retry_budget_covers_a_daemon_reload() {
    let covered = REQUEST_RETRY_GAP * (REQUEST_ATTEMPTS - 1);
    assert!(
        covered >= Duration::from_millis(1200),
        "retry budget {covered:?} is too short to outlast a daemon reload"
    );
    assert!(
        covered < SYNC_TIMEOUT,
        "the write retries must fit well inside the overall wait budget"
    );
}
