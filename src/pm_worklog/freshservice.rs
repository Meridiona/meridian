//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Freshservice comment poster. Freshservice has a native Time Entries API, but
// this module only wires the plain-comment primitive `super::post_comment` needs
// (the day-task "Generate worklog" → approve flow, which posts a rich status
// update, not a time-tracked worklog) — see `FreshserviceConfig`'s doc comment
// in `config.rs` for why this stops short of a full `PmProviderConfig` variant.
//
// Auth: HTTP Basic, the API key as username and any character as password
// (Freshservice's documented convention: `{api_key}:X`). `task_key` is the
// numeric ticket id (Freshservice tickets have no project-prefixed key like
// Jira/Linear — just `/api/v2/tickets/{id}`).

use anyhow::{bail, Context, Result};

use crate::config::FreshserviceConfig;

/// Freshservice's own status ids (`GET /api/v2/ticket_form_fields`), fixed across
/// every account regardless of custom status names layered on top.
const STATUS_OPEN: i32 = 2;
const STATUS_RESOLVED: i32 = 4;

/// Validate `task_key` is a Freshservice ticket id (plain numeric, no
/// project-prefixed key like Jira/Linear) and return it trimmed.
fn ticket_id(task_key: &str) -> Result<&str> {
    let id = task_key.trim();
    if id.is_empty() || id.parse::<u64>().is_err() {
        bail!("freshservice: task_key must be a numeric ticket id, got {task_key:?}");
    }
    Ok(id)
}

/// Post `body` as a note on the Freshservice ticket `task_key`. Always private
/// (`private: true`) — this is an internal status update, never a reply routed
/// to the requester. Returns the created note's id.
pub async fn post_comment(fs: &FreshserviceConfig, task_key: &str, body: &str) -> Result<String> {
    let ticket_id = ticket_id(task_key)?;

    let url = format!(
        "https://{}.freshservice.com/api/v2/tickets/{ticket_id}/notes",
        fs.domain
    );

    tracing::info!(task_key, body_len = body.len(), "freshservice note create");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .post(&url)
        .basic_auth(&fs.api_key, Some("X"))
        .json(&serde_json::json!({ "body": body, "private": true }))
        .send()
        .await
        .with_context(|| format!("POST Freshservice note for ticket {ticket_id}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Freshservice note POST for ticket {ticket_id} returned {status}: {text}");
    }

    let parsed: serde_json::Value =
        serde_json::from_str(&text).context("parsing Freshservice note response")?;
    parsed["conversation"]["id"]
        .as_i64()
        .map(|id| id.to_string())
        .with_context(|| format!("Freshservice note response missing conversation.id: {text}"))
}

/// Set the ticket's status by Freshservice's numeric status id. Shared by
/// [`close`]/[`reopen`] — the only two transitions `plan_tasks::done` calls.
async fn set_status(fs: &FreshserviceConfig, task_key: &str, status: i32) -> Result<()> {
    let ticket_id = ticket_id(task_key)?;
    let url = format!(
        "https://{}.freshservice.com/api/v2/tickets/{ticket_id}",
        fs.domain
    );

    tracing::info!(task_key, status, "freshservice status update");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .put(&url)
        .basic_auth(&fs.api_key, Some("X"))
        .json(&serde_json::json!({ "status": status }))
        .send()
        .await
        .with_context(|| format!("PUT Freshservice status for ticket {ticket_id}"))?;

    let resp_status = resp.status();
    if !resp_status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("Freshservice status PUT for ticket {ticket_id} returned {resp_status}: {text}");
    }
    Ok(())
}

/// Transition to Freshservice's "Resolved" status — the closest fit for "done":
/// the requested work is complete, pending the requester's own confirmation.
/// Freshservice's terminal "Closed" is deliberately not used here — that is an
/// administrative state a requester/agent applies after confirming, not
/// something a day-task checkbox should jump straight to.
pub async fn close(fs: &FreshserviceConfig, task_key: &str) -> Result<()> {
    set_status(fs, task_key, STATUS_RESOLVED).await
}

/// Transition back to "Open" — undoes [`close`]. Mirrors every other provider's
/// `reopen`: the default not-done state, not necessarily the exact status the
/// ticket was in before.
pub async fn reopen(fs: &FreshserviceConfig, task_key: &str) -> Result<()> {
    set_status(fs, task_key, STATUS_OPEN).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_numeric_task_key_is_a_clean_error_not_a_bad_request() {
        let fs = FreshserviceConfig {
            domain: "example".into(),
            api_key: "irrelevant".into(),
        };
        let err = post_comment(&fs, "INC-9", "hi")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("numeric ticket id"), "{err}");
    }
}
