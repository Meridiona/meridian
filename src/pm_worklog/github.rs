//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GitHub worklog poster. GitHub has NO native time tracking (it's a long-standing
// open feature request — neither the Issues API nor Projects v2 exposes a
// time-spent / worklog field). The robust, universally-available analog to a Jira
// worklog row is an issue comment: append-only, timestamped by GitHub, attributed
// to the authenticated user, and visible on the card whether or not the issue is
// in a Project. So a "worklog" on GitHub is a structured Markdown comment posted
// to the issue via the REST API (see `comment.rs` for the body + machine marker).
//
// We deliberately do NOT write a Projects v2 custom "Time Spent" Number field:
// fields can't be created via the API (a manual board setup step), fine-grained
// PATs have a documented gap for user-owned projects, and it needs project/item/
// field-id juggling — all of which hurt smooth onboarding. The issue comment is
// what every GitHub user has. (A Project Number-field running total is a possible
// future enhancement layered on top.)
//
// task_key is `owner/repo#123`; we parse it for the REST path. Auth: a PAT in the
// `Authorization: Bearer` header with the standard GitHub headers (User-Agent is
// required by the API).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::comment::{format_worklog_comment, seconds_to_human, PostedWorklog};
use crate::config::GitHubConfig;

/// A GitHub issue coordinate parsed from a `owner/repo#number` task key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

/// Parse `owner/repo#123` into its parts. Returns an error for any other shape.
pub fn parse_task_key(task_key: &str) -> Result<IssueRef> {
    let (repo_path, num) = task_key
        .rsplit_once('#')
        .with_context(|| format!("GitHub task_key {task_key:?} missing '#<number>'"))?;
    let number: u64 = num
        .parse()
        .with_context(|| format!("GitHub task_key {task_key:?} has non-numeric issue number"))?;
    let (owner, repo) = repo_path
        .split_once('/')
        .with_context(|| format!("GitHub task_key {task_key:?} missing 'owner/repo'"))?;
    if owner.is_empty() || repo.is_empty() {
        bail!("GitHub task_key {task_key:?} has an empty owner or repo");
    }
    Ok(IssueRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
    })
}

/// Post a worklog comment to the GitHub issue identified by `task_key`
/// (`owner/repo#123`). Returns the created comment's id.
pub async fn post_worklog(
    github: &GitHubConfig,
    task_key: &str,
    time_spent_seconds: i64,
    window_start_iso: &str,
    window_end_iso: &str,
    comment: &str,
) -> Result<PostedWorklog> {
    if time_spent_seconds < 60 {
        bail!("time_spent_seconds={time_spent_seconds} below the 60s worklog minimum");
    }
    let issue = parse_task_key(task_key)?;
    let body = format_worklog_comment(
        comment,
        time_spent_seconds,
        window_start_iso,
        window_end_iso,
    );

    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/comments",
        issue.owner, issue.repo, issue.number
    );

    tracing::info!(
        task_key,
        time_spent = %seconds_to_human(time_spent_seconds),
        comment_len = body.len(),
        "github worklog comment POST"
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
        .json(&json!({ "body": body }))
        .send()
        .await
        .with_context(|| format!("network error reaching GitHub at {url}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub comment POST for {task_key} returned {status}: {text}");
    }
    let value: Value = serde_json::from_str(&text).context("parsing GitHub comment response")?;
    let comment_id = comment_id_from_response(&value)
        .with_context(|| format!("GitHub comment response for {task_key} missing `id`"))?;

    Ok(PostedWorklog {
        id: comment_id,
        label: seconds_to_human(time_spent_seconds),
    })
}

/// Delete a previously-posted worklog comment (see `jira::delete_worklog` for
/// why). A 404 (already gone) is treated as success.
pub async fn delete_worklog(github: &GitHubConfig, task_key: &str, comment_id: &str) -> Result<()> {
    let issue = parse_task_key(task_key)?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/comments/{}",
        issue.owner, issue.repo, comment_id
    );

    tracing::info!(task_key, comment_id, "github worklog comment DELETE");

    let client = reqwest::Client::new();
    let resp = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
        .send()
        .await
        .with_context(|| format!("network error reaching GitHub at {url}"))?;

    let status = resp.status();
    if status.is_success() || status.as_u16() == 404 {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    bail!("GitHub comment DELETE for {task_key}/{comment_id} returned {status}: {text}");
}

/// GitHub returns the comment `id` as a JSON number; normalise it to a string.
fn comment_id_from_response(value: &Value) -> Option<String> {
    match value.get("id")? {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo_number() {
        let r = parse_task_key("acme/api#42").unwrap();
        assert_eq!(
            r,
            IssueRef {
                owner: "acme".into(),
                repo: "api".into(),
                number: 42
            }
        );
    }

    #[test]
    fn parses_repo_names_with_dots_and_dashes() {
        let r = parse_task_key("my-org/web.client#1007").unwrap();
        assert_eq!(r.owner, "my-org");
        assert_eq!(r.repo, "web.client");
        assert_eq!(r.number, 1007);
    }

    #[test]
    fn rejects_malformed_keys() {
        assert!(parse_task_key("KAN-123").is_err()); // jira-style
        assert!(parse_task_key("acme/api").is_err()); // no number
        assert!(parse_task_key("acme#42").is_err()); // no repo
        assert!(parse_task_key("acme/api#nope").is_err()); // non-numeric
    }

    #[test]
    fn normalises_numeric_id() {
        assert_eq!(
            comment_id_from_response(&json!({ "id": 123456 })).as_deref(),
            Some("123456")
        );
        assert_eq!(
            comment_id_from_response(&json!({ "id": "abc" })).as_deref(),
            Some("abc")
        );
        assert_eq!(comment_id_from_response(&json!({})), None);
    }
}
