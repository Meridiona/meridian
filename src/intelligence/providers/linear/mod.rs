//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Linear task connector. Pulls the issues assigned to the authenticated user
// (the API key's owner) into `pm_tasks` so the classifier can link sessions to
// them and the worklog driver can draft against them. Mirrors the Jira connector
// (`jira.rs`): fetch → filter → upsert → prune, gated by `pm_sync_state`.
//
// Linear's API is GraphQL-only (https://api.linear.app/graphql). The personal
// API key is sent RAW in the `Authorization` header (no `Bearer` prefix). We
// fetch the viewer's assigned issues and drop completed/canceled ones in Rust
// (mirrors Jira's `statusCategory != Done`) rather than guessing the server-side
// IssueFilter shape — the task_key we store is the human identifier (`ENG-123`).

use anyhow::{Context, Result};
use meridian_core::adapters::linear::LinearAdapter;
use meridian_core::adapters::ProviderAdapter;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::config::LinearConfig;

const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";
const MAX_RESULTS: usize = 100;
const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// GraphQL response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GqlResponse {
    data: Option<ViewerData>,
    errors: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ViewerData {
    viewer: Option<Viewer>,
}

#[derive(Deserialize)]
struct Viewer {
    #[serde(rename = "assignedIssues")]
    assigned_issues: IssueConnection,
}

#[derive(Deserialize)]
struct IssueConnection {
    nodes: Vec<LinearIssue>,
}

#[derive(Deserialize)]
struct LinearIssue {
    identifier: String,
    title: String,
    description: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    url: Option<String>,
    state: Option<WorkflowState>,
    team: Option<Team>,
    parent: Option<Parent>,
    assignee: Option<NamedUser>,
    #[serde(rename = "dueDate", default)]
    due_date: Option<String>,
    #[serde(rename = "startedAt", default)]
    started_at: Option<String>,
    #[serde(default)]
    labels: Option<LabelConnection>,
    #[serde(default)]
    cycle: Option<Cycle>,
    #[serde(default)]
    project: Option<Project>,
}

#[derive(Deserialize)]
struct WorkflowState {
    /// User-facing state name ("In Review", "Ready for Merge", …) — custom per
    /// team. Stored verbatim as `status_raw`.
    #[serde(default)]
    name: String,
    #[serde(rename = "type")]
    type_: String,
}

#[derive(Deserialize)]
struct Team {
    id: String,
    key: String,
}

#[derive(Deserialize)]
struct Parent {
    identifier: String,
    title: Option<String>,
}

#[derive(Deserialize)]
struct NamedUser {
    name: Option<String>,
}

#[derive(Deserialize)]
struct LabelConnection {
    nodes: Vec<LabelNode>,
}

#[derive(Deserialize)]
struct LabelNode {
    name: String,
}

#[derive(Deserialize)]
struct Cycle {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Project {
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether a Linear workflow-state type is terminal. State types are a fixed,
/// reliable taxonomy: backlog | unstarted | started | completed | canceled |
/// triage. `completed` and `canceled` are done; everything else is open.
fn native_terminal(type_: &str) -> bool {
    matches!(type_, "completed" | "canceled")
}

/// True if this issue should be dropped (already finished). Mirrors Jira's
/// `statusCategory != Done` exclusion so the candidate list stays actionable.
fn is_finished(issue: &LinearIssue) -> bool {
    issue
        .state
        .as_ref()
        .map(|s| matches!(s.type_.as_str(), "completed" | "canceled"))
        .unwrap_or(false)
}

/// Apply the optional team filter. `team_ids` is matched against BOTH the team
/// UUID and the team key (e.g. "ENG") so a user can configure either.
fn team_allowed(issue: &LinearIssue, team_ids: &[String]) -> bool {
    if team_ids.is_empty() {
        return true;
    }
    match &issue.team {
        Some(t) => team_ids.iter().any(|w| w == &t.id || w == &t.key),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip(linear),
    fields(provider = "linear", status_code = tracing::field::Empty)
)]
async fn fetch(linear: &LinearConfig) -> Result<Vec<(LinearIssue, serde_json::Value)>> {
    let query = format!(
        "query {{ viewer {{ assignedIssues(first: {MAX_RESULTS}) {{ nodes {{ \
           id identifier title description updatedAt createdAt completedAt canceledAt url \
           priority estimate \
           state {{ name type }} team {{ id key }} \
           parent {{ id identifier title }} assignee {{ id name displayName email }} \
           creator {{ id displayName email }} \
           dueDate startedAt labels {{ nodes {{ name }} }} cycle {{ id name }} \
           project {{ id name }} \
         }} }} }} }}"
    );
    let body = serde_json::json!({ "query": query });

    let client = reqwest::Client::new();
    let resp = client
        .post(LINEAR_GRAPHQL_URL)
        .header("Authorization", &linear.api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("POST Linear GraphQL")?;

    let status = resp.status();
    tracing::Span::current().record("status_code", status.as_u16() as i64);
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Linear GraphQL → {}: {}", status, text);
    }

    // Parse once as a Value so the raw issue nodes survive verbatim for the
    // canonical adapter (Stage 3b), then deserialise the typed view from the
    // same body. Node order is identical, so we zip them.
    let body_val: serde_json::Value =
        serde_json::from_str(&text).context("deserialising Linear response")?;
    let raw_issues: Vec<serde_json::Value> = body_val
        .get("data")
        .and_then(|d| d.get("viewer"))
        .and_then(|v| v.get("assignedIssues"))
        .and_then(|c| c.get("nodes"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let parsed: GqlResponse =
        serde_json::from_value(body_val).context("parsing Linear response")?;
    if let Some(errors) = &parsed.errors {
        anyhow::bail!("Linear GraphQL errors: {errors}");
    }
    let issues = parsed
        .data
        .and_then(|d| d.viewer)
        .map(|v| v.assigned_issues.nodes)
        .unwrap_or_default();
    tracing::debug!(count = issues.len(), "parsed Linear response");
    Ok(issues.into_iter().zip(raw_issues).collect())
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

/// The CDM columns (migration 056) derived from a raw Linear issue via the
/// shared [`LinearAdapter`]. All `Option` so a structurally-unusable payload
/// simply leaves the column NULL — never blocks the existing upsert. The
/// mapping itself lives in `meridian_core` and is tested there.
#[derive(Default)]
struct CdmColumns {
    canonical_id: Option<String>,
    status_category: Option<String>,
    raw_payload: Option<String>,
    reporter_name: Option<String>,
    completed_at: Option<String>,
    ancestor_path: Option<String>,
    project_ids: Option<String>,
}

fn cdm_columns(raw: &serde_json::Value) -> CdmColumns {
    let Ok(c) = LinearAdapter.to_canonical(raw) else {
        return CdmColumns::default();
    };
    CdmColumns {
        canonical_id: Some(c.canonical_id),
        // Enum → its snake_case serde wire form (e.g. "in_progress").
        status_category: c
            .status_category
            .and_then(|sc| serde_json::to_value(sc).ok())
            .and_then(|v| v.as_str().map(String::from)),
        raw_payload: Some(c.raw_payload.to_string()),
        reporter_name: c.reporter.map(|r| r.display_name),
        completed_at: c.completed_at,
        ancestor_path: serde_json::to_string(&c.ancestor_path).ok(),
        project_ids: serde_json::to_string(&c.project_ids).ok(),
    }
}

async fn upsert(
    pool: &SqlitePool,
    issues: &[(LinearIssue, serde_json::Value)],
    linear: &LinearConfig,
) -> Result<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for (issue, raw) in issues {
        if is_finished(issue) || !team_allowed(issue, &linear.team_ids) {
            continue;
        }
        let status = match issue.state.as_ref() {
            Some(s) => super::status::resolve("linear", &s.name, Some(native_terminal(&s.type_))),
            None => super::status::resolve("linear", "", Some(false)),
        };
        let description = issue.description.clone().unwrap_or_default();
        let url = issue.url.clone().unwrap_or_default();
        let project_key = issue
            .team
            .as_ref()
            .map(|t| t.key.clone())
            .unwrap_or_default();
        // parent.identifier → parent_key (stable grouping key).
        // epic_title: prefer the parent's title; fall back to the Linear project
        // name so tasks inside a project group correctly even without a parent
        // issue hierarchy. When the project is used as the grouping anchor,
        // parent_key is set to "project:<name>" so each project forms its own
        // group in the tasks view (not collapsed into one "__none__" bucket).
        let project_name: Option<String> = issue
            .project
            .as_ref()
            .and_then(|pr| pr.name.clone())
            .filter(|s| !s.is_empty());
        let (parent_key, epic_title): (Option<&str>, String) = match issue.parent.as_ref() {
            Some(p) => (
                Some(p.identifier.as_str()),
                p.title.clone().unwrap_or_default(),
            ),
            None => match &project_name {
                Some(name) => (None, name.clone()),
                None => (None, String::new()),
            },
        };
        // Stable grouping key: parent identifier beats project prefix.
        let epic_key_override: Option<String> = if parent_key.is_none() {
            project_name.as_ref().map(|n| format!("project:{n}"))
        } else {
            None
        };
        let assignee = issue.assignee.as_ref().and_then(|a| a.name.clone());
        let tags: Option<String> = issue.labels.as_ref().map(|lc| {
            lc.nodes
                .iter()
                .map(|l| l.name.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        });
        let sprint_name: Option<String> = issue
            .cycle
            .as_ref()
            .and_then(|c| c.name.clone())
            .filter(|s| !s.is_empty());

        // CDM columns (Stage 3b) from the raw issue via the shared adapter.
        let cdm = cdm_columns(raw);

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, parent_key, epic_title, assignee_name,
                due_date, start_date, tags, sprint_name,
                canonical_id, status_category, raw_payload, reporter_name,
                completed_at, ancestor_path, project_ids,
                updated_at, fetched_at)
             VALUES (?, 'linear', ?, ?, ?, ?, '', ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     ?, ?, ?, ?, ?, ?, ?,
                     ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'linear',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_raw       = excluded.status_raw,
               is_terminal      = excluded.is_terminal,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title,
               assignee_name    = excluded.assignee_name,
               due_date         = excluded.due_date,
               start_date       = excluded.start_date,
               tags             = excluded.tags,
               sprint_name      = excluded.sprint_name,
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
        .bind(&issue.identifier)
        .bind(&issue.title)
        .bind(&description)
        .bind(&status.raw)
        .bind(status.is_terminal)
        .bind(&project_key)
        .bind(&url)
        // Use the real parent_key when present; otherwise use the project prefix
        // so the UI groups project-scoped issues together (not into __none__).
        .bind(parent_key.or(epic_key_override.as_deref()))
        .bind(if epic_title.is_empty() {
            None
        } else {
            Some(epic_title)
        })
        .bind(assignee)
        .bind(&issue.due_date)
        .bind(&issue.started_at)
        .bind(tags.as_deref())
        .bind(sprint_name.as_deref())
        .bind(cdm.canonical_id)
        .bind(cdm.status_category)
        .bind(cdm.raw_payload)
        .bind(cdm.reporter_name)
        .bind(cdm.completed_at)
        .bind(cdm.ancestor_path)
        .bind(cdm.project_ids)
        .bind(&issue.updated_at)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", issue.identifier))?;

        kept.push(issue.identifier.clone());
    }
    Ok(kept)
}

// ---------------------------------------------------------------------------
// Prune (identical shape to the Jira connector, scoped to provider = 'linear')
// ---------------------------------------------------------------------------

async fn prune(pool: &SqlitePool, fetched_keys: &[String]) -> Result<usize> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    let emb_sql = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks WHERE provider = 'linear' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&emb_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q.execute(pool)
        .await
        .context("pruning linear pm_task_embeddings")?;

    let task_sql = format!(
        "DELETE FROM pm_tasks WHERE provider = 'linear' AND task_key NOT IN ({placeholders})"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning linear pm_tasks")?;
    Ok(result.rows_affected() as usize)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, linear))]
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    linear: &LinearConfig,
) -> Result<Option<Vec<String>>> {
    let threshold = format!("-{SYNC_INTERVAL_MINS} minutes");
    let (is_fresh,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = 'linear'
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking linear sync state")?;

    if is_fresh != 0 {
        return Ok(None);
    }

    match fetch(linear).await {
        Ok(issues) => {
            let raw_count = issues.len();
            let kept = upsert(pool, &issues, linear).await?;
            let n = kept.len();
            sqlx::query(
                "INSERT INTO pm_sync_state (provider, last_synced_at)
                 VALUES ('linear', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
            )
            .execute(pool)
            .await
            .context("updating linear sync state")?;

            // Only prune when the response was not truncated — otherwise tasks
            // beyond the first page would be wrongly deleted.
            if raw_count < MAX_RESULTS {
                if !kept.is_empty() {
                    match prune(pool, &kept).await {
                        Ok(0) => {}
                        Ok(p) => tracing::info!(pruned_count = p, "pruned stale linear tasks"),
                        Err(e) => tracing::warn!(error = %e, "linear prune failed"),
                    }
                } else {
                    // No live tasks at all → delete every linear row.
                    if let Err(e) = sqlx::query("DELETE FROM pm_tasks WHERE provider = 'linear'")
                        .execute(pool)
                        .await
                    {
                        tracing::warn!(error = %e, "linear full-clear failed");
                    }
                }
            }
            tracing::info!(upserted_count = n, "linear tasks refreshed");
            let _ = super::clear_sync_error(pool, "linear").await;
            Ok(Some(kept))
        }
        Err(e) => {
            tracing::warn!(error = %e, "linear fetch failed — keeping stale cache");
            let _ =
                super::stamp_sync_error(pool, "linear", &format!("Linear sync failed — {e}")).await;
            Ok(None)
        }
    }
}

/// Force an immediate Linear sync regardless of the staleness gate.
pub async fn force_refresh(
    pool: &SqlitePool,
    linear: &LinearConfig,
) -> Result<Option<Vec<String>>> {
    sqlx::query("DELETE FROM pm_sync_state WHERE provider = 'linear'")
        .execute(pool)
        .await
        .context("clearing linear sync state for force refresh")?;
    refresh_if_stale(pool, linear).await
}

#[cfg(test)]
mod tests;
