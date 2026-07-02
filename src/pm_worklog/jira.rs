//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Jira worklog poster — a faithful Rust port of `pm_worklog_update/jira_poster.py`.
// Turns (task_key, real_seconds, started_utc, comment) into a POST to
// `/rest/api/3/issue/{key}/worklog` and returns the new worklog id. Auth + the
// API base are resolved through `oauth::jira::resolve` — OAuth (Bearer, via the
// api.atlassian.com gateway) when a token store exists, else the legacy basic
// auth against the site URL.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, Utc};
use serde_json::{json, Value};

use crate::config::JiraConfig;
use crate::intelligence::oauth::jira::resolve;

/// Result of a successful worklog POST.
#[derive(Debug, Clone)]
pub struct WorklogPostResult {
    pub worklog_id: String,
    pub time_spent_jira: String,
    pub started_local: String,
}

/// Post one worklog entry to Jira. `started_utc_iso` is the window-start moment
/// as `YYYY-MM-DDTHH:MM:SSZ`. `time_spent_seconds` must be >= 60 (Jira's floor).
pub async fn post_worklog(
    jira: &JiraConfig,
    task_key: &str,
    time_spent_seconds: i64,
    started_utc_iso: &str,
    comment: &str,
) -> Result<WorklogPostResult> {
    if time_spent_seconds < 60 {
        bail!("time_spent_seconds={time_spent_seconds} below Jira's 60s minimum");
    }
    let jira_time = seconds_to_jira_time(time_spent_seconds)?;
    let started_local = render_started_local(started_utc_iso)?;

    let payload = json!({
        "timeSpent": jira_time,
        "started": started_local,
        "comment": build_adf_comment(comment),
    });

    let ctx = resolve(jira)
        .await
        .context("resolving Jira auth for worklog POST")?;
    let url = ctx.api_url(&format!("/rest/api/3/issue/{task_key}/worklog"));

    tracing::info!(
        task_key,
        time_spent = %jira_time,
        started = %started_local,
        comment_len = comment.len(),
        "jira worklog POST"
    );

    let client = reqwest::Client::new();
    let resp = ctx
        .apply(client.post(&url))
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("network error reaching Jira at {url}"))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Jira worklog POST for {task_key} returned {status}: {body}");
    }

    let parsed: Value =
        serde_json::from_str(&body).context("parsing Jira worklog response JSON")?;
    let worklog_id = parsed
        .get("id")
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string())
        })
        .context("Jira worklog response missing `id`")?;

    Ok(WorklogPostResult {
        worklog_id,
        time_spent_jira: jira_time,
        started_local,
    })
}

/// Delete a previously-posted worklog entry. Used when an already-`posted`
/// worklog is edited/re-matched (`meridian_core::worklogs::edit_worklog` /
/// `rematch_worklog` stash the old id in `unpost_worklog_id`): the stale
/// entry must be removed from Jira before the corrected content is reposted,
/// so nobody sees two worklog entries for the same window. A 404 (already
/// gone — e.g. deleted manually) is treated as success, matching the sweep's
/// idempotent-delete expectation.
pub async fn delete_worklog(jira: &JiraConfig, task_key: &str, worklog_id: &str) -> Result<()> {
    let ctx = resolve(jira)
        .await
        .context("resolving Jira auth for worklog DELETE")?;
    let url = ctx.api_url(&format!(
        "/rest/api/3/issue/{task_key}/worklog/{worklog_id}"
    ));

    tracing::info!(task_key, worklog_id, "jira worklog DELETE");

    let client = reqwest::Client::new();
    let resp = ctx
        .apply(client.delete(&url))
        .send()
        .await
        .with_context(|| format!("network error reaching Jira at {url}"))?;

    let status = resp.status();
    if status.is_success() || status.as_u16() == 404 {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    bail!("Jira worklog DELETE for {task_key}/{worklog_id} returned {status}: {body}");
}

/// Convert seconds → Jira's time-spent string (`"1h 30m"`), rounding to the
/// nearest minute. Jira rejects fractional minutes on the worklog API.
pub fn seconds_to_jira_time(seconds: i64) -> Result<String> {
    if seconds < 60 {
        bail!("seconds must be >= 60 for a Jira worklog (got {seconds})");
    }
    let minutes_total = (seconds + 30) / 60; // round-to-nearest
    let hours = minutes_total / 60;
    let minutes = minutes_total % 60;
    Ok(match (hours, minutes) {
        (h, m) if h > 0 && m > 0 => format!("{h}h {m}m"),
        (h, _) if h > 0 => format!("{h}h"),
        (_, m) => format!("{m}m"),
    })
}

/// Render a UTC ISO moment as `YYYY-MM-DDTHH:MM:SS.mmm+HHMM` (no colon in the
/// offset) in the host's local timezone — the format Jira's worklog API expects.
pub fn render_started_local(started_utc_iso: &str) -> Result<String> {
    let utc: DateTime<Utc> = parse_iso_utc(started_utc_iso)
        .with_context(|| format!("parsing started timestamp {started_utc_iso:?}"))?;
    let local: DateTime<Local> = utc.with_timezone(&Local);
    // `%z` yields `+HHMM` (no colon); `%3f` is millis.
    Ok(local.format("%Y-%m-%dT%H:%M:%S.%3f%z").to_string())
}

fn parse_iso_utc(iso: &str) -> Result<DateTime<Utc>> {
    // Accept trailing `Z` or an explicit offset.
    if let Ok(dt) = DateTime::parse_from_rfc3339(&iso.replace('Z', "+00:00")) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Fallback: naive UTC `YYYY-MM-DDTHH:MM:SS`.
    let naive =
        chrono::NaiveDateTime::parse_from_str(iso.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S")
            .context("unrecognised timestamp format")?;
    Ok(DateTime::from_naive_utc_and_offset(naive, Utc))
}

/// Wrap plain text in Atlassian Document Format for Jira Cloud.
fn build_adf_comment(text: &str) -> Value {
    json!({
        "type": "doc",
        "version": 1,
        "content": [
            { "type": "paragraph", "content": [ { "type": "text", "text": text } ] }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_time_rounds_to_nearest_minute() {
        assert_eq!(seconds_to_jira_time(60).unwrap(), "1m");
        assert_eq!(seconds_to_jira_time(89).unwrap(), "1m"); // 1.48m → 1m
        assert_eq!(seconds_to_jira_time(90).unwrap(), "2m"); // 1.5m → 2m
        assert_eq!(seconds_to_jira_time(3600).unwrap(), "1h");
        assert_eq!(seconds_to_jira_time(5400).unwrap(), "1h 30m");
        assert_eq!(seconds_to_jira_time(1079).unwrap(), "18m"); // 17.98 → 18
    }

    #[test]
    fn jira_time_rejects_below_minimum() {
        assert!(seconds_to_jira_time(59).is_err());
    }

    #[test]
    fn started_local_has_no_colon_in_offset() {
        let s = render_started_local("2026-05-30T05:00:00Z").unwrap();
        // e.g. 2026-05-30T10:30:00.000+0530 — offset must be +HHMM, no colon.
        let offset = &s[s.len() - 5..];
        assert!(
            offset.starts_with('+') || offset.starts_with('-'),
            "offset tail: {offset}"
        );
        assert!(
            !s[10..].contains("+00:00"),
            "offset should have no colon: {s}"
        );
        assert!(s.contains('.'), "should carry millis: {s}");
    }

    #[test]
    fn adf_comment_wraps_text() {
        let adf = build_adf_comment("hello");
        assert_eq!(adf["content"][0]["content"][0]["text"], "hello");
    }
}
