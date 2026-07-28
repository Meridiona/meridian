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
        "jira" => (
            "Jira sync failing",
            Some("Set JIRA_API_TOKEN and JIRA_BASE_URL in .env"),
        ),
        "linear" => ("Linear sync failing", Some("Set LINEAR_API_KEY in .env")),
        "trello" => (
            "Trello sync failing",
            Some("Run: meridian oauth-login trello"),
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
            Some("Set AZURE_DEVOPS_PAT in .env"),
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
/// **A provider that has NEVER synced escalates immediately**, deliberately:
/// now that transient failures no longer call [`stamp_sync_error`], a machine
/// that has never once reached the provider has no `pm_sync_state` row at all,
/// so an age test alone would no-op on exactly the installs that most need it —
/// someone who just connected and whose network blocks the provider outright.
/// `EXISTS` is false for a missing row, which buys that case for free.
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
    let (recently_synced,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = ?
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(provider)
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking sync recency for transient-failure escalation")?;

    if recently_synced != 0 {
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

    /// The trap this fix has to survive: transient failures no longer call
    /// `stamp_sync_error`, so a machine that has NEVER reached the provider has
    /// no `pm_sync_state` row at all. An age test alone would no-op on exactly
    /// the installs that need it most - someone who just connected behind a
    /// blocking proxy. `EXISTS` is false for a missing row, which covers it.
    #[tokio::test]
    async fn a_provider_that_has_never_synced_escalates_immediately() {
        let pool = make_db().await;

        let escalated = note_transient_sync_failure(&pool, "github", "dns error")
            .await
            .unwrap();

        assert!(
            escalated,
            "no pm_sync_state row means never-synced, which must not be silent"
        );
        assert!(notice(&pool, "pm.github").await.is_some());
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
}
