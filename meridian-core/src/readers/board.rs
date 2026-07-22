//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The ONE definition of "what is on your board".
//!
//! Board membership was defined twice, in two languages, with two different
//! answers: the Tasks page filtered `!is_terminal` in TypeScript, while the
//! worklog matcher filtered `is_terminal = 0 AND decision != 'excluded' AND
//! provider != 'local'` in SQL. Two definitions of the same idea always drift,
//! and that pair already had: a ticket could sit on your Tasks page and be
//! invisible to the matcher, with nothing anywhere saying so. So there is one
//! query, here, and everything that needs "the board" calls it.
//!
//! **The worklog matcher is no longer one of those callers, on purpose.** It now
//! compares a day's work against that day's PLANNED tasks
//! ([`crate::plan::load_plan_candidates`]) — a much better prior than every open
//! ticket you own. This module lost that caller and the ">40 open tickets, refuse
//! to guess" gate that went with it (the plan is capped at
//! [`crate::plan::MAX_PLAN_TASKS`], so the candidate list is short by
//! construction and the gate could never fire). What it kept is a narrower job:
//! the predicate the Tasks page stamps, and the full-board list a human picks a
//! ticket from by hand.
//!
//! # Who calls this
//! - [`crate::tasks`] — stamps `on_board` on each task; the Tasks page filters on it.
//! - The tray's `get_board_tickets` — the worklog panel's manual ticket picker.
//!   Unlike a prompt, a searchable human list has no length at which it stops
//!   working, which is why nothing here caps it.
//!
//! # Related
//! - [`crate::plan`] — the matcher's actual candidate set.
//! - [`crate::task_create`] — the `'local'` sentinel and the invariant behind it.
//! - [`crate::triage`] — the board-cleanup working set.

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tracing::Instrument;

/// One ticket on the board, carrying everything a picker or a page needs to
/// identify it. Deliberately thin: [`crate::tasks`] enriches with time and
/// hygiene. That shape doesn't belong here.
#[derive(Debug, Clone)]
pub struct BoardTask {
    pub task_key: String,
    pub provider: String,
    pub title: String,
    pub issue_type: String,
    pub epic_title: String,
    pub description_text: String,
}

/// Is this task on the board?
///
/// The single predicate. Kept as a pure function so [`crate::tasks`] — which
/// must fetch every row, terminal ones included, to report time spent on
/// finished work — can stamp membership without a second query that would drift
/// from [`fetch_open_board`]'s `WHERE`.
///
/// - `is_terminal` — Done, or stamped off-board by `mark_retained_offboard`.
/// - `curation_decision` — `Some("excluded")` means the user told board cleanup
///   to ignore this ticket. Honouring that here is why the matcher must not be
///   the only thing that knows about it.
pub fn is_on_board(is_terminal: bool, curation_decision: Option<&str>) -> bool {
    !is_terminal && curation_decision != Some("excluded")
}

/// Every ticket on the board, newest key first.
///
/// There is deliberately no `LIMIT`. This feeds a searchable human list, and
/// truncating it would silently hide the exact ticket someone opened the picker
/// to find.
///
/// `postable_only` excludes personal tasks (`provider = 'local'`, see
/// [`crate::task_create`]). This is the one place the board legitimately splits:
/// a personal task lives only in Meridian and has no tracker to post a comment
/// to, so a caller whose next step is `post_comment` must not be offered one —
/// it could only ever produce a draft that fails at delivery. Anything that
/// merely *displays* passes `false` and sees the whole board.
///
/// Degrades to a curation-free query on a pre-038 DB (no `pm_task_curation`).
#[tracing::instrument(skip(pool))]
pub async fn fetch_open_board(pool: &SqlitePool, postable_only: bool) -> Result<Vec<BoardTask>> {
    let local_clause = if postable_only {
        "AND COALESCE(pm_tasks.provider,'jira') != 'local' "
    } else {
        ""
    };
    let sql_with_curation = format!(
        "SELECT pm_tasks.task_key, COALESCE(pm_tasks.provider,'jira'), \
                COALESCE(pm_tasks.title,''), COALESCE(pm_tasks.issue_type,'Task'), \
                COALESCE(pm_tasks.epic_title,''), COALESCE(pm_tasks.description_text,'') \
         FROM pm_tasks \
         LEFT JOIN pm_task_curation c ON c.task_key = pm_tasks.task_key \
         WHERE COALESCE(pm_tasks.is_terminal,0)=0 \
           AND (c.decision IS NULL OR c.decision != 'excluded') \
           {local_clause}\
         ORDER BY pm_tasks.task_key DESC"
    );
    let local_plain = if postable_only {
        "AND COALESCE(provider,'jira') != 'local' "
    } else {
        ""
    };
    let sql_plain = format!(
        "SELECT task_key, COALESCE(provider,'jira'), COALESCE(title,''), \
                COALESCE(issue_type,'Task'), COALESCE(epic_title,''), \
                COALESCE(description_text,'') \
         FROM pm_tasks \
         WHERE COALESCE(is_terminal,0)=0 {local_plain}\
         ORDER BY task_key DESC"
    );

    type Row = (String, String, String, String, String, String);
    let rows = match sqlx::query_as::<_, Row>(&sql_with_curation)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("board.read.pm_tasks"))
        .await
    {
        Ok(rows) => rows,
        Err(_) => sqlx::query_as::<_, Row>(&sql_plain)
            .fetch_all(pool)
            .instrument(tracing::debug_span!("board.read.pm_tasks_plain"))
            .await
            .context("fetching the open board")?,
    };
    tracing::debug!(rows = rows.len(), postable_only, "board.read.pm_tasks");

    Ok(rows
        .into_iter()
        .map(
            |(task_key, provider, title, issue_type, epic_title, description_text)| BoardTask {
                task_key,
                provider,
                title,
                issue_type,
                epic_title,
                description_text,
            },
        )
        .collect())
}

/// The tracker that owns `task_key`, or `None` if it isn't a known ticket.
///
/// The `COALESCE(provider,'jira')` default is load-bearing and dangerous in
/// equal measure: it keeps pre-multi-provider rows working, but it also means an
/// unknown key silently resolves to Jira. Callers that are about to WRITE to a
/// tracker must therefore check for [`crate::task_create::LOCAL_PROVIDER`] and
/// refuse — a personal task has nothing to post to, and letting it default would
/// aim the write at Jira. `meridian::pm_worklog::generate::resolve_provider_for_key`
/// and the tray's `retarget_day_task_worklog` both do exactly that.
///
/// **Plan-snapshot fallback.** A focus task that was marked Done leaves `pm_tasks`
/// (checking it off closes the real ticket, which prunes it off the board), but it
/// is still on the day's committed plan via the snapshot captured at plan time
/// ([`crate::plan`]). Without a fallback its provider would resolve to `None`, and
/// the worklog matcher drops any match it can't resolve — so a day's work logged
/// against the very ticket that finishing it just closed would silently vanish.
/// So when `pm_tasks` has no row, the provider is read from the most recent
/// `daily_plan` snapshot for the key (the provider doesn't change across days, so
/// day-agnostic is fine). Degrades to `None` on a pre-044 DB (no `task_snapshot`),
/// exactly as before.
#[tracing::instrument(skip(pool))]
pub async fn provider_for_key(pool: &SqlitePool, task_key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(provider,'jira') FROM pm_tasks WHERE task_key = ?")
            .bind(task_key)
            .fetch_optional(pool)
            .instrument(tracing::debug_span!("board.read.provider_for_key"))
            .await
            .context("resolving the provider for a ticket")?;
    if let Some((p,)) = row {
        return Ok(Some(p));
    }

    // Left the board (a Done focus task, pruned from pm_tasks) — recover the
    // provider from the plan snapshot so its worklog still resolves. Any error
    // (pre-044 DB with no task_snapshot column, no JSON1) degrades to None, the
    // same answer this fn gave before the fallback existed.
    let snap: Option<(Option<String>,)> = match sqlx::query_as(
        "SELECT json_extract(task_snapshot, '$.provider') \
         FROM daily_plan \
         WHERE task_key = ? AND task_snapshot IS NOT NULL \
         ORDER BY plan_date DESC LIMIT 1",
    )
    .bind(task_key)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("board.read.provider_for_key.snapshot"))
    .await
    {
        Ok(row) => row,
        // Degrade to None (the answer this fn gave before the fallback existed),
        // but say so at the failure boundary rather than swallowing it silently: a
        // snapshot query that starts failing (pre-044 DB with no task_snapshot
        // column, no JSON1) would otherwise strand every off-board worklog's
        // provider with nothing in the logs pointing at why.
        Err(e) => {
            tracing::warn!(task_key, error = %e, "board: plan-snapshot provider fallback failed - resolving to None");
            None
        }
    };
    Ok(snap.and_then(|(p,)| p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_is_off_the_board() {
        assert!(!is_on_board(true, None));
    }

    #[test]
    fn open_and_undecided_is_on_the_board() {
        assert!(is_on_board(false, None));
    }

    #[test]
    fn excluded_by_cleanup_is_off_the_board() {
        assert!(!is_on_board(false, Some("excluded")));
    }

    #[test]
    fn other_curation_decisions_stay_on_the_board() {
        // Only 'excluded' removes a ticket. 'keep'/'fix' are decisions ABOUT a
        // ticket that stays.
        assert!(is_on_board(false, Some("keep")));
        assert!(is_on_board(false, Some("fix")));
    }
}
