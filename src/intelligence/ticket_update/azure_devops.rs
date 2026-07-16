//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Azure DevOps write-back via the work-item JSON-Patch API
// (`PATCH .../wit/workitems/{id}`, Content-Type application/json-patch+json).
// task_key is `{project}#{id}`. Most hygiene fields map to a single field op;
// parent needs a relation op. Hygiene "Close" is redirected (the done-state name
// varies by process); the status-set feature instead lists the type's real
// states and sets `System.State` by name, redirecting only on a workflow reject.
//
// Ref: https://learn.microsoft.com/en-us/rest/api/azure/devops/wit/work-items/update

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use super::statuses::{category_str, resolve_choice, SetStatusResult, StatusList, StatusOption};
use super::{ApplyResult, WriteField};
use crate::config::AzureDevOpsConfig;
use crate::pm_worklog::azure_devops::{parse_task_key, WorkItemRef};

pub async fn apply(cfg: &AzureDevOpsConfig, key: &str, write: &WriteField) -> Result<ApplyResult> {
    let item = parse_task_key(key)?;
    let client = azure_client()?;

    if let WriteField::Close | WriteField::Cancel | WriteField::Reopen = write {
        let reason = match write {
            WriteField::Cancel => {
                "set the work item's State to your process's cancelled state in Azure DevOps"
            }
            WriteField::Reopen => {
                "set the work item's State back to an active state in Azure DevOps"
            }
            _ => "set the work item's State to your process's done state in Azure DevOps",
        };
        return Ok(ApplyResult::redirected(
            "azure_devops",
            key,
            write.label(),
            edit_url(cfg, &item),
            reason,
        ));
    }

    let ops: Vec<Value> = match write {
        WriteField::DueDate(date) => {
            vec![set_field("Microsoft.VSTS.Scheduling.DueDate", json!(date))]
        }
        WriteField::Priority(name) => {
            vec![set_field(
                "Microsoft.VSTS.Common.Priority",
                json!(priority_to_int(name)),
            )]
        }
        WriteField::StoryPoints(points) => {
            vec![set_field(
                "Microsoft.VSTS.Scheduling.StoryPoints",
                json!(points),
            )]
        }
        WriteField::Summary(text) => vec![set_field("System.Title", json!(text))],
        WriteField::Description(text) => vec![set_field("System.Description", json!(text))],
        WriteField::AssignMe => {
            let me = my_unique_name(&client, cfg).await?;
            vec![set_field("System.AssignedTo", json!(me))]
        }
        WriteField::Parent(parent_key) => {
            let parent = parse_task_key(parent_key)?;
            vec![add_parent_relation(cfg, &parent)]
        }
        WriteField::Close | WriteField::Cancel | WriteField::Reopen => {
            unreachable!("redirected above")
        }
    };

    patch_work_item(&client, cfg, &item, &ops).await?;
    Ok(ApplyResult::applied("azure_devops", key, field_name(write)))
}

/// 20 s-timeout HTTP client, shared by the status list/set reads + writes.
fn azure_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client")
}

/// List the states valid for this work item's type (its "statuses") + the item's
/// current state. Azure sets State by NAME, so each option's `id` == its `name`,
/// and there's no separate state id (`current_id` is always `None`).
#[tracing::instrument(skip(cfg))]
pub(super) async fn list_statuses(cfg: &AzureDevOpsConfig, key: &str) -> Result<StatusList> {
    let item = parse_task_key(key)?;
    let client = azure_client()?;
    let (work_item_type, current_name) = work_item_type_and_state(&client, cfg, &item).await?;
    let statuses = fetch_states(&client, cfg, &item.project, &work_item_type).await?;
    tracing::debug!(key, statuses = statuses.len(), "azure status list");
    Ok(StatusList {
        statuses,
        current_id: None,
        current_name,
    })
}

/// Move the work item to the chosen state (id or name, case-insensitive — for
/// Azure they're identical) via a `System.State` JSON-patch. When the process
/// workflow rejects the transition, the PATCH errors; surface that as a redirect.
#[tracing::instrument(skip(cfg))]
pub(super) async fn set_status(
    cfg: &AzureDevOpsConfig,
    key: &str,
    choice: &str,
) -> Result<SetStatusResult> {
    let item = parse_task_key(key)?;
    let client = azure_client()?;
    let (work_item_type, _current) = work_item_type_and_state(&client, cfg, &item).await?;
    let statuses = fetch_states(&client, cfg, &item.project, &work_item_type).await?;
    let target = resolve_choice(&statuses, choice)
        .with_context(|| format!("no Azure DevOps state matches {choice:?}"))?
        .clone();
    let ops = vec![set_field("System.State", json!(target.name))];
    match try_patch_state(&client, cfg, &item, &ops).await? {
        None => Ok(SetStatusResult::applied(target)),
        // The workflow forbids this transition — redirect with Azure's error text.
        Some(err_text) => Ok(SetStatusResult::redirected(edit_url(cfg, &item), err_text)),
    }
}

/// GET the work item's type + current state (one call, two fields).
async fn work_item_type_and_state(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    item: &WorkItemRef,
) -> Result<(String, Option<String>)> {
    let url = format!(
        "{}/{}/_apis/wit/workitems/{}?fields=System.WorkItemType,System.State&api-version=7.1",
        cfg.api_base.trim_end_matches('/'),
        item.project,
        item.id
    );
    let resp = client
        .get(&url)
        .header("Authorization", basic_auth(cfg))
        .send()
        .await
        .context("GET work item type/state")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "Azure DevOps GET work item {} returned {status}: {text}",
            item.id
        );
    }
    let v: Value = serde_json::from_str(&text).context("parsing work item")?;
    let wtype = v
        .pointer("/fields/System.WorkItemType")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let state = v
        .pointer("/fields/System.State")
        .and_then(|s| s.as_str())
        .map(String::from);
    Ok((wtype, state))
}

/// GET the states valid for `work_item_type` → `StatusOption`s (id == name).
/// Inlined rather than crossing into the connector's `fetch_state_categories`.
async fn fetch_states(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    project: &str,
    work_item_type: &str,
) -> Result<Vec<StatusOption>> {
    let encoded = work_item_type.replace(' ', "%20");
    let url = format!(
        "{}/{}/_apis/wit/workitemtypes/{}/states?api-version=7.1",
        cfg.api_base.trim_end_matches('/'),
        project,
        encoded
    );
    let resp = client
        .get(&url)
        .header("Authorization", basic_auth(cfg))
        .send()
        .await
        .context("GET work item type states")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Azure DevOps GET states returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing states")?;
    Ok(parse_states(&v))
}

/// States response (`{"value":[{"name","category"},…]}`) → options.
fn parse_states(v: &Value) -> Vec<StatusOption> {
    v.get("value")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let name = s.get("name")?.as_str()?.to_string();
                    let category = s.get("category").and_then(|c| c.as_str()).unwrap_or("");
                    Some(StatusOption {
                        // Azure sets by name, so the id is the name.
                        id: name.clone(),
                        name,
                        category: azure_category(category).to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Map an Azure state `category` (StateCategory) to the canonical taxonomy.
fn azure_category(category: &str) -> &'static str {
    use meridian_core::StatusCategory::{Cancelled, Done, InProgress, InReview, Todo};
    match category {
        "Proposed" => category_str(Todo),
        "InProgress" => category_str(InProgress),
        "Resolved" => category_str(InReview),
        "Completed" => category_str(Done),
        "Removed" => category_str(Cancelled),
        _ => "unknown",
    }
}

/// PATCH `System.State`. `Ok(None)` = applied; `Ok(Some(text))` = the workflow
/// rejected it (HTTP error body, for the redirect reason); `Err` = network/parse.
async fn try_patch_state(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    item: &WorkItemRef,
    ops: &[Value],
) -> Result<Option<String>> {
    let url = format!(
        "{}/{}/_apis/wit/workitems/{}?api-version=7.1",
        cfg.api_base.trim_end_matches('/'),
        item.project,
        item.id
    );
    let body = serde_json::to_string(ops).context("serialising JSON-patch ops")?;
    let resp = client
        .patch(&url)
        .header("Authorization", basic_auth(cfg))
        .header("Content-Type", "application/json-patch+json")
        .body(body)
        .send()
        .await
        .with_context(|| format!("PATCH Azure work item {} state", item.id))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(None);
    }
    let text = resp.text().await.unwrap_or_default();
    Ok(Some(azure_error_message(&text, status)))
}

/// Azure's human error `message` from a failed body, or a status fallback.
fn azure_error_message(body: &str, status: reqwest::StatusCode) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("Azure DevOps rejected the state change ({status})"))
}

fn set_field(field: &str, value: Value) -> Value {
    json!({ "op": "add", "path": format!("/fields/{field}"), "value": value })
}

fn add_parent_relation(cfg: &AzureDevOpsConfig, parent: &WorkItemRef) -> Value {
    json!({
        "op": "add",
        "path": "/relations/-",
        "value": {
            "rel": "System.LinkTypes.Hierarchy-Reverse",
            "url": format!("{}/_apis/wit/workItems/{}", cfg.api_base.trim_end_matches('/'), parent.id),
        }
    })
}

/// Map a human priority name to Azure's 1–4 scale (1 highest).
fn priority_to_int(name: &str) -> i64 {
    match name.to_lowercase().as_str() {
        "highest" | "critical" | "urgent" | "blocker" => 1,
        "high" => 2,
        "medium" | "normal" => 3,
        "low" | "lowest" | "minor" => 4,
        _ => 2,
    }
}

async fn patch_work_item(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    item: &WorkItemRef,
    ops: &[Value],
) -> Result<()> {
    let url = format!(
        "{}/{}/_apis/wit/workitems/{}?api-version=7.1",
        cfg.api_base.trim_end_matches('/'),
        item.project,
        item.id
    );
    let body = serde_json::to_string(ops).context("serialising JSON-patch ops")?;
    let resp = client
        .patch(&url)
        .header("Authorization", basic_auth(cfg))
        .header("Content-Type", "application/json-patch+json")
        .body(body)
        .send()
        .await
        .with_context(|| format!("PATCH Azure work item {}", item.id))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!(
            "Azure DevOps PATCH for {} returned {status}: {text}",
            item.id
        );
    }
    Ok(())
}

pub(crate) async fn my_unique_name(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
) -> Result<String> {
    let url = format!(
        "{}/_apis/connectionData?api-version=7.1",
        cfg.api_base.trim_end_matches('/')
    );
    let resp = client
        .get(&url)
        .header("Authorization", basic_auth(cfg))
        .send()
        .await
        .context("GET connectionData")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Azure DevOps connectionData returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing connectionData")?;
    v.pointer("/authenticatedUser/properties/Account/$value")
        .and_then(|a| a.as_str())
        .or_else(|| {
            v.pointer("/authenticatedUser/uniqueName")
                .and_then(|a| a.as_str())
        })
        .map(String::from)
        .context("connectionData missing authenticated user identity")
}

fn basic_auth(cfg: &AzureDevOpsConfig) -> String {
    let raw = format!(":{}", cfg.pat);
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
    )
}

fn edit_url(cfg: &AzureDevOpsConfig, item: &WorkItemRef) -> String {
    format!(
        "{}/{}/_workitems/edit/{}",
        cfg.api_base.trim_end_matches('/'),
        item.project,
        item.id
    )
}

fn field_name(write: &WriteField) -> &'static str {
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
    fn maps_priority() {
        assert_eq!(priority_to_int("Critical"), 1);
        assert_eq!(priority_to_int("High"), 2);
        assert_eq!(priority_to_int("Medium"), 3);
        assert_eq!(priority_to_int("Low"), 4);
    }

    #[test]
    fn builds_field_op() {
        let op = set_field("System.Title", json!("Hello"));
        assert_eq!(op["op"], "add");
        assert_eq!(op["path"], "/fields/System.Title");
        assert_eq!(op["value"], "Hello");
    }

    #[test]
    fn builds_edit_url() {
        let cfg = crate::config::AzureDevOpsConfig {
            api_base: "https://dev.azure.com/myorg/".into(),
            project: "MyProject".into(),
            pat: "x".into(),
        };
        let item = WorkItemRef {
            id: 42,
            project: "MyProject".into(),
        };
        assert_eq!(
            edit_url(&cfg, &item),
            "https://dev.azure.com/myorg/MyProject/_workitems/edit/42"
        );
    }

    #[test]
    fn parent_relation_op() {
        let cfg = crate::config::AzureDevOpsConfig {
            api_base: "https://dev.azure.com/myorg".into(),
            project: "Proj".into(),
            pat: "x".into(),
        };
        let parent = WorkItemRef {
            id: 7,
            project: "Proj".into(),
        };
        let op = add_parent_relation(&cfg, &parent);
        assert_eq!(op["op"], "add");
        assert_eq!(op["path"], "/relations/-");
        assert_eq!(op["value"]["rel"], "System.LinkTypes.Hierarchy-Reverse");
        assert_eq!(
            op["value"]["url"],
            "https://dev.azure.com/myorg/_apis/wit/workItems/7"
        );
    }

    #[test]
    fn field_name_round_trips() {
        let cases: &[(&str, WriteField)] = &[
            ("duedate", WriteField::DueDate("2026-01-01".into())),
            ("assignee", WriteField::AssignMe),
            ("priority", WriteField::Priority("High".into())),
            ("story_points", WriteField::StoryPoints(3.0)),
            ("parent", WriteField::Parent("Proj#1".into())),
            ("summary", WriteField::Summary("t".into())),
            ("description", WriteField::Description("d".into())),
            ("close", WriteField::Close),
            ("cancel", WriteField::Cancel),
            ("reopen", WriteField::Reopen),
        ];
        for (expected, field) in cases {
            assert_eq!(
                field_name(field),
                *expected,
                "field_name mismatch for {expected}"
            );
        }
    }

    #[test]
    fn maps_state_categories() {
        assert_eq!(azure_category("Proposed"), "todo");
        assert_eq!(azure_category("InProgress"), "in_progress");
        assert_eq!(azure_category("Resolved"), "in_review");
        assert_eq!(azure_category("Completed"), "done");
        assert_eq!(azure_category("Removed"), "cancelled");
        assert_eq!(azure_category("Whatever"), "unknown");
    }

    #[test]
    fn parses_states_with_name_as_id() {
        let v = json!({
            "value": [
                { "name": "Active", "category": "InProgress" },
                { "name": "Closed", "category": "Completed" },
            ]
        });
        let opts = parse_states(&v);
        assert_eq!(opts.len(), 2);
        // Azure sets by name → id == name.
        assert_eq!(opts[0].id, "Active");
        assert_eq!(opts[0].name, "Active");
        assert_eq!(opts[0].category, "in_progress");
        assert_eq!(opts[1].category, "done");
    }
}
