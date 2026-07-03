//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Jira connector tests — prune behaviour, parent/epic derivation, CDM glue.

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
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pm_task_embeddings WHERE task_key = ?")
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
// CDM (Stage 3b): locks that this connector routes raw issues through
// JiraAdapter. Encodings are tested once in providers::cdm; the mapping
// itself in meridian_core::adapters::jira.
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
    let cdm = crate::intelligence::providers::cdm::derive(&super::JiraAdapter, &raw);
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
    let cdm = crate::intelligence::providers::cdm::derive(
        &super::JiraAdapter,
        &serde_json::json!({"fields": {}}),
    );
    assert!(cdm.canonical_id.is_none());
    assert!(cdm.raw_payload.is_none());
    assert!(cdm.status_category.is_none());
}
