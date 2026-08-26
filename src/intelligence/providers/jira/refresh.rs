//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The Jira refresh workflow: the staleness gate, the fetch-upsert-prune cycle,
//! and the two public entry points the rest of the daemon calls.
//!
//! # Why this is its own file
//! Split out of `jira/mod.rs` when that file passed the repo's 500-line cap.
//! The seam is a real one rather than an arbitrary cut: everything here is the
//! ORCHESTRATION - when to sync, in what order, and what to do when a step
//! fails - while `mod.rs` keeps the wire DTOs and the per-row database
//! operations those steps call.
//!
//! # Who calls this
//! [`refresh_if_stale`] from the daemon's PM sync tick
//! (`crate::intelligence::providers`), and [`force_refresh`] from the tray's
//! manual "Sync now".
//!
//! # Related
//! - [`super::fetch`] - the HTTP layer and its JQL, including the portable
//!   fallback for a site missing an issue type the query names.
//! - [`super::upsert`] / [`super::prune`] / [`super::backfill_worklogged`] -
//!   the row-level steps this sequences.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use crate::config::JiraConfig;
use crate::intelligence::providers::http::SyncFault;
use crate::intelligence::Trigger;

use super::{
    backfill_worklogged, discover_start_date_field, fetch, native_terminal, prune, upsert,
    MAX_RESULTS, SYNC_INTERVAL_MINS,
};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, jira))]
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    jira: &JiraConfig,
    trigger: Trigger,
) -> Result<Option<Vec<String>>> {
    let threshold = format!("-{SYNC_INTERVAL_MINS} minutes");
    let (is_fresh,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = 'jira'
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking jira sync state")?;

    if is_fresh != 0 {
        let cached_keys: Vec<(String,)> =
            sqlx::query_as("SELECT task_key FROM pm_tasks WHERE provider = 'jira'")
                .fetch_all(pool)
                .await
                .context("loading cached jira task keys")?;
        let keys: Vec<&str> = cached_keys.iter().map(|(k,)| k.as_str()).collect();
        tracing::debug!(
            cached_task_count = keys.len(),
            ?keys,
            "jira task cache is fresh"
        );
        return Ok(None);
    }

    tracing::debug!("jira task cache is stale — refreshing");

    // Resolve auth once per refresh: OAuth (with refresh-before-use) if a token
    // store exists, else static basic auth. A resolve failure means no usable
    // creds — keep the stale cache rather than erroring the whole tick.
    //
    // A clock-driven pass must never MINT a Jira OAuth token — spending the
    // rotating refresh token unattended is what permanently kills a grant when the
    // machine suspends mid-POST (see `Trigger::Unattended`). An expired token here
    // means keep the stale cache and let the next attended request refresh; that
    // is not a failure and must not be recorded as one, or a laptop that was shut
    // for the night would raise a sync error every morning.
    let ctx = if trigger == Trigger::Unattended {
        match crate::intelligence::oauth::jira::resolve_unattended(jira) {
            Some(ctx) => ctx,
            None => {
                tracing::debug!(
                    "jira token not usable without a refresh - deferring unattended sync"
                );
                return Ok(None);
            }
        }
    } else {
        match crate::intelligence::oauth::jira::resolve(jira).await {
            Ok(ctx) => ctx,
            Err(e) => {
                // A refresh that failed only because Atlassian was briefly
                // unreachable (network blip, timeout, 429, 5xx) does NOT mean the
                // token is dead — the stored refresh token is still valid and the
                // next tick will almost certainly succeed. Raising a "Reconnect
                // Jira / re-run oauth-login" sync error for that transient case is
                // exactly what made this fault flap on and off at random. Keep the
                // stale cache and stay quiet; the notice is reserved for a terminal
                // auth failure the user actually has to act on.
                // Deliberately NOT `record_sync_failure` like the fetch arm below:
                // this path needs the auth-method-specific remedy override that the
                // shared helper has no way to express (basic auth really does want
                // the `.env` wording). The classification policy is otherwise
                // identical, so a change to one should be mirrored here.
                //
                // `meridian_oauth::is_transient` only recognises a `TokenError` from
                // the token endpoint and answers `false` for anything else, so a raw
                // transport failure escaping the refresh would still land here as
                // terminal. Falling back to `http::classify` closes that hole
                // without ever making a genuinely dead grant look retryable.
                let fault = if meridian_oauth::is_transient(&e) {
                    SyncFault::retry(&e)
                } else {
                    crate::intelligence::providers::http::classify(&e)
                };
                match fault {
                    SyncFault::Retry { detail } => {
                        tracing::warn!(
                            error = %detail,
                            "jira auth temporarily unavailable - keeping stale cache, will retry next sync"
                        );
                        let _ = crate::intelligence::providers::note_transient_sync_failure(
                            pool, "jira", &detail,
                        )
                        .await;
                    }
                    SyncFault::Report { detail } => {
                        tracing::warn!(error = %detail, "jira auth unavailable - keeping stale cache");
                        let msg = format!("Jira auth failed: {detail}");
                        // Basic-auth (JIRA_API_TOKEN/JIRA_BASE_URL) and OAuth are
                        // mutually exclusive here - has_basic_auth() mirrors
                        // resolve()'s own choice - so the remedy must match whichever
                        // path this failure came from, not always point at .env.
                        let remedy = if crate::intelligence::oauth::jira::has_basic_auth(jira) {
                            "Set JIRA_API_TOKEN and JIRA_BASE_URL in .env"
                        } else {
                            "Reconnect Jira in Settings - Integrations"
                        };
                        let _ = crate::intelligence::providers::stamp_sync_error_with_remedy(
                            pool,
                            "jira",
                            &msg,
                            Some(remedy),
                        )
                        .await;
                    }
                }
                return Ok(None);
            }
        }
    };
    let auth_method = if jira.api_token.is_empty() {
        "oauth"
    } else {
        "api_token"
    };
    tracing::debug!(auth_method, "jira auth resolved");

    let start_date_field = discover_start_date_field(&ctx).await;
    if let Some(ref id) = start_date_field {
        tracing::debug!(field_id = %id, "discovered jira start date field");
    }

    // Backfill worklogged-but-missing tickets INSIDE the stale sync cycle only,
    // reusing the auth + field resolved just above. Running this on every tick
    // (as it used to, before the freshness check) meant a permanently-deleted
    // ticket triggered auth + field-discovery + fetch on every poll, a
    // rate-limit hazard. Best-effort: a failure never blocks the main sync.
    if let Err(e) = backfill_worklogged(pool, jira, &ctx, start_date_field.as_deref()).await {
        tracing::warn!(error = %e, "jira worklog backfill failed — will retry next sync");
    }

    match fetch(&ctx, start_date_field.as_deref()).await {
        Ok(fetch::ActiveFetch {
            issues,
            returned_by_server,
        }) => {
            let keys: Vec<String> = issues.iter().map(|(i, _)| i.key.clone()).collect();
            let n = keys.len();
            let project_key = issues
                .first()
                .map(|(i, _)| i.fields.project.key.as_str())
                .unwrap_or("-");
            let terminal_count = issues
                .iter()
                .filter(|(i, _)| {
                    native_terminal(&i.fields.status.status_category.key) == Some(true)
                })
                .count();
            tracing::debug!(fetched_count = n, "jira fetch completed");
            tracing::info!(
                issue_count = n,
                project_key,
                upserted = n,
                terminal_skipped = terminal_count,
                auth_method,
                "jira issues fetched"
            );
            upsert(pool, &issues, jira, &ctx, start_date_field.as_deref()).await?;
            sqlx::query(
                "INSERT INTO pm_sync_state (provider, last_synced_at)
                 VALUES ('jira', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
            )
            .execute(pool)
            .await
            .context("updating jira sync state")?;
            // Truncation is judged on what the SERVER returned, never on `n`
            // (the post-filter count). Dropping container-tier rows shrinks `n`,
            // so using it here would make a full — possibly truncated — page look
            // partial and let prune delete tickets that simply did not fit on it.
            if returned_by_server < MAX_RESULTS {
                match prune(pool, &keys).await {
                    Ok(0) => {}
                    Ok(pruned) => tracing::info!(pruned_count = pruned, "pruned stale jira tasks"),
                    Err(e) => tracing::warn!(error = %e, "jira prune failed"),
                }
            } else {
                tracing::debug!(
                    fetched_count = n,
                    returned_by_server,
                    max_results = MAX_RESULTS,
                    "skipping prune — response may be truncated"
                );
            }
            tracing::info!(upserted_count = n, "jira tasks refreshed");
            let _ = crate::intelligence::providers::clear_sync_error(pool, "jira").await;
            Ok(Some(keys))
        }
        Err(e) => {
            // The gap that left Jira users flapping: the transient guard on the
            // resolve path above covers REFRESHING the token, never USING it. An
            // Atlassian 5xx or a network blip during the search raised exactly
            // the terminal banner that path was fixed to suppress.
            crate::intelligence::providers::record_sync_failure(pool, "jira", "fetch", &e).await;
            Ok(None)
        }
    }
}

/// Force an immediate Jira sync regardless of the staleness gate.
/// Clears `pm_sync_state` for this provider so `refresh_if_stale` sees it as
/// stale, then delegates. The `last_synced_at` is updated inside the delegate,
/// so subsequent ticks won't double-fetch.
///
/// Always [`Trigger::Attended`]: every caller is a direct user action — connecting
/// a tracker, pressing Sync now, or a `meridian tasks-sync` / `ticket-update` CLI
/// invocation — so refreshing the OAuth token here is safe and expected. There is
/// deliberately no unattended force-sync: forcing a fetch while nobody is present
/// is the combination this whole distinction exists to prevent.
pub async fn force_refresh(pool: &SqlitePool, jira: &JiraConfig) -> Result<Option<Vec<String>>> {
    sqlx::query("DELETE FROM pm_sync_state WHERE provider = 'jira'")
        .execute(pool)
        .await
        .context("clearing jira sync state for force refresh")?;
    refresh_if_stale(pool, jira, Trigger::Attended).await
}
