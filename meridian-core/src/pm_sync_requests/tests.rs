//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for the PM sync request outbox.
//!
//! Split out of `mod.rs` purely for the 500-line file cap; they are the module's own
//! unit tests and belong to it. Each one pins a race or a policy that is invisible
//! from the SQL alone - read them alongside the doc comment on the function they
//! exercise.

use super::*;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;

async fn db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::query(
        "CREATE TABLE pm_sync_requests (
             provider TEXT NOT NULL PRIMARY KEY,
             mode TEXT NOT NULL DEFAULT 'gated',
             reason TEXT NOT NULL DEFAULT '',
             requested_at TEXT NOT NULL,
             claimed_at TEXT,
             completed_at TEXT,
             error TEXT,
             synced_count INTEGER
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

/// Repeated requests must COALESCE into one pending row. Ten planner opens
/// mean "a sync is wanted", not ten syncs.
#[tokio::test]
async fn repeated_requests_coalesce_into_one_row() {
    let pool = db().await;
    for _ in 0..10 {
        request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
            .await
            .unwrap();
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pm_sync_requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// A user action must not be downgraded by a passing window focus.
#[tokio::test]
async fn force_survives_a_later_gated_request() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "token_connected")
        .await
        .unwrap();
    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();

    let req = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    assert_eq!(req.mode, SyncMode::Force);
}

/// ...and a user action must be able to escalate a pending gated request.
#[tokio::test]
async fn gated_escalates_to_force() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();

    let req = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    assert_eq!(req.mode, SyncMode::Force);
}

/// A *spent* force must NOT be inherited. The row survives completion so "Sync
/// now" can read its result, so escalation has to be scoped to a still-pending
/// row - otherwise one tracker connect leaves `mode = 'force'` set forever and
/// every later planner open bypasses the staleness gate and hits the provider
/// for real, multiplying the token refreshes this design exists to reduce.
#[tokio::test]
async fn a_completed_force_does_not_escalate_the_next_gated_request() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "token_connected")
        .await
        .unwrap();
    claim(&pool, ALL_PROVIDERS).await.unwrap();
    complete(&pool, ALL_PROVIDERS, Some(4), None).await.unwrap();

    // A later window open wants the cheap, gated behaviour.
    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();

    let req = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    assert_eq!(
        req.mode,
        SyncMode::Gated,
        "a spent force must not escalate later gated requests"
    );
}

/// The in-flight case still escalates: a force that is claimed but not completed
/// will have its outcome discarded by `complete`'s guard and be re-serviced, so
/// the force intent must survive into that re-run.
#[tokio::test]
async fn an_in_flight_force_still_survives_a_gated_request() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    claim(&pool, ALL_PROVIDERS).await.unwrap();

    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();

    let req = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    assert_eq!(req.mode, SyncMode::Force);
}

/// A claim is exclusive: the second attempt sees nothing, so two watcher ticks
/// can never run the same sync twice.
#[tokio::test]
async fn claim_is_exclusive() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();
    assert!(claim(&pool, ALL_PROVIDERS).await.unwrap().is_some());
    assert!(
        claim(&pool, ALL_PROVIDERS).await.unwrap().is_none(),
        "a claimed request must not be claimable again"
    );
}

/// Nothing pending is a quiet `None`, not an error - the watcher ticks on this
/// constantly.
#[tokio::test]
async fn claim_with_no_request_is_none() {
    let pool = db().await;
    assert!(claim(&pool, ALL_PROVIDERS).await.unwrap().is_none());
}

/// A daemon killed between claim and complete must not strand PM sync forever.
/// Without the reset the row stays claimed, `claim` requires `claimed_at IS
/// NULL`, and no future tick could ever service it.
#[tokio::test]
async fn stale_claims_are_released_on_startup() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    claim(&pool, ALL_PROVIDERS).await.unwrap();
    // ... daemon dies here, no `complete` ever runs.

    assert!(
        claim(&pool, ALL_PROVIDERS).await.unwrap().is_none(),
        "precondition: a stranded claim blocks re-claiming"
    );

    assert_eq!(reset_stale_claims(&pool).await.unwrap(), 1);

    let req = claim(&pool, ALL_PROVIDERS)
        .await
        .unwrap()
        .expect("the request must be serviceable again after the reset");
    assert_eq!(req.mode, SyncMode::Force);
}

/// The reset must not disturb a request that already completed - that would
/// re-run finished work on every daemon boot.
#[tokio::test]
async fn reset_leaves_completed_requests_alone() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();
    claim(&pool, ALL_PROVIDERS).await.unwrap();
    complete(&pool, ALL_PROVIDERS, Some(2), None).await.unwrap();

    assert_eq!(reset_stale_claims(&pool).await.unwrap(), 0);
    assert!(
        claim(&pool, ALL_PROVIDERS).await.unwrap().is_none(),
        "a completed request must stay completed"
    );
}

/// The outcome is invisible until the daemon finishes, so a producer polling it
/// can tell "still working" from "done".
#[tokio::test]
async fn outcome_is_none_until_completed() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    assert!(outcome(&pool, ALL_PROVIDERS).await.unwrap().is_none());

    claim(&pool, ALL_PROVIDERS).await.unwrap();
    assert!(
        outcome(&pool, ALL_PROVIDERS).await.unwrap().is_none(),
        "in-flight must still read as pending"
    );

    complete(&pool, ALL_PROVIDERS, Some(7), None).await.unwrap();
    let out = outcome(&pool, ALL_PROVIDERS).await.unwrap().expect("done");
    assert_eq!(out.synced_count, Some(7));
    assert!(out.error.is_none());
}

/// A failure is reported, not swallowed.
#[tokio::test]
async fn outcome_carries_the_error() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    claim(&pool, ALL_PROVIDERS).await.unwrap();
    complete(&pool, ALL_PROVIDERS, None, Some("401 unauthorized"))
        .await
        .unwrap();

    let out = outcome(&pool, ALL_PROVIDERS).await.unwrap().expect("done");
    assert_eq!(out.error.as_deref(), Some("401 unauthorized"));
}

/// THE RACE THIS GUARDS: a request arriving mid-sync resets the row, and the
/// older sync's outcome must NOT stamp it complete - that would mark the new
/// request done without ever servicing it.
#[tokio::test]
async fn completion_does_not_clobber_a_request_that_arrived_mid_sync() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "dashboard_open")
        .await
        .unwrap();
    claim(&pool, ALL_PROVIDERS).await.unwrap();

    // A new request lands while the first sync is still running.
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();

    // The in-flight sync finishes and tries to report. This must be a NO-OP:
    // the new request cleared `claimed_at`, so the guard rejects it.
    complete(&pool, ALL_PROVIDERS, Some(3), None).await.unwrap();

    assert!(
        outcome(&pool, ALL_PROVIDERS).await.unwrap().is_none(),
        "the older sync's outcome must NOT mark the new request complete - \
         that would report success for a sync that never ran"
    );

    let req = claim(&pool, ALL_PROVIDERS)
        .await
        .unwrap()
        .expect("the mid-sync request must still be pending");
    assert_eq!(req.mode, SyncMode::Force);
    assert_eq!(req.reason, "sync_now");
}
