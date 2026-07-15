//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Jira fetch path — the /search/jql call and start-date field discovery.
//!
//! Split from the connector root purely for file size; the response shapes the
//! rest of the connector consumes ([`JiraIssue`] etc.) stay in [`super`] so the
//! upsert path owns them.
//!
//! # Who calls this
//! [`super::refresh_if_stale`], via `fetch::fetch` / `fetch::discover_start_date_field`.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::JiraIssue;
use crate::intelligence::oauth::jira::JiraReqCtx;

pub(super) const MAX_RESULTS: usize = 100;

#[derive(Deserialize)]
struct JiraSearchResponse {
    issues: Vec<JiraIssue>,
}

// ---------------------------------------------------------------------------
// Field discovery
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JiraFieldMeta {
    id: String,
    name: String,
}

/// Call /rest/api/3/field and return the ID of the field whose name best
/// matches "start date":
///   1. exact case-insensitive match on "start date"
///   2. name contains both "start" and "date" (case-insensitive)
///
/// Returns None if no match or if the request fails.
pub(super) async fn discover_start_date_field(ctx: &JiraReqCtx) -> Option<String> {
    let client = reqwest::Client::new();
    let url = ctx.api_url("/rest/api/3/field");
    let resp = ctx.apply(client.get(&url)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let fields: Vec<JiraFieldMeta> = resp.json().await.ok()?;

    // Priority 1: exact match
    if let Some(f) = fields
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case("start date"))
    {
        return Some(f.id.clone());
    }
    // Priority 2: name contains both "start" and "date"
    fields
        .iter()
        .find(|f| {
            let n = f.name.to_lowercase();
            n.contains("start") && n.contains("date")
        })
        .map(|f| f.id.clone())
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip(ctx),
    fields(
        provider = "jira",
        latency_ms = tracing::field::Empty,
        status_code = tracing::field::Empty,
    )
)]
pub(super) async fn fetch(
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
) -> Result<Vec<(JiraIssue, serde_json::Value)>> {
    search(
        ctx,
        start_date_field,
        "assignee = currentUser() AND statusCategory != Done AND type IN (Task, Feature) ORDER BY updated DESC",
    )
    .await
}

/// Fetch specific issues by key regardless of assignee/status/type — used to
/// backfill a `pm_tasks` row for a ticket that has worklog history but was
/// never (or is no longer) covered by the active-task scope of [`fetch`] (e.g.
/// it's Done, or assigned to someone else). Without this, such a ticket's
/// title can never appear on the timeline once its `pm_tasks` row is missing,
/// since [`fetch`]'s JQL will never surface it again.
pub(super) async fn fetch_by_keys(
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
    keys: &[String],
) -> Result<Vec<(JiraIssue, serde_json::Value)>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let jql = key_in_jql(keys);
    search(ctx, start_date_field, &jql).await
}

/// Builds a `key IN ("A-1","A-2")` JQL clause. Strips embedded quotes from
/// each key defensively — a Jira key is always `[A-Z]+-[0-9]+`, so this never
/// fires in practice, but it keeps a malformed key from breaking the query.
fn key_in_jql(keys: &[String]) -> String {
    let list = keys
        .iter()
        .map(|k| format!("\"{}\"", k.replace('"', "")))
        .collect::<Vec<_>>()
        .join(",");
    format!("key IN ({list})")
}

async fn search(
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
    jql: &str,
) -> Result<Vec<(JiraIssue, serde_json::Value)>> {
    let client = reqwest::Client::new();
    let url = ctx.api_url("/rest/api/3/search/jql");

    let mut fields = vec![
        "summary",
        "description",
        "issuetype",
        "project",
        "updated",
        "parent",
        "status",
        "duedate",
        "assignee",
        "labels",
        "customfield_10020",
        // CDM (Stage 3b): fed to the canonical adapter for the new columns.
        "reporter",
        "priority",
        "resolutiondate",
    ];
    if let Some(id) = start_date_field {
        fields.push(id);
    }

    let body = serde_json::json!({
        "jql": jql,
        "maxResults": MAX_RESULTS,
        "fields": fields,
    });

    let start = std::time::Instant::now();
    let resp = ctx
        .apply(client.post(&url))
        .json(&body)
        .send()
        .await
        .context("POST /search/jql")?;

    let status = resp.status();
    let elapsed_ms = start.elapsed().as_millis() as i64;
    tracing::Span::current().record("status_code", status.as_u16() as i64);
    tracing::Span::current().record("latency_ms", elapsed_ms);

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Jira /search/jql → {}: {}", status, text);
    }

    // Parse once as a Value so we can keep each raw issue object verbatim for
    // the canonical adapter (Stage 3b), then deserialise the typed view from
    // the same body. The `issues` array order is identical, so we zip them.
    let body_val: serde_json::Value = resp.json().await.context("deserialising Jira response")?;
    let raw_issues: Vec<serde_json::Value> = body_val
        .get("issues")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let data: JiraSearchResponse =
        serde_json::from_value(body_val).context("parsing Jira response")?;
    let issue_count = data.issues.len();
    let keys: Vec<&str> = data.issues.iter().map(|i| i.key.as_str()).collect();
    tracing::debug!(count = issue_count, ?keys, "parsed Jira response");
    Ok(data.issues.into_iter().zip(raw_issues).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_in_jql_quotes_and_joins_keys() {
        let keys = vec!["KAN-64".to_string(), "KAN-67".to_string()];
        assert_eq!(key_in_jql(&keys), r#"key IN ("KAN-64","KAN-67")"#);
    }

    #[test]
    fn key_in_jql_single_key() {
        let keys = vec!["KAN-1".to_string()];
        assert_eq!(key_in_jql(&keys), r#"key IN ("KAN-1")"#);
    }

    #[test]
    fn key_in_jql_strips_embedded_quotes() {
        let keys = vec!["KAN\"-1".to_string()];
        assert_eq!(key_in_jql(&keys), r#"key IN ("KAN-1")"#);
    }
}
