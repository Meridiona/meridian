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

/// The active-task scope: everything assigned to you that isn't Done.
///
/// `Bug` belongs here for the same reason `Task` does — a bug you are assigned
/// and working on is work, and Meridian's whole job is to notice work and log it
/// against the right ticket. It was missing, and because this JQL is the ONLY
/// thing that puts a Jira ticket into `pm_tasks`, a bug was invisible everywhere
/// downstream: absent from the Tasks page, absent from the worklog matcher's
/// candidates (so hours spent on it could only ever come back "no match"), and —
/// worst — a bug filed through the plan composer got mirrored as a `'local'`
/// shadow row, because `plan_tasks::create` reads "not in pm_tasks after a sync"
/// as "self-assign failed". That shadow is meant to be temporary, healed by the
/// next sync's UPSERT; for a bug the heal could never arrive, so a real Jira
/// ticket sat in Meridian as a personal task forever.
///
/// Jira is the only provider that filtered by type at all (Linear/GitHub/Trello
/// fetch everything assigned to you; Azure's WIQL is `AssignedTo = @me` alone),
/// so this one clause was the whole discrepancy.
///
/// Sub-tasks stay out deliberately — see the note in `CLAUDE.md`/memory: we
/// track tasks/features/bugs and their epics, never subtasks.
const ACTIVE_TASK_JQL: &str = "assignee = currentUser() AND statusCategory != Done \
     AND type IN (Task, Feature, Bug) ORDER BY updated DESC";

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
    search(ctx, start_date_field, ACTIVE_TASK_JQL).await
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

    /// This JQL is the ONLY door a Jira ticket enters `pm_tasks` through, so a
    /// type missing from it is invisible everywhere downstream — Tasks page,
    /// worklog candidates, plan. `Bug` was missing for exactly that reason and
    /// bugs could never be worklogged. Assert each type explicitly: dropping one
    /// is a silent, product-wide data loss with no error anywhere.
    #[test]
    fn active_task_jql_covers_tasks_features_and_bugs() {
        assert!(ACTIVE_TASK_JQL.contains("Task"));
        assert!(ACTIVE_TASK_JQL.contains("Feature"));
        assert!(ACTIVE_TASK_JQL.contains("Bug"));
    }

    /// The scope is "mine, and not finished". Both halves matter: without the
    /// assignee clause we would mirror the whole project; without the status one
    /// every closed ticket would crowd the matcher's candidate list.
    #[test]
    fn active_task_jql_is_scoped_to_my_unfinished_work() {
        assert!(ACTIVE_TASK_JQL.contains("assignee = currentUser()"));
        assert!(ACTIVE_TASK_JQL.contains("statusCategory != Done"));
    }
}
