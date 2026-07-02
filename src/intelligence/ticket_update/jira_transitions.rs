//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Jira status-transition write-back — close/reopen/cancel. Split out of
// `jira.rs` (CLAUDE.md's 500-line file cap) since these three share one
// shape: GET the issue's available transitions, pick the one that lands in
// the target status category (or fall back to a name heuristic), POST it.
// The edit API (`jira.rs::edit_fields`/`edit_update`) can't change status —
// Jira requires the separate transitions endpoint for that.
//
// Reference: https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-transitions/

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::intelligence::oauth::jira::JiraReqCtx;

/// Close a ticket by finding a transition into a `done`-category status and POSTing
/// it. The edit API can't change status — transitions are a separate endpoint.
pub(super) async fn close(ctx: &JiraReqCtx, client: &reqwest::Client, key: &str) -> Result<()> {
    let transitions = fetch_transitions(ctx, client, key).await?;
    let id = pick_done_transition(&transitions)
        .with_context(|| format!("no 'done' transition available for {key}"))?;
    post_transition(ctx, client, key, &id, "close").await
}

/// Choose the transition that lands in a done-category status. Prefers the
/// statusCategory key (`done`), falling back to a name heuristic.
fn pick_done_transition(transitions: &[Value]) -> Option<String> {
    let id_of = |t: &Value| t.get("id").and_then(|i| i.as_str()).map(String::from);
    // statusCategory.key == "done" is the authoritative signal.
    for t in transitions {
        let cat = t
            .pointer("/to/statusCategory/key")
            .and_then(|k| k.as_str())
            .unwrap_or("");
        if cat == "done" {
            return id_of(t);
        }
    }
    // Heuristic fallback on the transition name.
    transitions
        .iter()
        .find(|t| {
            let n = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_lowercase();
            n.contains("done")
                || n.contains("close")
                || n.contains("complete")
                || n.contains("resolve")
        })
        .and_then(id_of)
}

/// Reopen a ticket by finding a transition into a NOT-done-category status and POSTing
/// it — the inverse of `close`. Lands on whatever not-done status the workflow's
/// transitions offer (typically "In Progress", so unchecking the daily-plan checkbox
/// reads as "back to work" rather than "back to backlog"), not necessarily the exact
/// status the ticket held before it was closed.
pub(super) async fn reopen(ctx: &JiraReqCtx, client: &reqwest::Client, key: &str) -> Result<()> {
    let transitions = fetch_transitions(ctx, client, key).await?;
    let id = pick_reopen_transition(&transitions)
        .with_context(|| format!("no 'reopen' transition available for {key}"))?;
    post_transition(ctx, client, key, &id, "reopen").await
}

/// Choose the transition that lands in a not-done status. Prefers the "indeterminate"
/// (in progress) category, then "new" (To Do / backlog) over "done", falling back to a
/// name heuristic.
fn pick_reopen_transition(transitions: &[Value]) -> Option<String> {
    let id_of = |t: &Value| t.get("id").and_then(|i| i.as_str()).map(String::from);
    let category_of = |t: &Value| {
        t.pointer("/to/statusCategory/key")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string()
    };
    for want in ["indeterminate", "new"] {
        if let Some(t) = transitions.iter().find(|t| category_of(t) == want) {
            return id_of(t);
        }
    }
    // Heuristic fallback on the transition name.
    transitions
        .iter()
        .find(|t| {
            let n = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_lowercase();
            n.contains("reopen")
                || n.contains("in progress")
                || n.contains("to do")
                || n.contains("todo")
                || n.contains("open")
                || n.contains("backlog")
        })
        .and_then(id_of)
}

/// Cancel a ticket by finding a transition with a cancelled/won't-do name and POSTing it.
/// Jira has no standard "cancelled" statusCategory — these transitions typically fall under
/// the "done" category with recognisable names.
pub(super) async fn cancel(ctx: &JiraReqCtx, client: &reqwest::Client, key: &str) -> Result<()> {
    let transitions = fetch_transitions(ctx, client, key).await?;
    let id = pick_cancel_transition(&transitions)
        .with_context(|| format!("no 'cancelled' transition available for {key}"))?;
    post_transition(ctx, client, key, &id, "cancel").await
}

/// Choose the transition that represents a cancellation. Uses name heuristics since Jira
/// has no standard "cancelled" statusCategory (these often sit under "done").
fn pick_cancel_transition(transitions: &[Value]) -> Option<String> {
    let id_of = |t: &Value| t.get("id").and_then(|i| i.as_str()).map(String::from);
    transitions
        .iter()
        .find(|t| {
            let n = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_lowercase();
            n.contains("cancel")
                || n.contains("won't do")
                || n.contains("wont do")
                || n.contains("won't fix")
                || n.contains("wontfix")
                || n.contains("rejected")
                || n.contains("declined")
                || n.contains("invalid")
                || n.contains("duplicate")
        })
        .and_then(id_of)
}

/// `GET /issue/{key}/transitions` — the list of transitions available from the
/// ticket's current status. Shared by close/reopen/cancel.
async fn fetch_transitions(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    key: &str,
) -> Result<Vec<Value>> {
    let url = ctx.api_url(&format!("/rest/api/3/issue/{key}/transitions"));
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET transitions")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Jira GET transitions for {key} returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing transitions")?;
    Ok(v.get("transitions")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default())
}

/// `POST /issue/{key}/transitions` with a chosen transition id. Shared by
/// close/reopen/cancel; `action` only labels the error message.
async fn post_transition(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    key: &str,
    id: &str,
    action: &str,
) -> Result<()> {
    let url = ctx.api_url(&format!("/rest/api/3/issue/{key}/transitions"));
    let resp = ctx
        .apply(client.post(&url))
        .header("Accept", "application/json")
        .json(&json!({ "transition": { "id": id } }))
        .send()
        .await
        .context("POST transition")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("Jira POST {action} transition for {key} returned {status}: {text}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_done_category_transition() {
        let transitions = vec![
            json!({ "id": "11", "name": "In Progress", "to": { "statusCategory": { "key": "indeterminate" } } }),
            json!({ "id": "31", "name": "Done", "to": { "statusCategory": { "key": "done" } } }),
        ];
        assert_eq!(pick_done_transition(&transitions), Some("31".into()));
    }

    #[test]
    fn done_transition_name_fallback() {
        let transitions =
            vec![json!({ "id": "41", "name": "Close Issue", "to": { "statusCategory": {} } })];
        assert_eq!(pick_done_transition(&transitions), Some("41".into()));
    }

    #[test]
    fn picks_indeterminate_category_transition_to_reopen() {
        let transitions = vec![
            json!({ "id": "31", "name": "Done", "to": { "statusCategory": { "key": "done" } } }),
            json!({ "id": "11", "name": "To Do", "to": { "statusCategory": { "key": "new" } } }),
            json!({ "id": "21", "name": "In Progress", "to": { "statusCategory": { "key": "indeterminate" } } }),
        ];
        assert_eq!(pick_reopen_transition(&transitions), Some("21".into()));
    }

    #[test]
    fn reopen_falls_back_to_new_when_no_indeterminate() {
        let transitions = vec![
            json!({ "id": "31", "name": "Done", "to": { "statusCategory": { "key": "done" } } }),
            json!({ "id": "11", "name": "To Do", "to": { "statusCategory": { "key": "new" } } }),
        ];
        assert_eq!(pick_reopen_transition(&transitions), Some("11".into()));
    }

    #[test]
    fn reopen_transition_name_fallback() {
        let transitions =
            vec![json!({ "id": "41", "name": "Reopen Issue", "to": { "statusCategory": {} } })];
        assert_eq!(pick_reopen_transition(&transitions), Some("41".into()));
    }
}
