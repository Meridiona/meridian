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

/// How many CONSECUTIVE failed sync attempts a provider gets before its
/// otherwise-suppressed transient failures escalate to a user-facing notice.
///
/// This replaced a "no successful sync in the last 6 hours" rule. That rule was
/// correct when written: sync ran on a timer every few minutes, so six hours
/// without a success reliably meant something was broken - its own doc comment
/// said "comfortably longer than any provider's sync interval".
///
/// Sync is on-demand now (see [`crate::intelligence::run_pm_sync`]) and that
/// premise is gone. A machine that slept all weekend, or a user who has not
/// opened the dashboard since Friday, has no successful sync for DAYS and nothing
/// is wrong. Under the old rule the first two failures after any quiet period
/// escalated, so opening the dashboard on Monday before Wi-Fi associated raised a
/// red "sync failing" banner on a healthy install - with a body reading "No
/// successful sync in the last 6 hours" while describing normal use.
///
/// Consecutive failures are the honest evidence: a blocked proxy or a dead route
/// fails EVERY attempt, whereas a wake-time blip fails once or twice then works.
/// Elapsed quiet carries no signal at all any more.
const TRANSIENT_ESCALATION_ATTEMPTS: u32 = 4;

/// Consecutive transient failures per provider, for the CURRENT PROCESS only.
///
/// In memory rather than in `pm_sync_state`, deliberately:
///
/// * No migration, and it cannot corrupt a database - the same reasoning that put
///   the sync dedup lock and the OAuth refresh journal outside `meridian.db`.
/// * Resetting on restart is the SAFE direction. launchd `KeepAlive` restarts the
///   daemon on every wake, so a fresh process starts at zero and can never
///   inherit a stale streak and escalate on its first blip. The cost is that a
///   persistent block takes a few attempts within one session to surface, which
///   is the trade we want: quiet by default, loud only on real evidence.
static TRANSIENT_STREAKS: std::sync::Mutex<Option<std::collections::HashMap<String, u32>>> =
    std::sync::Mutex::new(None);

/// Increment and return this provider's consecutive-failure count.
fn bump_transient_streak(provider: &str) -> u32 {
    let mut guard = TRANSIENT_STREAKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    let counter = map.entry(provider.to_string()).or_insert(0);
    *counter = counter.saturating_add(1);
    *counter
}

/// Reset this provider's consecutive-failure count after a successful sync.
///
/// A success is the ONLY thing that clears it, which is what makes the count mean
/// "failures in a row" rather than "failures ever".
pub(crate) fn reset_transient_streak(provider: &str) {
    let mut guard = TRANSIENT_STREAKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(map) = guard.as_mut() {
        map.remove(provider);
    }
}

/// Test-only: force a provider's streak so escalation can be exercised without
/// calling the recorder N times.
#[cfg(test)]
pub(crate) fn set_transient_streak_for_test(provider: &str, value: u32) {
    let mut guard = TRANSIENT_STREAKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    map.insert(provider.to_string(), value);
}

/// Returns whether it escalated.
#[tracing::instrument(
    skip(pool, detail),
    fields(provider, escalated = tracing::field::Empty)
)]
pub async fn note_transient_sync_failure(
    pool: &SqlitePool,
    provider: &str,
    detail: &str,
) -> Result<bool> {
    let streak = bump_transient_streak(provider);
    let span = tracing::Span::current();
    span.record("streak", streak);

    if streak < TRANSIENT_ESCALATION_ATTEMPTS {
        // Silence, which is the normal outcome. `SyncFault::Retry` exists so a
        // network blip does not produce a banner for credentials that are fine,
        // and a wake-time blip is the single most common instance of one.
        tracing::debug!(
            provider,
            streak,
            needed = TRANSIENT_ESCALATION_ATTEMPTS,
            "transient sync failure - staying quiet until the streak is real"
        );
        span.record("escalated", false);
        return Ok(false);
    }

    // Silence has to be BOUNDED. A provider blocked persistently (corporate
    // proxy, TLS interception, a firewall rule) fails every single attempt
    // forever, and suppressing that outright would leave the board silently going
    // stale with no signal at all - strictly worse than a misleading banner.
    tracing::warn!(
        provider,
        streak,
        "provider unreachable on {streak} consecutive attempts - escalating to a notice"
    );
    // The message names the evidence, not a duration. The old wording ("No
    // successful sync in the last 6 hours") described normal on-demand operation
    // and so read as a false alarm even when the fault was real.
    let msg = format!("Unreachable on {streak} consecutive sync attempts - {detail}");
    stamp_sync_error_with_remedy(
        pool,
        provider,
        &msg,
        Some("Check your internet connection, VPN, or proxy settings"),
    )
    .await?;
    span.record("escalated", true);
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
///
/// # Telemetry
/// Instrumented because this is where a provider failure becomes user-visible
/// state, and the outcome is not derivable from the provider's own spans: the
/// same `anyhow::Error` can end as a silent retry or a raised notice depending
/// on [`http::classify`]. `outcome` records which, and `otel.status_code` marks
/// the span ERROR only on the terminal path - a retryable blip is an expected
/// outcome, not a fault, and marking it would drown the real ones.
#[tracing::instrument(
    skip(pool, err),
    fields(
        provider,
        stage,
        outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    )
)]
pub async fn record_sync_failure(
    pool: &SqlitePool,
    provider: &str,
    stage: &str,
    err: &anyhow::Error,
) {
    let span = tracing::Span::current();
    match http::classify(err) {
        http::SyncFault::Retry { detail } => {
            tracing::warn!(
                provider,
                stage,
                error = %detail,
                "provider unreachable - keeping stale cache, will retry next sync"
            );
            match note_transient_sync_failure(pool, provider, &detail).await {
                Ok(escalated) => {
                    span.record(
                        "outcome",
                        if escalated {
                            "transient_escalated"
                        } else {
                            "transient_quiet"
                        },
                    );
                }
                Err(e) => {
                    tracing::warn!(provider, stage, error = %e, "recording transient sync failure failed");
                    span.record("outcome", "transient_record_failed");
                    span.record("otel.status_code", "ERROR");
                }
            };
        }
        http::SyncFault::Report { detail } => {
            tracing::warn!(provider, stage, error = %detail, "provider sync failed");
            span.record("outcome", "reported");
            span.record("otel.status_code", "ERROR");
            if let Err(e) = stamp_sync_error(pool, provider, &detail).await {
                tracing::warn!(provider, stage, error = %e, "recording sync failure failed");
                span.record("outcome", "report_record_failed");
            }
        }
    }
}

/// Clear the last error for a provider after a successful sync.
pub async fn clear_sync_error(pool: &SqlitePool, provider: &str) -> Result<()> {
    // A success is the ONLY thing that resets the consecutive-failure streak,
    // which is what makes that count mean "failures in a row" rather than
    // "failures ever". Reset FIRST: if the DB write below fails, the streak is
    // still correct (the sync did succeed), whereas a streak left standing after
    // a success would escalate on the next single blip.
    reset_transient_streak(provider);
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
    // An empty fetch is NOT "everything is stale". Two things produce it — the
    // user genuinely has nothing open, and a scope/permission blip that returns
    // 200 with zero issues — and they are indistinguishable from here. Flagging
    // the whole board off-board on the second one is a bad trade against leaving
    // it alone on the first, which self-heals on the next sync.
    //
    // This also stopped an outright bug: an empty slice renders `NOT IN ()`,
    // which is invalid SQL, so this path previously raised an error that every
    // caller swallowed as a warning. It behaved correctly only by accident, and
    // only as long as nobody looked at the log.
    if fetched_keys.is_empty() {
        return Ok(0);
    }
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
mod sync_failure_tests;
