// meridian — normalises screenpipe activity into structured app sessions
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
}

#[derive(Deserialize)]
struct WorkflowState {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a Linear workflow-state type to meridian's status_category. State types:
/// backlog | unstarted | started | completed | canceled | triage.
fn map_state_type(type_: &str) -> &'static str {
    match type_ {
        "completed" | "canceled" => "done",
        "started" => "in_progress",
        _ => "todo",
    }
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
async fn fetch(linear: &LinearConfig) -> Result<Vec<LinearIssue>> {
    let query = format!(
        "query {{ viewer {{ assignedIssues(first: {MAX_RESULTS}) {{ nodes {{ \
           identifier title description updatedAt url \
           state {{ type }} team {{ id key }} \
           parent {{ identifier title }} assignee {{ name }} \
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

    let parsed: GqlResponse =
        serde_json::from_str(&text).context("deserialising Linear response")?;
    if let Some(errors) = &parsed.errors {
        anyhow::bail!("Linear GraphQL errors: {errors}");
    }
    let issues = parsed
        .data
        .and_then(|d| d.viewer)
        .map(|v| v.assigned_issues.nodes)
        .unwrap_or_default();
    tracing::debug!(count = issues.len(), "parsed Linear response");
    Ok(issues)
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(
    pool: &SqlitePool,
    issues: &[LinearIssue],
    linear: &LinearConfig,
) -> Result<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for issue in issues {
        if is_finished(issue) || !team_allowed(issue, &linear.team_ids) {
            continue;
        }
        let status = issue
            .state
            .as_ref()
            .map(|s| map_state_type(&s.type_))
            .unwrap_or("todo");
        let description = issue.description.clone().unwrap_or_default();
        let url = issue.url.clone().unwrap_or_default();
        let project_key = issue
            .team
            .as_ref()
            .map(|t| t.key.clone())
            .unwrap_or_default();
        let (parent_key, epic_title) = issue
            .parent
            .as_ref()
            .map(|p| {
                (
                    Some(p.identifier.as_str()),
                    p.title.clone().unwrap_or_default(),
                )
            })
            .unwrap_or((None, String::new()));
        let assignee = issue.assignee.as_ref().and_then(|a| a.name.clone());

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, parent_key, epic_title, assignee_name,
                updated_at, fetched_at)
             VALUES (?, 'linear', ?, ?, ?, '', ?, ?, ?, ?, ?, ?,
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'linear',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_category  = excluded.status_category,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title,
               assignee_name    = excluded.assignee_name,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at",
        )
        .bind(&issue.identifier)
        .bind(&issue.title)
        .bind(&description)
        .bind(status)
        .bind(&project_key)
        .bind(&url)
        .bind(parent_key)
        .bind(if epic_title.is_empty() {
            None
        } else {
            Some(epic_title)
        })
        .bind(assignee)
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
            Ok(Some(kept))
        }
        Err(e) => {
            tracing::warn!(error = %e, "linear fetch failed — keeping stale cache");
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
mod tests {
    use super::*;

    #[test]
    fn state_mapping() {
        assert_eq!(map_state_type("started"), "in_progress");
        assert_eq!(map_state_type("completed"), "done");
        assert_eq!(map_state_type("canceled"), "done");
        assert_eq!(map_state_type("backlog"), "todo");
        assert_eq!(map_state_type("triage"), "todo");
    }

    fn issue_with(team_id: &str, team_key: &str, state: &str) -> LinearIssue {
        LinearIssue {
            identifier: "ENG-1".into(),
            title: "t".into(),
            description: None,
            updated_at: "2026-06-01T00:00:00.000Z".into(),
            url: None,
            state: Some(WorkflowState {
                type_: state.into(),
            }),
            team: Some(Team {
                id: team_id.into(),
                key: team_key.into(),
            }),
            parent: None,
            assignee: None,
        }
    }

    #[test]
    fn finished_issues_detected() {
        assert!(is_finished(&issue_with("u", "ENG", "completed")));
        assert!(is_finished(&issue_with("u", "ENG", "canceled")));
        assert!(!is_finished(&issue_with("u", "ENG", "started")));
    }

    #[test]
    fn team_filter_matches_id_or_key() {
        let i = issue_with("uuid-123", "ENG", "started");
        assert!(team_allowed(&i, &[])); // empty = all
        assert!(team_allowed(&i, &["uuid-123".into()])); // by id
        assert!(team_allowed(&i, &["ENG".into()])); // by key
        assert!(!team_allowed(&i, &["OTHER".into()]));
    }

    #[test]
    fn parses_assigned_issues_response() {
        let raw = r#"{"data":{"viewer":{"assignedIssues":{"nodes":[
            {"identifier":"ENG-12","title":"Fix bug","description":"d","updatedAt":"2026-06-01T00:00:00.000Z",
             "url":"https://linear.app/x/issue/ENG-12","state":{"type":"started"},
             "team":{"id":"t1","key":"ENG"},"project":{"name":"P"},
             "parent":{"identifier":"ENG-1","title":"Epic"},"assignee":{"name":"Sam"}}
        ]}}}}"#;
        let parsed: GqlResponse = serde_json::from_str(raw).unwrap();
        let nodes = parsed.data.unwrap().viewer.unwrap().assigned_issues.nodes;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].identifier, "ENG-12");
        assert_eq!(nodes[0].state.as_ref().unwrap().type_, "started");
        assert_eq!(nodes[0].parent.as_ref().unwrap().identifier, "ENG-1");
    }
}
