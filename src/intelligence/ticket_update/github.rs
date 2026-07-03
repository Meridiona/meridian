//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GitHub write-back via the Issues REST API. task_key is `owner/repo#123`
// (reusing the worklog poster's parser). Issues natively support assignee,
// labels, title, body, and state — those we apply in-app. Due date, priority,
// story points, and parent/epic are Projects v2 concepts (no native issue field),
// so those redirect to the board.
//
// Reference: https://docs.github.com/en/rest/issues

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::{ApplyResult, WriteField};
use crate::config::GitHubConfig;
use crate::pm_worklog::github::{parse_task_key, IssueRef};

pub async fn apply(cfg: &GitHubConfig, key: &str, write: &WriteField) -> Result<ApplyResult> {
    let issue = parse_task_key(key)?;
    let client = reqwest::Client::new();

    match write {
        // Projects-v2-only fields — no native Issues API. Redirect to the card.
        WriteField::DueDate(_) | WriteField::Priority(_) | WriteField::StoryPoints(_) => {
            return Ok(ApplyResult::redirected(
                "github",
                key,
                field_name(write),
                issue_html_url(&issue),
                "GitHub keeps this on the Project board, not the issue — set it there",
            ));
        }
        // Parent = native sub-issues: make this issue a sub-issue of the parent.
        WriteField::Parent(parent_key) => {
            let parent = parse_task_key(parent_key)?;
            let child_id = issue_db_id(cfg, &client, &issue).await?;
            let url = format!("{}/sub_issues", issue_url(&parent));
            post(cfg, &client, &url, json!({ "sub_issue_id": child_id })).await?;
        }
        WriteField::AssignMe => {
            let me = my_login(cfg, &client).await?;
            post(
                cfg,
                &client,
                &assignees_url(&issue),
                json!({ "assignees": [me] }),
            )
            .await?;
        }
        WriteField::AddLabel(label) => {
            post(
                cfg,
                &client,
                &labels_url(&issue),
                json!({ "labels": [label] }),
            )
            .await?;
        }
        WriteField::Summary(text) => {
            patch(cfg, &client, &issue_url(&issue), json!({ "title": text })).await?;
        }
        WriteField::Description(text) => {
            patch(cfg, &client, &issue_url(&issue), json!({ "body": text })).await?;
        }
        WriteField::Close => {
            patch(
                cfg,
                &client,
                &issue_url(&issue),
                json!({ "state": "closed" }),
            )
            .await?;
        }
        WriteField::Cancel => {
            patch(
                cfg,
                &client,
                &issue_url(&issue),
                json!({ "state": "closed", "state_reason": "not_planned" }),
            )
            .await?;
        }
        WriteField::Reopen => {
            patch(
                cfg,
                &client,
                &issue_url(&issue),
                json!({ "state": "open", "state_reason": "reopened" }),
            )
            .await?;
        }
    }

    Ok(ApplyResult::applied("github", key, field_name(write)))
}

fn issue_url(i: &IssueRef) -> String {
    format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        i.owner, i.repo, i.number
    )
}
fn assignees_url(i: &IssueRef) -> String {
    format!("{}/assignees", issue_url(i))
}
fn labels_url(i: &IssueRef) -> String {
    format!("{}/labels", issue_url(i))
}
fn issue_html_url(i: &IssueRef) -> String {
    format!(
        "https://github.com/{}/{}/issues/{}",
        i.owner, i.repo, i.number
    )
}

/// The issue's REST database id (what the sub-issues API wants as `sub_issue_id`,
/// not the issue number).
async fn issue_db_id(
    cfg: &GitHubConfig,
    client: &reqwest::Client,
    issue: &IssueRef,
) -> Result<u64> {
    let resp = gh(cfg, client.get(issue_url(issue)))
        .send()
        .await
        .context("GET issue for id")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub GET issue returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing issue")?;
    v.get("id")
        .and_then(|i| i.as_u64())
        .context("issue response missing numeric id")
}

async fn my_login(cfg: &GitHubConfig, client: &reqwest::Client) -> Result<String> {
    let resp = gh(cfg, client.get("https://api.github.com/user"))
        .send()
        .await
        .context("GET /user")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub /user returned {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing /user")?;
    v.get("login")
        .and_then(|l| l.as_str())
        .map(String::from)
        .context("/user missing login")
}

async fn post(cfg: &GitHubConfig, client: &reqwest::Client, url: &str, body: Value) -> Result<()> {
    send(cfg, client.post(url), url, body).await
}
async fn patch(cfg: &GitHubConfig, client: &reqwest::Client, url: &str, body: Value) -> Result<()> {
    send(cfg, client.patch(url), url, body).await
}

async fn send(
    cfg: &GitHubConfig,
    rb: reqwest::RequestBuilder,
    url: &str,
    body: Value,
) -> Result<()> {
    let resp = gh(cfg, rb)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("network error reaching GitHub at {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("GitHub write to {url} returned {status}: {text}");
    }
    Ok(())
}

/// Apply the standard GitHub auth + required headers.
fn gh(cfg: &GitHubConfig, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.header("Authorization", format!("Bearer {}", cfg.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
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

    fn r() -> IssueRef {
        IssueRef {
            owner: "acme".into(),
            repo: "api".into(),
            number: 42,
        }
    }

    #[test]
    fn builds_rest_urls() {
        assert_eq!(
            issue_url(&r()),
            "https://api.github.com/repos/acme/api/issues/42"
        );
        assert_eq!(
            assignees_url(&r()),
            "https://api.github.com/repos/acme/api/issues/42/assignees"
        );
        assert_eq!(
            labels_url(&r()),
            "https://api.github.com/repos/acme/api/issues/42/labels"
        );
        assert_eq!(
            issue_html_url(&r()),
            "https://github.com/acme/api/issues/42"
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
            ("parent", WriteField::Parent("owner/repo#1".into())),
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

    // DueDate / Priority / StoryPoints are redirected for GitHub (Projects v2 only).
    // Assert they parse correctly — the apply path redirects them at the HTTP layer,
    // so the parse must succeed (None would silently redirect as "unwritable field").
    #[test]
    fn github_redirect_fields_still_parse() {
        assert!(matches!(
            WriteField::parse("duedate", "2026-06-30"),
            Some(WriteField::DueDate(_))
        ));
        assert!(matches!(
            WriteField::parse("priority", "High"),
            Some(WriteField::Priority(_))
        ));
        assert!(matches!(
            WriteField::parse("story_points", "5"),
            Some(WriteField::StoryPoints(_))
        ));
    }
}
