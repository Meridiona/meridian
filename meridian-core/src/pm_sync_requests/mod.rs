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
    /// The sequence number this claim covers - pass it back to [`complete`].
    ///
    /// Captured at claim time on purpose: a request arriving DURING the sync bumps
    /// `seq` past this value, so it stays pending and gets its own sync rather than
    /// being silently marked done by work that started before it was asked for.
    pub seq: i64,
}

// Every query below spells "nothing has completed yet" as
// `COALESCE(completed_seq, 0)` and "pending" as `seq > COALESCE(completed_seq, 0)`.
// `seq` starts at 1, so 0 is unreachable as a real watermark. It lives inline in the
// SQL rather than as a Rust constant because it cannot be interpolated into a query
// string without giving up the compile-time-checked literal.

/// Ask the daemon to sync PM tasks. Idempotent and coalescing: repeated calls
/// collapse into the single pending row rather than queueing, so opening the
/// dashboard ten times means "a sync is wanted", not ten syncs.
///
/// `mode` **escalates only**. A `Force` landing on a pending `Gated` upgrades it,
/// because a user who just connected a tracker must not have that downgraded by a
/// passing window focus; a `Gated` landing on a pending `Force` leaves the `Force`
/// intact.
///
/// # Returns the sequence number to wait on
///
/// Pass the returned `seq` to [`outcome`]. It is what makes concurrent producers
/// safe, and it replaces the previous design where a new request cleared the
/// completion stamps outright.
///
/// Clearing them looked right - the row should represent work still to do - but it
/// destroyed the *answer* to a request already in flight, and the tracker-connect
/// flow always has several in flight at once (`oauth_connected`,
/// `token_connected`, and the user's own "Sync now", within a few seconds). The
/// completion of an earlier sync then matched nothing, the work was redone, and
/// every waiter timed out reporting failure for a sync that had in fact succeeded.
///
/// So completion is now a **watermark**, never cleared: this only bumps `seq` and
/// re-opens the claim. A holder of seq N is satisfied by any `completed_seq >= N`.
///
/// `reason` is a producer tag for tracing only (`"plan_or_picker"`,
/// `"token_connected"`). Never pass user content - it is read back into logs.
#[tracing::instrument(skip(pool))]
pub async fn request(
    pool: &SqlitePool,
    provider: &str,
    mode: SyncMode,
    reason: &str,
) -> Result<i64> {
    // A transaction, not `RETURNING`: the upsert and the read-back of `seq` must be
    // atomic (a concurrent producer bumping `seq` in between would hand this caller
    // a number it never wrote, making it wait on somebody else's sync), and
    // `RETURNING` on an upsert needs SQLite 3.35+, which is not worth depending on
    // when the SQLCipher build is the thing supplying the library.
    let mut tx = pool
        .begin()
        .await
        .context("opening a PM sync request transaction")?;

    sqlx::query(
        "INSERT INTO pm_sync_requests
             (provider, mode, reason, requested_at,
              claimed_at, completed_at, error, synced_count, seq, completed_seq)
         VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
              NULL, NULL, NULL, NULL, 1, NULL)
         ON CONFLICT(provider) DO UPDATE SET
             -- The whole point: a new request is a new sequence number, so the
             -- outcome of whatever is already running stays attributable to it.
             seq = pm_sync_requests.seq + 1,
             -- Escalate to 'force', never back down from it while still pending.
             --
             -- The pendingness half is load-bearing: the row is kept after a sync
             -- finishes (so \"Sync now\" can read its result), so without it a SPENT
             -- 'force' would be inherited forever and every later gated request would
             -- silently escalate. One tracker connect would then make every planner
             -- open bypass the staleness gate and hit the provider for real -
             -- reinstating the constant polling this whole design removes, and
             -- multiplying exactly the token refreshes it exists to reduce.
             mode = CASE
                 WHEN excluded.mode = 'force'
                   OR (pm_sync_requests.mode = 'force'
                       AND pm_sync_requests.seq > COALESCE(pm_sync_requests.completed_seq, 0))
                      THEN 'force'
                 ELSE excluded.mode
             END,
             reason       = excluded.reason,
             requested_at = excluded.requested_at,
             -- Re-open the claim so the watcher sees pending work, but do NOT touch
             -- completed_at / completed_seq / error / synced_count: those describe
             -- the last sync that actually ran, and erasing them is what lost
             -- outcomes. `seq` above is what marks this as new work.
             claimed_at   = NULL",
    )
    .bind(provider)
    .bind(mode.as_str())
    .bind(reason)
    .execute(&mut *tx)
    .await
    .context("writing a PM sync request")?;

    let seq: i64 = sqlx::query_scalar("SELECT seq FROM pm_sync_requests WHERE provider = ?")
        .bind(provider)
        .fetch_one(&mut *tx)
        .await
        .context("reading back the PM sync request sequence")?;

    tx.commit().await.context("committing a PM sync request")?;

    tracing::debug!(
        provider,
        mode = mode.as_str(),
        reason,
        seq,
        "PM sync requested"
    );
    Ok(seq)
}

/// Is there work to claim? A pure READ, so an idle consumer touches no locks.
///
/// # Why this exists rather than just calling [`claim`]
///
/// [`claim`] is an `UPDATE`, and SQLite opens a write transaction and takes a
/// RESERVED lock to evaluate one even when it matches no rows. The daemon's watcher
/// ticks every 2 s forever, so calling `claim` unconditionally meant **~43,000 write
/// transactions a day on an idle machine** - work that did not exist before this
/// outbox, on a file a second process also writes. Worse than the cost: it made
/// every daemon kill far more likely to land while a write transaction was open,
/// which is the profile behind the `-shm` desync that wedged writes on
/// 1.91.0-staging.2 with `(code: 11) database disk image is malformed` on a database
/// that was provably healthy.
///
/// Gating on this read takes an idle daemon's write load to zero. WAL readers do not
/// take the write lock, so a tick that finds nothing is genuinely free.
///
/// **Advisory only.** A `true` here can go stale before [`claim`] runs, and that is
/// fine: `claim` is still the conditional `UPDATE` that decides, so exclusivity is
/// unchanged and a lost race just yields `None`. A `false` that was wrong costs one
/// tick of latency.
pub async fn has_pending(pool: &SqlitePool, provider: &str) -> Result<bool> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM pm_sync_requests
          WHERE provider = ?
            AND claimed_at IS NULL
            AND seq > COALESCE(completed_seq, 0)",
    )
    .bind(provider)
    .fetch_optional(pool)
    .await
    .context("checking for a pending PM sync request")?;
    Ok(found.is_some())
}

/// Claim the pending request for `provider`, if there is one, marking it in-flight
/// so a second watcher tick can't pick up the same work.
///
/// The claim is a conditional UPDATE (`WHERE claimed_at IS NULL AND <pending>`)
/// rather than a read-then-write, so two daemons racing on the same file - which the
/// single-instance guard makes unlikely but not impossible during a restart overlap -
/// cannot both claim it. SQLite serialises the statement, so exactly one sees a
/// non-zero `rows_affected`.
///
/// Pending is `seq > COALESCE(completed_seq, 0)`, not `completed_at IS NULL`: the
/// completion stamps are a watermark now and are never cleared, so the only thing
/// that makes a row claimable again is [`request`] bumping `seq` past it.
///
/// Reads `seq` back inside the same transaction as the claim, so the value handed to
/// [`complete`] is exactly the one this claim covers even if a producer bumps it a
/// microsecond later.
#[tracing::instrument(skip(pool))]
pub async fn claim(pool: &SqlitePool, provider: &str) -> Result<Option<SyncRequest>> {
    let mut tx = pool
        .begin()
        .await
        .context("opening a PM sync claim transaction")?;

    let claimed = sqlx::query(
        "UPDATE pm_sync_requests
            SET claimed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
          WHERE provider = ?
            AND claimed_at IS NULL
            AND seq > COALESCE(completed_seq, 0)",
    )
    .bind(provider)
    .execute(&mut *tx)
    .await
    .context("claiming a PM sync request")?;

    if claimed.rows_affected() == 0 {
        // Nothing to do - roll back rather than commit an empty transaction.
        return Ok(None);
    }

    let row: Option<(String, String, i64)> =
        sqlx::query_as("SELECT mode, reason, seq FROM pm_sync_requests WHERE provider = ?")
            .bind(provider)
            .fetch_optional(&mut *tx)
            .await
            .context("reading the claimed PM sync request")?;

    tx.commit().await.context("committing a PM sync claim")?;

    Ok(row.map(|(mode, reason, seq)| SyncRequest {
        provider: provider.to_string(),
        mode: SyncMode::from_str_or_gated(&mode),
        reason,
        seq,
    }))
}

/// Record the outcome of a serviced request in place. The row is deliberately kept
/// rather than deleted so `"Sync now"` can read a real result without holding the
/// credential, and so support has a content-free view of the last attempt.
///
/// `seq` is the value from the [`SyncRequest`] this call is reporting on. It records
/// the watermark, and it is also the guard.
///
/// # Why the guard is the sequence and NOT `claimed_at`
///
/// It used to be `claimed_at IS NOT NULL AND completed_at IS NULL`, chosen because
/// [`request`] nulled `claimed_at` and so a request arriving mid-sync would make this
/// UPDATE match nothing - deliberately, to stop an older sync's outcome marking a
/// newer request as done.
///
/// The intent was right and the mechanism was a data-loss bug. Discarding the write
/// also discarded the answer the *original* waiter was polling for, and since
/// tracker-connect fires several requests seconds apart, that was the normal case
/// rather than the edge one: the sync succeeded, the outcome was dropped, the work
/// was repeated, and the user was shown a failure.
///
/// Guarding on `seq > COALESCE(completed_seq, 0)` keeps the protection and loses the
/// bug. The watermark only ever moves forward, so this is idempotent and a late
/// duplicate cannot roll it back; a newer request has a HIGHER `seq` than the one
/// being completed, so it stays pending and gets its own sync. `claimed_at` is
/// cleared here rather than depended on, which is what makes that next sync
/// claimable immediately.
#[tracing::instrument(skip(pool))]
pub async fn complete(
    pool: &SqlitePool,
    provider: &str,
    seq: i64,
    synced_count: Option<i64>,
    error: Option<&str>,
) -> Result<()> {
    let res = sqlx::query(
        "UPDATE pm_sync_requests
            SET completed_at  = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                completed_seq = ?,
                claimed_at    = NULL,
                error         = ?,
                synced_count  = ?
          WHERE provider = ?
            AND ? > COALESCE(completed_seq, 0)",
    )
    .bind(seq)
    .bind(error)
    .bind(synced_count)
    .bind(provider)
    .bind(seq)
    .execute(pool)
    .await
    .context("completing a PM sync request")?;

    if res.rows_affected() == 0 {
        // Not an error: a duplicate or out-of-order completion for a watermark that
        // has already moved past `seq`. Worth a line, because it should be rare and
        // a steady stream of them would mean two consumers are running.
        tracing::debug!(
            provider,
            seq,
            "PM sync completion ignored - the watermark is already at or past it"
        );
    }
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
            AND seq > COALESCE(completed_seq, 0)",
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

/// The outcome for the request the caller wrote, for a producer that wants to show
/// one ("Sync now"). `None` while that request is still pending or in flight, so a
/// caller can poll this until it turns `Some`.
///
/// `want_seq` is the value [`request`] returned. `Some` means `completed_seq >=
/// want_seq` - i.e. a sync that finished at or after the caller's request, which is
/// what makes overlapping producers safe: two waiters can be satisfied by one sync,
/// and neither can be handed the result of a sync that finished BEFORE it asked.
///
/// Passing a stale `want_seq` (from a previous request) therefore returns a result
/// immediately, by design - it has genuinely been satisfied.
pub async fn outcome(
    pool: &SqlitePool,
    provider: &str,
    want_seq: i64,
) -> Result<Option<SyncOutcome>> {
    let row: Option<(Option<i64>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT completed_seq, error, synced_count FROM pm_sync_requests WHERE provider = ?",
    )
    .bind(provider)
    .fetch_optional(pool)
    .await
    .context("reading the PM sync outcome")?;

    Ok(match row {
        Some((Some(completed_seq), error, synced_count)) if completed_seq >= want_seq => {
            Some(SyncOutcome {
                error,
                synced_count,
            })
        }
        // No row, nothing completed yet, or the watermark has not reached this
        // caller's request.
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
