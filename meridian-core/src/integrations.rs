//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The DB half of `/api/integrations` — per-provider last sync error from
//! `pm_sync_state`. The env/OAuth-file half (which providers are *configured*)
//! is filesystem logic and lives tray-side, not in this DB crate.

use crate::SqlitePool;
use anyhow::Context;
use std::collections::BTreeMap;

/// provider → last_error for providers whose most recent sync failed. Tolerates
/// a DB without the table yet (daemon not initialised) by returning empty.
#[tracing::instrument(skip(pool))]
pub async fn sync_errors(pool: &SqlitePool) -> anyhow::Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as::<_, (String, String)>(
        "SELECT provider, last_error FROM pm_sync_state WHERE last_error IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .context("integrations: fetch pm_sync_state")?;
    Ok(rows.into_iter().collect())
}
