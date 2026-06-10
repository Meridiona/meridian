// meridian — normalises screenpipe activity into structured app sessions
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
struct WorkItemFields {
    #[serde(rename = "System.Title")]
    title: String,
    #[serde(rename = "System.WorkItemType")]
    work_item_type: String,
    #[serde(rename = "System.State")]
    state: String,
    #[serde(rename = "System.AreaPath", default)]
    #[allow(dead_code)]
    area_path: Option<String>,
    #[serde(rename = "System.ChangedDate", default)]
    changed_date: Option<String>,
    #[serde(rename = "System.Description", default)]
    description: Option<String>,
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
// Auth and helpers
// ---------------------------------------------------------------------------

/// Build the `Authorization: Basic …` header value for PAT auth.
/// Azure DevOps expects Base64(":token") — the username portion is empty.
fn basic_auth(pat: &str) -> String {
    let raw = format!(":{pat}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    format!("Basic {encoded}")
}

/// Map an Azure StateCategory value to Meridian's status_category.
fn to_status_category(category: &str) -> &'static str {
    match category {
        "Proposed" => "todo",
        "InProgress" | "Resolved" => "in_progress",
        "Completed" | "Removed" => "done",
        _ => "in_progress",
    }
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
        anyhow::bail!("Azure DevOps WIQL returned {status}: {text}");
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
         System.AreaPath,System.ChangedDate,System.Description&api-version=7.1",
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
    let all_ids = run_wiql(&client, cfg).await?;
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
        status_category: &'static str,
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
            Some(UpsertItem {
                detail: item,
                status_category: to_status_category(azure_category),
            })
        })
        .collect();

    let mut kept: Vec<String> = Vec::with_capacity(active.len());
    for u in &active {
        let task_key = format!("{}#{}", cfg.project, u.detail.id);
        let description = u.detail.fields.description.as_deref().unwrap_or("");
        let changed = u.detail.fields.changed_date.as_deref().unwrap_or("");
        let browser_url = format!(
            "{}/{}/_workitems/edit/{}",
            cfg.api_base, cfg.project, u.detail.id
        );

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_category,
                issue_type, url, updated_at)
             VALUES (?, 'azure_devops', ?, ?, ?, ?, ?, ?)
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'azure_devops',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_category  = excluded.status_category,
               issue_type       = excluded.issue_type,
               url              = excluded.url,
               updated_at       = excluded.updated_at",
        )
        .bind(&task_key)
        .bind(&u.detail.fields.title)
        .bind(description)
        .bind(u.status_category)
        .bind(&u.detail.fields.work_item_type)
        .bind(&browser_url)
        .bind(changed)
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
        "INSERT INTO pm_sync_state (provider, last_synced_at)
         VALUES ('azure_devops', ?)
         ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
    )
    .bind(&now)
    .execute(pool)
    .await
    .context("updating azure_devops sync state")?;
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
    fn test_to_status_category() {
        assert_eq!(to_status_category("Proposed"), "todo");
        assert_eq!(to_status_category("InProgress"), "in_progress");
        assert_eq!(to_status_category("Resolved"), "in_progress");
        assert_eq!(to_status_category("Completed"), "done");
        assert_eq!(to_status_category("Removed"), "done");
        assert_eq!(to_status_category("SomeCustomState"), "in_progress");
    }
}
