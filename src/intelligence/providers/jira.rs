// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::config::JiraConfig;

// ---------------------------------------------------------------------------
// Jira REST response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JiraSearchResponse {
    issues: Vec<JiraIssue>,
}

#[derive(Deserialize)]
struct JiraIssue {
    key: String,
    fields: JiraFields,
}

#[derive(Deserialize)]
struct JiraFields {
    summary: String,
    description: Option<serde_json::Value>,
    status: JiraStatus,
    issuetype: JiraIssueType,
    project: JiraProject,
    updated: String,
    #[serde(rename = "parent")]
    parent: Option<JiraParent>,
}

#[derive(Deserialize)]
struct JiraParent {
    key: String,
    fields: Option<JiraParentFields>,
}

#[derive(Deserialize)]
struct JiraParentFields {
    summary: Option<String>,
}

#[derive(Deserialize)]
struct JiraStatus {
    #[serde(rename = "statusCategory")]
    status_category: JiraStatusCategory,
}

#[derive(Deserialize)]
struct JiraStatusCategory {
    key: String,
}

#[derive(Deserialize)]
struct JiraIssueType {
    name: String,
}

#[derive(Deserialize)]
struct JiraProject {
    key: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn adf_to_plaintext(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(obj) => {
            let mut parts = Vec::new();
            if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_owned());
                }
            }
            if let Some(content) = obj.get("content").and_then(|v| v.as_array()) {
                for node in content {
                    let part = adf_to_plaintext(node);
                    if !part.is_empty() {
                        parts.push(part);
                    }
                }
            }
            parts.join(" ")
        }
        _ => String::new(),
    }
}

fn map_status_category(key: &str) -> &'static str {
    match key {
        "done" => "done",
        "indeterminate" => "in_progress",
        _ => "todo",
    }
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip(jira),
    fields(
        provider = "jira",
        latency_ms = tracing::field::Empty,
        status_code = tracing::field::Empty,
    )
)]
async fn fetch(jira: &JiraConfig) -> Result<Vec<JiraIssue>> {
    let client = reqwest::Client::new();
    let url = format!("{}/rest/api/3/search/jql", jira.base_url);

    let body = serde_json::json!({
        "jql": "assignee = currentUser() AND statusCategory != Done AND type IN (Task, Feature) ORDER BY updated DESC",
        "maxResults": 100,
        "fields": ["summary", "description", "issuetype", "project", "updated", "parent", "status"]
    });

    let start = std::time::Instant::now();
    let resp = client
        .post(&url)
        .basic_auth(&jira.email, Some(&jira.api_token))
        .json(&body)
        .send()
        .await
        .context("POST /search/jql")?;

    let status = resp.status();
    let elapsed_ms = start.elapsed().as_millis() as i64;
    tracing::Span::current().record("status_code", status.as_u16() as i64);
    tracing::Span::current().record("latency_ms", elapsed_ms);

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Jira /search/jql → {}: {}", status, text);
    }

    let data: JiraSearchResponse = resp.json().await.context("deserialising Jira response")?;
    let issue_count = data.issues.len();
    tracing::debug!(count = issue_count, "parsed Jira response");
    Ok(data.issues)
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(pool: &SqlitePool, issues: &[JiraIssue], jira: &JiraConfig) -> Result<()> {
    for issue in issues {
        if !jira.project_keys.is_empty() && !jira.project_keys.contains(&issue.fields.project.key) {
            continue;
        }

        let description = issue
            .fields
            .description
            .as_ref()
            .map(adf_to_plaintext)
            .unwrap_or_default();

        let cat = map_status_category(&issue.fields.status.status_category.key);
        let url = format!("{}/browse/{}", jira.base_url, issue.key);

        let (parent_key, epic_title) = issue
            .fields
            .parent
            .as_ref()
            .map(|p| {
                let title = p
                    .fields
                    .as_ref()
                    .and_then(|f| f.summary.as_deref())
                    .unwrap_or("");
                (Some(p.key.as_str()), title)
            })
            .unwrap_or((None, ""));

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, parent_key, epic_title, updated_at, fetched_at, expires_at)
             VALUES (?, 'jira', ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '+30 minutes'))
             ON CONFLICT(task_key) DO UPDATE SET
               title            = excluded.title,
               description_text = excluded.description_text,
               status_category  = excluded.status_category,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at,
               expires_at       = excluded.expires_at",
        )
        .bind(&issue.key)
        .bind(&issue.fields.summary)
        .bind(&description)
        .bind(cat)
        .bind(&issue.fields.issuetype.name)
        .bind(&issue.fields.project.key)
        .bind(&url)
        .bind(parent_key)
        .bind(if epic_title.is_empty() { None } else { Some(epic_title) })
        .bind(&issue.fields.updated)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", issue.key))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, jira))]
pub async fn refresh_if_stale(pool: &SqlitePool, jira: &JiraConfig) -> Result<()> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pm_tasks
         WHERE provider = 'jira'
           AND expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .fetch_one(pool)
    .await
    .context("checking jira task cache staleness")?;

    if count > 0 {
        tracing::debug!(cached_task_count = count, "jira task cache is fresh");
        return Ok(());
    }

    tracing::debug!("jira task cache is stale — refreshing");

    match fetch(jira).await {
        Ok(issues) => {
            let n = issues.len();
            tracing::debug!(fetched_count = n, "jira fetch completed");
            upsert(pool, &issues, jira).await?;
            tracing::info!(upserted_count = n, "jira tasks refreshed");
        }
        Err(e) => {
            tracing::warn!(error = %e, "jira fetch failed — keeping stale cache");
        }
    }

    Ok(())
}
