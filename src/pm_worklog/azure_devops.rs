// meridian — normalises screenpipe activity into structured app sessions
//
// Azure DevOps worklog poster. Azure DevOps has no native time-tracking API
// exposed to PAT auth; the equivalent is a work item comment (the Comments v1
// preview endpoint). The comment body uses the same Markdown structure as the
// GitHub and Linear poster so the machine-readable marker is consistent.
//
// task_key format: `{project}#{work_item_id}` — parsed here to reconstruct the
// REST URL: {api_base}/{project}/_apis/wit/workItems/{id}/comments
//
// Ref: https://learn.microsoft.com/en-us/rest/api/azure/devops/wit/comments/add

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::Value;

use super::comment::{format_worklog_comment, seconds_to_human, PostedWorklog};
use crate::config::AzureDevOpsConfig;

/// An Azure DevOps work item coordinate parsed from a `project#id` task key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemRef {
    pub project: String,
    pub id: u64,
}

/// Parse `{project}#{id}` into its parts.
pub fn parse_task_key(task_key: &str) -> Result<WorkItemRef> {
    let (project, id_str) = task_key
        .rsplit_once('#')
        .with_context(|| format!("Azure DevOps task_key {task_key:?} missing '#<id>'"))?;
    if project.is_empty() {
        bail!("Azure DevOps task_key {task_key:?} has an empty project");
    }
    let id: u64 = id_str.parse().with_context(|| {
        format!("Azure DevOps task_key {task_key:?} has non-numeric work item id")
    })?;
    Ok(WorkItemRef {
        project: project.to_owned(),
        id,
    })
}

/// Post a worklog comment to the Azure DevOps work item identified by `task_key`.
/// Returns a `PostedWorklog` with the comment ID and a human-readable label.
pub async fn post_worklog(
    cfg: &AzureDevOpsConfig,
    task_key: &str,
    time_spent_seconds: i64,
    window_start_iso: &str,
    window_end_iso: &str,
    comment: &str,
) -> Result<PostedWorklog> {
    if time_spent_seconds < 60 {
        bail!("time_spent_seconds={time_spent_seconds} below the 60s worklog minimum");
    }
    let item = parse_task_key(task_key)?;
    let body = format_worklog_comment(
        comment,
        time_spent_seconds,
        window_start_iso,
        window_end_iso,
    );

    // The Comments endpoint is a preview API; use the -preview.4 suffix as
    // documented for Azure DevOps Services (cloud). On-premises TFS may require
    // an older preview version — fall back is omitted here; the error surface is clear.
    let url = format!(
        "{}/{}/_apis/wit/workItems/{}/comments?api-version=7.1-preview.4",
        cfg.api_base, item.project, item.id
    );

    let raw = format!(":{}", cfg.pat);
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client")?;

    let payload = serde_json::json!({ "text": body });

    let resp = client
        .post(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Azure DevOps post comment request")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("Azure DevOps post comment returned {status}: {text}");
    }

    let json: Value = resp.json().await.context("parsing comment response")?;
    let comment_id = json["id"]
        .as_u64()
        .map(|n| n.to_string())
        .or_else(|| json["id"].as_str().map(|s| s.to_owned()))
        .context("Azure DevOps comment response missing 'id'")?;

    Ok(PostedWorklog {
        id: comment_id,
        label: seconds_to_human(time_spent_seconds),
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_task_key_valid() {
        let r = parse_task_key("Meridian#42").unwrap();
        assert_eq!(r.project, "Meridian");
        assert_eq!(r.id, 42);
    }

    #[test]
    fn test_parse_task_key_no_hash() {
        assert!(parse_task_key("Meridian42").is_err());
    }

    #[test]
    fn test_parse_task_key_empty_project() {
        assert!(parse_task_key("#42").is_err());
    }

    #[test]
    fn test_parse_task_key_non_numeric_id() {
        assert!(parse_task_key("Meridian#abc").is_err());
    }
}
