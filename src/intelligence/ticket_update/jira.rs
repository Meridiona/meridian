//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Jira write-back. Reuses `oauth::jira::resolve` for auth + the API base, exactly
// like the worklog poster. Field edits go through `PUT /rest/api/3/issue/{key}`
// (fields/update form); closing a ticket goes through the dedicated transitions
// endpoint (Jira refuses status changes via the edit API). Story points is a
// per-instance custom field, so we discover it by name before writing.
//
// Reference: https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::jira_transitions as transitions;
use super::{ApplyResult, WriteField};
use crate::config::JiraConfig;
use crate::intelligence::oauth::jira::{resolve, JiraReqCtx};

pub async fn apply(cfg: &JiraConfig, key: &str, write: &WriteField) -> Result<ApplyResult> {
    let ctx = resolve(cfg)
        .await
        .context("resolving Jira auth for write-back")?;
    let client = reqwest::Client::new();

    match write {
        WriteField::DueDate(date) => {
            edit_fields(&ctx, &client, key, json!({ "duedate": date })).await?;
        }
        WriteField::AssignMe => {
            let account_id = my_account_id(&ctx, &client).await?;
            edit_fields(
                &ctx,
                &client,
                key,
                json!({ "assignee": { "accountId": account_id } }),
            )
            .await?;
        }
        WriteField::AddLabel(label) => {
            // `update` add-op is additive — never clobbers existing labels.
            edit_update(&ctx, &client, key, json!({ "labels": [{ "add": label }] })).await?;
        }
        WriteField::Priority(name) => {
            edit_fields(&ctx, &client, key, json!({ "priority": { "name": name } })).await?;
        }
        WriteField::StoryPoints(points) => match story_points_field(&ctx, &client).await? {
            Some(field_id) => {
                edit_fields(&ctx, &client, key, json!({ field_id: points })).await?;
            }
            None => {
                return Ok(ApplyResult::redirected(
                    "jira",
                    key,
                    "story_points",
                    ctx.browse_url(key),
                    "no Story Points field on this Jira instance — add an estimate in the tracker",
                ));
            }
        },
        WriteField::Parent(parent_key) => {
            edit_fields(
                &ctx,
                &client,
                key,
                json!({ "parent": { "key": parent_key } }),
            )
            .await?;
        }
        WriteField::Summary(text) => {
            edit_fields(&ctx, &client, key, json!({ "summary": text })).await?;
        }
        WriteField::Description(text) => {
            edit_fields(&ctx, &client, key, json!({ "description": adf(text) })).await?;
        }
        WriteField::Close => {
            transitions::close(&ctx, &client, key).await?;
        }
        WriteField::Cancel => {
            transitions::cancel(&ctx, &client, key).await?;
        }
        WriteField::Reopen => {
            transitions::reopen(&ctx, &client, key).await?;
        }
    }

    Ok(ApplyResult::applied("jira", key, write_field_name(write)))
}

/// `PUT /issue/{key}` with a `fields` object — SET semantics.
async fn edit_fields(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    key: &str,
    fields: Value,
) -> Result<()> {
    put_issue(ctx, client, key, json!({ "fields": fields })).await
}

/// `PUT /issue/{key}` with an `update` object — ADD/REMOVE op semantics.
async fn edit_update(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    key: &str,
    update: Value,
) -> Result<()> {
    put_issue(ctx, client, key, json!({ "update": update })).await
}

async fn put_issue(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    key: &str,
    body: Value,
) -> Result<()> {
    let url = ctx.api_url(&format!("/rest/api/3/issue/{key}"));
    tracing::info!(task_key = key, "jira issue edit PUT");
    let resp = ctx
        .apply(client.put(&url))
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("network error reaching Jira at {url}"))?;
    let status = resp.status();
    // A successful edit returns 204 No Content.
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("Jira edit of {key} returned {status}: {text}");
    }
    Ok(())
}

/// Resolve the current user's accountId for "assign to me".
async fn my_account_id(ctx: &JiraReqCtx, client: &reqwest::Client) -> Result<String> {
    let url = ctx.api_url("/rest/api/3/myself");
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET /myself for assignee")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Jira /myself returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing /myself")?;
    v.get("accountId")
        .and_then(|a| a.as_str())
        .map(|s| s.to_string())
        .context("/myself response missing accountId")
}

/// Discover the Story Points custom field id by name. Jira instances vary
/// (`customfield_10016` is common but not guaranteed), so we read the field
/// catalogue and match on the standard names. Returns None if absent.
async fn story_points_field(ctx: &JiraReqCtx, client: &reqwest::Client) -> Result<Option<String>> {
    let url = ctx.api_url("/rest/api/3/field");
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET /field for story-points discovery")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Jira /field returned {status}: {text}");
    }
    let fields: Vec<Value> = serde_json::from_str(&text).context("parsing /field")?;
    Ok(pick_story_points(&fields))
}

/// Pick the story-points field id from the catalogue. Prefers the exact modern
/// names; falls back to anything whose name contains "story point".
fn pick_story_points(fields: &[Value]) -> Option<String> {
    let name_of = |f: &Value| {
        f.get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_lowercase()
    };
    let id_of = |f: &Value| f.get("id").and_then(|i| i.as_str()).map(String::from);

    // Exact, in priority order.
    for want in ["story point estimate", "story points"] {
        if let Some(f) = fields.iter().find(|f| name_of(f) == want) {
            return id_of(f);
        }
    }
    // Loose contains-match as a last resort.
    fields
        .iter()
        .find(|f| name_of(f).contains("story point"))
        .and_then(id_of)
}

/// Plain text → Atlassian Document Format (Jira Cloud descriptions are ADF).
fn adf(text: &str) -> Value {
    json!({
        "type": "doc",
        "version": 1,
        "content": [
            { "type": "paragraph", "content": [ { "type": "text", "text": text } ] }
        ]
    })
}

fn write_field_name(write: &WriteField) -> &'static str {
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
        WriteField::Cancel => "cancel",
        WriteField::Reopen => "reopen",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_story_points_by_modern_name() {
        let fields = vec![
            json!({ "id": "summary", "name": "Summary" }),
            json!({ "id": "customfield_10016", "name": "Story point estimate" }),
        ];
        assert_eq!(pick_story_points(&fields), Some("customfield_10016".into()));
    }

    #[test]
    fn discovers_story_points_legacy_name() {
        let fields = vec![json!({ "id": "customfield_10026", "name": "Story Points" })];
        assert_eq!(pick_story_points(&fields), Some("customfield_10026".into()));
    }

    #[test]
    fn story_points_absent_returns_none() {
        let fields = vec![json!({ "id": "summary", "name": "Summary" })];
        assert_eq!(pick_story_points(&fields), None);
    }

    #[test]
    fn adf_wraps_description() {
        assert_eq!(adf("hi")["content"][0]["content"][0]["text"], "hi");
    }
}
