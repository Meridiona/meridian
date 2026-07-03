//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! GitHub fetch path — the Projects v2 GraphQL walk (viewer login, paginated
//! item query, board-Status extraction).
//!
//! Split from the connector root purely for file size; [`super::GhTask`] (the
//! normalised row the upsert consumes) stays in the parent.
//!
//! # Who calls this
//! [`super::refresh_if_stale`], via `fetch::{fetch_viewer_login, fetch_project_items}`.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

use super::GhTask;
use crate::config::GitHubConfig;

const GRAPHQL_URL: &str = "https://api.github.com/graphql";

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
    nodes: Vec<Option<ProjectItem>>,
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
    #[serde(default)]
    labels: GhLabelConnection,
}

#[derive(Deserialize)]
struct Repo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize)]
struct AssigneeConnection {
    nodes: Vec<Option<LoginNode>>,
}

#[derive(Deserialize)]
struct LoginNode {
    login: String,
}

#[derive(Deserialize, Default)]
struct GhLabelConnection {
    nodes: Vec<GhLabel>,
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the verbatim value of the Projects v2 "Status" column, if set.
/// GitHub gives us only the raw column name — there is no semantic category —
/// so we hand it to the shared resolver to decide `is_terminal`.
fn extract_status_raw(field_values: &[serde_json::Value]) -> String {
    for fv in field_values {
        let field_name = fv.pointer("/field/name").and_then(|v| v.as_str());
        let value_name = fv.get("name").and_then(|v| v.as_str());
        if let (Some(f), Some(v)) = (field_name, value_name) {
            if f.eq_ignore_ascii_case("status") {
                return v.to_string();
            }
        }
    }
    String::new()
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

pub(super) async fn fetch_viewer_login(github: &GitHubConfig) -> Result<String> {
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

// The Issue selection carries the extra fields the canonical adapter (CDM
// Stage 3b) needs beyond the typed IssueContent view: the global node `id`
// (stable key), createdAt/closedAt/stateReason (category + completed_at),
// issueType, author (reporter), parent (sub-issue hierarchy), and per-assignee
// ids/names. `project { id }` on the item feeds project_ids.
const PROJECT_ITEMS_QUERY: &str = "query($id: ID!, $cursor: String) {
  node(id: $id) {
    ... on ProjectV2 {
      items(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          type
          project { id }
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
              id number title body state stateReason url
              createdAt updatedAt closedAt
              repository { nameWithOwner }
              issueType { name }
              author { login ... on User { id name } }
              parent { id }
              assignees(first: 10) { nodes { id login name } }
              labels(first: 10) { nodes { name } }
            }
          }
        }
      }
    }
  }
}";

#[tracing::instrument(skip(client, github), fields(project_id))]
pub(super) async fn fetch_project_items(
    client: &reqwest::Client,
    github: &GitHubConfig,
    project_id: &str,
    viewer_login: &str,
) -> Result<Vec<(GhTask, serde_json::Value)>> {
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

        // Parse once as a Value so each raw item node survives verbatim for
        // the canonical adapter (CDM Stage 3b), then deserialise the typed
        // view from the same body. Node order is identical, so we zip them.
        let body_val: serde_json::Value =
            serde_json::from_str(&text).context("deserialising project items")?;
        let raw_nodes: Vec<serde_json::Value> = body_val
            .pointer("/data/node/items/nodes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let parsed: GqlResponse<ProjectData> =
            serde_json::from_value(body_val).context("parsing project items")?;

        let project = match parsed.data.and_then(|d| d.node) {
            Some(p) => p,
            None => {
                tracing::warn!(project_id, "GitHub project not found or inaccessible");
                break;
            }
        };

        for (item, raw) in project.items.nodes.iter().zip(raw_nodes.iter()) {
            // A literal null node (redacted/inaccessible item) is skipped.
            let Some(item) = item else { continue };
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
                .flatten()
                .any(|a| a.login.eq_ignore_ascii_case(viewer_login))
            {
                continue;
            }
            let resolved = crate::intelligence::providers::status::resolve(
                "github",
                &extract_status_raw(&item.field_values.nodes),
                None,
            );
            let label_names: Vec<&str> = content
                .labels
                .nodes
                .iter()
                .map(|l| l.name.as_str())
                .filter(|s| !s.is_empty())
                .collect();
            let tags = if label_names.is_empty() {
                None
            } else {
                Some(label_names.join(", "))
            };
            tasks.push((
                GhTask {
                    task_key: format!("{}#{}", content.repository.name_with_owner, content.number),
                    repo_slug: content.repository.name_with_owner.clone(),
                    title: content.title.clone(),
                    body: content.body.clone().unwrap_or_default(),
                    status_raw: resolved.raw,
                    is_terminal: resolved.is_terminal,
                    url: content.url.clone(),
                    updated_at: content.updated_at.clone(),
                    assignee: viewer_login.to_string(),
                    tags,
                },
                raw.clone(),
            ));
        }

        if !project.items.page_info.has_next_page {
            break;
        }
        cursor = project.items.page_info.end_cursor;
    }

    Ok(tasks)
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
        // A real Projects v2 board mixes Issues with PRs and draft items, and the
        // GraphQL `nodes` array can itself contain a literal `null` (a redacted /
        // inaccessible item). GitHub returns `content: {}` for a PR/draft under our
        // `... on Issue` selection. All three shapes must deserialise without
        // failing the whole ProjectData parse and dropping the project.
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
              { "type": "DRAFT_ISSUE", "fieldValues": { "nodes": [] }, "content": null },
              null
            ]
          } } }
        }"#;
        let parsed: GqlResponse<ProjectData> =
            serde_json::from_str(json).expect("mixed-content response must deserialise");
        let nodes = parsed.data.unwrap().node.unwrap().items.nodes;
        assert_eq!(nodes.len(), 4);

        // The null node deserialises to None and the production loop skips it via
        // `.iter().flatten()` — only the three real items survive.
        assert!(nodes[3].is_none());
        assert_eq!(nodes.iter().flatten().count(), 3);

        // The ISSUE item's raw content parses into IssueContent…
        let issue: IssueContent =
            serde_json::from_value(nodes[0].as_ref().unwrap().content.clone().unwrap()).unwrap();
        assert_eq!(issue.number, 5);
        assert_eq!(issue.repository.name_with_owner, "org/repo");

        // …while the PR's `{}` does not — so the loop skips it instead of failing.
        assert!(serde_json::from_value::<IssueContent>(
            nodes[1].as_ref().unwrap().content.clone().unwrap()
        )
        .is_err());
        assert!(nodes[2].as_ref().unwrap().content.is_none());
    }

    #[test]
    fn extracts_status_column_verbatim() {
        // The user's real column name is preserved — never collapsed to a bucket.
        assert_eq!(
            extract_status_raw(&[status_field("In Review")]),
            "In Review"
        );
        assert_eq!(
            extract_status_raw(&[status_field("Ready for Deploy")]),
            "Ready for Deploy"
        );
        assert_eq!(extract_status_raw(&[]), "");
    }

    #[test]
    fn non_status_field_ignored() {
        let priority_field = serde_json::json!({
            "name": "High",
            "field": { "name": "Priority" }
        });
        assert_eq!(extract_status_raw(&[priority_field]), "");
    }

    #[test]
    fn custom_columns_no_longer_collapse() {
        // The reported bug: custom columns the old substring matcher didn't know
        // about silently became "todo". Now the raw name is kept and only
        // genuinely terminal columns resolve to is_terminal=true.
        let raw = extract_status_raw(&[status_field("Shipped")]);
        let resolved = crate::intelligence::providers::status::resolve("github", &raw, None);
        assert_eq!(resolved.raw, "Shipped");
        assert!(resolved.is_terminal);

        let raw = extract_status_raw(&[status_field("In Review")]);
        let resolved = crate::intelligence::providers::status::resolve("github", &raw, None);
        assert_eq!(resolved.raw, "In Review");
        assert!(!resolved.is_terminal);
    }
}
