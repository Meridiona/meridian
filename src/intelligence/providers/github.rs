//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GitHub task connector. Fetches open issues assigned to the viewer from
// configured GitHub Projects v2 (GraphQL API). task_key is `owner/repo#number`.

use anyhow::{Context, Result};
use meridian_core::adapters::github::GithubAdapter;
use meridian_core::adapters::ProviderAdapter;
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
async fn fetch_project_items(
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
            let resolved = super::status::resolve(
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

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

/// The CDM columns (migration 056) derived from a raw Projects v2 item via the
/// shared [`GithubAdapter`]. All `Option` so a structurally-unusable payload
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
    let Ok(c) = GithubAdapter.to_canonical(raw) else {
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

async fn upsert(pool: &SqlitePool, tasks: &[(GhTask, serde_json::Value)]) -> Result<()> {
    for (t, raw) in tasks {
        // CDM columns (Stage 3b) from the raw item via the shared adapter.
        let cdm = cdm_columns(raw);

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
            let _ =
                super::stamp_sync_error(pool, "github", &format!("GitHub auth failed — {e}")).await;
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

    let mut all_tasks: Vec<(GhTask, serde_json::Value)> = Vec::new();
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
        let _ = super::stamp_sync_error(
            pool,
            "github",
            "GitHub sync failed — all project fetches failed",
        )
        .await;
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
        let resolved = super::super::status::resolve("github", &raw, None);
        assert_eq!(resolved.raw, "Shipped");
        assert!(resolved.is_terminal);

        let raw = extract_status_raw(&[status_field("In Review")]);
        let resolved = super::super::status::resolve("github", &raw, None);
        assert_eq!(resolved.raw, "In Review");
        assert!(!resolved.is_terminal);
    }

    // -----------------------------------------------------------------------
    // CDM (Stage 3b): the new pm_tasks columns are derived from the raw item
    // through the shared adapter. This locks the daemon-side glue; the mapping
    // itself is tested in meridian_core::adapters::github.
    // -----------------------------------------------------------------------

    #[test]
    fn cdm_columns_derives_from_raw_item() {
        let raw = serde_json::json!({
            "type": "ISSUE",
            "project": {"id": "PVT_board"},
            "fieldValues": {"nodes": [
                {"name": "In Review", "field": {"name": "Status"}}
            ]},
            "content": {
                "id": "I_kwDOabc123",
                "number": 42,
                "state": "OPEN",
                "author": {"id": "U_lead", "login": "lead", "name": "Lead"},
                "parent": {"id": "I_kwDOparent"},
                "closedAt": null
            }
        });
        let cdm = super::cdm_columns(&raw);
        // Stable key is the global node id, namespaced.
        assert_eq!(cdm.canonical_id.as_deref(), Some("github:I_kwDOabc123"));
        // OPEN → no derivable category (board columns are user-defined).
        assert_eq!(cdm.status_category, None);
        assert_eq!(cdm.reporter_name.as_deref(), Some("Lead"));
        assert_eq!(cdm.completed_at, None);
        assert_eq!(
            cdm.ancestor_path.as_deref(),
            Some(r#"["github:I_kwDOparent"]"#)
        );
        assert_eq!(cdm.project_ids.as_deref(), Some(r#"["github:PVT_board"]"#));
        assert!(cdm.raw_payload.is_some());
    }

    #[test]
    fn cdm_columns_empty_on_unusable_payload() {
        // No content.id (the pre-CDM query shape) → adapter errors → all
        // columns NULL, never blocks the upsert.
        let cdm = super::cdm_columns(&serde_json::json!({
            "type": "ISSUE",
            "fieldValues": {"nodes": []},
            "content": {"number": 5, "title": "old shape"}
        }));
        assert!(cdm.canonical_id.is_none());
        assert!(cdm.raw_payload.is_none());
        assert!(cdm.status_category.is_none());
    }
}
