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

/// provider → last_error for providers whose most recent sync failed. Tolerates
/// a DB without the table yet (daemon not initialised) by returning empty.
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
