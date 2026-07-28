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

/// The whole point of suppressing transient failures: an ordinary blip on a
/// provider that is otherwise working must raise NOTHING. This is the bug
/// the user reported - a banner telling them to redo working credentials.
#[tokio::test]
async fn a_blip_on_a_healthy_provider_stays_silent() {
    let pool = make_db().await;
    set_last_sync(&pool, "github", "-5 minutes").await;

    let escalated = note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();

    assert!(!escalated);
    assert!(
        notice(&pool, "pm.github").await.is_none(),
        "a network blip must not raise a user-facing fault"
    );
}

/// And the counterweight: silence must be BOUNDED. A provider blocked by a
/// proxy or firewall fails on every tick forever, and going quiet would
/// leave the board silently stale - worse than the banner we removed.
#[tokio::test]
async fn a_persistently_unreachable_provider_stops_being_silent() {
    let pool = make_db().await;
    set_last_sync(&pool, "jira", "-7 hours").await;

    let escalated = note_transient_sync_failure(&pool, "jira", "connection timed out")
        .await
        .unwrap();

    assert!(escalated);
    let (title, detail, remedy) = notice(&pool, "pm.jira").await.expect("notice raised");
    assert_eq!(title, "Jira sync failing");
    assert!(
        detail.contains("connection timed out"),
        "the cause must survive into the notice: {detail}"
    );
    let remedy = remedy.expect("remedy set");
    assert!(
        remedy.contains("connection") || remedy.contains("proxy"),
        "an unreachable provider is a connectivity remedy, not a credentials one: {remedy}"
    );
    assert!(
        !remedy.contains("TOKEN"),
        "must not tell the user to redo credentials that are fine: {remedy}"
    );
}

/// The trap this has to survive: transient failures no longer call
/// `stamp_sync_error`, so a machine that has NEVER reached the provider has
/// no `pm_sync_state` row at all, and an age test alone would no-op forever
/// on exactly the installs that need it most - someone who just connected
/// behind a blocking proxy.
///
/// But it must not escalate on failure #1 either, or connecting a tracker
/// and hitting one DNS hiccup shows a fault seconds later - the same
/// false-positive banner this change exists to remove, just with
/// connectivity wording. One retry, then it surfaces.
#[tokio::test]
async fn a_never_synced_provider_gets_one_retry_before_escalating() {
    let pool = make_db().await;

    let first = note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();
    assert!(!first, "the first blip after connecting must stay silent");
    assert!(
        notice(&pool, "pm.github").await.is_none(),
        "no banner seconds after connecting a tracker"
    );

    let second = note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();
    assert!(second, "a second failure means it is not just a blip");
    assert!(notice(&pool, "pm.github").await.is_some());
}

/// The grace is exactly one retry, not an open-ended reprieve: the row the
/// first failure writes must carry the epoch sentinel, or it would read as
/// a recent sync and suppress escalation forever.
#[tokio::test]
async fn the_first_failure_row_does_not_look_like_a_sync() {
    let pool = make_db().await;
    note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();

    let (last,): (String,) =
        sqlx::query_as("SELECT last_synced_at FROM pm_sync_state WHERE provider = 'github'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        last.starts_with("1970"),
        "the grace row must not count as a sync: {last}"
    );
}

/// A row left by an earlier TERMINAL failure carries the epoch sentinel
/// (`stamp_sync_error` inserts `1970-01-01`), which is a row but not a
/// success. It must not be mistaken for one.
#[tokio::test]
async fn an_epoch_sentinel_row_is_not_a_recent_success() {
    let pool = make_db().await;
    stamp_sync_error(&pool, "github", "bad credentials")
        .await
        .unwrap();

    let escalated = note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();

    assert!(escalated, "an epoch last_synced_at is not a success");
}

/// The window itself, from both sides.
#[tokio::test]
async fn the_escalation_window_holds_on_both_sides() {
    let pool = make_db().await;

    set_last_sync(&pool, "jira", "-1 hours").await;
    assert!(
        !note_transient_sync_failure(&pool, "jira", "blip")
            .await
            .unwrap(),
        "inside the window must stay silent"
    );

    set_last_sync(&pool, "jira", "-7 hours").await;
    assert!(
        note_transient_sync_failure(&pool, "jira", "blip")
            .await
            .unwrap(),
        "outside the window must escalate"
    );
}

/// The escalation must self-heal: once the provider is reachable again the
/// next successful sync clears it, with no user action.
#[tokio::test]
async fn a_successful_sync_clears_an_escalated_notice() {
    let pool = make_db().await;
    // Two failures: the first is the never-synced grace attempt.
    note_transient_sync_failure(&pool, "github", "dns error")
        .await
        .unwrap();
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

/// The coupling that makes escalation reachable at all: recording an error
/// must NEVER write a fresh `last_synced_at`.
///
/// Every provider gates its whole fetch on that column being recent, and
/// [`note_transient_sync_failure`] keys its escalation clock on the SAME
/// column. So stamping `now` here would do two invisible things at once:
/// suppress the next tick's retry, and reset the escalation clock on every
/// failure so the threshold is never reached. The provider would go silent
/// permanently, which is the exact regression this whole change exists to
/// prevent. `azure_devops` had precisely this bug in its own `stamp_error`.
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

    // And on a row that already holds a real (old) success, recording a
    // failure must leave the clock alone rather than pushing it forward.
    set_last_sync(&pool, "github", "-10 hours").await;
    stamp_sync_error(&pool, "github", "boom").await.unwrap();
    assert!(
        note_transient_sync_failure(&pool, "github", "unreachable")
            .await
            .unwrap(),
        "recording an error must not reset the escalation clock"
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
    set_last_sync(&pool, "azure_devops", "-5 minutes").await;

    record_sync_failure(&pool, "azure_devops", "wiql", &status_err(503, "down")).await;

    assert!(
        notice(&pool, "pm.azure_devops").await.is_none(),
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
    set_last_sync(&pool, "azure_devops", "-5 minutes").await;
    let err = status_err(503, "down");

    record_sync_failure(&pool, "azure_devops", "wiql", &err).await;
    record_sync_failure(&pool, "azure_devops", "refresh", &err).await;

    assert!(
        notice(&pool, "pm.azure_devops").await.is_none(),
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
