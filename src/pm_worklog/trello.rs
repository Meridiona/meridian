// meridian — normalises screenpipe activity into structured app sessions
//
// Trello worklog poster. Trello has no native time-tracking API, so a worklog
// is a structured Markdown comment on the card via POST /1/cards/{shortLink}/
// actions/comments. Auth: key + token as query params (Trello's standard
// pattern). The task_key stored in pm_tasks is the card shortLink.
//
// The comment body follows the shared comment.rs format with a machine-readable
// meridian-worklog marker so it can be detected on re-import.

use anyhow::{bail, Context, Result};

use super::comment::{format_worklog_comment, seconds_to_human, PostedWorklog};
use crate::config::TrelloConfig;
use crate::intelligence::oauth::trello as oauth_trello;

const TRELLO_BASE: &str = "https://api.trello.com/1";

/// Post a worklog comment to the Trello card identified by `task_key`
/// (the card's shortLink, e.g. `HSkL1pnj`). Returns the created action id.
pub async fn post_worklog(
    trello: &TrelloConfig,
    task_key: &str,
    time_spent_seconds: i64,
    window_start_iso: &str,
    window_end_iso: &str,
    comment: &str,
) -> Result<PostedWorklog> {
    if time_spent_seconds < 60 {
        bail!("time_spent_seconds={time_spent_seconds} below the 60s worklog minimum");
    }

    let token = oauth_trello::load_token().context("loading Trello OAuth token")?;
    let short_link = parse_short_link(task_key)?;
    let body = format_worklog_comment(comment, time_spent_seconds, window_start_iso, window_end_iso);

    let url = format!(
        "{TRELLO_BASE}/cards/{short_link}/actions/comments\
         ?key={}&token={}",
        trello.app_key, token,
    );

    let client = reqwest::Client::new();

    tracing::info!(
        task_key,
        short_link = %short_link,
        time_spent = %seconds_to_human(time_spent_seconds),
        comment_len = body.len(),
        "trello worklog comment create"
    );

    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "text": body }))
        .send()
        .await
        .with_context(|| format!("POST Trello comment for card {short_link}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Trello comment API → {status} for card {short_link}: {text}");
    }

    let action: serde_json::Value =
        serde_json::from_str(&text).context("parsing Trello action response")?;
    let action_id = action["id"]
        .as_str()
        .with_context(|| format!("Trello action response missing id: {text}"))?
        .to_string();

    Ok(PostedWorklog {
        id: action_id,
        label: seconds_to_human(time_spent_seconds),
    })
}

/// Accept a card shortLink (8-char alphanumeric) or a full Trello card URL
/// (`https://trello.com/c/{shortLink}/...`). Returns the shortLink.
pub fn parse_short_link(task_key: &str) -> Result<String> {
    if task_key.starts_with("https://trello.com/c/") || task_key.starts_with("http://trello.com/c/")
    {
        let after = task_key.split_once("/c/").map(|x| x.1).unwrap_or(task_key);
        let short_link = after.split('/').next().unwrap_or(after);
        if short_link.is_empty() {
            bail!("could not extract shortLink from Trello URL: {task_key}");
        }
        return Ok(short_link.to_string());
    }
    if task_key.is_empty() {
        bail!("task_key is empty — expected a Trello card shortLink");
    }
    Ok(task_key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pm_worklog::comment::format_worklog_comment;

    #[test]
    fn parse_short_link_passthrough() {
        assert_eq!(parse_short_link("HSkL1pnj").unwrap(), "HSkL1pnj");
    }

    #[test]
    fn parse_short_link_from_url() {
        assert_eq!(
            parse_short_link("https://trello.com/c/HSkL1pnj/42-card-title").unwrap(),
            "HSkL1pnj"
        );
    }

    #[test]
    fn parse_short_link_url_no_title() {
        assert_eq!(
            parse_short_link("https://trello.com/c/HSkL1pnj").unwrap(),
            "HSkL1pnj"
        );
    }

    #[test]
    fn parse_short_link_empty_is_err() {
        assert!(parse_short_link("").is_err());
    }

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
}
