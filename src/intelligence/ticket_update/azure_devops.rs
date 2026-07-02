//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Azure DevOps write-back via the work-item JSON-Patch API
// (`PATCH .../wit/workitems/{id}`, Content-Type application/json-patch+json).
// task_key is `{project}#{id}`. Most hygiene fields map to a single field op;
// tags (our "labels") and parent need a read-modify-write / relation op. "Close"
// is redirected because the done-state name varies by process (Agile "Closed",
// Scrum/Basic "Done") and guessing wrong errors out.
//
// Ref: https://learn.microsoft.com/en-us/rest/api/azure/devops/wit/work-items/update

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use super::{ApplyResult, WriteField};
use crate::config::AzureDevOpsConfig;
use crate::pm_worklog::azure_devops::{parse_task_key, WorkItemRef};

pub async fn apply(cfg: &AzureDevOpsConfig, key: &str, write: &WriteField) -> Result<ApplyResult> {
    let item = parse_task_key(key)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client")?;

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
        WriteField::AddLabel(label) => {
            let tags = merged_tags(&client, cfg, &item, label).await?;
            vec![set_field("System.Tags", json!(tags))]
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

/// Current System.Tags merged with `label` (Azure tags are `; `-joined; there is
/// no add-op, so we read-modify-write).
async fn merged_tags(
    client: &reqwest::Client,
    cfg: &AzureDevOpsConfig,
    item: &WorkItemRef,
    label: &str,
) -> Result<String> {
    let url = format!(
        "{}/{}/_apis/wit/workitems/{}?fields=System.Tags&api-version=7.1",
        cfg.api_base.trim_end_matches('/'),
        item.project,
        item.id
    );
    let resp = client
        .get(&url)
        .header("Authorization", basic_auth(cfg))
        .send()
        .await
        .context("GET work item tags")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Azure DevOps GET tags returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing work item")?;
    let existing = v
        .pointer("/fields/System.Tags")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    Ok(merge_tags(existing, label))
}

/// Append `label` to a `; `-joined Azure tags string if not already present.
fn merge_tags(existing: &str, label: &str) -> String {
    let mut tags: Vec<String> = existing
        .split(';')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if !tags.iter().any(|t| t.eq_ignore_ascii_case(label)) {
        tags.push(label.to_string());
    }
    tags.join("; ")
}

async fn my_unique_name(client: &reqwest::Client, cfg: &AzureDevOpsConfig) -> Result<String> {
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
    fn maps_priority() {
        assert_eq!(priority_to_int("Critical"), 1);
        assert_eq!(priority_to_int("High"), 2);
        assert_eq!(priority_to_int("Medium"), 3);
        assert_eq!(priority_to_int("Low"), 4);
    }

    #[test]
    fn merges_tags_additively() {
        assert_eq!(merge_tags("backend; api", "urgent"), "backend; api; urgent");
        assert_eq!(merge_tags("", "first"), "first");
        // Idempotent on a present tag (case-insensitive).
        assert_eq!(merge_tags("Backend", "backend"), "Backend");
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
            ("labels", WriteField::AddLabel("bug".into())),
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
}
