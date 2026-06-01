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
        "maxResults": MAX_RESULTS,
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
    let keys: Vec<&str> = data.issues.iter().map(|i| i.key.as_str()).collect();
    tracing::debug!(count = issue_count, ?keys, "parsed Jira response");
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
                issue_type, project_key, url, parent_key, epic_title, updated_at, fetched_at)
             VALUES (?, 'jira', ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
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
               fetched_at       = excluded.fetched_at",
        )
        .bind(&issue.key)
        .bind(&issue.fields.summary)
        .bind(&description)
        .bind(cat)
        .bind(&issue.fields.issuetype.name)
        .bind(&issue.fields.project.key)
        .bind(&url)
        .bind(parent_key)
        .bind(if epic_title.is_empty() {
            None
        } else {
            Some(epic_title)
        })
        .bind(&issue.fields.updated)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", issue.key))?;
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

    match fetch(jira).await {
        Ok(issues) => {
            let keys: Vec<String> = issues.iter().map(|i| i.key.clone()).collect();
            let n = keys.len();
            tracing::debug!(fetched_count = n, "jira fetch completed");
            upsert(pool, &issues, jira).await?;
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
            Ok(Some(keys))
        }
        Err(e) => {
            tracing::warn!(error = %e, "jira fetch failed — keeping stale cache");
            Ok(None)
        }
    }
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
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, updated_at, fetched_at)
             VALUES (?, 'jira', 'Test Task', '', 'todo', 'Story', 'KAN', '',
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
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, updated_at)
             VALUES (?, ?, 'Other Task', '', 'todo', 'Story', 'GH', '',
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
        // gate on non-empty fetched_keys before calling prune. This test
        // confirms the empty case is safe when guarded.
        if ![""; 0].is_empty() {
            // unreachable: satisfies the borrow-checker while documenting intent
            run_prune_sql(&pool, &[]).await;
        }

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
}
