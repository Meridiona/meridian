//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Linear write-back via the `issueUpdate` GraphQL mutation. Linear addresses
// issues by UUID, not the human identifier (`ENG-123`), so we resolve the UUID
// first (the same `issue(id:)` lookup the worklog poster uses). Most hygiene
// fields map cleanly: dueDate, assignee (viewer id), priority (int), estimate,
// parent, title, description, and close (the team's completed workflow state).
// Adding a single label by name needs team-label resolution that is awkward to do
// reliably blind, so it is redirected.
//
// Reference: https://linear.app/developers/graphql (Mutation.issueUpdate)

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::{ApplyResult, WriteField};
use crate::config::LinearConfig;

const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";

pub async fn apply(cfg: &LinearConfig, key: &str, write: &WriteField) -> Result<ApplyResult> {
    let client = reqwest::Client::new();

    // Label-add is redirected — resolving a label name → team label id blind is
    // error-prone, and labels are an optional defect.
    if let WriteField::AddLabel(_) = write {
        return Ok(ApplyResult::redirected(
            "linear",
            key,
            "labels",
            issue_url(&client, cfg, key).await,
            "add the label in Linear (team-scoped label picker)",
        ));
    }

    let uuid = resolve_issue_uuid(&client, cfg, key).await?;

    let input: Value = match write {
        WriteField::DueDate(date) => json!({ "dueDate": date }),
        WriteField::AssignMe => {
            let me = viewer_id(&client, cfg).await?;
            json!({ "assigneeId": me })
        }
        WriteField::Priority(name) => json!({ "priority": priority_to_int(name) }),
        WriteField::StoryPoints(points) => json!({ "estimate": points.round() as i64 }),
        WriteField::Parent(parent_key) => {
            let parent_uuid = resolve_issue_uuid(&client, cfg, parent_key).await?;
            json!({ "parentId": parent_uuid })
        }
        WriteField::Summary(text) => json!({ "title": text }),
        WriteField::Description(text) => json!({ "description": text }),
        WriteField::Close => {
            let state_id = completed_state_id(&client, cfg, &uuid).await?;
            json!({ "stateId": state_id })
        }
        WriteField::AddLabel(_) => unreachable!("handled above"),
    };

    let mutation = "mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {\n  \
        issueUpdate(id: $id, input: $input) { success }\n}";
    let payload = json!({ "query": mutation, "variables": { "id": uuid, "input": input } });
    let data = graphql(&client, cfg, &payload)
        .await
        .with_context(|| format!("Linear issueUpdate for {key}"))?;
    if data["issueUpdate"]["success"].as_bool() != Some(true) {
        bail!("Linear issueUpdate for {key} did not report success: {data}");
    }

    Ok(ApplyResult::applied("linear", key, field_name(write)))
}

/// Map a human priority name to Linear's int scale (0 None, 1 Urgent, 2 High,
/// 3 Medium, 4 Low). Unknown names default to Medium.
fn priority_to_int(name: &str) -> i64 {
    match name.to_lowercase().as_str() {
        "urgent" | "highest" | "critical" | "blocker" => 1,
        "high" => 2,
        "medium" | "normal" => 3,
        "low" | "lowest" | "minor" => 4,
        "none" | "no priority" => 0,
        _ => 3,
    }
}

async fn resolve_issue_uuid(
    client: &reqwest::Client,
    cfg: &LinearConfig,
    id: &str,
) -> Result<String> {
    let query = "query ResolveIssue($id: String!) { issue(id: $id) { id } }";
    let payload = json!({ "query": query, "variables": { "id": id } });
    let data = graphql(client, cfg, &payload).await?;
    data["issue"]["id"]
        .as_str()
        .map(String::from)
        .with_context(|| format!("Linear issue {id} not found"))
}

async fn viewer_id(client: &reqwest::Client, cfg: &LinearConfig) -> Result<String> {
    let payload = json!({ "query": "query { viewer { id } }" });
    let data = graphql(client, cfg, &payload).await?;
    data["viewer"]["id"]
        .as_str()
        .map(String::from)
        .context("Linear viewer query missing id")
}

/// The team's completed workflow state for the issue (type == "completed").
async fn completed_state_id(
    client: &reqwest::Client,
    cfg: &LinearConfig,
    uuid: &str,
) -> Result<String> {
    let query = "query IssueStates($id: String!) { issue(id: $id) { team { states { nodes { id name type } } } } }";
    let payload = json!({ "query": query, "variables": { "id": uuid } });
    let data = graphql(client, cfg, &payload).await?;
    let nodes = data
        .pointer("/issue/team/states/nodes")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    pick_completed_state(&nodes).context("no completed workflow state on this Linear team")
}

fn pick_completed_state(nodes: &[Value]) -> Option<String> {
    nodes
        .iter()
        .find(|s| s.get("type").and_then(|t| t.as_str()) == Some("completed"))
        .and_then(|s| s.get("id").and_then(|i| i.as_str()).map(String::from))
}

/// Human URL for the redirect fallback.
async fn issue_url(client: &reqwest::Client, cfg: &LinearConfig, id: &str) -> String {
    let query = "query IssueUrl($id: String!) { issue(id: $id) { url } }";
    let payload = json!({ "query": query, "variables": { "id": id } });
    match graphql(client, cfg, &payload).await {
        Ok(d) => d["issue"]["url"].as_str().unwrap_or("").to_string(),
        Err(_) => format!("https://linear.app/issue/{id}"),
    }
}

async fn graphql(client: &reqwest::Client, cfg: &LinearConfig, payload: &Value) -> Result<Value> {
    let resp = client
        .post(LINEAR_GRAPHQL_URL)
        .header("Authorization", &cfg.api_key)
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

fn field_name(write: &WriteField) -> &'static str {
    match write {
        WriteField::DueDate(_) => "duedate",
        WriteField::AssignMe => "assignee",
        WriteField::AddLabel(_) => "labels",
        WriteField::Priority(_) => "priority",
        WriteField::StoryPoints(_) => "story_points",
        WriteField::Parent(_) => "parent",
        WriteField::Summary(_) => "summary",
        WriteField::Description(_) => "description",
        WriteField::Close => "close",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_priority_names() {
        assert_eq!(priority_to_int("Urgent"), 1);
        assert_eq!(priority_to_int("High"), 2);
        assert_eq!(priority_to_int("medium"), 3);
        assert_eq!(priority_to_int("Low"), 4);
        assert_eq!(priority_to_int("whatever"), 3);
    }

    #[test]
    fn picks_completed_state() {
        let nodes = vec![
            json!({ "id": "s1", "name": "In Progress", "type": "started" }),
            json!({ "id": "s2", "name": "Done", "type": "completed" }),
        ];
        assert_eq!(pick_completed_state(&nodes), Some("s2".into()));
    }

    #[test]
    fn no_completed_state_is_none() {
        let nodes = vec![json!({ "id": "s1", "name": "Todo", "type": "unstarted" })];
        assert_eq!(pick_completed_state(&nodes), None);
    }
}
