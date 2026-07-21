//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tickets one generated worklog posts to (`day_task_worklog_targets`,
//! migration 062) — the match branch of a draft, one row per ticket.
//!
//! # What this is
//! A day's strand of work often advances more than one planned task, so a draft
//! carries 0..N targets rather than a single `match_task_key`. Each row owns its
//! own delivery state, and that is the whole point: posting is not atomic. An
//! approve across three tickets can succeed on two and fail on the third, and a
//! comment cannot be un-posted — so the retry has to know, per ticket, which ones
//! already landed. [`WorklogTarget::posted`] (`posted_comment_id IS NOT NULL`) is
//! that fact, and it is what makes retry safe instead of duplicating comments.
//!
//! # Who calls this
//! - [`load`] → [`super::get_day_task_worklog`] → the tray `get_day_task_worklog`
//!   command → the day-task detail panel.
//! - [`replace`] → [`super::upsert_draft`] (a fresh model answer) and
//!   [`super::retarget_draft`] (the user collapsing the draft onto one ticket).
//! - [`insert`] → [`super::mark_created`], when approve creates a proposed ticket
//!   and it becomes this draft's only target.
//! - [`dismiss`] → the tray `dismiss_worklog_target` command, when the user drops
//!   one ticket the model matched.
//! - [`mark_posted`] / [`mark_error`] → the daemon's per-target approve loop in
//!   `src/pm_worklog/generate.rs`.
//!
//! # Related
//! - [`super`] — the parent draft (the propose branch, the update, the lifecycle).

use crate::SqlitePool;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection};
use tracing::Instrument;

/// One ticket this draft's status update posts to, as the CLI prints it and the
/// tray/UI consume it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorklogTarget {
    pub task_key: String,
    /// The tracker this ticket lives on. Per-target: a day's plan can span two.
    pub provider: String,
    /// The model's honest confidence, or `1.0` when [`Self::manual`] — in which
    /// case it is meaningless and the UI must not render it as a score.
    pub confidence: f64,
    /// The user picked this ticket themselves, overriding the model.
    pub manual: bool,
    /// The ticket's title, hydrated from `pm_tasks` at read time (never persisted)
    /// so the panel can show more than a bare key. `None` once the ticket has
    /// dropped off the board.
    #[serde(default)]
    pub task_title: Option<String>,
    /// The comment is live on the tracker. Terminal — never post here again.
    pub posted: bool,
    pub posted_comment_id: Option<String>,
    pub browse_url: Option<String>,
    /// A `post_comment` call was started for this ticket and its outcome was never
    /// recorded (migration 063) — the process died mid-request. The comment may or
    /// may not be live, and nothing here can tell which.
    ///
    /// Never auto-retried: `post_comment` has no dedup marker, so a retry that
    /// guesses wrong puts a second copy on someone's board. Only a human looking at
    /// the ticket can resolve it.
    pub outcome_unknown: bool,
    /// The last failure posting to THIS ticket, if any. Others may have succeeded.
    pub error: Option<String>,
}

/// A target to write — the subset a caller knows before anything is posted.
#[derive(Debug, Clone)]
pub struct TargetInput {
    pub task_key: String,
    pub provider: String,
    pub confidence: f64,
    pub manual: bool,
}

#[derive(FromRow)]
struct RawTarget {
    task_key: String,
    provider: String,
    confidence: f64,
    manual: i64,
    task_title: Option<String>,
    posted_comment_id: Option<String>,
    browse_url: Option<String>,
    post_attempt_at: Option<String>,
    last_error: Option<String>,
}

impl From<RawTarget> for WorklogTarget {
    fn from(r: RawTarget) -> Self {
        Self {
            task_key: r.task_key,
            provider: r.provider,
            confidence: r.confidence,
            manual: r.manual != 0,
            task_title: r.task_title,
            posted: r.posted_comment_id.is_some(),
            // An attempt on the books with no comment id: it started and never
            // resolved. See [`begin_post`] for the state table.
            outcome_unknown: r.post_attempt_at.is_some() && r.posted_comment_id.is_none(),
            posted_comment_id: r.posted_comment_id,
            browse_url: r.browse_url,
            error: r.last_error,
        }
    }
}

/// Every target of `(day_local, task_id)`, in the order the model returned them
/// (`position`). A missing table (pre-062 DB) degrades to empty, never an error —
/// mirroring [`super::get_day_task_worklog`], which is the only caller and must
/// stay tolerant of an un-migrated DB.
pub async fn load(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> anyhow::Result<Vec<WorklogTarget>> {
    let rows = sqlx::query_as::<_, RawTarget>(
        "SELECT t.task_key, t.provider, t.confidence, t.manual, pt.title AS task_title, \
                t.posted_comment_id, t.browse_url, t.post_attempt_at, t.last_error \
         FROM day_task_worklog_targets t \
         LEFT JOIN pm_tasks pt ON pt.task_key = t.task_key \
         WHERE t.day_local = ? AND t.task_id = ? \
         ORDER BY t.position, t.task_key",
    )
    .bind(day_local)
    .bind(task_id)
    .fetch_all(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.read.day_task_worklog_targets"
    ))
    .await;

    match rows {
        Ok(rows) => {
            tracing::debug!(
                rows = rows.len(),
                "day_task_worklogs.read.day_task_worklog_targets"
            );
            Ok(rows.into_iter().map(Into::into).collect())
        }
        Err(e) => {
            tracing::warn!(error = %e, "day_task_worklogs: targets read skipped (pre-062 DB?)");
            Ok(Vec::new())
        }
    }
}

/// Every planned-ticket match the day's drafted (or posted) worklogs recorded,
/// as `task_key -> [day-task id, …]`.
///
/// This is the DETERMINISTIC plan-adherence signal. When a worklog is drafted, its
/// matched ticket is written here (`posted_comment_id` NULL until posted), so a
/// ticket that appears here was worked on that day - whether or not the user has
/// approved the worklog yet. The daily summary reads this instead of asking the
/// model which tickets got done (see [`crate::day_evidence::adherence`]).
///
/// Drafted AND posted rows both count - the row's mere existence is the match; its
/// posted-ness is a delivery detail the summary does not care about. A day-task can
/// match several tickets and a ticket can be matched by several day-tasks, so this
/// is a genuine many-to-many. A missing table (pre-062 DB) degrades to an empty
/// map, never an error, matching [`load`].
///
/// # Who calls this
/// [`crate::day_evidence::collect`] - the daily summary's plan ledger.
pub async fn matched_tickets_for_day(
    pool: &SqlitePool,
    day_local: &str,
) -> anyhow::Result<std::collections::HashMap<String, Vec<String>>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT task_key, task_id FROM day_task_worklog_targets \
         WHERE day_local = ? ORDER BY task_key, position, task_id",
    )
    .bind(day_local)
    .fetch_all(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.read.matched_tickets_for_day"
    ))
    .await;

    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    match rows {
        Ok(rows) => {
            for (task_key, task_id) in rows {
                let ids = map.entry(task_key).or_default();
                // A ticket matched by the same day-task twice (a regenerate that
                // re-drafted) must not double-count that workstream's minutes.
                if !ids.contains(&task_id) {
                    ids.push(task_id);
                }
            }
            tracing::debug!(
                tickets = map.len(),
                "day_task_worklogs.read.matched_tickets_for_day"
            );
            Ok(map)
        }
        Err(e) => {
            tracing::warn!(error = %e, "day_task_worklogs: matched-tickets read skipped (pre-062 DB?)");
            Ok(map)
        }
    }
}

/// Replace the whole target set of `(day_local, task_id)`.
///
/// Destructive by design — a regenerate or a retarget supersedes the previous
/// answer outright. **Callers MUST have established that the parent row is still
/// `drafted`** before calling: a posted target's comment is live on the tracker,
/// and deleting the row would lose the only record that it must never be posted
/// to again. Both callers ([`super::upsert_draft`], [`super::retarget_draft`])
/// gate on their `WHERE state = 'drafted'` guard actually matching a row.
///
/// Takes a `&mut SqliteConnection` rather than the pool so it runs in the SAME
/// transaction as that guard write — otherwise a concurrent `approve()` could land
/// between the guard matching and this DELETE, post a comment, and have its
/// `posted_comment_id` erased here (then re-posted on the next approve). The delete
/// and the re-inserts are several statements, exactly the child-table
/// DELETE-then-INSERT pattern that must be atomic.
pub async fn replace(
    conn: &mut SqliteConnection,
    day_local: &str,
    task_id: &str,
    targets: &[TargetInput],
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM day_task_worklog_targets WHERE day_local = ? AND task_id = ?")
        .bind(day_local)
        .bind(task_id)
        .execute(&mut *conn)
        .instrument(tracing::debug_span!(
            "day_task_worklogs.write.targets_clear"
        ))
        .await
        .context("clearing the worklog's previous targets")?;

    for (i, t) in targets.iter().enumerate() {
        insert_at(&mut *conn, day_local, task_id, t, i as i64, now).await?;
    }
    tracing::debug!(
        targets = targets.len(),
        "day_task_worklogs: targets replaced"
    );
    Ok(())
}

/// Add one target, keeping any that already exist. Used when approve creates a
/// proposed ticket: the created ticket becomes the draft's target.
pub async fn insert(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    target: &TargetInput,
    now: &str,
) -> anyhow::Result<()> {
    // The MAX(position) read and the INSERT run on one connection so they stay
    // consistent with each other.
    let mut conn = pool.acquire().await.context("acquiring a connection")?;
    let next: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM day_task_worklog_targets \
         WHERE day_local = ? AND task_id = ?",
    )
    .bind(day_local)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await
    .context("finding the next target position")?;
    insert_at(&mut conn, day_local, task_id, target, next, now).await
}

/// `INSERT OR IGNORE` one target at `position`. Ignore-on-conflict keeps a repeat
/// key a no-op rather than resetting a target that may already be posted.
async fn insert_at(
    conn: &mut SqliteConnection,
    day_local: &str,
    task_id: &str,
    t: &TargetInput,
    position: i64,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO day_task_worklog_targets \
            (day_local, task_id, task_key, provider, confidence, manual, position, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(day_local)
    .bind(task_id)
    .bind(&t.task_key)
    .bind(&t.provider)
    .bind(t.confidence)
    .bind(i64::from(t.manual))
    .bind(position)
    .bind(now)
    .execute(&mut *conn)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.write.targets_insert"
    ))
    .await
    .context("adding a worklog target")?;
    Ok(())
}

/// Drop one ticket the user doesn't want this update posted to.
///
/// Refuses a target whose comment is already live (`posted_comment_id IS NOT
/// NULL`): deleting it would not remove the comment from the tracker, it would
/// only make Meridian forget that it is there — and a later retry would post a
/// second copy. Returns `Ok(())` on success, `Err` when there was no dismissable
/// target, so the caller can say why rather than silently no-op.
#[tracing::instrument(skip(pool))]
pub async fn dismiss(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
) -> anyhow::Result<()> {
    let res = sqlx::query(
        "DELETE FROM day_task_worklog_targets \
         WHERE day_local = ? AND task_id = ? AND task_key = ? AND posted_comment_id IS NULL",
    )
    .bind(day_local)
    .bind(task_id)
    .bind(task_key)
    .execute(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.write.targets_dismiss"
    ))
    .await
    .context("dismissing a worklog target")?;

    if res.rows_affected() == 0 {
        // Two ways to land here, and they are not the same story: the comment is
        // already live (the guard above did its job), or the key simply isn't on
        // this draft. Look rather than guess — telling someone their update was
        // posted when it wasn't is worse than the extra query costs.
        let posted: Option<String> = sqlx::query_scalar(
            "SELECT posted_comment_id FROM day_task_worklog_targets \
             WHERE day_local = ? AND task_id = ? AND task_key = ?",
        )
        .bind(day_local)
        .bind(task_id)
        .bind(task_key)
        .fetch_optional(pool)
        .await
        .context("checking why a worklog target could not be dismissed")?
        .flatten();
        if posted.is_some() {
            anyhow::bail!(
                "this update is already posted to {task_key} - a comment can't be taken back"
            );
        }
        anyhow::bail!("{task_key} isn't on this draft - it may already be gone");
    }
    tracing::info!(task_key, "worklog: target dismissed by the user");
    Ok(())
}

/// Claim the right to post to `task_key`, write-ahead, immediately BEFORE calling
/// the provider. Returns `true` iff this caller now owns the attempt.
///
/// This is the guard that makes "post to N tickets" survive a crash. Without it,
/// dying between `post_comment` returning and [`mark_posted`] committing leaves a
/// live comment nothing has recorded, and the retry — seeing `posted = false` —
/// posts a second copy. `post_comment` carries no dedup marker to catch that
/// afterwards (by design; see its module docs), so the intent has to be on disk
/// before the call, not after.
///
/// The CAS (`posted_comment_id IS NULL AND post_attempt_at IS NULL`) returns
/// `false` for a ticket that is already posted, already in flight, or whose
/// outcome is unknown — so a concurrent approve and a post-crash retry are both
/// refused by the same predicate rather than by separate checks that could drift.
///
/// Every caller MUST resolve what it claims: [`mark_posted`] on success,
/// [`revert_post`] on a definite failure. Leaving it unresolved is exactly the
/// "outcome unknown" state, and that is a dead end only a human can clear.
pub async fn begin_post(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
    now: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "UPDATE day_task_worklog_targets SET post_attempt_at = ? \
         WHERE day_local = ? AND task_id = ? AND task_key = ? \
           AND posted_comment_id IS NULL AND post_attempt_at IS NULL",
    )
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .bind(task_key)
    .execute(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.write.targets_begin_post"
    ))
    .await
    .context("claiming a worklog target for posting")?;
    Ok(res.rows_affected() > 0)
}

/// Release a claim taken by [`begin_post`] after a DEFINITE failure — the provider
/// call returned an error, so nothing was posted and a later retry is known-safe.
///
/// Only ever called when the failure is confirmed. It must NOT be called for the
/// "we don't know what happened" case: leaving `post_attempt_at` set is what stops
/// the retry from double-posting, and clearing it here would throw away the only
/// evidence that a comment might already be live.
pub async fn revert_post(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklog_targets SET post_attempt_at = NULL \
         WHERE day_local = ? AND task_id = ? AND task_key = ? AND posted_comment_id IS NULL",
    )
    .bind(day_local)
    .bind(task_id)
    .bind(task_key)
    .execute(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.write.targets_revert_post"
    ))
    .await
    .context("releasing a worklog target after a failed post")?;
    Ok(())
}

/// Record that the comment is live on `task_key`. Terminal for this target — the
/// approve loop skips it on every later pass. Clears the [`begin_post`] claim: the
/// attempt is resolved, and `posted_comment_id` now carries the answer.
pub async fn mark_posted(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
    comment_id: &str,
    browse_url: Option<&str>,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklog_targets \
         SET posted_comment_id = ?, browse_url = ?, posted_at = ?, \
             post_attempt_at = NULL, last_error = NULL \
         WHERE day_local = ? AND task_id = ? AND task_key = ?",
    )
    .bind(comment_id)
    .bind(browse_url)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .bind(task_key)
    .execute(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.write.targets_posted"
    ))
    .await
    .context("marking a worklog target posted")?;
    Ok(())
}

/// Record a per-target failure. Leaves the target unposted and retryable; the
/// other targets of the same draft are unaffected.
pub async fn mark_error(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklog_targets SET last_error = ? \
         WHERE day_local = ? AND task_id = ? AND task_key = ?",
    )
    .bind(error)
    .bind(day_local)
    .bind(task_id)
    .bind(task_key)
    .execute(pool)
    .await
    .context("recording a worklog target failure")?;
    Ok(())
}
