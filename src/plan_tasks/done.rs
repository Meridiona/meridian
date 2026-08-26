//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Marking a task done / not-done — from either side of the fence, through ONE entry
//! point.
//!
//! # The one branch
//! Same split, and the same reasoning, as [`super::edit`]: a personal task's `pm_tasks`
//! row IS the task, so we flip `is_terminal` directly; a board ticket's row is a mirror,
//! so the transition goes to the tracker via [`crate::intelligence::ticket_update`]'s
//! `close`/`reopen` and a force-sync pulls the truth back.
//!
//! This routing lives HERE rather than inside `ticket_update` for the reason spelled
//! out in [`super::edit`]'s header — that module resolves credentials and dispatches
//! HTTP, and a `'local'` arm there would have none of those. Before this module
//! existed the plan's checkbox called the tracker path unconditionally, so ticking a
//! personal task failed with `provider "local" is not configured`.
//!
//! # Who calls this
//! [`super::cli`]'s `plan-task-done` → the tray's `set_plan_task_done` → the "today's
//! focus" checkbox in `ui/components/timeline/OverviewPanel.tsx`.
//!
//! # Related
//! - [`meridian_core::task_create::set_local_terminal`] — the personal-task writer.
//! - [`super::edit`] — the same routing for a task's title/description.

use anyhow::{Context, Result};
use meridian_core::task_create;
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

use crate::config::Config;
use crate::intelligence::sync_delegate::Delegation;
use crate::intelligence::ticket_update::{self, ApplyStatus};
use crate::plan_tasks::edit::EditResult;

/// Mark `task_key` done (`done = true`) or not-done, wherever it lives.
///
/// Returns the same `applied` / `redirected` shape as [`super::edit::edit`]: some
/// trackers (Trello, Azure DevOps) have no reliable done/not-done mapping and hand
/// back a browse URL instead of writing. A personal task is always `applied`.
#[tracing::instrument(skip(pool, config))]
pub async fn set_done(
    pool: &SqlitePool,
    config: &Config,
    task_key: &str,
    done: bool,
) -> Result<EditResult> {
    let span = tracing::info_span!(
        "plan_task.done",
        task_key,
        done,
        provider = Empty,
        personal = Empty,
        status = Empty,
    );
    async move {
        let provider = task_create::provider_of(pool, task_key)
            .await?
            .with_context(|| format!("no task {task_key} - it may have left the board"))?;
        tracing::Span::current().record("provider", provider.as_str());

        if provider == task_create::LOCAL_PROVIDER {
            tracing::Span::current().record("personal", true);
            return set_done_personal(pool, task_key, done).await;
        }
        tracing::Span::current().record("personal", false);
        set_done_on_tracker(pool, config, task_key, &provider, done).await
    }
    .instrument(span)
    .await
}

/// Our row is the task — write it.
async fn set_done_personal(pool: &SqlitePool, task_key: &str, done: bool) -> Result<EditResult> {
    let now = chrono::Utc::now().to_rfc3339();
    let changed = task_create::set_local_terminal(pool, task_key, done, &now)
        .await
        .context("updating the personal task's status")?;
    if !changed {
        anyhow::bail!("could not update {task_key}");
    }
    tracing::Span::current().record("status", "applied");
    tracing::info!(done, "plan_task: personal task status set");
    Ok(EditResult {
        task_key: task_key.to_string(),
        provider: task_create::LOCAL_PROVIDER.to_string(),
        status: "applied".to_string(),
        browse_url: None,
        reason: None,
    })
}

/// The tracker owns the task — transition it there, then pull the truth back into our
/// mirror so the checkbox settles without waiting out the sync gate.
async fn set_done_on_tracker(
    pool: &SqlitePool,
    config: &Config,
    task_key: &str,
    provider: &str,
    done: bool,
) -> Result<EditResult> {
    let field = if done { "close" } else { "reopen" };
    let res = ticket_update::apply(config, provider, task_key, field, "")
        .await
        .with_context(|| format!("applying {field} on {provider}"))?;

    if let ApplyStatus::Redirected { browse_url, reason } = &res.status {
        tracing::Span::current().record("status", "redirected");
        tracing::info!(field, "plan_task: status change redirected to the tracker");
        return Ok(EditResult {
            task_key: task_key.to_string(),
            provider: provider.to_string(),
            status: "redirected".to_string(),
            browse_url: Some(browse_url.clone()),
            reason: Some(reason.clone()),
        });
    }

    // Best-effort — the tracker has already accepted the transition, so a sync hiccup
    // must not read as a failed toggle.
    //
    // Delegated to the daemon (`intelligence::sync_delegate`), which is the only process
    // that may spend the rotating Jira OAuth token. Nothing below reads `pm_tasks`, so
    // this does not wait for the outcome - the request row is written and the daemon
    // picks it up within ~2 s, well before the user's next board read.
    match crate::intelligence::sync_delegate::sync_after_write(pool, config, "plan-task-done").await
    {
        Delegation::Synced { .. } => {}
        Delegation::Failed { error } => {
            tracing::warn!(%error, "plan_task: post-toggle sync failed - the change still landed");
        }
        Delegation::Pending => {
            tracing::warn!("plan_task: post-toggle sync still running - the change still landed");
        }
    }
    tracing::Span::current().record("status", "applied");
    tracing::info!(field, "plan_task: tracker task status set");
    Ok(EditResult {
        task_key: task_key.to_string(),
        provider: provider.to_string(),
        status: "applied".to_string(),
        browse_url: None,
        reason: None,
    })
}
