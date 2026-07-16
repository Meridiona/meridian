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
use super::statuses::{category_str, SetStatusResult, StatusList, StatusOption};
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

/// List the statuses `key` can move to + its current status. The reachable set
/// comes from the transitions endpoint (each transition's `to` status); the
/// current status is a separate `?fields=status` read.
#[tracing::instrument(skip(cfg))]
pub(super) async fn list_statuses(cfg: &JiraConfig, key: &str) -> Result<StatusList> {
    let ctx = resolve(cfg)
        .await
        .context("resolving Jira auth for status list")?;
    let client = reqwest::Client::new();
    let transitions = transitions::fetch_transitions(&ctx, &client, key).await?;
    let statuses = transition_status_options(&transitions);
    let (current_id, current_name) = current_status(&ctx, &client, key)
        .await
        .unwrap_or((None, None));
    tracing::debug!(key, statuses = statuses.len(), "jira status list");
    Ok(StatusList {
        statuses,
        current_id,
        current_name,
    })
}

/// Move `key` to the chosen status (a transition target id, or its name,
/// case-insensitive). Jira changes status only via a transition, so we resolve
/// the choice to the transition whose `to` status matches and POST it. When no
/// reachable transition lands there, redirect (the workflow forbids the move).
#[tracing::instrument(skip(cfg))]
pub(super) async fn set_status(
    cfg: &JiraConfig,
    key: &str,
    choice: &str,
) -> Result<SetStatusResult> {
    let ctx = resolve(cfg)
        .await
        .context("resolving Jira auth for status set")?;
    let client = reqwest::Client::new();
    let transitions = transitions::fetch_transitions(&ctx, &client, key).await?;
    match pick_transition_for_choice(&transitions, choice) {
        Some((transition_id, target)) => {
            transitions::post_transition(&ctx, &client, key, &transition_id, "set-status").await?;
            Ok(SetStatusResult::applied(target))
        }
        None => Ok(SetStatusResult::redirected(
            ctx.browse_url(key),
            format!("Your board can't move {key} straight to that status from its current one."),
        )),
    }
}

/// Map a Jira `statusCategory.key` to Meridian's canonical taxonomy. Transitions
/// only expose the three native categories, so backlog/in_review/cancelled never
/// appear here (a "Won't Do" transition reports the `done` category).
fn jira_category(cat_key: &str) -> &'static str {
    use meridian_core::StatusCategory::{Done, InProgress, Todo};
    match cat_key {
        "new" => category_str(Todo),
        "indeterminate" => category_str(InProgress),
        "done" => category_str(Done),
        _ => "unknown",
    }
}

/// Build a `StatusOption` per transition, keyed on its TARGET status (`to.id` /
/// `to.name`) — that's the status the ticket ends up in, and the handle the
/// setter resolves against.
fn transition_status_options(transitions: &[Value]) -> Vec<StatusOption> {
    transitions
        .iter()
        .filter_map(|t| {
            let to = t.get("to")?;
            let id = to.get("id")?.as_str()?.to_string();
            let name = to.get("name")?.as_str()?.to_string();
            let cat = to
                .pointer("/statusCategory/key")
                .and_then(|k| k.as_str())
                .unwrap_or("");
            Some(StatusOption {
                id,
                name,
                category: jira_category(cat).to_string(),
            })
        })
        .collect()
}

/// Find the transition whose target status matches `choice` (target id exact, or
/// target name case-insensitive). Returns `(transition_id, target_status)`.
fn pick_transition_for_choice(
    transitions: &[Value],
    choice: &str,
) -> Option<(String, StatusOption)> {
    let options = transition_status_options(transitions);
    // Zip options back to their transition ids (same order, same filter).
    transitions
        .iter()
        .filter(|t| t.pointer("/to/id").and_then(|i| i.as_str()).is_some())
        .zip(options)
        .find(|(_, opt)| opt.id == choice || opt.name.eq_ignore_ascii_case(choice))
        .and_then(|(t, opt)| {
            let tid = t.get("id")?.as_str()?.to_string();
            Some((tid, opt))
        })
}

/// The ticket's current status id + name (`GET /issue/{key}?fields=status`).
async fn current_status(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    key: &str,
) -> Result<(Option<String>, Option<String>)> {
    let url = ctx.api_url(&format!("/rest/api/3/issue/{key}?fields=status"));
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET issue status")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Jira GET status for {key} returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing issue status")?;
    let id = v
        .pointer("/fields/status/id")
        .and_then(|i| i.as_str())
        .map(String::from);
    let name = v
        .pointer("/fields/status/name")
        .and_then(|n| n.as_str())
        .map(String::from);
    Ok((id, name))
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
pub(crate) async fn my_account_id(ctx: &JiraReqCtx, client: &reqwest::Client) -> Result<String> {
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

    fn transitions() -> Vec<Value> {
        vec![
            json!({ "id": "11", "to": { "id": "3", "name": "In Progress", "statusCategory": { "key": "indeterminate" } } }),
            json!({ "id": "31", "to": { "id": "5", "name": "Done", "statusCategory": { "key": "done" } } }),
            json!({ "id": "41", "to": { "id": "1", "name": "To Do", "statusCategory": { "key": "new" } } }),
        ]
    }

    #[test]
    fn maps_native_categories() {
        assert_eq!(jira_category("new"), "todo");
        assert_eq!(jira_category("indeterminate"), "in_progress");
        assert_eq!(jira_category("done"), "done");
        assert_eq!(jira_category("weird"), "unknown");
    }

    #[test]
    fn builds_options_from_transition_targets() {
        let opts = transition_status_options(&transitions());
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0].id, "3");
        assert_eq!(opts[0].name, "In Progress");
        assert_eq!(opts[0].category, "in_progress");
        assert_eq!(opts[1].category, "done");
        assert_eq!(opts[2].category, "todo");
    }

    #[test]
    fn picks_transition_by_target_id() {
        // Choosing target status id "5" must yield transition id "31".
        let (tid, target) = pick_transition_for_choice(&transitions(), "5").unwrap();
        assert_eq!(tid, "31");
        assert_eq!(target.name, "Done");
    }

    #[test]
    fn picks_transition_by_target_name_ci() {
        // The Undo path passes the previous status NAME (case-insensitive).
        let (tid, target) = pick_transition_for_choice(&transitions(), "to do").unwrap();
        assert_eq!(tid, "41");
        assert_eq!(target.id, "1");
    }

    #[test]
    fn unreachable_choice_is_none() {
        // Not a reachable transition target → caller redirects.
        assert!(pick_transition_for_choice(&transitions(), "Blocked").is_none());
    }
}
