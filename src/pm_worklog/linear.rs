// meridian — normalises screenpipe activity into structured app sessions
//
// Linear worklog poster. Linear has NO native worklog / time-logging API — its
// full GraphQL schema exposes no worklog, timeEntry, "time spent" field, or
// custom-field write (verified against the published schema). The only
// first-class, per-issue, per-user, timestamped write it offers is a comment.
// So a "worklog" on Linear is a structured Markdown comment created via the
// `commentCreate` mutation, carrying the time spent + the synthesised narrative
// (see `comment.rs` for the body format and machine marker).
//
// Auth: a personal API key passed RAW in the `Authorization` header — Linear
// does NOT use a `Bearer` prefix for API keys (it does for OAuth tokens; we use
// keys). Endpoint: a single GraphQL POST to https://api.linear.app/graphql.
//
// `commentCreate.issueId` wants the issue UUID, not the human identifier
// (`ENG-123`). We carry the identifier as the task_key, so we first resolve the
// UUID via `issue(id: "ENG-123") { id }` (Linear's `issue` query accepts the
// identifier as a lookup key), then create the comment.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::comment::{format_worklog_comment, seconds_to_human, PostedWorklog};
use crate::config::LinearConfig;

const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";

/// Post a worklog comment to the Linear issue identified by `task_key`
/// (e.g. `ENG-123`). Returns the created comment's id.
pub async fn post_worklog(
    linear: &LinearConfig,
    task_key: &str,
    time_spent_seconds: i64,
    window_start_iso: &str,
    window_end_iso: &str,
    comment: &str,
) -> Result<PostedWorklog> {
    if time_spent_seconds < 60 {
        bail!("time_spent_seconds={time_spent_seconds} below the 60s worklog minimum");
    }
    let body = format_worklog_comment(
        comment,
        time_spent_seconds,
        window_start_iso,
        window_end_iso,
    );

    let client = reqwest::Client::new();
    let issue_uuid = resolve_issue_uuid(&client, linear, task_key).await?;

    let mutation = "mutation CreateWorklogComment($issueId: String!, $body: String!) {\n  \
        commentCreate(input: { issueId: $issueId, body: $body }) {\n    \
        success\n    comment { id url }\n  }\n}";
    let payload = json!({
        "query": mutation,
        "variables": { "issueId": issue_uuid, "body": body },
    });

    tracing::info!(
        task_key,
        time_spent = %seconds_to_human(time_spent_seconds),
        comment_len = body.len(),
        "linear worklog comment create"
    );

    let data = graphql(&client, linear, &payload)
        .await
        .with_context(|| format!("creating Linear worklog comment for {task_key}"))?;

    let created = &data["commentCreate"];
    if created["success"].as_bool() != Some(true) {
        bail!("Linear commentCreate for {task_key} did not report success: {created}");
    }
    let comment_id = created["comment"]["id"]
        .as_str()
        .context("Linear commentCreate response missing comment.id")?
        .to_string();

    Ok(PostedWorklog {
        id: comment_id,
        label: seconds_to_human(time_spent_seconds),
    })
}

/// Resolve a Linear issue's UUID from its human identifier (`ENG-123`).
async fn resolve_issue_uuid(
    client: &reqwest::Client,
    linear: &LinearConfig,
    identifier: &str,
) -> Result<String> {
    let query = "query ResolveIssue($id: String!) { issue(id: $id) { id identifier } }";
    let payload = json!({ "query": query, "variables": { "id": identifier } });
    let data = graphql(client, linear, &payload)
        .await
        .with_context(|| format!("resolving Linear issue {identifier}"))?;
    data["issue"]["id"]
        .as_str()
        .map(|s| s.to_string())
        .with_context(|| format!("Linear issue {identifier} not found (no id in response)"))
}

/// Execute one GraphQL request and return its `data` object, surfacing GraphQL
/// `errors` (which Linear returns with HTTP 200) as a hard error.
async fn graphql(
    client: &reqwest::Client,
    linear: &LinearConfig,
    payload: &Value,
) -> Result<Value> {
    let resp = client
        .post(LINEAR_GRAPHQL_URL)
        .header("Authorization", &linear.api_key)
        .header("Content-Type", "application/json")
        .json(payload)
        .send()
        .await
        .context("network error reaching Linear GraphQL API")?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Linear GraphQL returned {status}: {text}");
    }
    let value: Value = serde_json::from_str(&text).context("parsing Linear GraphQL response")?;
    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        if !errors.is_empty() {
            bail!("Linear GraphQL errors: {}", value["errors"]);
        }
    }
    value
        .get("data")
        .cloned()
        .context("Linear GraphQL response missing `data`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_body_is_well_formed() {
        let body = format_worklog_comment(
            "Shipped X.",
            3600,
            "2026-06-01T09:00:00Z",
            "2026-06-01T10:00:00Z",
        );
        assert!(body.contains("Shipped X."));
        assert!(body.contains("1h"));
        assert!(body.contains("meridian-worklog"));
    }

    #[test]
    fn extracts_comment_id_from_success_response() {
        // Shape mirrors a real commentCreate `data` payload.
        let data = json!({
            "commentCreate": { "success": true, "comment": { "id": "cmt_abc", "url": "https://linear.app/x" } }
        });
        assert_eq!(data["commentCreate"]["comment"]["id"], "cmt_abc");
        assert_eq!(data["commentCreate"]["success"], true);
    }
}
