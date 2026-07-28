//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

pub mod azure_devops;
pub mod cdm;
pub mod github;
pub mod http;
pub mod jira;
pub mod linear;
pub mod status;
pub mod trello;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Write a sync error for a provider. Writes to both `pm_sync_state.last_error`
/// (for the connect-status indicators) and `system_notices` (for the global
/// UI fault bus that surfaces banners on every page).
pub async fn stamp_sync_error(pool: &SqlitePool, provider: &str, error: &str) -> Result<()> {
    stamp_sync_error_with_remedy(pool, provider, error, None).await
}

/// Like [`stamp_sync_error`], but lets the caller override the default
/// per-provider remedy text. Needed when a provider supports more than one
/// auth method (e.g. Jira's static API token vs. OAuth) and the generic
/// remedy would point at the wrong one — see `providers::jira::refresh_if_stale`.
pub async fn stamp_sync_error_with_remedy(
    pool: &SqlitePool,
    provider: &str,
    error: &str,
    remedy_override: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES (?, '1970-01-01T00:00:00Z', ?)
         ON CONFLICT(provider) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(provider)
    .bind(error)
    .execute(pool)
    .await?;

    let (title, remedy): (&str, Option<&str>) = match provider {
        // The DEFAULT, used by every Jira path that does not know which auth
        // method is in play (notably the fetch failure). It has to be the answer
        // that is right for both, which is Settings - the `.env` wording is kept
        // only as an explicit override on the resolve path, where
        // `has_basic_auth()` has actually established the user configured it
        // that way.
        "jira" => (
            "Jira sync failing",
            Some("Reconnect Jira in Settings - Integrations"),
        ),
        // Every remedy below names a place in the app rather than a file or a
        // CLI command. All five trackers are connected through Settings ->
        // Integrations, so a user who has only ever clicked Connect has no
        // `.env` to edit and no terminal to run `meridian oauth-login` in.
        // `every_default_remedy_names_a_place_in_the_app` pins this.
        "linear" => (
            "Linear sync failing",
            Some("Reconnect Linear in Settings - Integrations"),
        ),
        "trello" => (
            "Trello sync failing",
            Some("Reconnect Trello in Settings - Integrations"),
        ),
        // NOT "Set GITHUB_TOKEN in .env": GitHub connects via the in-app browser
        // device flow, which writes `GITHUB_TOKEN` to `.env` itself (see
        // `intelligence::oauth`). So the variable name is accurate and the
        // instruction is unfollowable — a user who clicked Connect has never
        // seen a token. Settings covers both that flow and the PAT one.
        "github" => (
            "GitHub sync failing",
            Some("Reconnect GitHub in Settings - Integrations"),
        ),
        "azure_devops" => (
            "Azure DevOps sync failing",
            Some("Reconnect Azure DevOps in Settings - Integrations"),
        ),
        _ => ("PM sync failing", None),
    };
    let remedy = remedy_override.or(remedy);
    let _ = crate::notices::raise(
        pool,
        &format!("pm.{provider}"),
        "error",
        title,
        error,
        remedy,
    )
    .await;
    Ok(())
}

/// How stale the last SUCCESSFUL sync may get before a run of otherwise-
/// suppressed transient failures is escalated to a user-facing notice.
///
/// Comfortably longer than any provider's sync interval, so an ordinary flaky
/// hour never trips it — but short enough that a persistently blocked provider
/// surfaces the same working day.
const TRANSIENT_ESCALATION_HOURS: i64 = 6;

/// Record a transient (retryable) sync failure.
///
/// Normally raises NOTHING — [`http::SyncFault::Retry`] exists precisely so a
/// network blip stops producing a "check your credentials" banner for
/// credentials that are fine.
///
/// But silence has to be BOUNDED. A provider blocked persistently (corporate
/// proxy, TLS interception, a firewall rule) is unreachable on every tick
/// forever, and suppressing that outright would leave the user's board silently
/// going stale with no signal at all — strictly worse than the misleading banner
/// this change removes. So the failure escalates to a normal sync notice once
/// the provider has not synced successfully within
/// [`TRANSIENT_ESCALATION_HOURS`].
///
/// **A provider that has NEVER synced needs its own handling**, because it has
/// no `pm_sync_state` row at all, so an age test alone would no-op forever on
/// exactly the installs that most need it — someone who just connected and whose
/// network blocks the provider outright.
///
/// It gets ONE retry rather than escalating on the first failure. Escalating
/// immediately would reintroduce the very thing this change removes, just with
/// connectivity wording instead of credentials wording: connect a tracker, have
/// the first attempt land on an ordinary blip (a DNS hiccup, a proxy handshake,
/// a laptop still waking up) and a "sync failing" banner appears seconds later.
/// So the first failure writes the row and stays silent; the next one sees the
/// row with an epoch `last_synced_at`, which is not recent, and escalates. One
/// sync interval of grace, and the persistently-blocked case still surfaces.
///
/// The remedy points at connectivity, NOT credentials: by construction we only
/// reach here for failures that are not the user's token.
///
/// Returns whether it escalated.
pub async fn note_transient_sync_failure(
    pool: &SqlitePool,
    provider: &str,
    detail: &str,
) -> Result<bool> {
    let threshold = format!("-{TRANSIENT_ESCALATION_HOURS} hours");
    let (has_row, recently_synced): (i64, i64) = sqlx::query_as(
        "SELECT
             EXISTS(SELECT 1 FROM pm_sync_state WHERE provider = ?),
             EXISTS(
                 SELECT 1 FROM pm_sync_state
                 WHERE provider = ?
                   AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
             )",
    )
    .bind(provider)
    .bind(provider)
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking sync recency for transient-failure escalation")?;

    if recently_synced != 0 {
        return Ok(false);
    }

    if has_row == 0 {
        // First-ever failure on a provider that has never synced. Record the
        // attempt so the NEXT one escalates, but raise nothing: a tracker
        // connected seconds ago that hits one blip must not immediately show a
        // fault. The epoch sentinel is deliberate - it is not a recent sync, so
        // the next failure takes the escalation branch below.
        sqlx::query(
            "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
             VALUES (?, '1970-01-01T00:00:00Z', ?)",
        )
        .bind(provider)
        .bind(detail)
        .execute(pool)
        .await
        .context("recording the first transient failure for a never-synced provider")?;
        tracing::debug!(
            provider,
            "first transient failure on a never-synced provider - staying quiet until the next attempt"
        );
        return Ok(false);
    }

    tracing::warn!(
        provider,
        hours = TRANSIENT_ESCALATION_HOURS,
        "provider unreachable with no recent successful sync - escalating to a notice"
    );
    // The title already names the provider, so the body only has to say what is
    // wrong and carry the cause.
    let msg =
        format!("No successful sync in the last {TRANSIENT_ESCALATION_HOURS} hours - {detail}");
    stamp_sync_error_with_remedy(
        pool,
        provider,
        &msg,
        Some("Check your internet connection, VPN, or proxy settings"),
    )
    .await?;
    Ok(true)
}

/// Record a provider sync failure against the shared classification.
///
/// The one place an `anyhow::Error` from a provider becomes user-visible state.
/// A retryable failure stays quiet (while still feeding the escalation clock);
/// a terminal one raises the notice. Both carry the FULL cause chain, because
/// [`http::classify`] formats it.
///
/// # Why this is shared rather than inlined
/// `intelligence::run_pm_sync`'s catch-all used to do
/// `stamp_sync_error(name, &e.to_string())` for ANY propagated error —
/// unconditionally terminal, and `{e}` rather than `{e:#}`, i.e. both of the
/// bugs this module exists to fix. That made it a silent undo button: a provider
/// could classify its own failure correctly and then have a second, generic,
/// truncated write land on top of it and win. `azure_devops` hit exactly that,
/// because it is the one provider whose `force_refresh` propagated `Err` after
/// already recording. Routing both the provider and the catch-all through this
/// function means there is no longer a path that reports a failure WITHOUT
/// classifying it.
///
/// `stage` names the failing step for the log only; the user-facing text is the
/// classified detail.
///
/// # Best-effort by construction
/// Returns `()`, not `Result`, so a failure to PERSIST the record can never
/// replace the provider error that caused it. With a `Result` the callers wrote
/// `record_sync_failure(...).await?`, which meant a transient `pm_sync_state`
/// write failure turned a classified provider failure into an sqlx error — and
/// in `azure_devops` that error then propagated back into
/// `intelligence::run_pm_sync`'s catch-all, so the user would be shown a
/// DATABASE error instead of "Azure DevOps sync failing". The classified outcome
/// has to stay authoritative.
///
/// Nothing diagnostic is lost: the `tracing::warn!` carrying the full cause
/// chain is emitted BEFORE the write, so the provider error reaches the logs and
/// the telemetry backend either way, and the persistence failure is logged
/// beside it rather than swallowed.
pub async fn record_sync_failure(
    pool: &SqlitePool,
    provider: &str,
    stage: &str,
    err: &anyhow::Error,
) {
    match http::classify(err) {
        http::SyncFault::Retry { detail } => {
            tracing::warn!(
                provider,
                stage,
                error = %detail,
                "provider unreachable - keeping stale cache, will retry next sync"
            );
            if let Err(e) = note_transient_sync_failure(pool, provider, &detail).await {
                tracing::warn!(provider, stage, error = %e, "recording transient sync failure failed");
            }
        }
        http::SyncFault::Report { detail } => {
            tracing::warn!(provider, stage, error = %detail, "provider sync failed");
            if let Err(e) = stamp_sync_error(pool, provider, &detail).await {
                tracing::warn!(provider, stage, error = %e, "recording sync failure failed");
            }
        }
    }
}

/// Clear the last error for a provider after a successful sync.
pub async fn clear_sync_error(pool: &SqlitePool, provider: &str) -> Result<()> {
    sqlx::query("UPDATE pm_sync_state SET last_error = NULL WHERE provider = ?")
        .bind(provider)
        .execute(pool)
        .await?;
    let _ = crate::notices::clear(pool, &format!("pm.{provider}")).await;
    Ok(())
}

/// Flag worklog- or daily-plan-retained rows that fell out of the active-task
/// fetch as off-board (`is_terminal = 1`).
///
/// Every provider's `prune` deletes `pm_tasks` rows the active-task fetch no
/// longer returns — EXCEPT those with `pm_worklogs` history or `daily_plan`
/// membership, which are kept forever so the timeline still has their title
/// and the plan checkbox's Undo (reopen) still has a row to act on.
///
/// **On the unbounded `daily_plan` retention (intentional):** a task_key that
/// ever appeared on any day's plan stays unpruneable in `pm_tasks` for the life
/// of the install — `daily_plan` has no TTL/GC. This is deliberate and mirrors the
/// `pm_worklogs` history retention beside it: a past day's plan, and any worklog
/// resolved against those tickets, must still render the ticket's title long after
/// it left the active board, so scoping this to a recency window would silently
/// break historical plan/worklog views for older tickets. The growth is human-
/// paced (a handful of planned tasks per day), so the row count it pins is small
/// and bounded in practice by how much a person actually plans — not by ticket
/// churn — which is why we accept keeping them over losing historical resolution.
///
/// The gap: a
/// kept row's `is_terminal` was frozen at whatever it was when the task last
/// appeared in the fetch, so a ticket that goes Done or is reassigned away
/// while retained lingers on the board as "active" indefinitely (the board is
/// `WHERE is_terminal = 0`). The active-task fetch is, by construction, exactly
/// "assigned to me AND not-done AND in-scope-type", so any row NOT in
/// `fetched_keys` is no longer one of those — it belongs off the board. We stamp
/// it terminal (kept for the timeline, hidden from the board and from worklog
/// candidate matching, which also filters `is_terminal = 0`; the plan checkbox
/// itself reads `is_terminal` straight off this row). If the ticket is
/// reassigned back / reopened it returns in the next fetch and the upsert resets
/// `is_terminal` from its real status, so this self-corrects.
///
/// `fetched_keys` empty (you have zero active tasks) → every retained row for
/// this provider is stamped. Returns the number of rows flagged.
pub async fn mark_retained_offboard(
    pool: &SqlitePool,
    provider: &str,
    fetched_keys: &[String],
) -> Result<u64> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE pm_tasks SET is_terminal = 1 \
         WHERE provider = ? AND is_terminal = 0 \
           AND task_key NOT IN ({placeholders}) \
           AND (task_key IN (SELECT DISTINCT task_key FROM pm_worklogs WHERE provider = ?) \
                OR task_key IN (SELECT DISTINCT task_key FROM daily_plan))"
    );
    let mut q = sqlx::query(&sql).bind(provider);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q = q.bind(provider);
    let result = q
        .execute(pool)
        .await
        .with_context(|| format!("flagging retained off-board {provider} tasks"))?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
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
}
