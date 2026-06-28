//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Azure DevOps (VSTS) task connector. Fetches work items assigned to the
// authenticated user via the WIQL API, then batch-resolves full item detail.
// State filtering is done client-side using the per-type states API so no state
// name is ever injected into WIQL (handles custom states and apostrophes safely).
//
// task_key format: `{project}#{work_item_id}` e.g. `Meridian#42`.
// Supports all three URL shapes: dev.azure.com/{org}, {org}.visualstudio.com,
// and on-premises {server}/{collection}. The api_base field is the resolved root.
//
// Verified end-to-end against dev.azure.com (cloud). On-premises support is
// built to the REST API spec but not live-tested.

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::config::AzureDevOpsConfig;

const SYNC_INTERVAL_MINS: i64 = 5;
const BATCH_SIZE: usize = 200;

// ---------------------------------------------------------------------------
// REST response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WiqlResponse {
    #[serde(rename = "workItems")]
    work_items: Vec<WorkItemRef>,
}

#[derive(Deserialize)]
struct WorkItemRef {
    id: u64,
}

#[derive(Deserialize)]
struct WorkItemBatchResponse {
    value: Vec<WorkItemDetail>,
}

#[derive(Deserialize)]
struct WorkItemDetail {
    id: u64,
    fields: WorkItemFields,
}

#[derive(Deserialize)]
struct AzureIdentity {
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Deserialize)]
struct WorkItemFields {
    #[serde(rename = "System.Title")]
    title: String,
    #[serde(rename = "System.WorkItemType")]
    work_item_type: String,
    #[serde(rename = "System.State")]
    state: String,
    #[serde(rename = "System.ChangedDate", default)]
    changed_date: Option<String>,
    #[serde(rename = "System.Description", default)]
    description: Option<String>,
    /// Acceptance criteria (Scrum/Agile PBI, Bug, Feature, Epic).
    #[serde(rename = "Microsoft.VSTS.Common.AcceptanceCriteria", default)]
    acceptance_criteria: Option<String>,
    /// Repro steps — present on Bug work items only.
    #[serde(rename = "Microsoft.VSTS.TCM.ReproSteps", default)]
    repro_steps: Option<String>,
    /// Semicolon-delimited tag list e.g. `"backend; auth; spike"`.
    #[serde(rename = "System.Tags", default)]
    tags: Option<String>,
    /// Full iteration path e.g. `MyProject\Sprint 14`. Last segment = sprint name.
    #[serde(rename = "System.IterationPath", default)]
    iteration_path: Option<String>,
    #[serde(rename = "System.TeamProject", default)]
    team_project: Option<String>,
    #[serde(rename = "System.AssignedTo", default)]
    assigned_to: Option<AzureIdentity>,
    /// Direct parent work item ID (set by the hierarchy relation).
    #[serde(rename = "System.Parent", default)]
    parent_id: Option<u64>,
    /// Start date — present on all process types (Scheduling namespace).
    #[serde(rename = "Microsoft.VSTS.Scheduling.StartDate", default)]
    start_date: Option<String>,
    /// Target date — cross-process equivalent of due date (TargetDate, not DueDate
    /// which is Agile-process-only).
    #[serde(rename = "Microsoft.VSTS.Scheduling.TargetDate", default)]
    target_date: Option<String>,
}

#[derive(Deserialize)]
struct StatesResponse {
    value: Vec<StateDetail>,
}

#[derive(Deserialize)]
struct StateDetail {
    name: String,
    category: String,
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

/// Strip HTML tags and decode the most common entities so description fields
/// stored in pm_tasks contain readable plain text rather than raw HTML markup.
/// Azure DevOps returns HTML for Description, AcceptanceCriteria, ReproSteps, etc.
fn html_to_plaintext(html: &str) -> String {
    // Remove script/style blocks entirely (including their content).
    let mut s = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    while let Some(ch) = chars.next() {
        if ch == '<' {
            in_tag = true;
            // Peek ahead: <br, </p, </div, </li → emit a space so words don't run together.
            let lookahead: String = chars.clone().take(4).collect();
            let l = lookahead.to_ascii_lowercase();
            if l.starts_with("br")
                || l.starts_with("/p")
                || l.starts_with("/di")
                || l.starts_with("/li")
            {
                s.push(' ');
            }
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            s.push(ch);
        }
    }
    // Decode the most common HTML entities.
    let s = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ");
    // Collapse runs of whitespace (including newlines) to a single space.
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for ch in s.chars() {
        if ch.is_ascii_whitespace() || ch == '\u{00a0}' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_owned()
}

/// Extract the last path segment from an Azure DevOps iteration path.
/// `"MyProject\\Sprint 14"` → `"Sprint 14"`.
fn sprint_from_iteration_path(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).trim().to_owned()
}

// ---------------------------------------------------------------------------
// Auth and helpers
// ---------------------------------------------------------------------------

/// Build the `Authorization: Basic …` header value for PAT auth.
/// Azure DevOps expects Base64(":token") — the username portion is empty.
fn basic_auth(pat: &str) -> String {
    let raw = format!(":{pat}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    format!("Basic {encoded}")
}

/// Whether an Azure StateCategory is terminal. StateCategory is a fixed, reliable
/// metaschema field: Proposed | InProgress | Resolved | Completed | Removed.
/// Only Completed / Removed are terminal.
fn native_terminal(category: &str) -> bool {
    matches!(category, "Completed" | "Removed")
}

// ---------------------------------------------------------------------------
// API calls
// ---------------------------------------------------------------------------

/// Run a WIQL query and return the work item IDs assigned to @me.
async fn run_wiql(client: &reqwest::Client, cfg: &AzureDevOpsConfig) -> Result<Vec<u64>> {
    let url = format!(
        "{}/{}/_apis/wit/wiql?api-version=7.1",
        cfg.api_base, cfg.project
    );
    let body = json!({
        "query": "SELECT [System.Id] FROM WorkItems WHERE [System.AssignedTo] = @me ORDER BY [System.ChangedDate] DESC"
    });
    let resp = client
        .post(&url)
        .header("Authorization", basic_auth(&cfg.pat))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("Azure DevOps WIQL request")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = match status.as_u16() {
            401 => "permission_error: PAT is invalid or expired — regenerate it in Azure DevOps User settings → Personal access tokens".to_string(),
            403 => "permission_error: PAT lacks required scope — create a token with Work Items → Read & write scope".to_string(),
            _ => format!("sync_error: HTTP {status}: {text}"),
        };
        anyhow::bail!("{msg}");
    }
    let wiql: WiqlResponse = resp.json().await.context("parsing WIQL response")?;
    Ok(wiql.work_items.iter().map(|w| w.id).collect())
}

/// Fetch full details for a batch of work item IDs (≤200 per request).
async fn fetch_batch(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    ids: &[u64],
) -> Result<Vec<WorkItemDetail>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let ids_str = ids
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let url = format!(
        "{}/{}/_apis/wit/workitems?ids={}&\
         fields=System.Id,System.Title,System.WorkItemType,System.State,\
         System.ChangedDate,System.Description,System.Tags,System.IterationPath,\
         System.TeamProject,System.AssignedTo,System.Parent,\
         Microsoft.VSTS.Common.AcceptanceCriteria,\
         Microsoft.VSTS.TCM.ReproSteps,\
         Microsoft.VSTS.Scheduling.StartDate,\
         Microsoft.VSTS.Scheduling.TargetDate&api-version=7.1",
        cfg.api_base, cfg.project, ids_str
    );
    let resp = client
        .get(&url)
        .header("Authorization", basic_auth(&cfg.pat))
        .send()
        .await
        .context("Azure DevOps work items batch request")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Azure DevOps work items batch returned {status}: {text}");
    }
    let batch: WorkItemBatchResponse = resp.json().await.context("parsing batch response")?;
    Ok(batch.value)
}

/// Fetch the state-name → StateCategory map for one work item type.
/// On failure returns an empty map and logs a warning; the caller treats unknown
/// states as in_progress so a degraded states API response doesn't break the sync.
async fn fetch_state_categories(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    work_item_type: &str,
) -> HashMap<String, String> {
    // Work item type names are alphanumeric with spaces ("User Story"); only spaces need encoding.
    let encoded = work_item_type.replace(' ', "%20");
    let url = format!(
        "{}/{}/_apis/wit/workitemtypes/{}/states?api-version=7.1",
        cfg.api_base, cfg.project, encoded
    );
    let result: Result<StatesResponse> = async {
        let resp = client
            .get(&url)
            .header("Authorization", basic_auth(&cfg.pat))
            .send()
            .await
            .context("states request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("{status}: {text}");
        }
        resp.json().await.context("parsing states response")
    }
    .await;

    match result {
        Ok(s) => s.value.into_iter().map(|d| (d.name, d.category)).collect(),
        Err(e) => {
            tracing::warn!(
                work_item_type = %work_item_type, error = %e,
                "could not fetch Azure DevOps state categories — treating as in_progress"
            );
            HashMap::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Sync entry points
// ---------------------------------------------------------------------------

/// Refresh Azure DevOps tasks if the cache is stale (> SYNC_INTERVAL_MINS).
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    cfg: &AzureDevOpsConfig,
) -> Result<Option<Vec<String>>> {
    let last_sync: Option<(String,)> = sqlx::query_as(
        "SELECT last_synced_at FROM pm_sync_state WHERE provider = 'azure_devops' LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("reading azure_devops sync state")?;

    if let Some((ts,)) = last_sync {
        if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&ts) {
            let age_mins = chrono::Utc::now()
                .signed_duration_since(last.with_timezone(&chrono::Utc))
                .num_minutes();
            if age_mins < SYNC_INTERVAL_MINS {
                return Ok(None);
            }
        }
    }
    force_refresh(pool, cfg).await.map(Some)
}

/// Unconditionally refresh the Azure DevOps task cache.
#[tracing::instrument(skip(pool, cfg), fields(provider = "azure_devops"))]
pub async fn force_refresh(pool: &SqlitePool, cfg: &AzureDevOpsConfig) -> Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;

    // 1. WIQL: all work items assigned to me.
    let all_ids = match run_wiql(&client, cfg).await {
        Ok(ids) => ids,
        Err(e) => {
            let msg = e.to_string();
            stamp_error(pool, &msg).await?;
            return Err(e);
        }
    };
    tracing::debug!(count = all_ids.len(), "azure_devops: WIQL returned IDs");

    if all_ids.is_empty() {
        prune(pool, &[]).await?;
        stamp_sync(pool).await?;
        return Ok(vec![]);
    }

    // 2. Batch-fetch full detail (≤BATCH_SIZE per request).
    let mut details: Vec<WorkItemDetail> = Vec::with_capacity(all_ids.len());
    for chunk in all_ids.chunks(BATCH_SIZE) {
        let batch = fetch_batch(&client, cfg, chunk).await?;
        details.extend(batch);
    }

    // 2b. Fetch ancestor items (parents not already in the WIQL set) so we can
    //     resolve epic_title without a DB traversal. These are never upserted into
    //     pm_tasks — they only populate the in-memory lookup used by resolve_epic_title.
    let mut fetched_set: std::collections::HashSet<u64> = details.iter().map(|d| d.id).collect();
    let mut ancestor_details: Vec<WorkItemDetail> = Vec::new();
    let mut parents_needed: std::collections::HashSet<u64> = details
        .iter()
        .filter_map(|d| d.fields.parent_id)
        .filter(|pid| !fetched_set.contains(pid))
        .collect();
    for _ in 0..4 {
        if parents_needed.is_empty() {
            break;
        }
        let ids_to_fetch: Vec<u64> = parents_needed.drain().collect();
        match fetch_batch(&client, cfg, &ids_to_fetch).await {
            Ok(batch) => {
                let next: std::collections::HashSet<u64> = batch
                    .iter()
                    .filter_map(|d| d.fields.parent_id)
                    .filter(|pid| !fetched_set.contains(pid))
                    .collect();
                fetched_set.extend(ids_to_fetch);
                ancestor_details.extend(batch);
                parents_needed = next;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not fetch ADO ancestor items — epic_title may be incomplete");
                break;
            }
        }
    }
    // Build id → (title, work_item_type, parent_id) from all fetched items.
    let id_meta: HashMap<u64, (&str, &str, Option<u64>)> = details
        .iter()
        .chain(ancestor_details.iter())
        .map(|d| {
            (
                d.id,
                (
                    d.fields.title.as_str(),
                    d.fields.work_item_type.as_str(),
                    d.fields.parent_id,
                ),
            )
        })
        .collect();

    // 3. Per-type state→category maps (fetched once per type per sync).
    let mut type_states: HashMap<String, HashMap<String, String>> = HashMap::new();
    for item in &details {
        let wit = item.fields.work_item_type.clone();
        if let std::collections::hash_map::Entry::Vacant(e) = type_states.entry(wit) {
            let map = fetch_state_categories(&client, cfg, e.key()).await;
            e.insert(map);
        }
    }

    // 4. Filter and upsert — Completed / Removed items are dropped.
    struct UpsertItem<'a> {
        detail: &'a WorkItemDetail,
        status: super::status::ResolvedStatus,
    }
    let active: Vec<UpsertItem<'_>> = details
        .iter()
        .filter_map(|item| {
            let azure_category = type_states
                .get(&item.fields.work_item_type)
                .and_then(|m| m.get(&item.fields.state))
                .map(|s| s.as_str())
                .unwrap_or("InProgress");
            if matches!(azure_category, "Completed" | "Removed") {
                return None;
            }
            // Store the user-facing System.State name verbatim; the StateCategory
            // gives the reliable terminal signal (always false for kept items).
            Some(UpsertItem {
                detail: item,
                status: super::status::resolve(
                    "azure_devops",
                    &item.fields.state,
                    Some(native_terminal(azure_category)),
                ),
            })
        })
        .collect();

    let mut kept: Vec<String> = Vec::with_capacity(active.len());
    for u in &active {
        let task_key = format!("{}#{}", cfg.project, u.detail.id);
        let f = &u.detail.fields;

        // Build composite description: base + acceptance criteria + repro steps.
        // All three are HTML fields from the API — strip tags before storing.
        let mut desc_parts: Vec<String> = Vec::new();
        let base = html_to_plaintext(f.description.as_deref().unwrap_or(""));
        if !base.is_empty() {
            desc_parts.push(base);
        }
        if let Some(ac) = &f.acceptance_criteria {
            let ac = html_to_plaintext(ac);
            if !ac.is_empty() {
                desc_parts.push(format!("Acceptance Criteria: {ac}"));
            }
        }
        if let Some(rs) = &f.repro_steps {
            let rs = html_to_plaintext(rs);
            if !rs.is_empty() {
                desc_parts.push(format!("Repro Steps: {rs}"));
            }
        }
        let description = desc_parts.join("\n\n");

        // Tags: normalise to comma-separated, trimming each tag.
        let tags: Option<String> = f.tags.as_deref().map(|t| {
            t.split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        });

        // Sprint: last segment of the iteration path.
        let sprint = f
            .iteration_path
            .as_deref()
            .map(sprint_from_iteration_path)
            .filter(|s| !s.is_empty());

        let changed = f.changed_date.as_deref().unwrap_or("");
        let browser_url = format!(
            "{}/{}/_workitems/edit/{}",
            cfg.api_base, cfg.project, u.detail.id
        );
        let project_key = f.team_project.as_deref();
        let assignee_name = f.assigned_to.as_ref().map(|a| a.display_name.as_str());
        let parent_key: Option<String> = f.parent_id.map(|id| format!("{}#{}", cfg.project, id));
        let epic_title: Option<&str> = resolve_epic_title(&id_meta, f.parent_id);

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, updated_at, sprint_name, tags,
                assignee_name, start_date, due_date, parent_key, epic_title)
             VALUES (?, 'azure_devops', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'azure_devops',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_raw       = excluded.status_raw,
               is_terminal      = excluded.is_terminal,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               updated_at       = excluded.updated_at,
               sprint_name      = excluded.sprint_name,
               tags             = excluded.tags,
               assignee_name    = excluded.assignee_name,
               start_date       = excluded.start_date,
               due_date         = excluded.due_date,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title",
        )
        .bind(&task_key)
        .bind(&f.title)
        .bind(&description)
        .bind(&u.status.raw)
        .bind(u.status.is_terminal)
        .bind(&f.work_item_type)
        .bind(project_key)
        .bind(&browser_url)
        .bind(changed)
        .bind(sprint.as_deref())
        .bind(tags.as_deref())
        .bind(assignee_name)
        .bind(f.start_date.as_deref())
        .bind(f.target_date.as_deref())
        .bind(parent_key.as_deref())
        .bind(epic_title)
        .execute(pool)
        .await
        .with_context(|| format!("upserting Azure DevOps work item {}", u.detail.id))?;

        kept.push(task_key);
    }

    prune(pool, &kept).await?;
    stamp_sync(pool).await?;

    tracing::info!(
        provider = "azure_devops",
        total_assigned = all_ids.len(),
        active = kept.len(),
        "azure_devops tasks refreshed"
    );
    Ok(kept)
}

// ---------------------------------------------------------------------------
// Epic resolution
// ---------------------------------------------------------------------------

/// Walk the parent chain from `parent_id` and return the title of the nearest
/// ancestor whose `work_item_type == "Epic"`. Returns `None` when `parent_id`
/// is `None`, when no Epic is found in the chain, or when the chain exceeds 10
/// hops (cycle guard).
fn resolve_epic_title<'a>(
    id_meta: &'a HashMap<u64, (&'a str, &'a str, Option<u64>)>,
    parent_id: Option<u64>,
) -> Option<&'a str> {
    let mut current = parent_id?;
    for _ in 0..10 {
        let (title, wit, pid) = id_meta.get(&current)?;
        if *wit == "Epic" {
            return Some(title);
        }
        current = (*pid)?;
    }
    None
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn prune(pool: &SqlitePool, kept_keys: &[String]) -> Result<()> {
    if kept_keys.is_empty() {
        sqlx::query(
            "DELETE FROM pm_task_embeddings WHERE task_key IN \
             (SELECT task_key FROM pm_tasks WHERE provider = 'azure_devops')",
        )
        .execute(pool)
        .await
        .context("pruning all azure_devops pm_task_embeddings")?;
        sqlx::query("DELETE FROM pm_tasks WHERE provider = 'azure_devops'")
            .execute(pool)
            .await
            .context("pruning all azure_devops pm_tasks")?;
        return Ok(());
    }

    let placeholders = kept_keys.iter().map(|_| "?").collect::<Vec<_>>().join(", ");

    let sql_embed = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks \
          WHERE provider = 'azure_devops' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&sql_embed);
    for k in kept_keys {
        q = q.bind(k);
    }
    q.execute(pool)
        .await
        .context("pruning stale azure_devops pm_task_embeddings")?;

    let sql_tasks = format!(
        "DELETE FROM pm_tasks \
         WHERE provider = 'azure_devops' AND task_key NOT IN ({placeholders})"
    );
    let mut q2 = sqlx::query(&sql_tasks);
    for k in kept_keys {
        q2 = q2.bind(k);
    }
    let result = q2
        .execute(pool)
        .await
        .context("pruning stale azure_devops pm_tasks")?;

    if result.rows_affected() > 0 {
        tracing::info!(
            removed = result.rows_affected(),
            "pruned stale azure_devops tasks"
        );
    }
    Ok(())
}

async fn stamp_sync(pool: &SqlitePool) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES ('azure_devops', ?, NULL)
         ON CONFLICT(provider) DO UPDATE SET
           last_synced_at = excluded.last_synced_at,
           last_error     = NULL",
    )
    .bind(&now)
    .execute(pool)
    .await
    .context("updating azure_devops sync state")?;
    let _ = crate::notices::clear(pool, "pm.azure_devops").await;
    Ok(())
}

async fn stamp_error(pool: &SqlitePool, error: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES ('azure_devops', ?, ?)
         ON CONFLICT(provider) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(&now)
    .bind(error)
    .execute(pool)
    .await
    .context("recording azure_devops sync error")?;
    let _ = crate::notices::raise(
        pool,
        "pm.azure_devops",
        "error",
        "Azure DevOps sync failing",
        error,
        Some("Set AZURE_DEVOPS_PAT in .env"),
    )
    .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_auth_format() {
        let header = basic_auth("mytoken");
        assert!(header.starts_with("Basic "));
        let encoded = header.strip_prefix("Basic ").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(decoded, b":mytoken");
    }

    #[test]
    fn test_html_to_plaintext_basic() {
        assert_eq!(html_to_plaintext("<div>Bug test<br> </div>"), "Bug test");
        assert_eq!(
            html_to_plaintext("<p>Hello &amp; world</p>"),
            "Hello & world"
        );
        assert_eq!(
            html_to_plaintext("<div>Step 1</div><div>Step 2</div>"),
            "Step 1 Step 2",
        );
        assert_eq!(html_to_plaintext(""), "");
        assert_eq!(html_to_plaintext("plain text"), "plain text");
        assert_eq!(
            html_to_plaintext("<ul><li>item 1</li><li>item 2</li></ul>"),
            "item 1 item 2",
        );
    }

    #[test]
    fn test_native_terminal() {
        // Only Completed / Removed are terminal StateCategories.
        assert!(native_terminal("Completed"));
        assert!(native_terminal("Removed"));
        assert!(!native_terminal("Proposed"));
        assert!(!native_terminal("InProgress"));
        assert!(!native_terminal("Resolved"));
        assert!(!native_terminal("SomeCustomState"));
    }

    #[test]
    fn test_html_to_plaintext() {
        assert_eq!(html_to_plaintext("<div>Bug test<br> </div>"), "Bug test");
        assert_eq!(
            html_to_plaintext("<p>Hello &amp; world</p>"),
            "Hello & world"
        );
        assert_eq!(
            html_to_plaintext("<div>Step 1</div><div>Step 2</div>"),
            "Step 1 Step 2",
        );
        assert_eq!(html_to_plaintext(""), "");
        assert_eq!(html_to_plaintext("plain text"), "plain text");
        assert_eq!(
            html_to_plaintext("<ul><li>item 1</li><li>item 2</li></ul>"),
            "item 1 item 2",
        );
    }

    #[test]
    fn test_sprint_from_iteration_path() {
        assert_eq!(
            sprint_from_iteration_path(r"MyProject\Sprint 14"),
            "Sprint 14"
        );
        assert_eq!(sprint_from_iteration_path("MyProject"), "MyProject");
        assert_eq!(sprint_from_iteration_path(r"A\B\C"), "C");
        assert_eq!(sprint_from_iteration_path(""), "");
    }

    // ---- epic_title resolution (resolve_epic_title) ------------------------
    //
    // Cases mirror the live hegdeakarsh2002 board so the tests track production:
    //   #34 Epic  "Auth & Security Overhaul"
    //     └ #35 Issue → #37 Task
    //   #58 Epic  "Performance & Infrastructure"
    //     └ #59 Issue → #61 Task
    // The map value is (title, work_item_type, parent_id), exactly as built in
    // force_refresh from the fetched + ancestor work items.

    /// Builds an id→(title, type, parent) map from `(id, title, type, parent)` tuples.
    fn meta(
        rows: &[(u64, &'static str, &'static str, Option<u64>)],
    ) -> HashMap<u64, (&'static str, &'static str, Option<u64>)> {
        rows.iter().map(|&(id, t, w, p)| (id, (t, w, p))).collect()
    }

    #[test]
    fn epic_resolves_from_direct_epic_parent() {
        // #35 (Issue) → parent #34 (Epic). One hop to the Epic.
        let m = meta(&[(34, "Auth & Security Overhaul", "Epic", None)]);
        assert_eq!(
            resolve_epic_title(&m, Some(34)),
            Some("Auth & Security Overhaul")
        );
    }

    #[test]
    fn epic_resolves_through_multi_hop_chain() {
        // #61 (Task) → #59 (Issue) → #58 (Epic). The nearest Epic ancestor wins.
        let m = meta(&[
            (
                59,
                "Migrate services to Kubernetes (EKS)",
                "Issue",
                Some(58),
            ),
            (58, "Performance & Infrastructure", "Epic", None),
        ]);
        assert_eq!(
            resolve_epic_title(&m, Some(59)),
            Some("Performance & Infrastructure")
        );
    }

    #[test]
    fn epic_is_none_when_no_parent() {
        // Epics themselves (#34/#58) and any top-level item have no parent_id.
        let m = meta(&[(34, "Auth & Security Overhaul", "Epic", None)]);
        assert_eq!(resolve_epic_title(&m, None), None);
    }

    #[test]
    fn epic_is_none_when_chain_has_no_epic() {
        // Issue → Issue with no Epic anywhere in the chain.
        let m = meta(&[
            (10, "Sub-issue", "Issue", Some(11)),
            (11, "Parent issue", "Issue", None),
        ]);
        assert_eq!(resolve_epic_title(&m, Some(10)), None);
    }

    #[test]
    fn epic_is_none_when_parent_missing_from_map() {
        // Ancestor fetch failed / item not returned: graceful None, no panic.
        let m = meta(&[]);
        assert_eq!(resolve_epic_title(&m, Some(999)), None);
    }

    #[test]
    fn epic_resolution_terminates_on_cycle() {
        // A→B→A would loop forever without the 10-hop guard; neither is an Epic.
        let m = meta(&[(1, "A", "Issue", Some(2)), (2, "B", "Issue", Some(1))]);
        assert_eq!(resolve_epic_title(&m, Some(1)), None);
    }

    #[test]
    fn epic_picks_nearest_epic_when_nested() {
        // Task → Feature(Epic-category? no) — here a closer Epic shadows a farther one.
        // #200 → #201 (Epic "Near") → #202 (Epic "Far"): nearest wins.
        let m = meta(&[(201, "Near", "Epic", Some(202)), (202, "Far", "Epic", None)]);
        assert_eq!(resolve_epic_title(&m, Some(201)), Some("Near"));
    }

    #[test]
    fn parent_key_format_matches_task_key_shape() {
        // parent_key is built inline as `{project}#{parent_id}` — the same shape as
        // task_key — so it joins against pm_tasks.task_key. Lock the contract.
        let project = "hegdeakarsh2002";
        let parent_id: Option<u64> = Some(59);
        let parent_key = parent_id.map(|id| format!("{project}#{id}"));
        assert_eq!(parent_key.as_deref(), Some("hegdeakarsh2002#59"));
        let none_parent: Option<u64> = None;
        assert_eq!(none_parent.map(|id| format!("{project}#{id}")), None);
    }
}
