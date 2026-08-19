//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The DB half of `/api/integrations` — per-provider last sync error from
//! `pm_sync_state`. The env/OAuth-file half (which providers are *configured*)
//! is filesystem logic and lives tray-side, not in this DB crate.

use crate::SqlitePool;
use anyhow::Context;
use std::collections::BTreeMap;
use tracing::Instrument;

/// Delete all rows for `provider` from `pm_task_embeddings`, `pm_tasks`, and
/// `pm_sync_state`. Called after credentials are stripped so stale tasks
/// disappear from the UI immediately rather than lingering until the next sync.
/// Errors are propagated; the caller should treat this as best-effort (warn on
/// failure, but do not block the disconnect).
#[tracing::instrument(skip(pool))]
pub async fn clear_provider_tasks(pool: &SqlitePool, provider: &str) -> anyhow::Result<()> {
    // Embeddings first — they FK-reference pm_tasks.task_key.
    sqlx::query(
        "DELETE FROM pm_task_embeddings \
         WHERE task_key IN (SELECT task_key FROM pm_tasks WHERE provider = ?)",
    )
    .bind(provider)
    .execute(pool)
    .instrument(tracing::debug_span!(
        "integrations.delete.pm_task_embeddings"
    ))
    .await
    .context("clear pm_task_embeddings")?;

    sqlx::query("DELETE FROM pm_tasks WHERE provider = ?")
        .bind(provider)
        .execute(pool)
        .instrument(tracing::debug_span!("integrations.delete.pm_tasks"))
        .await
        .context("clear pm_tasks")?;

    sqlx::query("DELETE FROM pm_sync_state WHERE provider = ?")
        .bind(provider)
        .execute(pool)
        .instrument(tracing::debug_span!("integrations.delete.pm_sync_state"))
        .await
        .context("clear pm_sync_state")?;

    tracing::info!(provider, "cleared provider tasks from DB");
    Ok(())
}

/// provider → last_error for providers whose most recent sync failed.
///
/// **Propagates** on a DB without `pm_sync_state` yet (daemon not initialised).
/// An earlier version of this comment claimed it returned empty in that case;
/// it does not, and callers that need tolerance have to supply it themselves —
/// [`sync_state`] below is the one that genuinely degrades, and documents why.
#[tracing::instrument(skip(pool))]
pub async fn sync_errors(pool: &SqlitePool) -> anyhow::Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as::<_, (String, String)>(
        "SELECT provider, last_error FROM pm_sync_state WHERE last_error IS NOT NULL",
    )
    .fetch_all(pool)
    .instrument(tracing::debug_span!("integrations.read.pm_sync_state"))
    .await
    .context("integrations: fetch pm_sync_state")?;
    tracing::debug!(rows = rows.len(), "integrations.read.pm_sync_state");
    let map: BTreeMap<String, String> = rows.into_iter().collect();
    tracing::info!(providers = map.len(), "integrations sync-errors served");
    Ok(map)
}

/// One tracker's sync bookkeeping, as recorded by the daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncState {
    /// When the daemon last completed a sync for this provider, if ever.
    pub last_synced_at: Option<String>,
    /// The most recent sync failure, if the last attempt failed. Carries the
    /// provider's raw error body, so it must never be shipped anywhere — only
    /// its PRESENCE is safe to report.
    pub last_error: Option<String>,
}

/// provider → [`SyncState`] for every tracker the daemon has ever synced.
///
/// A provider absent from the result has no `pm_sync_state` row, which means
/// the daemon has never completed a sync for it — NOT that it is disconnected.
/// Connectedness is filesystem state (`.env` keys plus OAuth token files) and
/// is resolved tray-side; see `tray/src-tauri/src/commands/integrations.rs`.
///
/// **Degrades to empty** when `pm_sync_state` does not exist yet, unlike its
/// sibling [`sync_errors`] which propagates. That divergence is deliberate and
/// follows the caller: this exists for the analytics health snapshot, which is
/// documented never to fail and whose whole purpose is to report "unknown"
/// rather than block the event it decorates. The `usage_rollup` reader takes
/// the opposite stance for the same underlying condition — there a missing
/// table means a counter is genuinely zero, while a table that exists but
/// fails to read is a real error worth retrying.
///
/// # Who calls this
/// `tray/src-tauri/src/analytics/health.rs` — to classify each connected
/// tracker as ok / stale / error / never-synced.
#[tracing::instrument(skip(pool))]
pub async fn sync_state(pool: &SqlitePool) -> BTreeMap<String, SyncState> {
    let rows: Vec<(String, Option<String>, Option<String>)> = match sqlx::query_as(
        "SELECT provider, last_synced_at, last_error FROM pm_sync_state",
    )
    .fetch_all(pool)
    .instrument(tracing::debug_span!("integrations.read.pm_sync_state_all"))
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Missing table (pre-migration) or a transient read error. Either
            // way the snapshot reports "unknown" for every provider rather
            // than failing; see the doc above.
            tracing::debug!(error = %e, "integrations: sync_state unavailable — reporting unknown");
            return BTreeMap::new();
        }
    };
    tracing::debug!(rows = rows.len(), "integrations.read.pm_sync_state_all");

    rows.into_iter()
        .map(|(provider, last_synced_at, last_error)| {
            (
                provider,
                SyncState {
                    last_synced_at,
                    last_error,
                },
            )
        })
        .collect()
}
