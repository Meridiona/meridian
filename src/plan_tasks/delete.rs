//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Deleting a personal task the user created by mistake — the daemon half of
//! "there must be an option to delete a task I created".
//!
//! # Personal tasks only
//! Unlike [`super::edit`], there is no tracker branch here. A tracker-synced ticket
//! is owned by the tracker; Meridian only mirrors it, and removing a ticket from a
//! board is the tracker's own delete, not something this app does on the user's
//! behalf. Pointed at anything other than a `provider = 'local'` row, this refuses
//! rather than silently doing something destructive to someone's real board.
//!
//! # Who calls this
//! [`super::cli`]'s `plan-task-delete` → the tray's `delete_plan_task` → the plan's
//! task card / detail dialog.
//!
//! # Related
//! - [`meridian_core::task_create::delete_local_task`] — the actual write. Soft: it
//!   sets `deleted_at` rather than removing the row (that fn's doc has the why), so
//!   every task listing simply stops showing it — nothing else has to be told.

use anyhow::{Context, Result};
use meridian_core::task_create;
use serde::Serialize;
use sqlx::SqlitePool;

/// The outcome of a delete, serialized to the CLI's one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteResult {
    pub task_key: String,
    pub deleted: bool,
}

/// Delete `task_key` if — and only if — it is a personal task.
#[tracing::instrument(skip(pool))]
pub async fn delete(pool: &SqlitePool, task_key: &str) -> Result<DeleteResult> {
    let provider = task_create::provider_of(pool, task_key)
        .await?
        .with_context(|| format!("no task {task_key} - it may already be gone"))?;
    if provider != task_create::LOCAL_PROVIDER {
        anyhow::bail!(
            "{task_key} is a {provider} ticket - only a personal task you created \
             yourself can be deleted here"
        );
    }

    let now = chrono::Utc::now().to_rfc3339();
    let deleted = task_create::delete_local_task(pool, task_key, &now)
        .await
        .context("deleting the personal task")?;
    tracing::info!(
        task_key,
        deleted,
        "plan_task: personal task delete requested"
    );
    Ok(DeleteResult {
        task_key: task_key.to_string(),
        deleted,
    })
}
