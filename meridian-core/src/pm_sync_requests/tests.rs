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

/// The table as the REAL migrations build it, not a hand-written copy.
///
/// It used to be a hand-written `CREATE TABLE` mirroring migration 082. That is a
/// schema the tests can silently diverge from: adding `seq`/`completed_seq` in
/// migration 083 left every one of these tests passing against a table that did not
/// have the columns the queries now use, so the suite proved nothing about the code
/// that shipped. Running the migrator instead means these tests also assert that
/// 082 + 083 actually apply in order, which is the property real installs depend on.
async fn db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("../src/migrations")
        .run(&pool)
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
    complete(&pool, ALL_PROVIDERS, 1, Some(4), None)
        .await
        .unwrap();

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
/// gets a NEW sequence number from the arriving request, so it stays pending past
/// the running sync's completion and is serviced again - and the force intent must
/// survive into that re-run rather than being downgraded to the arriving gated mode.
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
    complete(&pool, ALL_PROVIDERS, 1, Some(2), None)
        .await
        .unwrap();

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
    assert!(outcome(&pool, ALL_PROVIDERS, 1).await.unwrap().is_none());

    claim(&pool, ALL_PROVIDERS).await.unwrap();
    assert!(
        outcome(&pool, ALL_PROVIDERS, 1).await.unwrap().is_none(),
        "in-flight must still read as pending"
    );

    complete(&pool, ALL_PROVIDERS, 1, Some(7), None)
        .await
        .unwrap();
    let out = outcome(&pool, ALL_PROVIDERS, 1)
        .await
        .unwrap()
        .expect("done");
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
    complete(&pool, ALL_PROVIDERS, 1, None, Some("401 unauthorized"))
        .await
        .unwrap();

    let out = outcome(&pool, ALL_PROVIDERS, 1)
        .await
        .unwrap()
        .expect("done");
    assert_eq!(out.error.as_deref(), Some("401 unauthorized"));
}

/// **THE BUG THIS FILE EXISTS FOR, and both halves of it at once.**
///
/// A request arriving mid-sync must not be marked done by the sync that was already
/// running (or "Sync now" reports success for work that never ran), AND the sync that
/// was already running must still be able to report its result to whoever asked for
/// it (or every waiter times out and reports failure for a sync that succeeded).
///
/// The 082 design could only get one of those. It guarded `complete` on `claimed_at
/// IS NOT NULL`, which the new request had just nulled, so the completion was
/// discarded entirely - protecting the new request by throwing away the old
/// request's answer. On 1.91.0-staging.2 that was the normal case rather than an
/// edge one, because connecting a tracker fires `oauth_connected`,
/// `token_connected` and the user's "Sync now" within a few seconds: the sync
/// worked, the answer was dropped, the work was repeated, and the user saw a
/// failure.
///
/// The sequence watermark gets both. Asserting only the first half is what let the
/// bug ship, so this test asserts them together.
#[tokio::test]
async fn a_mid_sync_request_neither_steals_nor_destroys_the_running_sync_s_outcome() {
    let pool = db().await;
    let first = request(&pool, ALL_PROVIDERS, SyncMode::Gated, "plan_or_picker")
        .await
        .unwrap();
    let claimed = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    assert_eq!(
        claimed.seq, first,
        "the claim must cover the request it read"
    );

    // A second producer asks while the first sync is still running - the connect
    // flow does exactly this.
    let second = request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    assert!(second > first, "a new request must advance the sequence");

    // The in-flight sync finishes and reports against the seq it claimed.
    complete(&pool, ALL_PROVIDERS, claimed.seq, Some(3), None)
        .await
        .unwrap();

    // Half one - the FIX. The first waiter gets its answer instead of timing out.
    let out = outcome(&pool, ALL_PROVIDERS, first)
        .await
        .unwrap()
        .expect("the waiter that asked for this sync must receive its outcome");
    assert_eq!(out.synced_count, Some(3));

    // Half two - the ORIGINAL PROTECTION, preserved. The later request was not
    // serviced by work that started before it existed.
    assert!(
        outcome(&pool, ALL_PROVIDERS, second)
            .await
            .unwrap()
            .is_none(),
        "a request made mid-sync must NOT be satisfied by the sync already running - \
         that would report success for a sync that never ran"
    );

    // ...and it is still serviceable, with its escalation intact.
    let req = claim(&pool, ALL_PROVIDERS)
        .await
        .unwrap()
        .expect("the mid-sync request must still be pending");
    assert_eq!(req.mode, SyncMode::Force);
    assert_eq!(req.reason, "sync_now");
    assert_eq!(req.seq, second);
}

/// Two waiters, one sync: the whole point of a watermark. Both producers asked
/// before anything was serviced, so one sync must satisfy both rather than each
/// needing its own provider round trip.
#[tokio::test]
async fn one_sync_satisfies_every_waiter_that_asked_before_it_ran() {
    let pool = db().await;
    let a = request(&pool, ALL_PROVIDERS, SyncMode::Force, "oauth_connected")
        .await
        .unwrap();
    let b = request(&pool, ALL_PROVIDERS, SyncMode::Force, "token_connected")
        .await
        .unwrap();

    let claimed = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    complete(&pool, ALL_PROVIDERS, claimed.seq, Some(11), None)
        .await
        .unwrap();

    for (label, seq) in [("first", a), ("second", b)] {
        assert!(
            outcome(&pool, ALL_PROVIDERS, seq).await.unwrap().is_some(),
            "the {label} waiter must be satisfied by the single sync that covered it"
        );
    }
    assert!(
        claim(&pool, ALL_PROVIDERS).await.unwrap().is_none(),
        "coalesced requests must not leave extra work behind - that is a second \
         provider round trip for one user action"
    );
}

/// `has_pending` is the gate that keeps an IDLE daemon from writing at all.
///
/// The watcher ticks every 2 s forever and used to call `claim` unconditionally - an
/// `UPDATE`, so SQLite opened a write transaction and took a lock even when it
/// matched nothing. That was ~43,000 write transactions a day on an idle machine,
/// against a file a second process also writes, and it made every daemon kill far
/// likelier to land mid-write.
///
/// So the states where the answer must be `false` matter more than the one where it
/// is `true`: each is a tick that now touches no lock at all.
#[tokio::test]
async fn has_pending_is_false_in_every_idle_state() {
    let pool = db().await;

    assert!(
        !has_pending(&pool, ALL_PROVIDERS).await.unwrap(),
        "a fresh install with no row must not provoke a claim - this is the state \
         almost every tick runs in"
    );

    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    assert!(
        has_pending(&pool, ALL_PROVIDERS).await.unwrap(),
        "real work must still be seen"
    );

    let claimed = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    assert!(
        !has_pending(&pool, ALL_PROVIDERS).await.unwrap(),
        "an in-flight request is not claimable, so ticks during a long sync must be \
         free too"
    );

    complete(&pool, ALL_PROVIDERS, claimed.seq, Some(2), None)
        .await
        .unwrap();
    assert!(
        !has_pending(&pool, ALL_PROVIDERS).await.unwrap(),
        "a completed row is the steady state after any sync - it must never read as \
         work, or the daemon would re-sync forever"
    );

    request(&pool, ALL_PROVIDERS, SyncMode::Gated, "plan_or_picker")
        .await
        .unwrap();
    assert!(
        has_pending(&pool, ALL_PROVIDERS).await.unwrap(),
        "a new request after a completion must be visible again"
    );
}

/// A duplicate or out-of-order completion must never move the watermark backwards,
/// so a retrying consumer cannot un-answer a waiter that was already satisfied.
#[tokio::test]
async fn the_completion_watermark_only_moves_forward() {
    let pool = db().await;
    request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    let first = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    complete(&pool, ALL_PROVIDERS, first.seq, Some(5), None)
        .await
        .unwrap();

    let second = request(&pool, ALL_PROVIDERS, SyncMode::Force, "sync_now")
        .await
        .unwrap();
    let claimed = claim(&pool, ALL_PROVIDERS).await.unwrap().expect("pending");
    complete(&pool, ALL_PROVIDERS, claimed.seq, Some(9), None)
        .await
        .unwrap();

    // A late duplicate for the OLD seq arrives (a retry, a doubled tick).
    complete(&pool, ALL_PROVIDERS, first.seq, Some(1), Some("stale"))
        .await
        .unwrap();

    let out = outcome(&pool, ALL_PROVIDERS, second)
        .await
        .unwrap()
        .expect("the newer waiter must stay satisfied");
    assert_eq!(
        out.synced_count,
        Some(9),
        "the stale retry overwrote the result"
    );
    assert_eq!(out.error, None, "the stale retry resurrected an old error");
}
