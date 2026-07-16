//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Azure DevOps connector DB helpers — prune stale rows, stamp sync state.
//!
//! Split from the connector root purely for file size.
//!
//! # Who calls this
//! [`super::force_refresh`], via `db::{prune, stamp_sync, stamp_error}`.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Delete `pm_tasks` rows no longer returned by the active-task fetch (closed,
/// reassigned, etc.) — EXCEPT a task_key that has worklog history
/// (`pm_worklogs`), which is kept forever so a completed work item's title
/// never disappears from the timeline once it's closed.
pub(super) async fn prune(pool: &SqlitePool, kept_keys: &[String]) -> Result<()> {
    if kept_keys.is_empty() {
        sqlx::query(
            "DELETE FROM pm_task_embeddings WHERE task_key IN \
             (SELECT task_key FROM pm_tasks WHERE provider = 'azure_devops')",
        )
        .execute(pool)
        .await
        .context("pruning all azure_devops pm_task_embeddings")?;
        sqlx::query(
            "DELETE FROM pm_tasks WHERE provider = 'azure_devops' \
             AND task_key NOT IN (SELECT DISTINCT task_key FROM pm_worklogs)",
        )
        .execute(pool)
        .await
        .context("pruning all azure_devops pm_tasks")?;
        // No active tasks: every worklog-retained row is off the board now.
        flag_retained_offboard(pool, kept_keys).await?;
        return Ok(());
    }

    let placeholders = kept_keys.iter().map(|_| "?").collect::<Vec<_>>().join(", ");

    let sql_embed = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks \
          WHERE provider = 'azure_devops' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&sql_embed);
    for k in kept_keys {
        q = q.bind(k);
    }
    q.execute(pool)
        .await
        .context("pruning stale azure_devops pm_task_embeddings")?;

    let sql_tasks = format!(
        "DELETE FROM pm_tasks \
         WHERE provider = 'azure_devops' AND task_key NOT IN ({placeholders}) \
         AND task_key NOT IN (SELECT DISTINCT task_key FROM pm_worklogs)"
    );
    let mut q2 = sqlx::query(&sql_tasks);
    for k in kept_keys {
        q2 = q2.bind(k);
    }
    let result = q2
        .execute(pool)
        .await
        .context("pruning stale azure_devops pm_tasks")?;

    if result.rows_affected() > 0 {
        tracing::info!(
            removed = result.rows_affected(),
            "pruned stale azure_devops tasks"
        );
    }
    // Worklog-retained rows the DELETE kept but that are no longer in the active
    // fetch (closed, reassigned) must leave the board.
    flag_retained_offboard(pool, kept_keys).await?;
    Ok(())
}

/// Stamp worklog-retained azure_devops rows that fell out of the active fetch as
/// off-board — thin wrapper over the shared [`super::super::mark_retained_offboard`].
async fn flag_retained_offboard(pool: &SqlitePool, kept_keys: &[String]) -> Result<()> {
    let flagged =
        crate::intelligence::providers::mark_retained_offboard(pool, "azure_devops", kept_keys)
            .await?;
    if flagged > 0 {
        tracing::info!(flagged, "flagged retained azure_devops tasks off-board");
    }
    Ok(())
}

pub(super) async fn stamp_sync(pool: &SqlitePool) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES ('azure_devops', ?, NULL)
         ON CONFLICT(provider) DO UPDATE SET
           last_synced_at = excluded.last_synced_at,
           last_error     = NULL",
    )
    .bind(&now)
    .execute(pool)
    .await
    .context("updating azure_devops sync state")?;
    let _ = crate::notices::clear(pool, "pm.azure_devops").await;
    Ok(())
}

pub(super) async fn stamp_error(pool: &SqlitePool, error: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES ('azure_devops', ?, ?)
         ON CONFLICT(provider) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(&now)
    .bind(error)
    .execute(pool)
    .await
    .context("recording azure_devops sync error")?;
    let _ = crate::notices::raise(
        pool,
        "pm.azure_devops",
        "error",
        "Azure DevOps sync failing",
        error,
        Some("Set AZURE_DEVOPS_PAT in .env"),
    )
    .await;
    Ok(())
}
