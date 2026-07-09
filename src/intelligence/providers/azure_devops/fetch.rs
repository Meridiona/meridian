//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Azure DevOps fetch path — WIQL query, work-item batch resolve, per-type
//! state-category lookup, and PAT auth.
//!
//! Split from the connector root purely for file size; the work-item shapes
//! the rest of the connector consumes ([`WorkItemDetail`] etc.) stay in
//! [`super`] so the upsert/epic paths own them.
//!
//! # Who calls this
//! [`super::force_refresh`], via `fetch::{run_wiql, fetch_batch, fetch_state_categories}`.

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

use super::WorkItemDetail;
use crate::config::AzureDevOpsConfig;

/// Max ids per work-items batch request.
pub(super) const BATCH_SIZE: usize = 200;

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
pub(super) fn basic_auth(pat: &str) -> String {
    let raw = format!(":{pat}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    format!("Basic {encoded}")
}

// ---------------------------------------------------------------------------
// API calls
// ---------------------------------------------------------------------------

/// Run a WIQL query and return the work item IDs assigned to @me.
pub(super) async fn run_wiql(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
) -> Result<Vec<u64>> {
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
/// Returns each item paired with its raw JSON — the CDM columns (Stage 3b) are
/// derived from the raw payload via the shared adapter, kept verbatim alongside
/// the typed view the rest of this module already relies on.
pub(super) async fn fetch_batch(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    ids: &[u64],
) -> Result<Vec<(WorkItemDetail, serde_json::Value)>> {
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
         System.ChangedDate,System.CreatedDate,System.Description,System.Tags,\
         System.IterationPath,System.TeamProject,System.AssignedTo,System.CreatedBy,\
         System.Parent,Microsoft.VSTS.Common.Priority,\
         Microsoft.VSTS.Common.AcceptanceCriteria,\
         Microsoft.VSTS.Common.ClosedDate,\
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
    let text = resp.text().await.context("reading batch response body")?;
    // Parse once as a Value so each raw work item survives verbatim for the
    // canonical adapter, then deserialise the typed view from the same body.
    // Array order is identical, so we zip them.
    let body_val: serde_json::Value =
        serde_json::from_str(&text).context("parsing batch response")?;
    let raw_items: Vec<serde_json::Value> = body_val
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let batch: WorkItemBatchResponse =
        serde_json::from_value(body_val).context("parsing batch response")?;
    Ok(batch.value.into_iter().zip(raw_items).collect())
}

/// Fetch the state-name → StateCategory map for one work item type.
/// On failure returns an empty map and logs a warning; the caller treats unknown
/// states as in_progress so a degraded states API response doesn't break the sync.
pub(super) async fn fetch_state_categories(
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
