//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Editing a task's title/description — from either side of the fence, through ONE
//! entry point.
//!
//! # The one branch
//! A personal task's `pm_tasks` row IS the task, so we write it directly. A board
//! ticket's row is a mirror — the tracker is authoritative, and writing our copy would
//! be silently clobbered by the next sync's UPSERT — so it goes through
//! [`crate::intelligence::ticket_update`] and then a force-sync pulls the truth back.
//!
//! This routing lives HERE, not inside `ticket_update`, on purpose. That module's
//! whole contract is "resolve credentials from `Config`, dispatch to an HTTP client";
//! a `'local'` arm there would have no config, no credentials and no HTTP, would need
//! a `&SqlitePool` threaded through five provider arms that never use it, and would
//! make `Close`/`AssignMe`/`Priority` meaningless for a personal task. This module
//! already holds BOTH the pool and the config, so it is the natural seam — the caller
//! still sees one function and never branches.
//!
//! # Who calls this
//! [`super::cli`]'s `plan-task-edit` → the tray's `edit_plan_task` → the plan's task
//! detail dialog.
//!
//! # Related
//! - [`meridian_core::task_create`] — the personal-task writer (scoped to `'local'`).
//! - [`crate::intelligence::ticket_update`] — the tracker-side writer, kept pure.

use anyhow::{Context, Result};
use meridian_core::task_create;
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

use crate::config::Config;
use crate::intelligence::sync_delegate::Delegation;
use crate::intelligence::ticket_update::{self, ApplyStatus};

/// The outcome of an edit, serialized to the CLI's one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct EditResult {
    pub task_key: String,
    pub provider: String,
    /// `applied` — it landed; `redirected` — this tracker has no API for it, so the
    /// UI should offer the ticket in the browser instead.
    pub status: String,
    /// Set only on `redirected`.
    pub browse_url: Option<String>,
    /// Set only on `redirected` — why, phrased for the user.
    pub reason: Option<String>,
}

/// Rewrite `task_key`'s title and/or description. `None` leaves a field alone.
///
/// A tracker edit is TWO writes (title and body are separate fields on every
/// provider), so a partial failure is possible: the title can land and the
/// description fail. We apply the title first and let an error surface — the user
/// sees which half took effect when the dialog refreshes, rather than us silently
/// swallowing it or rolling back a write the tracker already accepted.
#[tracing::instrument(skip(pool, config, title, description))]
pub async fn edit(
    pool: &SqlitePool,
    config: &Config,
    task_key: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<EditResult> {
    let span = tracing::info_span!(
        "plan_task.edit",
        task_key,
        provider = Empty,
        personal = Empty,
        status = Empty,
    );
    async move {
        if title.is_none() && description.is_none() {
            anyhow::bail!("nothing to change - pass a title and/or a description");
        }
        if let Some(t) = title {
            if t.trim().is_empty() {
                anyhow::bail!("a task's title cannot be empty");
            }
        }

        let provider = task_create::provider_of(pool, task_key)
            .await?
            .with_context(|| format!("no task {task_key} - it may have left the board"))?;
        tracing::Span::current().record("provider", provider.as_str());

        if provider == task_create::LOCAL_PROVIDER {
            tracing::Span::current().record("personal", true);
            return edit_personal(pool, task_key, title, description).await;
        }
        tracing::Span::current().record("personal", false);
        edit_on_tracker(pool, config, task_key, &provider, title, description).await
    }
    .instrument(span)
    .await
}

/// Our row is the task — write it.
async fn edit_personal(
    pool: &SqlitePool,
    task_key: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<EditResult> {
    let now = chrono::Utc::now().to_rfc3339();
    let changed = task_create::update_task_text(pool, task_key, title, description, &now)
        .await
        .context("updating the personal task")?;
    if !changed {
        anyhow::bail!("could not update {task_key}");
    }
    tracing::Span::current().record("status", "applied");
    tracing::info!("plan_task: personal task edited");
    Ok(EditResult {
        task_key: task_key.to_string(),
        provider: task_create::LOCAL_PROVIDER.to_string(),
        status: "applied".to_string(),
        browse_url: None,
        reason: None,
    })
}

/// The tracker owns the task — write it there, then pull the truth back into our
/// mirror so the card reflects the edit without waiting out the sync gate.
async fn edit_on_tracker(
    pool: &SqlitePool,
    config: &Config,
    task_key: &str,
    provider: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<EditResult> {
    let mut last = None;
    for (field, value) in [("summary", title), ("description", description)] {
        let Some(v) = value else { continue };
        let res = ticket_update::apply(config, provider, task_key, field, v)
            .await
            .with_context(|| format!("updating the {field} on {provider}"))?;
        // A redirect means this provider has no API for the field — report it as-is
        // and stop; the UI offers the ticket in the browser.
        if let ApplyStatus::Redirected { browse_url, reason } = &res.status {
            tracing::Span::current().record("status", "redirected");
            tracing::info!(field, "plan_task: edit redirected to the tracker");
            return Ok(EditResult {
                task_key: task_key.to_string(),
                provider: provider.to_string(),
                status: "redirected".to_string(),
                browse_url: Some(browse_url.clone()),
                reason: Some(reason.clone()),
            });
        }
        last = Some(res);
    }

    if last.is_some() {
        // Reflect the applied write back into our mirror (the `ticket-update` CLI
        // does exactly this after an Applied write). Best-effort — the tracker has
        // already accepted it, so a sync hiccup must not read as a failed edit.
        //
        // Delegated to the daemon (`intelligence::sync_delegate`), the only process that
        // may spend the rotating Jira OAuth token. Nothing below reads `pm_tasks`, so the
        // outcome is not awaited.
        match crate::intelligence::sync_delegate::sync_after_write(pool, config, "plan-task-edit")
            .await
        {
            Delegation::Synced { .. } => {}
            Delegation::Failed { error } => {
                tracing::warn!(%error, "plan_task: post-edit sync failed - the edit still landed");
            }
            Delegation::Pending => {
                tracing::warn!("plan_task: post-edit sync still running - the edit still landed");
            }
        }
    }
    tracing::Span::current().record("status", "applied");
    tracing::info!("plan_task: tracker task edited");
    Ok(EditResult {
        task_key: task_key.to_string(),
        provider: provider.to_string(),
        status: "applied".to_string(),
        browse_url: None,
        reason: None,
    })
}
