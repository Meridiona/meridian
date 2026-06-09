// meridian — normalises screenpipe activity into structured app sessions
//
// GitHub task connector. Fetches open issues assigned to the viewer from
// configured GitHub Projects v2 (GraphQL API). task_key is `owner/repo#number`.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::GitHubConfig;

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// GraphQL response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
}

#[derive(Deserialize)]
struct ViewerData {
    viewer: Viewer,
}

#[derive(Deserialize)]
struct Viewer {
    login: String,
}

#[derive(Deserialize)]
struct ProjectData {
    node: Option<ProjectNode>,
}

#[derive(Deserialize)]
struct ProjectNode {
    items: ProjectItemConnection,
}

#[derive(Deserialize)]
struct ProjectItemConnection {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<ProjectItem>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct ProjectItem {
    #[serde(rename = "type")]
    item_type: Option<String>,
    #[serde(rename = "fieldValues")]
    field_values: FieldValueConnection,
    // Raw JSON, parsed into IssueContent per ISSUE item. GitHub returns an empty
    // `{}` for non-Issue content (PRs, draft issues), which would fail a typed
    // Option<IssueContent> on the whole response and skip the entire project.
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct FieldValueConnection {
    nodes: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct IssueContent {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    url: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    repository: Repo,
    assignees: AssigneeConnection,
}

#[derive(Deserialize)]
struct Repo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize)]
struct AssigneeConnection {
    nodes: Vec<LoginNode>,
}

#[derive(Deserialize)]
struct LoginNode {
    login: String,
}

/// One normalised issue ready to upsert.
struct GhTask {
    task_key: String,
    repo_slug: String,
    title: String,
    body: String,
    status: &'static str,
    url: String,
    updated_at: String,
    assignee: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a GitHub Project Status field value to meridian's status_category.
fn map_project_status(field_values: &[serde_json::Value]) -> &'static str {
    for fv in field_values {
        let field_name = fv.pointer("/field/name").and_then(|v| v.as_str());
        let value_name = fv.get("name").and_then(|v| v.as_str());
        if let (Some(f), Some(v)) = (field_name, value_name) {
            if f.eq_ignore_ascii_case("status") {
                let lower = v.to_lowercase();
                return if lower.contains("progress") || lower.contains("doing") {
                    "in_progress"
                } else if lower.contains("done")
                    || lower.contains("complete")
                    || lower.contains("closed")
                {
                    "done"
                } else {
                    "todo"
                };
            }
        }
    }
    "todo"
}

fn post_graphql(
    client: &reqwest::Client,
    github: &GitHubConfig,
    body: serde_json::Value,
) -> reqwest::RequestBuilder {
    client
        .post(GRAPHQL_URL)
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Content-Type", "application/json")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
        .json(&body)
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

async fn fetch_viewer_login(github: &GitHubConfig) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = post_graphql(&client, github, json!({ "query": "{ viewer { login } }" }))
        .send()
        .await
        .context("POST /graphql viewer")?;
    let text = resp.text().await.unwrap_or_default();
    let parsed: GqlResponse<ViewerData> =
        serde_json::from_str(&text).context("deserialising viewer response")?;
    parsed
        .data
        .map(|d| d.viewer.login)
        .ok_or_else(|| anyhow::anyhow!("GraphQL viewer response missing data"))
}

const PROJECT_ITEMS_QUERY: &str = "query($id: ID!, $cursor: String) {
  node(id: $id) {
    ... on ProjectV2 {
      items(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          type
          fieldValues(first: 8) {
            nodes {
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2SingleSelectField { name } }
              }
            }
          }
          content {
            ... on Issue {
              number title body state url updatedAt
              repository { nameWithOwner }
              assignees(first: 10) { nodes { login } }
            }
          }
        }
      }
    }
  }
}";

#[tracing::instrument(skip(client, github), fields(project_id))]
async fn fetch_project_items(
    client: &reqwest::Client,
    github: &GitHubConfig,
    project_id: &str,
    viewer_login: &str,
) -> Result<Vec<GhTask>> {
    let mut tasks = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let body = json!({
            "query": PROJECT_ITEMS_QUERY,
            "variables": { "id": project_id, "cursor": cursor }
        });
        let resp = post_graphql(client, github, body)
            .send()
            .await
            .context("POST /graphql project items")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GitHub GraphQL → {}: {}", status, text);
        }

        let parsed: GqlResponse<ProjectData> =
            serde_json::from_str(&text).context("deserialising project items")?;

        let project = match parsed.data.and_then(|d| d.node) {
            Some(p) => p,
            None => {
                tracing::warn!(project_id, "GitHub project not found or inaccessible");
                break;
            }
        };

        for item in &project.items.nodes {
            if item.item_type.as_deref() != Some("ISSUE") {
                continue;
            }
            // Parse the raw content per ISSUE item; a PR/draft `{}` simply yields
            // None here instead of failing the whole project's deserialisation.
            let content: IssueContent = match item
                .content
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                Some(c) => c,
                None => continue,
            };
            if content.state != "OPEN" {
                continue;
            }
            if !content
                .assignees
                .nodes
                .iter()
                .any(|a| a.login.eq_ignore_ascii_case(viewer_login))
            {
                continue;
            }
            tasks.push(GhTask {
                task_key: format!("{}#{}", content.repository.name_with_owner, content.number),
                repo_slug: content.repository.name_with_owner.clone(),
                title: content.title.clone(),
                body: content.body.clone().unwrap_or_default(),
                status: map_project_status(&item.field_values.nodes),
                url: content.url.clone(),
                updated_at: content.updated_at.clone(),
                assignee: viewer_login.to_string(),
            });
        }

        if !project.items.page_info.has_next_page {
            break;
        }
        cursor = project.items.page_info.end_cursor;
    }

    Ok(tasks)
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(pool: &SqlitePool, tasks: &[GhTask]) -> Result<()> {
    for t in tasks {
        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, assignee_name, updated_at, fetched_at)
             VALUES (?, 'github', ?, ?, ?, 'Issue', ?, ?, ?, ?,
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'github',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_category  = excluded.status_category,
               project_key      = excluded.project_key,
               url              = excluded.url,
               assignee_name    = excluded.assignee_name,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at",
        )
        .bind(&t.task_key)
        .bind(&t.title)
        .bind(&t.body)
        .bind(t.status)
        .bind(&t.repo_slug)
        .bind(&t.url)
        .bind(&t.assignee)
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
        "DELETE FROM pm_tasks WHERE provider = 'github' AND task_key NOT IN ({placeholders})"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning github pm_tasks")?;
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
            tracing::warn!(error = %e, "github viewer fetch failed — keeping stale cache");
            return Ok(None);
        }
    };

    let client = reqwest::Client::new();

    // Each project is an independent paginated GraphQL walk — fetch them all
    // concurrently, then fold the results preserving any_ok/all_ok semantics.
    let results = futures::future::join_all(
        github
            .project_ids
            .iter()
            .map(|id| fetch_project_items(&client, github, id, &viewer_login)),
    )
    .await;

    let mut all_tasks: Vec<GhTask> = Vec::new();
    let mut any_ok = false;
    let mut all_ok = true;
    for (project_id, result) in github.project_ids.iter().zip(results) {
        match result {
            Ok(tasks) => {
                tracing::debug!(project_id, count = tasks.len(), "fetched project items");
                all_tasks.extend(tasks);
                any_ok = true;
            }
            Err(e) => {
                tracing::warn!(project_id, error = %e, "github project fetch failed — skipping");
                all_ok = false;
            }
        }
    }

    if !any_ok {
        tracing::warn!("all github project fetches failed — keeping stale cache");
        return Ok(None);
    }

    let keys: Vec<String> = all_tasks.iter().map(|t| t.task_key.clone()).collect();
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
    } else if !keys.is_empty() {
        match prune(pool, &keys).await {
            Ok(0) => {}
            Ok(p) => tracing::info!(pruned_count = p, "pruned stale github tasks"),
            Err(e) => tracing::warn!(error = %e, "github prune failed"),
        }
    } else if let Err(e) = sqlx::query("DELETE FROM pm_tasks WHERE provider = 'github'")
        .execute(pool)
        .await
    {
        tracing::warn!(error = %e, "github full-clear failed");
    }

    tracing::info!(upserted_count = keys.len(), "github tasks refreshed");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status_field(value: &str) -> serde_json::Value {
        serde_json::json!({
            "name": value,
            "field": { "name": "Status" }
        })
    }

    #[test]
    fn mixed_content_project_deserialises() {
        // A real Projects v2 board mixes Issues with PRs and draft items. GitHub
        // returns `content: {}` for a PR/draft under our `... on Issue` selection
        // and `null` for a redacted item. Before the fix these broke the whole
        // ProjectData parse (typed Option<IssueContent>), skipping the project.
        let json = r#"{
          "data": { "node": { "items": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [
              { "type": "ISSUE", "fieldValues": { "nodes": [] },
                "content": { "number": 5, "title": "real issue", "state": "OPEN",
                             "url": "https://x/5", "updatedAt": "2026-01-01T00:00:00Z",
                             "repository": { "nameWithOwner": "org/repo" },
                             "assignees": { "nodes": [{ "login": "me" }] } } },
              { "type": "PULL_REQUEST", "fieldValues": { "nodes": [] }, "content": {} },
              { "type": "DRAFT_ISSUE", "fieldValues": { "nodes": [] }, "content": null }
            ]
          } } }
        }"#;
        let parsed: GqlResponse<ProjectData> =
            serde_json::from_str(json).expect("mixed-content response must deserialise");
        let nodes = parsed.data.unwrap().node.unwrap().items.nodes;
        assert_eq!(nodes.len(), 3);

        // The ISSUE item's raw content parses into IssueContent…
        let issue: IssueContent =
            serde_json::from_value(nodes[0].content.clone().unwrap()).unwrap();
        assert_eq!(issue.number, 5);
        assert_eq!(issue.repository.name_with_owner, "org/repo");

        // …while the PR's `{}` does not — so the loop skips it instead of failing.
        assert!(serde_json::from_value::<IssueContent>(nodes[1].content.clone().unwrap()).is_err());
        assert!(nodes[2].content.is_none());
    }

    #[test]
    fn project_status_todo() {
        assert_eq!(map_project_status(&[status_field("Todo")]), "todo");
        assert_eq!(map_project_status(&[]), "todo");
    }

    #[test]
    fn project_status_in_progress() {
        assert_eq!(
            map_project_status(&[status_field("In Progress")]),
            "in_progress"
        );
        assert_eq!(map_project_status(&[status_field("Doing")]), "in_progress");
    }

    #[test]
    fn project_status_done() {
        assert_eq!(map_project_status(&[status_field("Done")]), "done");
        assert_eq!(map_project_status(&[status_field("Completed")]), "done");
    }

    #[test]
    fn project_status_unknown_defaults_todo() {
        // Columns that aren't clearly "in progress" or "done" fall back to todo,
        // so a Backlog / In Review / Blocked issue still surfaces as an open task.
        assert_eq!(map_project_status(&[status_field("Backlog")]), "todo");
        assert_eq!(map_project_status(&[status_field("In Review")]), "todo");
        assert_eq!(map_project_status(&[status_field("Blocked")]), "todo");
    }

    #[test]
    fn non_status_field_ignored() {
        let priority_field = serde_json::json!({
            "name": "High",
            "field": { "name": "Priority" }
        });
        assert_eq!(map_project_status(&[priority_field]), "todo");
    }
}
