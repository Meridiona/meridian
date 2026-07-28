//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GitHub task connector. Fetches open issues assigned to the viewer from
// configured GitHub Projects v2 (GraphQL API). task_key is `owner/repo#number`.

use anyhow::{Context, Result};
use meridian_core::adapters::github::GithubAdapter;
use sqlx::SqlitePool;

use crate::config::GitHubConfig;
use crate::intelligence::providers::http::SyncFault;

mod fetch;
#[cfg(test)]
mod tests;

use fetch::{fetch_project_items, fetch_viewer_login};

/// One normalised issue ready to upsert.
struct GhTask {
    task_key: String,
    repo_slug: String,
    title: String,
    body: String,
    /// Verbatim Projects v2 "Status" column name (e.g. "In Review"). Empty when
    /// the item has no Status field set.
    status_raw: String,
    /// Whether that column means the issue is done — resolved via the shared
    /// status resolver (override → keyword heuristic; GitHub has no native
    /// done/closed category on the board column itself).
    is_terminal: bool,
    url: String,
    updated_at: String,
    assignee: String,
    tags: Option<String>,
}

const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(pool: &SqlitePool, tasks: &[(GhTask, serde_json::Value)]) -> Result<()> {
    for (t, raw) in tasks {
        // CDM columns (Stage 3b) from the raw item via the shared adapter.
        let cdm = super::cdm::derive(&GithubAdapter, raw);

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, assignee_name, tags,
                canonical_id, status_category, raw_payload, reporter_name,
                completed_at, ancestor_path, project_ids,
                updated_at, fetched_at)
             VALUES (?, 'github', ?, ?, ?, ?, 'Issue', ?, ?, ?, ?,
                     ?, ?, ?, ?, ?, ?, ?,
                     ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'github',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_raw       = excluded.status_raw,
               is_terminal      = excluded.is_terminal,
               project_key      = excluded.project_key,
               url              = excluded.url,
               assignee_name    = excluded.assignee_name,
               tags             = excluded.tags,
               canonical_id     = excluded.canonical_id,
               status_category  = excluded.status_category,
               raw_payload      = excluded.raw_payload,
               reporter_name    = excluded.reporter_name,
               completed_at     = excluded.completed_at,
               ancestor_path    = excluded.ancestor_path,
               project_ids      = excluded.project_ids,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at",
        )
        .bind(&t.task_key)
        .bind(&t.title)
        .bind(&t.body)
        .bind(&t.status_raw)
        .bind(t.is_terminal)
        .bind(&t.repo_slug)
        .bind(&t.url)
        .bind(&t.assignee)
        .bind(t.tags.as_deref())
        .bind(cdm.canonical_id)
        .bind(cdm.status_category)
        .bind(cdm.raw_payload)
        .bind(cdm.reporter_name)
        .bind(cdm.completed_at)
        .bind(cdm.ancestor_path)
        .bind(cdm.project_ids)
        .bind(&t.updated_at)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", t.task_key))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prune (scoped to provider = 'github')
// ---------------------------------------------------------------------------

/// Delete `pm_tasks` rows no longer returned by the active-task fetch (closed,
/// reassigned, etc.) — EXCEPT a task_key that has worklog history
/// (`pm_worklogs`) or sits on a daily plan (`daily_plan`), both kept forever:
/// worklog history so a completed ticket's title never disappears from the
/// timeline once it's closed, daily-plan membership so closing a "today's
/// focus" item from the plan checkbox doesn't delete the very row its Undo
/// (reopen) needs — see `src/plan_tasks/done.rs`.
async fn prune(pool: &SqlitePool, fetched_keys: &[String]) -> Result<usize> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    let emb_sql = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks WHERE provider = 'github' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&emb_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q.execute(pool)
        .await
        .context("pruning github pm_task_embeddings")?;

    let task_sql = format!(
        "DELETE FROM pm_tasks WHERE provider = 'github' AND task_key NOT IN ({placeholders}) \
         AND task_key NOT IN (SELECT DISTINCT task_key FROM pm_worklogs) \
         AND task_key NOT IN (SELECT DISTINCT task_key FROM daily_plan)"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning github pm_tasks")?;

    // Worklog-retained rows the DELETE above kept but that are no longer in the
    // active fetch (closed, reassigned, out-of-scope) must leave the board.
    let flagged = super::mark_retained_offboard(pool, "github", fetched_keys).await?;
    if flagged > 0 {
        tracing::info!(flagged, "flagged retained github tasks off-board");
    }
    Ok(result.rows_affected() as usize)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, github))]
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    github: &GitHubConfig,
) -> Result<Option<Vec<String>>> {
    if github.project_ids.is_empty() {
        tracing::debug!("no GITHUB_PROJECT_IDS configured — skipping github sync");
        return Ok(None);
    }

    let threshold = format!("-{SYNC_INTERVAL_MINS} minutes");
    let (is_fresh,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = 'github'
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking github sync state")?;

    if is_fresh != 0 {
        return Ok(None);
    }

    let viewer_login = match fetch_viewer_login(github).await {
        Ok(l) => l,
        Err(e) => {
            // A connect failure here is the network, not the credential. It
            // used to raise "GitHub auth failed / Set GITHUB_TOKEN in .env",
            // telling the user to redo a token that was never broken.
            match super::http::classify(&e) {
                SyncFault::Retry { detail } => tracing::warn!(
                    error = %detail,
                    "github unreachable - keeping stale cache, will retry next sync"
                ),
                SyncFault::Report { detail } => {
                    tracing::warn!(
                        error = %detail,
                        "github viewer fetch failed - keeping stale cache"
                    );
                    let _ = super::stamp_sync_error(
                        pool,
                        "github",
                        &format!("GitHub sync failed: {detail}"),
                    )
                    .await;
                }
            }
            return Ok(None);
        }
    };

    let client = super::http::client();

    // Each project is an independent paginated GraphQL walk — fetch them all
    // concurrently, then fold the results preserving any_ok/all_ok semantics.
    let results = futures::future::join_all(
        github
            .project_ids
            .iter()
            .map(|id| fetch_project_items(&client, github, id, &viewer_login)),
    )
    .await;

    let mut all_tasks: Vec<(GhTask, serde_json::Value)> = Vec::new();
    let mut any_ok = false;
    let mut all_ok = true;
    // Kept (not just counted) so the all-failed branch below can ask whether
    // every project failed for a retryable reason.
    let mut failures: Vec<anyhow::Error> = Vec::new();
    for (project_id, result) in github.project_ids.iter().zip(results) {
        match result {
            Ok(tasks) => {
                tracing::debug!(project_id, count = tasks.len(), "fetched project items");
                all_tasks.extend(tasks);
                any_ok = true;
            }
            Err(e) => {
                let chain = format!("{e:#}");
                tracing::warn!(project_id, error = %chain, "github project fetch failed - skipping");
                failures.push(e);
                all_ok = false;
            }
        }
    }

    if !any_ok {
        // One network outage hits every concurrent project fetch identically,
        // so "all of them failed, all for a retryable reason" is the signature
        // of an unreachable network rather than a broken board or credential.
        // Stay quiet and retry; a mixed or terminal set still reaches the user.
        // Report the first TERMINAL failure if there is one; stay quiet only
        // when every single failure was retryable.
        let terminal = failures
            .iter()
            .map(super::http::classify)
            .find_map(|f| match f {
                SyncFault::Report { detail } => Some(detail),
                SyncFault::Retry { .. } => None,
            });
        match terminal {
            // `failures` is never empty here (a non-empty project list that
            // produced no successes produced errors), so this is the
            // all-retryable case.
            None => tracing::warn!(
                projects = failures.len(),
                "github unreachable for every project - keeping stale cache, will retry next sync"
            ),
            Some(detail) => {
                tracing::warn!(
                    error = %detail,
                    "all github project fetches failed - keeping stale cache"
                );
                let _ = super::stamp_sync_error(
                    pool,
                    "github",
                    &format!("GitHub sync failed - every project fetch failed: {detail}"),
                )
                .await;
            }
        }
        return Ok(None);
    }

    let keys: Vec<String> = all_tasks.iter().map(|(t, _)| t.task_key.clone()).collect();
    upsert(pool, &all_tasks).await?;

    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at)
         VALUES ('github', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
    )
    .execute(pool)
    .await
    .context("updating github sync state")?;

    // Prune only when EVERY project fetched successfully. On a partial failure
    // the fetched keys cover just the projects that succeeded, so pruning to
    // them would delete a failed project's still-valid tasks (a transient 500 /
    // rate-limit would wipe unrelated tasks until the next clean sync).
    if !all_ok {
        tracing::warn!(
            "partial github fetch — skipping prune to preserve tasks from failed project(s)"
        );
    } else {
        // `prune` is safe for an empty `keys` slice: it emits `NOT IN ()`
        // (always true in modern SQLite) so it full-clears, and crucially it
        // deletes dependent `pm_task_embeddings` rows BEFORE `pm_tasks`. The old
        // empty-keys fallback deleted `pm_tasks` directly and hit the
        // `pm_task_embeddings.task_key` foreign key, failing the full-clear.
        match prune(pool, &keys).await {
            Ok(0) => {}
            Ok(p) => tracing::info!(pruned_count = p, "pruned stale github tasks"),
            Err(e) => tracing::warn!(error = %e, "github prune failed"),
        }
    }

    tracing::info!(upserted_count = keys.len(), "github tasks refreshed");
    let _ = super::clear_sync_error(pool, "github").await;
    Ok(Some(keys))
}

/// Force an immediate GitHub sync regardless of the staleness gate.
pub async fn force_refresh(
    pool: &SqlitePool,
    github: &GitHubConfig,
) -> Result<Option<Vec<String>>> {
    sqlx::query("DELETE FROM pm_sync_state WHERE provider = 'github'")
        .execute(pool)
        .await
        .context("clearing github sync state for force refresh")?;
    refresh_if_stale(pool, github).await
}
