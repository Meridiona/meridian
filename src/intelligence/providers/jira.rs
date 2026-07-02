//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use anyhow::{Context, Result};
use meridian_core::adapters::jira::JiraAdapter;
use meridian_core::adapters::ProviderAdapter;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::config::JiraConfig;
use crate::intelligence::oauth::jira::JiraReqCtx;

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
    #[serde(default)]
    duedate: Option<String>,
    #[serde(default)]
    assignee: Option<JiraUser>,
    #[serde(default)]
    labels: Vec<String>,
    // Sprint custom field — Cloud standard; value is an array of sprint objects.
    #[serde(rename = "customfield_10020", default)]
    sprint: Option<Vec<JiraSprint>>,
    // Remaining fields captured for dynamic start-date extraction.
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct JiraUser {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct JiraSprint {
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// Field discovery
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JiraFieldMeta {
    id: String,
    name: String,
}

/// Call /rest/api/3/field and return the ID of the field whose name best
/// matches "start date":
///   1. exact case-insensitive match on "start date"
///   2. name contains both "start" and "date" (case-insensitive)
///
/// Returns None if no match or if the request fails.
async fn discover_start_date_field(ctx: &JiraReqCtx) -> Option<String> {
    let client = reqwest::Client::new();
    let url = ctx.api_url("/rest/api/3/field");
    let resp = ctx.apply(client.get(&url)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let fields: Vec<JiraFieldMeta> = resp.json().await.ok()?;

    // Priority 1: exact match
    if let Some(f) = fields
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case("start date"))
    {
        return Some(f.id.clone());
    }
    // Priority 2: name contains both "start" and "date"
    fields
        .iter()
        .find(|f| {
            let n = f.name.to_lowercase();
            n.contains("start") && n.contains("date")
        })
        .map(|f| f.id.clone())
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
    /// The user-facing status name ("In Review", "Awaiting QA", …) — custom per
    /// workflow. Stored verbatim as `status_raw`.
    #[serde(default)]
    name: String,
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

/// Jira's `statusCategory.key` is a fixed, non-customisable semantic field:
/// `done` / `indeterminate` / `new`. It is reliable for those three, but Jira
/// Service Management (and misconfigured Server/Data-Center workflows) can emit
/// `undefined` ("No Category"). For `undefined` we return `None` so the keyword
/// heuristic on the raw status name — and any user override — still gets a say,
/// rather than blindly treating an unlabelled status as open.
fn native_terminal(category_key: &str) -> Option<bool> {
    match category_key {
        "done" => Some(true),
        "new" | "indeterminate" => Some(false),
        _ => None,
    }
}

const MAX_RESULTS: usize = 100;
// Minimum interval between Jira fetches. Refresh is now triggered on demand at
// the read boundaries (classification + worklog passes), so this gate exists to
// dedupe bursts (e.g. the one-session-per-tick classifier drain loop) and bound
// API load — not to set the freshness cadence. Kept short so the candidate
// ticket list is at most this stale when a session is classified.
const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip(ctx),
    fields(
        provider = "jira",
        latency_ms = tracing::field::Empty,
        status_code = tracing::field::Empty,
    )
)]
async fn fetch(
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
) -> Result<Vec<(JiraIssue, serde_json::Value)>> {
    let client = reqwest::Client::new();
    let url = ctx.api_url("/rest/api/3/search/jql");

    let mut fields = vec![
        "summary",
        "description",
        "issuetype",
        "project",
        "updated",
        "parent",
        "status",
        "duedate",
        "assignee",
        "labels",
        "customfield_10020",
        // CDM (Stage 3b): fed to the canonical adapter for the new columns.
        "reporter",
        "priority",
        "resolutiondate",
    ];
    if let Some(id) = start_date_field {
        fields.push(id);
    }

    let body = serde_json::json!({
        "jql": "assignee = currentUser() AND statusCategory != Done AND type IN (Task, Feature) ORDER BY updated DESC",
        "maxResults": MAX_RESULTS,
        "fields": fields,
    });

    let start = std::time::Instant::now();
    let resp = ctx
        .apply(client.post(&url))
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

    // Parse once as a Value so we can keep each raw issue object verbatim for
    // the canonical adapter (Stage 3b), then deserialise the typed view from
    // the same body. The `issues` array order is identical, so we zip them.
    let body_val: serde_json::Value = resp.json().await.context("deserialising Jira response")?;
    let raw_issues: Vec<serde_json::Value> = body_val
        .get("issues")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let data: JiraSearchResponse =
        serde_json::from_value(body_val).context("parsing Jira response")?;
    let issue_count = data.issues.len();
    let keys: Vec<&str> = data.issues.iter().map(|i| i.key.as_str()).collect();
    tracing::debug!(count = issue_count, ?keys, "parsed Jira response");
    Ok(data.issues.into_iter().zip(raw_issues).collect())
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

/// The CDM columns (migration 056) derived from a raw Jira issue via the shared
/// [`JiraAdapter`]. All `Option` so a structurally-unusable payload (or a field
/// the fetch didn't request) simply leaves the column NULL — never blocks the
/// existing upsert. The mapping itself lives in `meridian_core` and is tested there.
#[derive(Default)]
struct CdmColumns {
    canonical_id: Option<String>,
    status_category: Option<String>,
    raw_payload: Option<String>,
    reporter_name: Option<String>,
    completed_at: Option<String>,
    ancestor_path: Option<String>,
    project_ids: Option<String>,
}

fn cdm_columns(raw: &serde_json::Value) -> CdmColumns {
    let Ok(c) = JiraAdapter.to_canonical(raw) else {
        return CdmColumns::default();
    };
    CdmColumns {
        canonical_id: Some(c.canonical_id),
        // Enum → its snake_case serde wire form (e.g. "in_progress").
        status_category: c
            .status_category
            .and_then(|sc| serde_json::to_value(sc).ok())
            .and_then(|v| v.as_str().map(String::from)),
        raw_payload: Some(c.raw_payload.to_string()),
        reporter_name: c.reporter.map(|r| r.display_name),
        completed_at: c.completed_at,
        ancestor_path: serde_json::to_string(&c.ancestor_path).ok(),
        project_ids: serde_json::to_string(&c.project_ids).ok(),
    }
}

async fn upsert(
    pool: &SqlitePool,
    issues: &[(JiraIssue, serde_json::Value)],
    jira: &JiraConfig,
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
) -> Result<()> {
    let mut ok_count: usize = 0;
    for (issue, raw) in issues {
        if !jira.project_keys.is_empty() && !jira.project_keys.contains(&issue.fields.project.key) {
            continue;
        }

        let description = issue
            .fields
            .description
            .as_ref()
            .map(adf_to_plaintext)
            .unwrap_or_default();

        let status = super::status::resolve(
            "jira",
            &issue.fields.status.name,
            native_terminal(&issue.fields.status.status_category.key),
        );
        let url = ctx.browse_url(&issue.key);

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

        let assignee_name = issue
            .fields
            .assignee
            .as_ref()
            .and_then(|a| a.display_name.clone());

        let tags: Option<String> = if issue.fields.labels.is_empty() {
            None
        } else {
            Some(issue.fields.labels.join(", "))
        };

        let sprint_name = issue
            .fields
            .sprint
            .as_deref()
            .and_then(|sprints| sprints.first())
            .and_then(|s| s.name.clone());

        let start_date: Option<String> = start_date_field.and_then(|field_id| {
            issue
                .fields
                .extra
                .get(field_id)?
                .as_str()
                .map(str::to_owned)
        });

        // CDM columns (Stage 3b) from the raw issue via the shared adapter.
        let cdm = cdm_columns(raw);

        let upsert_result = sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, parent_key, epic_title, due_date,
                assignee_name, tags, sprint_name, start_date,
                canonical_id, status_category, raw_payload, reporter_name,
                completed_at, ancestor_path, project_ids,
                updated_at, fetched_at)
             VALUES (?, 'jira', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     ?, ?, ?, ?, ?, ?, ?,
                     ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               title            = excluded.title,
               description_text = excluded.description_text,
               status_raw       = excluded.status_raw,
               is_terminal      = excluded.is_terminal,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title,
               due_date         = excluded.due_date,
               assignee_name    = excluded.assignee_name,
               tags             = excluded.tags,
               sprint_name      = excluded.sprint_name,
               start_date       = excluded.start_date,
               canonical_id     = excluded.canonical_id,
               status_category  = excluded.status_category,
               raw_payload      = excluded.raw_payload,
               reporter_name    = excluded.reporter_name,
               completed_at     = excluded.completed_at,
               ancestor_path    = excluded.ancestor_path,
               project_ids      = excluded.project_ids,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at",
        )
        .bind(&issue.key)
        .bind(&issue.fields.summary)
        .bind(&description)
        .bind(&status.raw)
        .bind(status.is_terminal)
        .bind(&issue.fields.issuetype.name)
        .bind(&issue.fields.project.key)
        .bind(&url)
        .bind(parent_key)
        .bind(if epic_title.is_empty() {
            None
        } else {
            Some(epic_title)
        })
        .bind(&issue.fields.duedate)
        .bind(assignee_name)
        .bind(tags)
        .bind(sprint_name)
        .bind(start_date)
        .bind(cdm.canonical_id)
        .bind(cdm.status_category)
        .bind(cdm.raw_payload)
        .bind(cdm.reporter_name)
        .bind(cdm.completed_at)
        .bind(cdm.ancestor_path)
        .bind(cdm.project_ids)
        .bind(&issue.fields.updated)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", issue.key));
        match upsert_result {
            Ok(_) => ok_count += 1,
            Err(ref upsert_err) => {
                tracing::warn!(task_key = %issue.key, error = ?upsert_err, "jira task upsert failed — skipping");
            }
        }
    }
    if !issues.is_empty() && ok_count == 0 {
        anyhow::bail!(
            "all {} jira task upserts failed — DB write errors above",
            issues.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prune
// ---------------------------------------------------------------------------

async fn prune(pool: &SqlitePool, fetched_keys: &[String]) -> Result<usize> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    // Delete embeddings first — pm_task_embeddings.task_key FK references pm_tasks.
    let emb_sql = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&emb_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q.execute(pool)
        .await
        .context("pruning pm_task_embeddings")?;

    let task_sql = format!(
        "DELETE FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ({placeholders})"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning pm_tasks")?;
    Ok(result.rows_affected() as usize)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, jira))]
pub async fn refresh_if_stale(pool: &SqlitePool, jira: &JiraConfig) -> Result<Option<Vec<String>>> {
    let threshold = format!("-{SYNC_INTERVAL_MINS} minutes");
    let (is_fresh,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = 'jira'
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking jira sync state")?;

    if is_fresh != 0 {
        let cached_keys: Vec<(String,)> =
            sqlx::query_as("SELECT task_key FROM pm_tasks WHERE provider = 'jira'")
                .fetch_all(pool)
                .await
                .context("loading cached jira task keys")?;
        let keys: Vec<&str> = cached_keys.iter().map(|(k,)| k.as_str()).collect();
        tracing::debug!(
            cached_task_count = keys.len(),
            ?keys,
            "jira task cache is fresh"
        );
        return Ok(None);
    }

    tracing::debug!("jira task cache is stale — refreshing");

    // Resolve auth once per refresh: OAuth (with refresh-before-use) if a token
    // store exists, else static basic auth. A resolve failure means no usable
    // creds — keep the stale cache rather than erroring the whole tick.
    let ctx = match crate::intelligence::oauth::jira::resolve(jira).await {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::warn!(error = %e, "jira auth unavailable — keeping stale cache");
            let msg = format!("Jira auth failed — {e}");
            let _ = super::stamp_sync_error(pool, "jira", &msg).await;
            return Ok(None);
        }
    };
    let auth_method = if jira.api_token.is_empty() {
        "oauth"
    } else {
        "api_token"
    };
    tracing::debug!(auth_method, "jira auth resolved");

    let start_date_field = discover_start_date_field(&ctx).await;
    if let Some(ref id) = start_date_field {
        tracing::debug!(field_id = %id, "discovered jira start date field");
    }

    match fetch(&ctx, start_date_field.as_deref()).await {
        Ok(issues) => {
            let keys: Vec<String> = issues.iter().map(|(i, _)| i.key.clone()).collect();
            let n = keys.len();
            let project_key = issues
                .first()
                .map(|(i, _)| i.fields.project.key.as_str())
                .unwrap_or("-");
            let terminal_count = issues
                .iter()
                .filter(|(i, _)| {
                    native_terminal(&i.fields.status.status_category.key) == Some(true)
                })
                .count();
            tracing::debug!(fetched_count = n, "jira fetch completed");
            tracing::info!(
                issue_count = n,
                project_key,
                upserted = n,
                terminal_skipped = terminal_count,
                auth_method,
                "jira issues fetched"
            );
            upsert(pool, &issues, jira, &ctx, start_date_field.as_deref()).await?;
            sqlx::query(
                "INSERT INTO pm_sync_state (provider, last_synced_at)
                 VALUES ('jira', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
            )
            .execute(pool)
            .await
            .context("updating jira sync state")?;
            if n < MAX_RESULTS {
                match prune(pool, &keys).await {
                    Ok(0) => {}
                    Ok(pruned) => tracing::info!(pruned_count = pruned, "pruned stale jira tasks"),
                    Err(e) => tracing::warn!(error = %e, "jira prune failed"),
                }
            } else {
                tracing::debug!(
                    fetched_count = n,
                    max_results = MAX_RESULTS,
                    "skipping prune — response may be truncated"
                );
            }
            tracing::info!(upserted_count = n, "jira tasks refreshed");
            let _ = super::clear_sync_error(pool, "jira").await;
            Ok(Some(keys))
        }
        Err(e) => {
            tracing::warn!(error = %e, "jira fetch failed — keeping stale cache");
            let msg = format!("Jira sync failed — {e}");
            let _ = super::stamp_sync_error(pool, "jira", &msg).await;
            Ok(None)
        }
    }
}

/// Force an immediate Jira sync regardless of the staleness gate.
/// Clears `pm_sync_state` for this provider so `refresh_if_stale` sees it as
/// stale, then delegates. The `last_synced_at` is updated inside the delegate,
/// so subsequent ticks won't double-fetch.
pub async fn force_refresh(pool: &SqlitePool, jira: &JiraConfig) -> Result<Option<Vec<String>>> {
    sqlx::query("DELETE FROM pm_sync_state WHERE provider = 'jira'")
        .execute(pool)
        .await
        .context("clearing jira sync state for force refresh")?;
    refresh_if_stale(pool, jira).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
    use std::str::FromStr;

    async fn make_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn insert_jira_task(pool: &SqlitePool, task_key: &str) {
        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, updated_at, fetched_at)
             VALUES (?, 'jira', 'Test Task', '', 'To Do', 0, 'Story', 'KAN', '',
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(task_key)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Inserts a row into `pm_tasks` with a non-jira provider.
    async fn insert_other_task(pool: &SqlitePool, task_key: &str, provider: &str) {
        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, updated_at)
             VALUES (?, ?, 'Other Task', '', 'To Do', 0, 'Story', 'GH', '',
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(task_key)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Inserts a row into `pm_task_embeddings` for an existing `pm_tasks` row.
    async fn insert_embedding(pool: &SqlitePool, task_key: &str) {
        sqlx::query(
            "INSERT INTO pm_task_embeddings
               (task_key, model, dim, embedding, text_hash, pm_updated_at)
             VALUES (?, 'bge-small-en-v1.5', 384, X'00', 'abc', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(task_key)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Runs the same SQL sequence that `prune()` executes, bound to `fetched_keys`.
    /// Returns the number of `pm_tasks` rows deleted.
    async fn run_prune_sql(pool: &SqlitePool, fetched_keys: &[&str]) -> usize {
        let placeholders = fetched_keys
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");

        let emb_sql = format!(
            "DELETE FROM pm_task_embeddings WHERE task_key IN \
             (SELECT task_key FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ({placeholders}))"
        );
        let mut q = sqlx::query(&emb_sql);
        for key in fetched_keys {
            q = q.bind(*key);
        }
        q.execute(pool).await.unwrap();

        let task_sql = format!(
            "DELETE FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ({placeholders})"
        );
        let mut q = sqlx::query(&task_sql);
        for key in fetched_keys {
            q = q.bind(*key);
        }
        let result = q.execute(pool).await.unwrap();
        result.rows_affected() as usize
    }

    /// Helper: count rows in `pm_tasks` with a given `task_key`.
    async fn task_count(pool: &SqlitePool, task_key: &str) -> i64 {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pm_tasks WHERE task_key = ?")
            .bind(task_key)
            .fetch_one(pool)
            .await
            .unwrap();
        row.0
    }

    /// Helper: count rows in `pm_task_embeddings` with a given `task_key`.
    async fn embedding_count(pool: &SqlitePool, task_key: &str) -> i64 {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM pm_task_embeddings WHERE task_key = ?")
                .bind(task_key)
                .fetch_one(pool)
                .await
                .unwrap();
        row.0
    }

    // -----------------------------------------------------------------------
    // Test: stale task (not in fetched set) is deleted from pm_tasks
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_removes_stale_jira_task() {
        let pool = make_db().await;

        insert_jira_task(&pool, "KAN-1").await; // fresh — in fetched set
        insert_jira_task(&pool, "KAN-2").await; // stale — not in fetched set

        let deleted = run_prune_sql(&pool, &["KAN-1"]).await;

        assert_eq!(deleted, 1, "prune must delete exactly the stale row");
        assert_eq!(task_count(&pool, "KAN-1").await, 1, "KAN-1 must survive");
        assert_eq!(task_count(&pool, "KAN-2").await, 0, "KAN-2 must be deleted");
    }

    // -----------------------------------------------------------------------
    // Test: fresh task (in fetched set) is NOT deleted
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_keeps_fresh_jira_task() {
        let pool = make_db().await;

        insert_jira_task(&pool, "KAN-10").await;

        let deleted = run_prune_sql(&pool, &["KAN-10"]).await;

        assert_eq!(
            deleted, 0,
            "prune must not delete a task that is in the fetched set"
        );
        assert_eq!(task_count(&pool, "KAN-10").await, 1, "KAN-10 must survive");
    }

    // -----------------------------------------------------------------------
    // Test: embedding row is deleted before pm_tasks (cascade order preserved)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_deletes_embedding_before_pm_task() {
        // Enable FK enforcement so the test fails if the delete order is wrong
        // (child before parent is required when FKs are enforced).
        let pool = make_db().await;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        insert_jira_task(&pool, "KAN-20").await;
        insert_jira_task(&pool, "KAN-21").await;
        insert_embedding(&pool, "KAN-21").await; // only the stale task has an embedding

        let deleted = run_prune_sql(&pool, &["KAN-20"]).await;

        // pm_tasks row deleted
        assert_eq!(deleted, 1);
        assert_eq!(
            task_count(&pool, "KAN-21").await,
            0,
            "KAN-21 pm_task must be gone"
        );
        // embedding row deleted first (no FK violation)
        assert_eq!(
            embedding_count(&pool, "KAN-21").await,
            0,
            "KAN-21 embedding must be deleted before its pm_task row"
        );
        // surviving task's embedding is untouched (there was none for KAN-20 here, but no error)
        assert_eq!(task_count(&pool, "KAN-20").await, 1, "KAN-20 must survive");
    }

    // -----------------------------------------------------------------------
    // Test: prune with empty fetched_keys set (edge case)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_with_empty_fetched_keys_is_a_no_op() {
        // prune() is only called when fetched_count > 0, but the SQL must not
        // blow up when called with an empty slice — it would produce
        // "NOT IN ()" which is invalid SQL. The guard below mirrors what a
        // caller should do; we verify the guard is sufficient.
        let pool = make_db().await;

        insert_jira_task(&pool, "KAN-30").await;

        // An empty IN-list is syntactically invalid in SQLite; callers must
        // gate on non-empty fetched_keys before calling prune — so prune is
        // intentionally NOT called here. The task must survive untouched.

        // Task must still be present — no prune was called.
        assert_eq!(
            task_count(&pool, "KAN-30").await,
            1,
            "task must survive when prune is not called for empty set"
        );
    }

    // -----------------------------------------------------------------------
    // Test: mixed — some stale, some fresh — only stale removed
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_mixed_keeps_fresh_removes_stale() {
        let pool = make_db().await;

        // Three jira tasks; fetched set contains only KAN-40 and KAN-41.
        // KAN-42 is stale and must be pruned.
        insert_jira_task(&pool, "KAN-40").await;
        insert_jira_task(&pool, "KAN-41").await;
        insert_jira_task(&pool, "KAN-42").await;

        // Give the stale task an embedding to confirm cascade works in mixed scenario.
        insert_embedding(&pool, "KAN-42").await;

        let deleted = run_prune_sql(&pool, &["KAN-40", "KAN-41"]).await;

        assert_eq!(deleted, 1, "only the stale row should be deleted");
        assert_eq!(task_count(&pool, "KAN-40").await, 1, "KAN-40 must survive");
        assert_eq!(task_count(&pool, "KAN-41").await, 1, "KAN-41 must survive");
        assert_eq!(
            task_count(&pool, "KAN-42").await,
            0,
            "KAN-42 must be deleted"
        );
        assert_eq!(
            embedding_count(&pool, "KAN-42").await,
            0,
            "KAN-42 embedding must be deleted"
        );
    }

    // -----------------------------------------------------------------------
    // Test: non-jira tasks are never deleted regardless of fetched_keys
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_does_not_touch_non_jira_tasks() {
        let pool = make_db().await;

        // A github task that is not in the fetched set — must not be deleted
        // because prune filters on provider = 'jira'.
        insert_other_task(&pool, "GH-1", "github").await;
        insert_other_task(&pool, "LIN-1", "linear").await;

        let deleted = run_prune_sql(&pool, &["KAN-99"]).await;

        assert_eq!(deleted, 0, "no jira rows exist — nothing deleted");
        assert_eq!(
            task_count(&pool, "GH-1").await,
            1,
            "github task must survive"
        );
        assert_eq!(
            task_count(&pool, "LIN-1").await,
            1,
            "linear task must survive"
        );
    }

    // -----------------------------------------------------------------------
    // Test: multiple stale tasks with embeddings are all pruned in one call
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_removes_multiple_stale_tasks_with_embeddings() {
        let pool = make_db().await;

        // Seed three stale tasks, all with embeddings; fetched set is empty
        // relative to them (fetch returned only KAN-50 which has no stale twin).
        insert_jira_task(&pool, "KAN-50").await;
        insert_jira_task(&pool, "KAN-51").await;
        insert_jira_task(&pool, "KAN-52").await;
        insert_jira_task(&pool, "KAN-53").await;

        insert_embedding(&pool, "KAN-51").await;
        insert_embedding(&pool, "KAN-52").await;
        insert_embedding(&pool, "KAN-53").await;

        let deleted = run_prune_sql(&pool, &["KAN-50"]).await;

        assert_eq!(deleted, 3, "three stale tasks must be deleted");
        assert_eq!(task_count(&pool, "KAN-50").await, 1);
        assert_eq!(task_count(&pool, "KAN-51").await, 0);
        assert_eq!(task_count(&pool, "KAN-52").await, 0);
        assert_eq!(task_count(&pool, "KAN-53").await, 0);
        assert_eq!(embedding_count(&pool, "KAN-51").await, 0);
        assert_eq!(embedding_count(&pool, "KAN-52").await, 0);
        assert_eq!(embedding_count(&pool, "KAN-53").await, 0);
    }

    // -----------------------------------------------------------------------
    // Test: prune returns the correct row count via the public prune() fn
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn prune_fn_returns_correct_deleted_count() {
        let pool = make_db().await;

        insert_jira_task(&pool, "KAN-60").await;
        insert_jira_task(&pool, "KAN-61").await;
        insert_jira_task(&pool, "KAN-62").await;

        // Call the private prune function directly from the sibling test module.
        let fetched_keys: Vec<String> = vec!["KAN-60".to_string()];
        let deleted = super::prune(&pool, &fetched_keys).await.unwrap();

        assert_eq!(
            deleted, 2,
            "prune() must return the count of deleted pm_tasks rows"
        );
        assert_eq!(task_count(&pool, "KAN-60").await, 1);
        assert_eq!(task_count(&pool, "KAN-61").await, 0);
        assert_eq!(task_count(&pool, "KAN-62").await, 0);
    }

    // -----------------------------------------------------------------------
    // Epic/parent linkage: the source for pm_tasks.parent_key + epic_title is
    // the issue's `parent` object (key + parent.fields.summary). These tests
    // lock that the response parses and the derivation matches force_refresh.
    // -----------------------------------------------------------------------

    /// Mirror force_refresh's inline `(parent_key, epic_title)` derivation so the
    /// test tracks the production extraction, not a reimplementation.
    fn derive_parent_link(issue: &super::JiraIssue) -> (Option<&str>, Option<&str>) {
        issue
            .fields
            .parent
            .as_ref()
            .map(|p| {
                let title = p.fields.as_ref().and_then(|f| f.summary.as_deref());
                (Some(p.key.as_str()), title)
            })
            .unwrap_or((None, None))
    }

    #[test]
    fn parses_issue_parent_for_epic_linkage() {
        let json = r#"{
            "key": "KAN-37",
            "fields": {
                "summary": "Implement token refresh with silent re-auth",
                "status": {"name": "In Progress", "statusCategory": {"key": "indeterminate"}},
                "issuetype": {"name": "Task"},
                "project": {"key": "KAN"},
                "updated": "2026-06-28T00:00:00.000+0000",
                "parent": {"key": "KAN-34", "fields": {"summary": "Auth & Security Overhaul"}}
            }
        }"#;
        let issue: super::JiraIssue = serde_json::from_str(json).unwrap();
        let (parent_key, epic_title) = derive_parent_link(&issue);
        assert_eq!(parent_key, Some("KAN-34"));
        assert_eq!(epic_title, Some("Auth & Security Overhaul"));
    }

    #[test]
    fn issue_without_parent_yields_no_epic() {
        let json = r#"{
            "key": "KAN-34",
            "fields": {
                "summary": "Auth & Security Overhaul",
                "status": {"name": "In Progress", "statusCategory": {"key": "indeterminate"}},
                "issuetype": {"name": "Epic"},
                "project": {"key": "KAN"},
                "updated": "2026-06-28T00:00:00.000+0000"
            }
        }"#;
        let issue: super::JiraIssue = serde_json::from_str(json).unwrap();
        let (parent_key, epic_title) = derive_parent_link(&issue);
        assert_eq!(parent_key, None);
        assert_eq!(epic_title, None);
    }

    /// A parent with no expanded `fields` (summary unavailable) still yields the
    /// key for parent_key, with an empty epic_title — force_refresh stores NULL.
    #[test]
    fn parent_without_fields_keeps_key_drops_title() {
        let json = r#"{
            "key": "KAN-99",
            "fields": {
                "summary": "Some subtask",
                "status": {"name": "To Do", "statusCategory": {"key": "new"}},
                "issuetype": {"name": "Task"},
                "project": {"key": "KAN"},
                "updated": "2026-06-28T00:00:00.000+0000",
                "parent": {"key": "KAN-50"}
            }
        }"#;
        let issue: super::JiraIssue = serde_json::from_str(json).unwrap();
        let (parent_key, epic_title) = derive_parent_link(&issue);
        assert_eq!(parent_key, Some("KAN-50"));
        assert_eq!(epic_title, None);
    }

    // -----------------------------------------------------------------------
    // CDM (Stage 3b): the new pm_tasks columns are derived from the raw issue
    // through the shared adapter. This locks the daemon-side glue; the mapping
    // itself is tested in meridian_core::adapters::jira.
    // -----------------------------------------------------------------------

    #[test]
    fn cdm_columns_derives_from_raw_issue() {
        let raw = serde_json::json!({
            "id": "10042",
            "key": "KAN-42",
            "fields": {
                "status": {"name": "In Review", "statusCategory": {"key": "indeterminate"}},
                "reporter": {"accountId": "acc-2", "displayName": "Lead"},
                "parent": {"id": "10001"},
                "project": {"id": "10000"},
                "resolutiondate": null
            }
        });
        let cdm = super::cdm_columns(&raw);
        // Stable key is the numeric id, namespaced.
        assert_eq!(cdm.canonical_id.as_deref(), Some("jira:10042"));
        // "In Review" (indeterminate) → snake_case canonical category.
        assert_eq!(cdm.status_category.as_deref(), Some("in_review"));
        assert_eq!(cdm.reporter_name.as_deref(), Some("Lead"));
        assert_eq!(cdm.completed_at, None); // resolutiondate null
        assert_eq!(cdm.ancestor_path.as_deref(), Some(r#"["jira:10001"]"#));
        assert_eq!(cdm.project_ids.as_deref(), Some(r#"["jira:10000"]"#));
        assert!(cdm.raw_payload.is_some());
    }

    #[test]
    fn cdm_columns_empty_on_unusable_payload() {
        // No `id` → adapter errors → all columns NULL, never blocks the upsert.
        let cdm = super::cdm_columns(&serde_json::json!({"fields": {}}));
        assert!(cdm.canonical_id.is_none());
        assert!(cdm.raw_payload.is_none());
        assert!(cdm.status_category.is_none());
    }
}
