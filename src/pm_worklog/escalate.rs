//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Escalate a PERSONAL (`provider = 'local'`) task onto a real tracker.
//!
//! # What this is
//! A personal task's day-work is auto-logged onto the task's own row
//! ([`super::generate::auto_log_local_targets`]) - a purely local record. When the
//! user decides that work belongs on their real board, they escalate it from the
//! task dialog, two ways:
//! - [`escalate_create`] - CREATE a brand-new ticket on the connected tracker,
//!   seeded with the personal task's own title/description, and post its logged
//!   update there as the first comment.
//! - [`escalate_match`] - post the logged update as a comment onto an EXISTING
//!   real ticket the user picks.
//!
//! # The personal task is REPLACED by the real ticket (product decision)
//! Escalation GRADUATES the personal task into the real ticket - once its work
//! belongs on a tracker it is not "local" anymore, so it stops being a personal
//! task everywhere ([`graduate_local_task`]):
//! - Create-new: the personal `pm_tasks` row BECOMES the new ticket in place - its
//!   `task_key`/`provider`/`url` are rewritten (the new key doesn't exist yet, so
//!   there's no collision). Its auto-logged update rides along on the row so it's
//!   still shown.
//! - Match-existing: the real ticket already has a row, so the personal row is
//!   RETIRED (`is_terminal = 1`, hidden like any off-boarded task) and its logged
//!   update is copied onto the real row so the work still shows there.
//!
//! Either way every reference to the old local key is repointed to the real key -
//! `daily_plan` (Today's focus / plan), `day_task_worklog_targets` (the worklog
//! panel), and `day_tasks.linked_ticket` (the card) - so no surface keeps calling
//! it personal. The op is one transaction: it fully lands or not at all.
//!
//! # Reuses the ordinary write-back primitives
//! Nothing here is personal-task-specific once it reaches the tracker: it calls the
//! same [`super::create::create_ticket`] and [`super::post_comment::post_comment`]
//! the day-task worklog approve path uses, so a personal task graduates through the
//! exact provider write-back everything else does.
//!
//! # Who calls this
//! The `worklog-escalate-create` / `worklog-escalate-match` CLI subcommands
//! (`main.rs`) → the tray `escalate_personal_task_create` /
//! `escalate_personal_task_match` commands → the task dialog's escalate buttons.
//!
//! # Related
//! - [`super::generate`] - produces the auto-logged update this posts.
//! - [`meridian_core::task_detail`] - the personal task's title/description/logged text.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

use meridian_core::task_create::LOCAL_PROVIDER;

use super::generate::{browse_url, fetch_sample_task_key};
use crate::config::Config;

/// The outcome of an escalation, serialized to the CLI's one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct EscalateResult {
    /// The real ticket the personal task now points at (created or matched).
    pub linked_ticket: String,
    /// That ticket's provider (`jira`, `linear`, …).
    pub provider: String,
    /// A browse URL for the ticket, when one can be formed.
    pub browse_url: Option<String>,
    /// True when [`escalate_create`] filed a brand-new ticket (vs matched one).
    pub created: bool,
}

/// Load a personal task's `(title, description, logged_update)`, erroring if it
/// isn't a personal task or has no logged update to post. Shared pre-flight for
/// both escalate paths - there is nothing to escalate without an update.
async fn load_escalatable(pool: &SqlitePool, task_key: &str) -> Result<(String, String, String)> {
    let today = chrono::Utc::now().date_naive();
    let detail = meridian_core::task_detail::get_task_detail(pool, task_key, today)
        .await
        .context("loading the personal task")?
        .with_context(|| format!("personal task {task_key} not found"))?;
    if detail.provider != LOCAL_PROVIDER {
        bail!("{task_key} is not a personal task - it already lives on a tracker");
    }
    let update = detail
        .local_worklog_text
        .filter(|s| !s.trim().is_empty())
        .with_context(|| {
            format!("personal task {task_key} has no logged update to post - generate its worklog first")
        })?;
    Ok((detail.title, detail.description, update))
}

/// Graduate a personal task (`local_key`) into the real ticket (`real_key`) it was
/// escalated onto: after this the work is no longer a personal task anywhere. Runs
/// in ONE transaction so it fully lands or not at all.
///
/// - The `pm_tasks` identity: if `real_key` already has a row (matched an existing
///   ticket), the personal row is RETIRED (`is_terminal = 1`) and its logged update
///   copied onto the real row; otherwise (a freshly created ticket, no row yet) the
///   personal row is rewritten in place to BECOME `real_key` (keeping its logged
///   update so it's still shown).
/// - Every reference to `local_key` is repointed to `real_key`, dedup-aware against
///   each table's unique key: `daily_plan` (Today's focus), `day_task_worklog_targets`
///   (the worklog panel, whose `provider`/`browse_url` are refreshed to the real
///   tracker's), and `day_tasks.linked_ticket` (the card).
async fn graduate_local_task(
    pool: &SqlitePool,
    local_key: &str,
    real_key: &str,
    real_provider: &str,
    real_url: Option<&str>,
) -> Result<()> {
    let mut tx = pool.begin().await.context("begin graduate transaction")?;

    let real_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM pm_tasks WHERE task_key = ?")
        .bind(real_key)
        .fetch_optional(&mut *tx)
        .await
        .context("checking whether the real ticket already has a row")?;

    if real_exists.is_some() {
        // Match-existing: carry the logged update onto the real row (only if it has
        // none of its own), then retire the personal row rather than hard-delete it
        // - matching how off-boarded tasks are hidden.
        sqlx::query(
            "UPDATE pm_tasks SET \
               local_worklog_text = COALESCE(local_worklog_text, (SELECT local_worklog_text FROM pm_tasks WHERE task_key = ?1)), \
               local_worklog_posted_at = COALESCE(local_worklog_posted_at, (SELECT local_worklog_posted_at FROM pm_tasks WHERE task_key = ?1)) \
             WHERE task_key = ?2",
        )
        .bind(local_key)
        .bind(real_key)
        .execute(&mut *tx)
        .await
        .context("copying the logged update onto the real ticket")?;
        sqlx::query("UPDATE pm_tasks SET is_terminal = 1 WHERE task_key = ?")
            .bind(local_key)
            .execute(&mut *tx)
            .await
            .context("retiring the graduated personal task")?;
    } else {
        // Create-new: the new key has no row yet, so the personal row becomes it.
        sqlx::query(
            "UPDATE pm_tasks SET task_key = ?, provider = ?, url = COALESCE(?, url), is_terminal = 0 \
             WHERE task_key = ?",
        )
        .bind(real_key)
        .bind(real_provider)
        .bind(real_url)
        .bind(local_key)
        .execute(&mut *tx)
        .await
        .context("rewriting the personal task into the new ticket")?;
    }

    // daily_plan PK (plan_date, task_key): move rows, dropping any that would
    // collide with a plan row the real ticket already has that day.
    sqlx::query("UPDATE OR IGNORE daily_plan SET task_key = ? WHERE task_key = ?")
        .bind(real_key)
        .bind(local_key)
        .execute(&mut *tx)
        .await
        .context("repointing the plan to the real ticket")?;
    sqlx::query("DELETE FROM daily_plan WHERE task_key = ?")
        .bind(local_key)
        .execute(&mut *tx)
        .await
        .context("clearing leftover plan rows")?;

    // day_task_worklog_targets PK (day_local, task_id, task_key): move rows and
    // refresh provider + browse_url to the real tracker's.
    sqlx::query(
        "UPDATE OR IGNORE day_task_worklog_targets \
         SET task_key = ?, provider = ?, browse_url = COALESCE(?, browse_url) \
         WHERE task_key = ?",
    )
    .bind(real_key)
    .bind(real_provider)
    .bind(real_url)
    .bind(local_key)
    .execute(&mut *tx)
    .await
    .context("repointing worklog targets to the real ticket")?;
    sqlx::query("DELETE FROM day_task_worklog_targets WHERE task_key = ?")
        .bind(local_key)
        .execute(&mut *tx)
        .await
        .context("clearing leftover worklog targets")?;

    // day_tasks.linked_ticket has no unique constraint - a plain repoint.
    sqlx::query("UPDATE day_tasks SET linked_ticket = ? WHERE linked_ticket = ?")
        .bind(real_key)
        .bind(local_key)
        .execute(&mut *tx)
        .await
        .context("repointing the day-task card link")?;

    tx.commit().await.context("commit graduate transaction")?;
    Ok(())
}

/// Create a real ticket from a personal task and post its logged update there.
///
/// Targets the first connected tracker (the personal task has no provider of its
/// own to inherit). The ticket is filed with the task's own title/description; its
/// auto-logged update is posted as the first comment. On success the personal task
/// is kept and linked to the new ticket.
#[tracing::instrument(skip(pool, config))]
pub async fn escalate_create(
    pool: &SqlitePool,
    config: &Config,
    task_key: &str,
) -> Result<EscalateResult> {
    let span = tracing::info_span!(
        "worklog.escalate.create",
        task_key,
        provider = Empty,
        linked_ticket = Empty,
    );
    async move {
        let (title, description, update) = load_escalatable(pool, task_key).await?;
        let provider = config
            .pm_providers
            .first()
            .map(|p| p.provider_name().to_string())
            .context("no PM tracker is connected - connect one first")?;
        tracing::Span::current().record("provider", provider.as_str());

        let sample = fetch_sample_task_key(pool, &provider).await;
        let new_key = super::create::create_ticket(
            config,
            &provider,
            &title,
            &description,
            "Task",
            sample.as_deref(),
        )
        .await
        .context("creating the ticket on the tracker")?;
        tracing::Span::current().record("linked_ticket", new_key.as_str());

        // Post the logged update as the new ticket's first comment. If this fails
        // the ticket already exists, so we still link (the user can retry the
        // comment) rather than orphan a freshly-created ticket.
        if let Err(e) = super::post_comment::post_comment(config, &provider, &new_key, &update).await
        {
            tracing::warn!(task_key, new_key, error = %e, "worklog: created the ticket but posting the update failed");
        }
        let url = browse_url(config, &provider, &new_key);
        graduate_local_task(pool, task_key, &new_key, &provider, url.as_deref()).await?;

        tracing::info!(task_key, new_key, provider, "worklog: escalated personal task to a new ticket");
        Ok(EscalateResult {
            browse_url: url,
            linked_ticket: new_key,
            provider,
            created: true,
        })
    }
    .instrument(span)
    .await
}

/// Post a personal task's logged update onto an EXISTING real ticket the user
/// picked, then keep-and-link the personal task to it. The target's provider is
/// resolved from the board; a personal target is rejected (escalation is onto a
/// real tracker, not another local task).
#[tracing::instrument(skip(pool, config))]
pub async fn escalate_match(
    pool: &SqlitePool,
    config: &Config,
    task_key: &str,
    target_key: &str,
) -> Result<EscalateResult> {
    let span = tracing::info_span!(
        "worklog.escalate.match",
        task_key,
        target_key,
        provider = Empty,
    );
    async move {
        let (_title, _description, update) = load_escalatable(pool, task_key).await?;
        let provider = meridian_core::board::provider_for_key(pool, target_key)
            .await
            .context("resolving the target ticket's provider")?
            .with_context(|| format!("{target_key} is not on the board"))?;
        if provider == LOCAL_PROVIDER {
            bail!("{target_key} is another personal task - escalate onto a real tracker ticket");
        }
        tracing::Span::current().record("provider", provider.as_str());

        super::post_comment::post_comment(config, &provider, target_key, &update)
            .await
            .with_context(|| format!("posting the update to {target_key}"))?;
        let url = browse_url(config, &provider, target_key);
        graduate_local_task(pool, task_key, target_key, &provider, url.as_deref()).await?;

        tracing::info!(
            task_key,
            target_key,
            provider,
            "worklog: escalated personal task onto an existing ticket"
        );
        Ok(EscalateResult {
            browse_url: url,
            linked_ticket: target_key.to_string(),
            provider,
            created: false,
        })
    }
    .instrument(span)
    .await
}
