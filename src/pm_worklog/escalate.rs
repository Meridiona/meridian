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
//! it personal. The DB repoint ([`graduate_local_task`]) is a single transaction:
//! it fully lands or not at all.
//!
//! # Retry safety around the tracker call
//! The tracker mutation (create a ticket, or post a comment) is NOT part of that
//! transaction - it happens first and cannot be rolled back. So a crash/timeout
//! between it and the graduate commit would, unguarded, let a retry file a second
//! ticket or post a second comment. A claim on the personal task row
//! ([`begin_escalation`], migration 073) plus an `escalation_ticket_key` stashed
//! before the comment/graduate make both paths idempotent: a concurrent escalation
//! is refused, and a retry reuses the created/posted key rather than redoing the
//! tracker call. This mirrors the day-task worklog's begin_create/mark_created.
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

/// How long an escalation claim ([`begin_escalation`]) is honoured before a retry
/// may re-take it. A claim younger than this blocks a concurrent escalation of the
/// same task; an older one is assumed to belong to a crashed attempt and can be
/// re-taken (its [`EscalationClaim::ticket_key`] carries the created/posted key so
/// the retry still doesn't redo the tracker call).
const ESCALATION_CLAIM_STALE_MINS: i64 = 10;

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

/// The outcome of claiming a personal task for escalation ([`begin_escalation`]).
struct EscalationClaim {
    /// We won (or re-took a stale) claim and may proceed. `false` means a fresh
    /// concurrent escalation holds the claim and the caller must back off.
    won: bool,
    /// The tracker mutation a PRIOR (interrupted) attempt already performed - the
    /// created ticket key, or the matched target key. Present only after a crash
    /// between the tracker call and the graduate commit; the retry reuses it to
    /// skip the create / the comment. `None` on a first attempt.
    ticket_key: Option<String>,
}

/// Atomically claim a personal task for escalation (migration 073). Returns
/// `won = true` when the claim was free or stale (a crashed attempt, older than
/// [`ESCALATION_CLAIM_STALE_MINS`]) and we may proceed; `won = false` when a fresh
/// concurrent escalation holds it. Also returns any `escalation_ticket_key` a prior
/// attempt stashed, so the caller reuses it instead of redoing the tracker call.
///
/// This is the guard that makes escalation retry-safe: the non-idempotent tracker
/// call ([`super::create::create_ticket`] / [`super::post_comment::post_comment`])
/// runs BEFORE [`graduate_local_task`] commits, so without this a crash/timeout in
/// that window would let a retry file a second ticket or post a second comment.
async fn begin_escalation(
    pool: &SqlitePool,
    local_key: &str,
    now: &str,
) -> Result<EscalationClaim> {
    // Staleness is measured from the SAME clock as the claim (`now`), not a fresh
    // `Utc::now()`, so the comparison is deterministic and testable. `now` is always
    // an RFC3339 string we produced; a parse failure falls back to "nothing is
    // stale" (an empty string sorts before every real timestamp), which is the safe
    // side - it only ever refuses a re-take, never grants a spurious one.
    let stale_before = chrono::DateTime::parse_from_rfc3339(now)
        .map(|t| (t - chrono::Duration::minutes(ESCALATION_CLAIM_STALE_MINS)).to_rfc3339())
        .unwrap_or_default();
    let res = sqlx::query(
        "UPDATE pm_tasks SET escalation_attempt_at = ? \
         WHERE task_key = ? AND provider = ? \
           AND (escalation_attempt_at IS NULL OR escalation_attempt_at < ?)",
    )
    .bind(now)
    .bind(local_key)
    .bind(LOCAL_PROVIDER)
    .bind(&stale_before)
    .execute(pool)
    .instrument(tracing::debug_span!("escalate.write.begin_claim"))
    .await
    .context("claiming the personal task for escalation")?;
    let ticket_key: Option<String> =
        sqlx::query_scalar("SELECT escalation_ticket_key FROM pm_tasks WHERE task_key = ?")
            .bind(local_key)
            .fetch_optional(pool)
            .await
            .context("reading any prior escalation ticket key")?
            .flatten();
    Ok(EscalationClaim {
        won: res.rows_affected() > 0,
        ticket_key,
    })
}

/// Persist the tracker mutation this escalation has already performed BEFORE the
/// comment/graduate that follow it - the created ticket key (create path) or the
/// matched target key (match path). A retry reads it via [`begin_escalation`] and
/// skips the create / the comment, so neither is ever done twice.
async fn mark_escalation_ticket(
    pool: &SqlitePool,
    local_key: &str,
    ticket_key: &str,
) -> Result<()> {
    sqlx::query("UPDATE pm_tasks SET escalation_ticket_key = ? WHERE task_key = ?")
        .bind(ticket_key)
        .bind(local_key)
        .execute(pool)
        .instrument(tracing::debug_span!("escalate.write.mark_ticket"))
        .await
        .context("recording the escalation's tracker ticket")?;
    Ok(())
}

/// Release a failed escalation's claim so the user can retry immediately (rather
/// than wait out [`ESCALATION_CLAIM_STALE_MINS`]). Best-effort - a failure to clear
/// only delays the next retry to the staleness window, never corrupts state. Keeps
/// `escalation_ticket_key` intact so a retry still reuses any ticket already created.
async fn clear_escalation_claim(pool: &SqlitePool, local_key: &str) {
    if let Err(e) =
        sqlx::query("UPDATE pm_tasks SET escalation_attempt_at = NULL WHERE task_key = ?")
            .bind(local_key)
            .execute(pool)
            .await
    {
        tracing::warn!(task_key = local_key, error = %e, "worklog: failed to release escalation claim - a retry waits out the staleness window");
    }
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
#[tracing::instrument(skip(pool), fields(matched = Empty))]
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
        .instrument(tracing::debug_span!("escalate.read.real_row"))
        .await
        .context("checking whether the real ticket already has a row")?;
    tracing::Span::current().record("matched", real_exists.is_some());

    if real_exists.is_some() {
        // Match-existing: carry the logged update onto the real row (only if it has
        // none of its own), then retire the personal row rather than hard-delete it
        // - matching how off-boarded tasks are hidden. The escalation claim columns
        // go away with the retired row's exclusion from escalation (it is no longer
        // queryable as `provider = 'local'`), so they need no explicit clear here.
        sqlx::query(
            "UPDATE pm_tasks SET \
               local_worklog_text = COALESCE(local_worklog_text, (SELECT local_worklog_text FROM pm_tasks WHERE task_key = ?1)), \
               local_worklog_posted_at = COALESCE(local_worklog_posted_at, (SELECT local_worklog_posted_at FROM pm_tasks WHERE task_key = ?1)) \
             WHERE task_key = ?2",
        )
        .bind(local_key)
        .bind(real_key)
        .execute(&mut *tx)
        .instrument(tracing::debug_span!("escalate.write.copy_update"))
        .await
        .context("copying the logged update onto the real ticket")?;
        sqlx::query("UPDATE pm_tasks SET is_terminal = 1 WHERE task_key = ?")
            .bind(local_key)
            .execute(&mut *tx)
            .instrument(tracing::debug_span!("escalate.write.retire_local"))
            .await
            .context("retiring the graduated personal task")?;
    } else {
        // Create-new: the new key has no row yet, so the personal row becomes it.
        // Clear the escalation claim columns as it graduates - it is no longer a
        // personal task, so the claim/marker have served their purpose.
        sqlx::query(
            "UPDATE pm_tasks SET task_key = ?, provider = ?, url = COALESCE(?, url), \
               is_terminal = 0, escalation_attempt_at = NULL, escalation_ticket_key = NULL \
             WHERE task_key = ?",
        )
        .bind(real_key)
        .bind(real_provider)
        .bind(real_url)
        .bind(local_key)
        .execute(&mut *tx)
        .instrument(tracing::debug_span!("escalate.write.rewrite_local"))
        .await
        .context("rewriting the personal task into the new ticket")?;
    }

    // daily_plan PK (plan_date, task_key): move rows, dropping any that would
    // collide with a plan row the real ticket already has that day.
    sqlx::query("UPDATE OR IGNORE daily_plan SET task_key = ? WHERE task_key = ?")
        .bind(real_key)
        .bind(local_key)
        .execute(&mut *tx)
        .instrument(tracing::debug_span!("escalate.write.repoint_plan"))
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
    .instrument(tracing::debug_span!("escalate.write.repoint_targets"))
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
        .instrument(tracing::debug_span!("escalate.write.repoint_card"))
        .await
        .context("repointing the day-task card link")?;

    tx.commit().await.context("commit graduate transaction")?;
    tracing::info!(
        local_key,
        real_key,
        real_provider,
        "worklog: graduated personal task into the real ticket"
    );
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

        // Claim the task so a concurrent escalation (double-click, retry-after-
        // timeout) can't run alongside this one and file a second ticket.
        let now = chrono::Utc::now().to_rfc3339();
        let claim = begin_escalation(pool, task_key, &now).await?;
        if !claim.won {
            bail!("an escalation of {task_key} is already in progress - try again in a moment");
        }

        // Reuse a ticket a prior (interrupted) attempt already created rather than
        // file a second one; otherwise create it and record the key BEFORE posting
        // its comment / graduating, so any retry from here on reuses it.
        let new_key = match claim.ticket_key {
            Some(k) => {
                tracing::info!(task_key, new_key = %k, "worklog: reusing the ticket a prior escalation attempt created");
                k
            }
            None => {
                let sample = fetch_sample_task_key(pool, &provider).await;
                let k = match super::create::create_ticket(
                    config,
                    &provider,
                    &title,
                    &description,
                    "Task",
                    sample.as_deref(),
                )
                .await
                {
                    Ok(k) => k,
                    Err(e) => {
                        clear_escalation_claim(pool, task_key).await;
                        return Err(e).context("creating the ticket on the tracker");
                    }
                };
                mark_escalation_ticket(pool, task_key, &k).await?;
                k
            }
        };
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

        // Claim the task so a concurrent escalation can't double-post the comment.
        let now = chrono::Utc::now().to_rfc3339();
        let claim = begin_escalation(pool, task_key, &now).await?;
        if !claim.won {
            bail!("an escalation of {task_key} is already in progress - try again in a moment");
        }

        // Skip the comment if a prior (interrupted) attempt already posted it to
        // THIS target - reposting would put a second copy on the ticket. Otherwise
        // post it and record the target BEFORE graduating, so a retry skips it.
        if claim.ticket_key.as_deref() == Some(target_key) {
            tracing::info!(task_key, target_key, "worklog: comment already posted by a prior escalation attempt - skipping the re-post");
        } else {
            if let Err(e) = super::post_comment::post_comment(config, &provider, target_key, &update)
                .await
            {
                clear_escalation_claim(pool, task_key).await;
                return Err(e).with_context(|| format!("posting the update to {target_key}"));
            }
            mark_escalation_ticket(pool, task_key, target_key).await?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Minimal schema for the escalation DB ops: pm_tasks with the migration-073
    /// claim columns, plus the three tables graduate_local_task repoints.
    async fn seeded() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE pm_tasks (task_key TEXT PRIMARY KEY, provider TEXT, url TEXT, \
                title TEXT, is_terminal INTEGER DEFAULT 0, local_worklog_text TEXT, \
                local_worklog_posted_at TEXT, escalation_attempt_at TEXT, escalation_ticket_key TEXT)",
            "CREATE TABLE daily_plan (plan_date TEXT, task_key TEXT, PRIMARY KEY (plan_date, task_key))",
            "CREATE TABLE day_task_worklog_targets (day_local TEXT, task_id TEXT, task_key TEXT, \
                provider TEXT, browse_url TEXT, PRIMARY KEY (day_local, task_id, task_key))",
            "CREATE TABLE day_tasks (id TEXT PRIMARY KEY, linked_ticket TEXT)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn seed_local(pool: &SqlitePool, key: &str) {
        sqlx::query("INSERT INTO pm_tasks (task_key, provider, title, local_worklog_text) VALUES (?, 'local', 'Personal', 'did work')")
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_claim_is_won_once_then_blocks_a_concurrent_escalation() {
        let pool = seeded().await;
        seed_local(&pool, "LOCAL-1").await;
        let now = "2026-07-22T10:00:00+00:00";

        let first = begin_escalation(&pool, "LOCAL-1", now).await.unwrap();
        assert!(first.won, "the first claim wins");
        assert!(first.ticket_key.is_none());

        // A second escalation moments later must lose - the claim is fresh.
        let second = begin_escalation(&pool, "LOCAL-1", "2026-07-22T10:00:05+00:00")
            .await
            .unwrap();
        assert!(!second.won, "a concurrent escalation is refused");
    }

    #[tokio::test]
    async fn a_stale_claim_can_be_retaken_and_reuses_the_created_ticket() {
        let pool = seeded().await;
        seed_local(&pool, "LOCAL-1").await;

        // A crashed attempt claimed 20 min ago and had already created KAN-9.
        begin_escalation(&pool, "LOCAL-1", "2026-07-22T10:00:00+00:00")
            .await
            .unwrap();
        mark_escalation_ticket(&pool, "LOCAL-1", "KAN-9")
            .await
            .unwrap();

        // A retry now (past the staleness window) re-takes the claim and sees the
        // created key, so it reuses KAN-9 rather than filing a second ticket.
        let now = chrono::Utc::now().to_rfc3339();
        let retry = begin_escalation(&pool, "LOCAL-1", &now).await.unwrap();
        assert!(retry.won, "a stale claim is re-takeable");
        assert_eq!(retry.ticket_key.as_deref(), Some("KAN-9"));
    }

    #[tokio::test]
    async fn clearing_the_claim_lets_a_retry_proceed_immediately() {
        let pool = seeded().await;
        seed_local(&pool, "LOCAL-1").await;

        begin_escalation(&pool, "LOCAL-1", "2026-07-22T10:00:00+00:00")
            .await
            .unwrap();
        clear_escalation_claim(&pool, "LOCAL-1").await;

        // Immediately (no staleness wait) a retry wins, because the failed attempt
        // released the claim.
        let retry = begin_escalation(&pool, "LOCAL-1", "2026-07-22T10:00:03+00:00")
            .await
            .unwrap();
        assert!(retry.won);
    }

    #[tokio::test]
    async fn graduate_create_rewrites_the_row_and_repoints_references() {
        let pool = seeded().await;
        seed_local(&pool, "LOCAL-1").await;
        sqlx::query("INSERT INTO daily_plan VALUES ('2026-07-22', 'LOCAL-1')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO day_tasks VALUES ('T1', 'LOCAL-1')")
            .execute(&pool)
            .await
            .unwrap();

        graduate_local_task(&pool, "LOCAL-1", "KAN-9", "jira", Some("http://x/KAN-9"))
            .await
            .unwrap();

        // The personal row BECAME the real ticket (no LOCAL-1 left, KAN-9 present,
        // claim columns cleared) and its logged update rode along.
        let gone: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM pm_tasks WHERE task_key = 'LOCAL-1'")
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert!(gone.is_none(), "LOCAL-1 is graduated away");
        let row: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT provider, local_worklog_text, escalation_ticket_key FROM pm_tasks WHERE task_key = 'KAN-9'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "jira");
        assert_eq!(
            row.1.as_deref(),
            Some("did work"),
            "the logged update stays"
        );
        assert!(row.2.is_none(), "claim columns cleared on graduate");
        let plan: (String,) = sqlx::query_as("SELECT task_key FROM daily_plan")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(plan.0, "KAN-9", "the plan follows the ticket");
        let link: (String,) = sqlx::query_as("SELECT linked_ticket FROM day_tasks WHERE id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(link.0, "KAN-9", "the card link follows the ticket");
    }

    #[tokio::test]
    async fn graduate_match_retires_the_local_row_and_copies_its_update() {
        let pool = seeded().await;
        seed_local(&pool, "LOCAL-1").await;
        // The real ticket already exists (match-existing).
        sqlx::query(
            "INSERT INTO pm_tasks (task_key, provider, title) VALUES ('KAN-7', 'jira', 'Real')",
        )
        .execute(&pool)
        .await
        .unwrap();

        graduate_local_task(&pool, "LOCAL-1", "KAN-7", "jira", None)
            .await
            .unwrap();

        // LOCAL-1 is retired (is_terminal), KAN-7 got the logged update copied.
        let terminal: (i64,) =
            sqlx::query_as("SELECT is_terminal FROM pm_tasks WHERE task_key = 'LOCAL-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(terminal.0, 1, "the personal row is retired, not deleted");
        let copied: (Option<String>,) =
            sqlx::query_as("SELECT local_worklog_text FROM pm_tasks WHERE task_key = 'KAN-7'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            copied.0.as_deref(),
            Some("did work"),
            "the update is shown on the real ticket"
        );
    }
}
