//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for the sync-failure recording policy in [`super`].
//!
//! Split out of `providers/mod.rs` on size (the module passed the 500-line
//! rule with these inline). They cover the three functions that decide whether
//! a provider failure ever reaches the user: [`super::stamp_sync_error`],
//! [`super::note_transient_sync_failure`], and [`super::record_sync_failure`].
//!
//! Nearly every case here guards a SILENT failure mode - one that looks correct
//! in review and simply stops warning, or starts warning wrongly, forever.

use super::*;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;

async fn make_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    pool
}

/// Stamp a last-successful-sync time relative to now, e.g. `"-7 hours"`.
async fn set_last_sync(pool: &SqlitePool, provider: &str, modifier: &str) {
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at)
         VALUES (?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?))
         ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
    )
    .bind(provider)
    .bind(modifier)
    .execute(pool)
    .await
    .unwrap();
}

async fn notice(pool: &SqlitePool, id: &str) -> Option<(String, String, Option<String>)> {
    sqlx::query_as("SELECT title, detail, remedy FROM system_notices WHERE notice_id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// Each test gets its own provider name where the streak matters: the streak
/// lives in a process-global map, and cargo runs these in parallel threads, so
/// sharing a name would let one test's failures leak into another's count.
///
/// Reset explicitly too, because the map outlives any single test.
fn fresh_streak(provider: &str) {
    reset_transient_streak(provider);
}

/// A single network blip must stay SILENT. This is the whole reason
/// `SyncFault::Retry` exists - one DNS hiccup must not produce a "check your
/// credentials" banner for credentials that are fine.
#[tokio::test]
async fn a_single_blip_stays_silent() {
    let pool = make_db().await;
    fresh_streak("blip_single");
    set_last_sync(&pool, "blip_single", "-2 minutes").await;

    let escalated = note_transient_sync_failure(&pool, "blip_single", "dns error")
        .await
        .unwrap();
    assert!(!escalated, "one transient failure must not escalate");
    assert!(
        notice(&pool, "pm.blip_single").await.is_none(),
        "one blip must raise no notice"
    );
}

/// THE REGRESSION THIS REWRITE EXISTS FOR.
///
/// A machine that has not synced for days is NORMAL once sync is on-demand: it
/// was shut, or nobody opened the dashboard. The old rule ("no successful sync in
/// the last 6 hours") treated that as evidence of a fault, so the first couple of
/// failures after any quiet period escalated - meaning a Monday-morning Wi-Fi
/// blip raised a red banner on a healthy install.
///
/// A long quiet period must now carry NO weight at all.
#[tokio::test]
async fn a_blip_after_days_of_quiet_stays_silent() {
    let pool = make_db().await;
    fresh_streak("blip_quiet");
    // Three days since the last success - a shut laptop, not a fault.
    set_last_sync(&pool, "blip_quiet", "-72 hours").await;

    for attempt in 1..=2 {
        let escalated = note_transient_sync_failure(&pool, "blip_quiet", "dns error")
            .await
            .unwrap();
        assert!(
            !escalated,
            "attempt {attempt} after a long quiet period must not escalate - \
             elapsed quiet is not evidence of a fault under on-demand sync"
        );
    }
    assert!(
        notice(&pool, "pm.blip_quiet").await.is_none(),
        "a wake-time blip must raise no notice however long the machine slept"
    );
}

/// Silence still has to be BOUNDED. A provider blocked persistently (corporate
/// proxy, TLS interception, a firewall rule) fails every single attempt, and
/// suppressing that outright would leave the board silently going stale with no
/// signal at all - strictly worse than a misleading banner.
#[tokio::test]
async fn a_persistently_unreachable_provider_stops_being_silent() {
    let pool = make_db().await;
    fresh_streak("blocked");
    set_last_sync(&pool, "blocked", "-2 minutes").await;

    for attempt in 1..TRANSIENT_ESCALATION_ATTEMPTS {
        assert!(
            !note_transient_sync_failure(&pool, "blocked", "connection timed out")
                .await
                .unwrap(),
            "attempt {attempt} is below the threshold and must stay silent"
        );
    }
    let escalated = note_transient_sync_failure(&pool, "blocked", "connection timed out")
        .await
        .unwrap();
    assert!(
        escalated,
        "the {TRANSIENT_ESCALATION_ATTEMPTS}th consecutive failure must escalate"
    );

    let (_, detail, remedy) = notice(&pool, "pm.blocked")
        .await
        .expect("a notice must exist once the streak is real");
    assert!(
        detail.contains("consecutive"),
        "the message must name the real evidence (consecutive failures), not a \
         duration that now describes normal operation - got {detail:?}"
    );
    assert!(
        !detail.contains("6 hours"),
        "the old duration wording must not come back - got {detail:?}"
    );
    assert!(
        remedy.is_some_and(|r| r.to_lowercase().contains("connection")),
        "a connectivity fault must point at connectivity"
    );
}

/// A SUCCESS is the only thing that resets the streak. Without this, failures
/// accumulate across hours of healthy operation and the Nth blip of the day
/// escalates for no reason.
#[tokio::test]
async fn a_success_resets_the_streak() {
    let pool = make_db().await;
    fresh_streak("resets");
    set_last_sync(&pool, "resets", "-2 minutes").await;

    for _ in 1..TRANSIENT_ESCALATION_ATTEMPTS {
        assert!(!note_transient_sync_failure(&pool, "resets", "blip")
            .await
            .unwrap());
    }
    // One good sync in between wipes the slate.
    clear_sync_error(&pool, "resets").await.unwrap();

    assert!(
        !note_transient_sync_failure(&pool, "resets", "blip")
            .await
            .unwrap(),
        "after a success the count restarts, so this is failure #1 and stays silent"
    );
    assert!(
        notice(&pool, "pm.resets").await.is_none(),
        "no notice may survive a successful sync"
    );
}

/// The streak is per provider. A flaky GitHub must not push Jira toward a notice.
#[tokio::test]
async fn streaks_do_not_leak_between_providers() {
    let pool = make_db().await;
    fresh_streak("leak_a");
    fresh_streak("leak_b");

    for _ in 1..TRANSIENT_ESCALATION_ATTEMPTS {
        assert!(!note_transient_sync_failure(&pool, "leak_a", "blip")
            .await
            .unwrap());
    }
    assert!(
        !note_transient_sync_failure(&pool, "leak_b", "blip")
            .await
            .unwrap(),
        "provider b is on its FIRST failure and must be unaffected by a's streak"
    );
}

/// A never-synced provider (no `pm_sync_state` row at all) follows the same rule.
///
/// It used to need its own branch, because an age test on a missing row no-opped
/// forever - exactly on the installs that most needed it, someone who just
/// connected and whose network blocks the provider. Counting attempts removes
/// that special case entirely, and this pins that it really is gone.
#[tokio::test]
async fn a_never_synced_provider_escalates_on_the_same_streak() {
    let pool = make_db().await;
    fresh_streak("virgin");
    // Deliberately NO set_last_sync: there is no row.

    for _ in 1..TRANSIENT_ESCALATION_ATTEMPTS {
        assert!(!note_transient_sync_failure(&pool, "virgin", "dns error")
            .await
            .unwrap());
    }
    assert!(
        note_transient_sync_failure(&pool, "virgin", "dns error")
            .await
            .unwrap(),
        "a provider that never synced must still escalate on a real streak"
    );
}

/// The test seam exists and drives escalation, so a future test does not have to
/// know the threshold's value to exercise the loud path.
#[tokio::test]
async fn the_streak_can_be_forced_for_tests() {
    let pool = make_db().await;
    set_transient_streak_for_test("forced", TRANSIENT_ESCALATION_ATTEMPTS - 1);
    assert!(
        note_transient_sync_failure(&pool, "forced", "blip")
            .await
            .unwrap(),
        "one more failure on top of a forced near-threshold streak must escalate"
    );
    reset_transient_streak("forced");
}

/// The escalation must self-heal: once the provider is reachable again the
/// next successful sync clears it, with no user action.
#[tokio::test]
async fn a_successful_sync_clears_an_escalated_notice() {
    let pool = make_db().await;
    // Drive the streak to one below the threshold, so the next failure is the
    // one that escalates. Forced rather than looped so the test does not have to
    // restate the threshold's value.
    set_transient_streak_for_test("github", TRANSIENT_ESCALATION_ATTEMPTS - 1);
    note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();
    assert!(notice(&pool, "pm.github").await.is_some());

    clear_sync_error(&pool, "github").await.unwrap();

    assert!(
        notice(&pool, "pm.github").await.is_none(),
        "escalation must clear itself once the provider is reachable"
    );
}

/// Recording an error must NEVER write a fresh `last_synced_at`.
///
/// Every provider gates its whole fetch on that column being recent, so stamping
/// `now` while recording a FAILURE would suppress the next attempt's retry - the
/// provider would go quiet permanently, having convinced itself it had just
/// succeeded. `azure_devops` had precisely this bug in its own `stamp_error`.
///
/// This used to also guard the escalation clock, which keyed on the same column.
/// That clock is gone (escalation counts consecutive attempts now), but the
/// retry-suppression half is untouched and is the reason this still matters.
#[tokio::test]
async fn recording_an_error_never_looks_like_a_successful_sync() {
    let pool = make_db().await;

    // First-ever failure: the inserted row must carry the epoch sentinel,
    // not "now".
    stamp_sync_error(&pool, "jira", "boom").await.unwrap();
    let (last,): (String,) =
        sqlx::query_as("SELECT last_synced_at FROM pm_sync_state WHERE provider = 'jira'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        last.starts_with("1970"),
        "an error must not be recorded as a sync: {last}"
    );

    // And on a row that already holds a real (old) success, recording a failure
    // must leave that timestamp alone rather than pushing it forward.
    set_last_sync(&pool, "github", "-10 hours").await;
    let (before,): (String,) =
        sqlx::query_as("SELECT last_synced_at FROM pm_sync_state WHERE provider = 'github'")
            .fetch_one(&pool)
            .await
            .unwrap();
    stamp_sync_error(&pool, "github", "boom").await.unwrap();
    let (after,): (String,) =
        sqlx::query_as("SELECT last_synced_at FROM pm_sync_state WHERE provider = 'github'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "recording an error must not move last_synced_at - doing so would make the \
         provider skip its next fetch as though it had just succeeded"
    );
}

/// Every provider that has a `stamp_error`-shaped path must share the one
/// above. Enumerated rather than spot-checked because the failure is silent:
/// a provider writing its own row with `last_synced_at = now` looks fine in
/// review and simply stops retrying and escalating forever.
#[tokio::test]
async fn no_provider_records_an_error_as_a_fresh_sync() {
    for provider in ["jira", "github", "linear", "trello", "azure_devops"] {
        let pool = make_db().await;
        stamp_sync_error(&pool, provider, "boom").await.unwrap();
        let (last,): (String,) =
            sqlx::query_as("SELECT last_synced_at FROM pm_sync_state WHERE provider = ?")
                .bind(provider)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            last.starts_with("1970"),
            "{provider} recorded a failure as a sync at {last}"
        );
    }
}

// ── the generic catch-all ────────────────────────────────────────────────
//
// `intelligence::run_pm_sync`'s error arm reports ANY propagated failure.
// It used to do so unconditionally and with `{e}`, which made it a silent
// undo button: a provider could classify its own failure correctly and then
// have this second, generic, truncated write land on top and win. It now
// routes through `record_sync_failure`, so these pin its behaviour.

fn status_err(status: u16, body: &str) -> anyhow::Error {
    anyhow::Error::new(http::HttpStatusError::new(
        status,
        "Azure DevOps WIQL",
        body,
    ))
    .context("Azure DevOps WIQL request")
}

#[tokio::test]
async fn a_transient_failure_at_the_catch_all_raises_no_notice() {
    let pool = make_db().await;
    // A UNIQUE provider key, not a real provider name. The consecutive-failure
    // streak lives in a process-global map, and several tests in this file drive
    // transient failures through `azure_devops`; sharing the key let one test's
    // failures count toward another's streak under cargo's parallel threads.
    // Nothing here depends on the name being a real provider - the assertion is
    // that no notice is raised at all.
    reset_transient_streak("catchall_transient");
    set_last_sync(&pool, "catchall_transient", "-5 minutes").await;

    record_sync_failure(
        &pool,
        "catchall_transient",
        "wiql",
        &status_err(503, "down"),
    )
    .await;

    assert!(
        notice(&pool, "pm.catchall_transient").await.is_none(),
        "a 503 reaching the catch-all must not raise a credentials banner"
    );
}

#[tokio::test]
async fn a_terminal_failure_at_the_catch_all_carries_the_whole_chain() {
    let pool = make_db().await;

    record_sync_failure(
        &pool,
        "azure_devops",
        "wiql",
        &status_err(401, "permission_error: PAT is invalid"),
    )
    .await;

    let (title, detail, _) = notice(&pool, "pm.azure_devops")
        .await
        .expect("a 401 must still reach the user");
    assert_eq!(title, "Azure DevOps sync failing");
    assert!(
        detail.contains("Azure DevOps WIQL request"),
        "outer context lost: {detail}"
    );
    assert!(
        detail.contains("401"),
        "cause chain truncated - the `{{e}}` bug is back: {detail}"
    );
    assert!(
        detail.contains("PAT is invalid"),
        "cause body lost: {detail}"
    );
}

/// The specific shape the review caught: a provider records its own failure,
/// then the error propagates and the catch-all records it again. Both writes
/// now go through the same classifier, so the second cannot downgrade the
/// first into an unclassified, truncated notice.
#[tokio::test]
async fn recording_the_same_failure_twice_cannot_degrade_it() {
    let pool = make_db().await;
    let err = status_err(401, "permission_error: PAT is invalid");

    record_sync_failure(&pool, "azure_devops", "wiql", &err).await;
    let first = notice(&pool, "pm.azure_devops").await.expect("first write");
    record_sync_failure(&pool, "azure_devops", "refresh", &err).await;
    let second = notice(&pool, "pm.azure_devops")
        .await
        .expect("second write");

    assert_eq!(first.1, second.1, "second write changed the detail");
    assert!(second.1.contains("401"), "second write truncated the cause");
}

/// And the transient half of the same shape: a re-report must not turn a
/// suppressed blip into a banner.
#[tokio::test]
async fn re_reporting_a_transient_failure_still_raises_nothing() {
    let pool = make_db().await;
    // A UNIQUE provider key, not a real provider name. The consecutive-failure
    // streak lives in a process-global map, and several tests in this file drive
    // transient failures through `azure_devops`; sharing the key let one test's
    // failures count toward another's streak under cargo's parallel threads.
    // Nothing here depends on the name being a real provider - the assertion is
    // that no notice is raised at all.
    reset_transient_streak("rereport_transient");
    set_last_sync(&pool, "rereport_transient", "-5 minutes").await;
    let err = status_err(503, "down");

    record_sync_failure(&pool, "rereport_transient", "wiql", &err).await;
    record_sync_failure(&pool, "rereport_transient", "refresh", &err).await;

    assert!(
        notice(&pool, "pm.rereport_transient").await.is_none(),
        "a re-reported blip must stay silent"
    );
}

/// A failure to PERSIST the record must never take the provider error down
/// with it. The `()` return makes that structural - it cannot propagate -
/// and this checks it also survives the write actually failing rather than
/// panicking. A dead pool is the cheapest way to force that.
///
/// The case this protects: with the old `Result` signature and
/// `record_sync_failure(...).await?`, a transient DB error turned a
/// classified provider failure into an sqlx error, which in `azure_devops`
/// then propagated back into `run_pm_sync`'s catch-all - so the user was
/// shown a DATABASE error instead of "Azure DevOps sync failing".
#[tokio::test]
async fn a_persistence_failure_never_replaces_the_provider_error() {
    let pool = make_db().await;
    pool.close().await;

    // Both branches, against a pool that cannot serve either write.
    record_sync_failure(&pool, "azure_devops", "wiql", &status_err(503, "down")).await;
    record_sync_failure(&pool, "azure_devops", "wiql", &status_err(401, "bad")).await;
}

/// The GitHub remedy must not name an unfollowable action: GitHub connects
/// via the in-app browser device flow, so a user who clicked Connect has
/// never seen a `GITHUB_TOKEN` to set.
#[tokio::test]
async fn the_github_remedy_points_somewhere_the_user_can_actually_go() {
    let pool = make_db().await;
    stamp_sync_error(&pool, "github", "bad credentials")
        .await
        .unwrap();

    let (_, _, remedy) = notice(&pool, "pm.github").await.expect("notice raised");
    let remedy = remedy.expect("remedy set");
    assert!(
        remedy.contains("Settings"),
        "remedy must name a place in the app: {remedy}"
    );
    assert!(
        !remedy.contains(".env"),
        "a browser-connected user cannot edit .env: {remedy}"
    );
}

/// Generalises the test above to every tracker. All five are connected
/// through Settings -> Integrations, so a default remedy naming a file or a
/// terminal command is unfollowable for the people who actually see it.
///
/// Scoped to the DEFAULT registry: `stamp_sync_error_with_remedy` overrides
/// still exist for genuinely env-configured installs (Jira basic auth), and
/// those are correct precisely because that user did edit `.env`.
#[tokio::test]
async fn every_default_remedy_names_a_place_in_the_app() {
    for provider in ["jira", "github", "linear", "trello", "azure_devops"] {
        let pool = make_db().await;
        stamp_sync_error(&pool, provider, "boom").await.unwrap();

        let (_, _, remedy) = notice(&pool, &format!("pm.{provider}"))
            .await
            .unwrap_or_else(|| panic!("{provider} raised no notice"));
        let remedy = remedy.unwrap_or_else(|| panic!("{provider} has no remedy"));
        assert!(
            remedy.contains("Settings"),
            "{provider} remedy does not name a place in the app: {remedy}"
        );
        assert!(
            !remedy.contains(".env") && !remedy.contains("meridian "),
            "{provider} remedy asks for a file edit or a CLI command: {remedy}"
        );
    }
}
