//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Single-owner PM sync: the request side of the outbox (`pm_sync_requests`,
//! migration 082).
//!
//! # Why sync is a request instead of an action
//!
//! An Atlassian OAuth refresh token is single-use and rotating. The old token dies
//! the instant the new one is issued, so a lost response leaves the grant
//! recoverable only inside a 10-minute window and permanently dead after it. That
//! makes the token a resource with exactly ONE safe writer.
//!
//! It had several: the tray refreshed in-process, the daemon refreshed on its poll
//! loop, and the tray spawned fresh `meridian pm-sync` / `tasks-sync` processes that
//! each refreshed too. The advisory file lock meant to serialise them could not
//! actually do it - its 10 s timeout is shorter than the ~26 s a refresh can take
//! (3 attempts x 8 s plus backoff), and on timeout the code proceeded WITHOUT the
//! lock rather than backing off. So two processes could spend the same token, and
//! the only thing preventing corruption was Atlassian's grace window handing the
//! loser the current pair.
//!
//! Producers now write a row here and the **daemon is the sole consumer**, so the
//! credential is held by one process by construction rather than by lock discipline.
//!
//! # Who calls this
//! - Producers: `tray/src-tauri/src/commands/tasks.rs` (window opens, tracker
//!   connect, "Sync now"), and the `meridian tasks-sync` / `pm-sync` CLIs when a
//!   daemon is running.
//! - Consumer: the daemon's sync-request watcher (`src/intelligence/sync_requests.rs`).
//!
//! # Related
//! - [`crate::notifications`] - the outbox pattern this mirrors.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// The all-providers request every current producer writes. A specific provider
/// name scopes a request to one board, reserved for a future caller that needs it.
pub const ALL_PROVIDERS: &str = "*";

/// Whether the daemon should honour the per-provider staleness window or bypass it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Honour the staleness window - the cheap common case (a window opening).
    Gated,
    /// Bypass it: the user explicitly asked (connected a tracker, pressed "Sync
    /// now", ran a CLI).
    Force,
}

impl SyncMode {
    /// The stored discriminant. Kept as text so the row is readable in `sqlite3`
    /// during support work.
    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::Gated => "gated",
            SyncMode::Force => "force",
        }
    }

    /// Parse a stored discriminant, defaulting to the SAFER option. An unknown or
    /// corrupt value must never silently become a `Force` that bypasses the
    /// staleness gate and hammers the provider's API.
    pub fn from_str_or_gated(s: &str) -> Self {
        match s {
            "force" => SyncMode::Force,
            _ => SyncMode::Gated,
        }
    }
}

/// One pending request, as claimed by the daemon.
#[derive(Debug, Clone)]
pub struct SyncRequest {
    pub provider: String,
    pub mode: SyncMode,
    pub reason: String,
}

/// Ask the daemon to sync PM tasks. Idempotent and coalescing: repeated calls
/// collapse into the single pending row rather than queueing, so opening the
/// dashboard ten times means "a sync is wanted", not ten syncs.
///
/// `mode` **escalates only**. A `Force` landing on a pending `Gated` upgrades it,
/// because a user who just connected a tracker must not have that downgraded by a
/// passing window focus; a `Gated` landing on a pending `Force` leaves the `Force`
/// intact. Writing a new request also clears any previous completion stamps, so the
/// row unambiguously represents work still to do.
///
/// `reason` is a producer tag for tracing only (`"dashboard_open"`,
/// `"token_connected"`). Never pass user content - it is read back into logs.
#[tracing::instrument(skip(pool))]
pub async fn request(
    pool: &SqlitePool,
    provider: &str,
    mode: SyncMode,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO pm_sync_requests
             (provider, mode, reason, requested_at, claimed_at, completed_at, error, synced_count)
         VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), NULL, NULL, NULL, NULL)
         ON CONFLICT(provider) DO UPDATE SET
             -- Escalate to 'force', never back down from it while still pending.
             --
             -- The `completed_at IS NULL` half is load-bearing: the row is kept after a
             -- sync finishes (so \"Sync now\" can read its result), so without it a
             -- SPENT 'force' would be inherited forever and every later gated request
             -- would silently escalate. One tracker connect would then make every
             -- planner open bypass the staleness gate and hit the provider for real -
             -- reinstating the constant polling this whole design removes, and
             -- multiplying exactly the token refreshes it exists to reduce.
             mode = CASE
                 WHEN excluded.mode = 'force'
                   OR (pm_sync_requests.mode = 'force'
                       AND pm_sync_requests.completed_at IS NULL)
                      THEN 'force'
                 ELSE excluded.mode
             END,
             reason       = excluded.reason,
             requested_at = excluded.requested_at,
             -- A fresh request re-opens the row: drop the in-flight and completion
             -- marks so the watcher sees pending work again.
             claimed_at   = NULL,
             completed_at = NULL,
             error        = NULL,
             synced_count = NULL",
    )
    .bind(provider)
    .bind(mode.as_str())
    .bind(reason)
    .execute(pool)
    .await
    .context("writing a PM sync request")?;
    tracing::debug!(provider, mode = mode.as_str(), reason, "PM sync requested");
    Ok(())
}

/// Claim the pending request for `provider`, if there is one, marking it in-flight
/// so a second watcher tick can't pick up the same work.
///
/// The claim is a conditional UPDATE (`WHERE claimed_at IS NULL AND completed_at IS
/// NULL`) rather than a read-then-write, so two daemons racing on the same file -
/// which the single-instance guard makes unlikely but not impossible during a
/// restart overlap - cannot both claim it. SQLite serialises the statement, so
/// exactly one sees a non-zero `rows_affected`.
#[tracing::instrument(skip(pool))]
pub async fn claim(pool: &SqlitePool, provider: &str) -> Result<Option<SyncRequest>> {
    let claimed = sqlx::query(
        "UPDATE pm_sync_requests
            SET claimed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
          WHERE provider = ?
            AND claimed_at IS NULL
            AND completed_at IS NULL",
    )
    .bind(provider)
    .execute(pool)
    .await
    .context("claiming a PM sync request")?;

    if claimed.rows_affected() == 0 {
        return Ok(None);
    }

    let row: Option<(String, String)> =
        sqlx::query_as("SELECT mode, reason FROM pm_sync_requests WHERE provider = ?")
            .bind(provider)
            .fetch_optional(pool)
            .await
            .context("reading the claimed PM sync request")?;

    Ok(row.map(|(mode, reason)| SyncRequest {
        provider: provider.to_string(),
        mode: SyncMode::from_str_or_gated(&mode),
        reason,
    }))
}

/// Record the outcome of a serviced request in place. The row is deliberately kept
/// rather than deleted so `"Sync now"` can read a real result without holding the
/// credential, and so support has a content-free view of the last attempt.
///
/// Writes only if the row is still the one that was claimed. The guard is
/// **`claimed_at IS NOT NULL`**, and that specific predicate is the whole point:
/// [`request`] resets `claimed_at` to `NULL`, so a request that arrived mid-sync
/// makes this UPDATE match nothing. Guarding on `completed_at IS NULL` alone would
/// NOT work - the fresh request leaves that NULL too, so the older sync's outcome
/// would stamp the new request as done without it ever being serviced, and the
/// user's "Sync now" would report success for a sync that never ran.
#[tracing::instrument(skip(pool))]
pub async fn complete(
    pool: &SqlitePool,
    provider: &str,
    synced_count: Option<i64>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE pm_sync_requests
            SET completed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                error        = ?,
                synced_count = ?
          WHERE provider = ?
            AND claimed_at IS NOT NULL
            AND completed_at IS NULL",
    )
    .bind(error)
    .bind(synced_count)
    .bind(provider)
    .execute(pool)
    .await
    .context("completing a PM sync request")?;
    Ok(())
}

/// Release claims left in flight by a daemon that died mid-sync, so the request
/// becomes claimable again. Call once at watcher startup.
///
/// Without this a crash, a `meridian restart`, or a SIGKILL between [`claim`] and
/// [`complete`] strands the row **claimed but never completed** - and since [`claim`]
/// requires `claimed_at IS NULL`, no future tick would ever pick it up. PM sync
/// would then be silently dead until the next new request happened to reset the row.
/// Same reasoning as the daemon's `cleanup_incomplete_runs` for partial ETL runs.
///
/// Only ever widens what is claimable, so it is safe to run unconditionally on every
/// boot: a genuinely in-flight sync cannot exist yet, because the only consumer is
/// the watcher this runs before.
#[tracing::instrument(skip(pool))]
pub async fn reset_stale_claims(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE pm_sync_requests
            SET claimed_at = NULL
          WHERE claimed_at IS NOT NULL
            AND completed_at IS NULL",
    )
    .execute(pool)
    .await
    .context("resetting stale PM sync request claims")?;
    let n = res.rows_affected();
    if n > 0 {
        tracing::info!(
            reset = n,
            "released PM sync claims stranded by a previous daemon exit"
        );
    }
    Ok(n)
}

/// The outcome of the last request for `provider`, for a producer that wants to
/// show one ("Sync now"). `None` while the request is still pending or in flight,
/// so a caller can poll this until it turns `Some`.
pub async fn outcome(pool: &SqlitePool, provider: &str) -> Result<Option<SyncOutcome>> {
    let row: Option<(Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT completed_at, error, synced_count FROM pm_sync_requests WHERE provider = ?",
    )
    .bind(provider)
    .fetch_optional(pool)
    .await
    .context("reading the PM sync outcome")?;

    Ok(match row {
        Some((Some(_completed), error, synced_count)) => Some(SyncOutcome {
            error,
            synced_count,
        }),
        // No row, or a row still pending / in flight.
        _ => None,
    })
}

/// A completed request's result.
#[derive(Debug, Clone)]
pub struct SyncOutcome {
    /// `None` on success, the failure detail otherwise.
    pub error: Option<String>,
    /// Tasks refreshed, when the daemon reported a count.
    pub synced_count: Option<i64>,
}

#[cfg(test)]
mod tests;
