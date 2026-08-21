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
/// **This same one-retry grace applies to a provider that has synced
/// successfully before**, not just a brand-new one - see the `is_sentinel`
/// branch in [`note_transient_sync_failure`]. Without it, a provider whose
/// `last_synced_at` goes stale because the DAEMON WAS ASLEEP (laptop closed
/// overnight, a longer Wi-Fi/VPN drop) rather than because it kept failing
/// escalates on its very first retry after waking - the exact false-positive
/// this module exists to remove, just triggered by a gap in *time* instead of
/// a gap in connectivity. The grace row and this ONE mechanism (the epoch
/// sentinel) cover both cases identically, so the escalation check cannot tell
/// them apart and does not need to.
///
/// The remedy points at connectivity, NOT credentials: by construction we only
/// reach here for failures that are not the user's token.
///
/// Write the epoch-sentinel grace row for a provider that has never synced.
///
/// **Deliberately does NOT write `last_error`** (unlike the escalating write
/// in [`stamp_sync_error_with_remedy`]): `meridian_core::readers::integrations::sync_errors`
/// reads `last_error IS NOT NULL` to drive the per-provider "Sync failed"
/// badge on the Integrations page, independent of whether a system-wide
/// notice was raised. Setting it here would show that badge during a period
/// this function is specifically trying to keep quiet - `detail` is still
/// logged (below) so the attempt is not lost from telemetry, just kept out of
/// the persisted, user-visible column.
///
/// # Why `ON CONFLICT DO NOTHING`
/// The caller's `has_row` check is a SEPARATE statement from this insert, and
/// `pm_sync_state.provider` is a PRIMARY KEY - so anything that can run two
/// sync passes for one provider concurrently can have both observe "no row"
/// and both insert. That is not hypothetical here: `meridian ticket-update`
/// opens its OWN pool to the same `meridian.db` and calls
/// `intelligence::run_pm_force_sync` (`src/main.rs`) while the daemon's poll
/// loop is running `run_pm_sync` against the same file, so the two racers are
/// separate PROCESSES and no in-process lock would cover them.
///
/// Without the clause the loser gets `UNIQUE constraint failed`, which
/// [`record_sync_failure`] swallows by design - so the grace row would be
/// silently lost, the next failure would take the same branch again, and the
/// escalation clock would never start. That is the exact failure this whole
/// module exists to prevent, reintroduced by a race.
///
/// Doing nothing on conflict is the correct resolution rather than a
/// compromise: the row the winner wrote carries the same epoch sentinel this
/// one would have written, so both racers leave the state they intended.
///
/// Extracted from [`note_transient_sync_failure`] to give the conflict
/// behaviour a testable seam - the race itself has no deterministic hook, but
/// "inserting over an existing row is a no-op, not an error" does.
async fn insert_grace_row(pool: &SqlitePool, provider: &str, detail: &str) -> Result<()> {
    tracing::debug!(
        provider,
        detail,
        "grace row: recording the attempt without a user-visible error"
    );
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES (?, '1970-01-01T00:00:00Z', NULL)
         ON CONFLICT(provider) DO NOTHING",
    )
    .bind(provider)
    .execute(pool)
    .await
    .context("recording the first transient failure for a never-synced provider")?;
    Ok(())
}

/// Downgrade a STALE-BUT-PREVIOUSLY-SUCCESSFUL provider's `last_synced_at` to
/// the epoch sentinel, marking its one retry of grace as used - WITHOUT
/// writing `last_error`, for the same reason [`insert_grace_row`] doesn't (see
/// its doc comment). Mirrors that function's shape but is an UPDATE, not an
/// INSERT: the row already exists here, carrying a real (if old) past-success
/// timestamp that this call intentionally overwrites, so the NEXT failure's
/// `is_sentinel` check reads it identically to a never-synced provider's grace
/// row and escalates.
async fn mark_first_stale_failure(pool: &SqlitePool, provider: &str, detail: &str) -> Result<()> {
    tracing::debug!(
        provider,
        detail,
        "first transient failure since a genuine past success - staying quiet until the next attempt"
    );
    sqlx::query(
        "UPDATE pm_sync_state SET last_synced_at = '1970-01-01T00:00:00Z' WHERE provider = ?",
    )
    .bind(provider)
    .execute(pool)
    .await
    .context("recording a resumed provider's first quiet failure since its last success")?;
    Ok(())
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
    let threshold = format!("-{TRANSIENT_ESCALATION_HOURS} hours");
    let (has_row, recently_synced, is_sentinel, has_error): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
             EXISTS(SELECT 1 FROM pm_sync_state WHERE provider = ?),
             EXISTS(
                 SELECT 1 FROM pm_sync_state
                 WHERE provider = ?
                   AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
             ),
             EXISTS(
                 SELECT 1 FROM pm_sync_state
                 WHERE provider = ? AND last_synced_at = '1970-01-01T00:00:00Z'
             ),
             EXISTS(
                 SELECT 1 FROM pm_sync_state WHERE provider = ? AND last_error IS NOT NULL
             )",
    )
    .bind(provider)
    .bind(provider)
    .bind(&threshold)
    .bind(provider)
    .bind(provider)
    .fetch_one(pool)
    .await
    .context("checking sync recency for transient-failure escalation")?;

    if recently_synced != 0 {
        tracing::Span::current().record("escalated", false);
        return Ok(false);
    }

    if has_row == 0 {
        // First-ever failure on a provider that has never synced. Record the
        // attempt so the NEXT one escalates, but raise nothing: a tracker
        // connected seconds ago that hits one blip must not immediately show a
        // fault. The epoch sentinel is deliberate - it is not a recent sync, so
        // the next failure takes the escalation branch below.
        insert_grace_row(pool, provider, detail).await?;
        tracing::debug!(
            provider,
            "first transient failure on a never-synced provider - staying quiet until the next attempt"
        );
        tracing::Span::current().record("escalated", false);
        return Ok(false);
    }

    // Not recently synced, and the provider HAS synced before. Give it the
    // SAME one retry of grace a never-synced provider gets, unless there is
    // already something to weigh against staying quiet: `is_sentinel` covers
    // "the grace tick already ran" (this branch set `last_synced_at` to the
    // sentinel with no `last_error`, and a SECOND consecutive failure now
    // finds it), and `has_error` covers "a fault is already recorded here"
    // (a terminal failure elsewhere already wrote `last_error` and raised its
    // own notice - nothing is gained by staying quiet on top of that). Only a
    // row that has synced before, has since gone stale, and carries neither
    // signal - a clean transition into staleness - gets the grace write.
    if has_error == 0 && is_sentinel == 0 {
        mark_first_stale_failure(pool, provider, detail).await?;
        tracing::Span::current().record("escalated", false);
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
    tracing::Span::current().record("escalated", true);
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
mod sync_failure_tests;
