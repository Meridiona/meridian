//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use anyhow::{Context, Result};
use meridian_core::adapters::jira::JiraAdapter;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::config::JiraConfig;
use crate::intelligence::oauth::jira::JiraReqCtx;

mod active_filter;
mod fetch;
mod refresh;
#[cfg(test)]
mod tests;

use fetch::{discover_start_date_field, fetch, fetch_by_keys, MAX_RESULTS};

// The refresh workflow lives in `refresh.rs` (see its header for the seam);
// re-exported so every existing `providers::jira::refresh_if_stale` call site
// keeps working.
pub use refresh::{force_refresh, refresh_if_stale};

// ---------------------------------------------------------------------------
// Jira REST response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JiraIssue {
    key: String,
    fields: JiraFields,
}

#[derive(Deserialize)]
struct JiraFields {
    summary: String,
    description: Option<serde_json::Value>,
    status: JiraStatus,
    issuetype: JiraIssueType,
    project: JiraProject,
    updated: String,
    #[serde(rename = "parent")]
    parent: Option<JiraParent>,
    #[serde(default)]
    duedate: Option<String>,
    #[serde(default)]
    assignee: Option<JiraUser>,
    #[serde(default)]
    labels: Vec<String>,
    // Sprint custom field — Cloud standard; value is an array of sprint objects.
    #[serde(rename = "customfield_10020", default)]
    sprint: Option<Vec<JiraSprint>>,
    // Remaining fields captured for dynamic start-date extraction.
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct JiraUser {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct JiraSprint {
    name: Option<String>,
}

#[derive(Deserialize)]
struct JiraParent {
    key: String,
    fields: Option<JiraParentFields>,
}

#[derive(Deserialize)]
struct JiraParentFields {
    summary: Option<String>,
}

#[derive(Deserialize)]
struct JiraStatus {
    /// The user-facing status name ("In Review", "Awaiting QA", …) — custom per
    /// workflow. Stored verbatim as `status_raw`.
    #[serde(default)]
    name: String,
    #[serde(rename = "statusCategory")]
    status_category: JiraStatusCategory,
}

#[derive(Deserialize)]
struct JiraStatusCategory {
    key: String,
}

#[derive(Deserialize)]
struct JiraIssueType {
    name: String,
    /// Jira's own rung number for this type: `-1` sub-task, `0` standard work
    /// (Task/Story/Bug/Feature), `1` Epic, `2`+ the Premium/Advanced-Roadmaps
    /// container tiers (Initiative, Capability, and custom ones).
    ///
    /// This is what `fetch::is_work_item` filters on, and it is the reason the
    /// scope is correct on boards we have never seen: a container's NAME is
    /// arbitrary (plenty of orgs configure "Feature" as a tier above Story)
    /// but its LEVEL is not.
    ///
    /// `Option` because older Jira Server/Data Center responses omit the field.
    /// Absent means "unknown", and unknown is treated as work — see
    /// `is_work_item` for why erring toward inclusion is the right failure.
    #[serde(rename = "hierarchyLevel", default)]
    hierarchy_level: Option<i64>,
}

#[derive(Deserialize)]
struct JiraProject {
    key: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn adf_to_plaintext(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(obj) => {
            let mut parts = Vec::new();
            if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_owned());
                }
            }
            if let Some(content) = obj.get("content").and_then(|v| v.as_array()) {
                for node in content {
                    let part = adf_to_plaintext(node);
                    if !part.is_empty() {
                        parts.push(part);
                    }
                }
            }
            parts.join(" ")
        }
        _ => String::new(),
    }
}

/// Jira's `statusCategory.key` is a fixed, non-customisable semantic field:
/// `done` / `indeterminate` / `new`. It is reliable for those three, but Jira
/// Service Management (and misconfigured Server/Data-Center workflows) can emit
/// `undefined` ("No Category"). For `undefined` we return `None` so the keyword
/// heuristic on the raw status name — and any user override — still gets a say,
/// rather than blindly treating an unlabelled status as open.
fn native_terminal(category_key: &str) -> Option<bool> {
    match category_key {
        "done" => Some(true),
        "new" | "indeterminate" => Some(false),
        _ => None,
    }
}

// Minimum interval between Jira fetches. Refresh is now triggered on demand at
// the read boundaries (classification + worklog passes), so this gate exists to
// dedupe bursts (e.g. the one-session-per-tick classifier drain loop) and bound
// API load — not to set the freshness cadence. Kept short so the candidate
// ticket list is at most this stale when a session is classified.
const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(
    pool: &SqlitePool,
    issues: &[(JiraIssue, serde_json::Value)],
    jira: &JiraConfig,
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
) -> Result<()> {
    let mut ok_count: usize = 0;
    for (issue, raw) in issues {
        if !jira.project_keys.is_empty() && !jira.project_keys.contains(&issue.fields.project.key) {
            continue;
        }

        let description = issue
            .fields
            .description
            .as_ref()
            .map(adf_to_plaintext)
            .unwrap_or_default();

        let status = super::status::resolve(
            "jira",
            &issue.fields.status.name,
            native_terminal(&issue.fields.status.status_category.key),
        );
        let url = ctx.browse_url(&issue.key);

        let (parent_key, epic_title) = issue
            .fields
            .parent
            .as_ref()
            .map(|p| {
                let title = p
                    .fields
                    .as_ref()
                    .and_then(|f| f.summary.as_deref())
                    .unwrap_or("");
                (Some(p.key.as_str()), title)
            })
            .unwrap_or((None, ""));

        let assignee_name = issue
            .fields
            .assignee
            .as_ref()
            .and_then(|a| a.display_name.clone());

        let tags: Option<String> = if issue.fields.labels.is_empty() {
            None
        } else {
            Some(issue.fields.labels.join(", "))
        };

        let sprint_name = issue
            .fields
            .sprint
            .as_deref()
            .and_then(|sprints| sprints.first())
            .and_then(|s| s.name.clone());

        let start_date: Option<String> = start_date_field.and_then(|field_id| {
            issue
                .fields
                .extra
                .get(field_id)?
                .as_str()
                .map(str::to_owned)
        });

        // CDM columns (Stage 3b) from the raw issue via the shared adapter.
        let cdm = super::cdm::derive(&JiraAdapter, raw);

        let upsert_result = sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, parent_key, epic_title, due_date,
                assignee_name, tags, sprint_name, start_date,
                canonical_id, status_category, raw_payload, reporter_name,
                completed_at, ancestor_path, project_ids,
                updated_at, fetched_at)
             VALUES (?, 'jira', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     ?, ?, ?, ?, ?, ?, ?,
                     ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               title            = excluded.title,
               description_text = excluded.description_text,
               status_raw       = excluded.status_raw,
               is_terminal      = excluded.is_terminal,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title,
               due_date         = excluded.due_date,
               assignee_name    = excluded.assignee_name,
               tags             = excluded.tags,
               sprint_name      = excluded.sprint_name,
               start_date       = excluded.start_date,
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
        .bind(&issue.key)
        .bind(&issue.fields.summary)
        .bind(&description)
        .bind(&status.raw)
        .bind(status.is_terminal)
        .bind(&issue.fields.issuetype.name)
        .bind(&issue.fields.project.key)
        .bind(&url)
        .bind(parent_key)
        .bind(if epic_title.is_empty() {
            None
        } else {
            Some(epic_title)
        })
        .bind(&issue.fields.duedate)
        .bind(assignee_name)
        .bind(tags)
        .bind(sprint_name)
        .bind(start_date)
        .bind(cdm.canonical_id)
        .bind(cdm.status_category)
        .bind(cdm.raw_payload)
        .bind(cdm.reporter_name)
        .bind(cdm.completed_at)
        .bind(cdm.ancestor_path)
        .bind(cdm.project_ids)
        .bind(&issue.fields.updated)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", issue.key));
        match upsert_result {
            Ok(_) => ok_count += 1,
            Err(ref upsert_err) => {
                tracing::warn!(task_key = %issue.key, error = ?upsert_err, "jira task upsert failed — skipping");
            }
        }
    }
    if !issues.is_empty() && ok_count == 0 {
        anyhow::bail!(
            "all {} jira task upserts failed — DB write errors above",
            issues.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prune
// ---------------------------------------------------------------------------

/// Delete `pm_tasks` rows no longer returned by the active-task fetch (closed,
/// reassigned, etc.) — EXCEPT a task_key that has worklog history
/// (`pm_worklogs`) or sits on a daily plan (`daily_plan`), both kept forever:
/// worklog history so a completed ticket's title never disappears from the
/// timeline once it's Done, daily-plan membership so closing a "today's focus"
/// item from the plan checkbox doesn't delete the very row its Undo (reopen)
/// needs — see `src/plan_tasks/done.rs`.
async fn prune(pool: &SqlitePool, fetched_keys: &[String]) -> Result<usize> {
    // Nothing fetched → prune nothing, deliberately.
    //
    // The tempting reading is "the active scope returned no tickets, so every
    // jira row is stale" — and that would delete the user's whole board. An
    // empty result has two causes we cannot tell apart here: the user really has
    // nothing open, and a transient scope/permission problem that answers 200
    // with zero issues. Wrongly deleting is expensive and looks like data loss;
    // wrongly keeping leaves stale rows that the next sync clears. So we keep.
    //
    // It is also a correctness fix, not only a policy one: an empty slice
    // renders `NOT IN ()`, which SQLite rejects. The old code relied on the
    // caller to gate (`tests::prune_with_empty_fetched_keys_is_a_no_op` says as
    // much) and the caller never did — so this ran, failed, and was swallowed as
    // a warning. The filtering in `fetch` widened the ways to reach it: `keys`
    // is now empty whenever every returned row was a container tier, not just
    // when the user has no open work.
    if fetched_keys.is_empty() {
        tracing::debug!("jira: active fetch returned no work items — skipping prune");
        return Ok(0);
    }
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    // Delete embeddings first — pm_task_embeddings.task_key FK references pm_tasks.
    let emb_sql = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&emb_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q.execute(pool)
        .await
        .context("pruning pm_task_embeddings")?;

    let task_sql = format!(
        "DELETE FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ({placeholders}) \
         AND task_key NOT IN (SELECT DISTINCT task_key FROM pm_worklogs) \
         AND task_key NOT IN (SELECT DISTINCT task_key FROM daily_plan)"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning pm_tasks")?;

    // Worklog-retained rows the DELETE above kept but that are no longer in the
    // active fetch (Done, reassigned, out-of-scope type) must leave the board.
    let flagged = super::mark_retained_offboard(pool, "jira", fetched_keys).await?;
    if flagged > 0 {
        tracing::info!(flagged, "flagged retained jira tasks off-board");
    }
    Ok(result.rows_affected() as usize)
}

// ---------------------------------------------------------------------------
// Backfill (worklogged tasks the active-task scope will never surface again)
// ---------------------------------------------------------------------------

/// Batch size for `fetch_by_keys` JQL calls — keeps the request well under
/// Jira's JQL length limits and `MAX_RESULTS` per page.
const BACKFILL_BATCH_SIZE: usize = 50;

/// Fetch and upsert a `pm_tasks` row for any Jira task_key that has worklog
/// history but is missing from `pm_tasks` — a ticket that's Done, reassigned,
/// or otherwise fell outside [`fetch`]'s active-task JQL and so was never (or
/// no longer) covered by the regular sync. Without this, such a ticket's
/// title can never appear on the timeline: `fetch`'s JQL will never surface
/// it again, no matter how many times the regular sync runs.
///
/// Called only from inside a real (cache-stale) sync cycle in
/// [`refresh_if_stale`], so it reuses that cycle's already-resolved `ctx` and
/// `start_date_field` rather than re-resolving auth + re-discovering the field
/// itself. This is deliberate: it must **not** run on every tick.
///
/// A permanently-deleted/moved Jira ticket that still has `pm_worklogs` rows
/// never comes back from `fetch_by_keys`, so it stays in the `missing` set. By
/// gating this behind the [`SYNC_INTERVAL_MINS`] freshness check, a dead ticket
/// is re-attempted at most once per sync cycle (not once per poll tick), which
/// bounds the wasted network calls to the same cadence as the regular sync —
/// no rate-limit hazard. (A dedicated tombstone to stop re-fetching a confirmed
/// dead ticket entirely would need its own column and is left as follow-up.)
async fn backfill_worklogged(
    pool: &SqlitePool,
    jira: &JiraConfig,
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
) -> Result<usize> {
    let missing: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT task_key FROM pm_worklogs
         WHERE provider = 'jira' AND task_key NOT IN
           (SELECT task_key FROM pm_tasks WHERE provider = 'jira')",
    )
    .fetch_all(pool)
    .await
    .context("finding jira worklogs missing a pm_tasks row")?;
    if missing.is_empty() {
        return Ok(0);
    }
    let keys: Vec<String> = missing.into_iter().map(|(k,)| k).collect();

    let mut backfilled = 0usize;
    for batch in keys.chunks(BACKFILL_BATCH_SIZE) {
        let issues = fetch_by_keys(ctx, start_date_field, batch)
            .await
            .context("backfilling jira worklogged tasks")?;
        backfilled += issues.len();
        upsert(pool, &issues, jira, ctx, start_date_field).await?;
    }
    if backfilled > 0 {
        tracing::info!(
            requested = keys.len(),
            backfilled,
            "backfilled pm_tasks rows for worklogged jira tickets"
        );
    }
    Ok(backfilled)
}
