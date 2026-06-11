//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Runtime coverage for the multi-provider worklog write path: that
// `upsert_pm_worklog` snapshots the provider from `pm_tasks` onto the worklog
// row, defaults to 'jira' when the task is absent, and that
// `fetch_approved_worklogs` surfaces the provider for the approved-poster to
// route on. These exercise the real SQL (bind counts + the COALESCE sub-select),
// which the in-module unit tests do not.

use meridian::pm_worklog::db::{fetch_approved_worklogs, upsert_pm_worklog};
use meridian::pm_worklog::models::{GroundedNarrative, JiraUpdate, UpdateState};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

async fn make_db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
    pool
}

async fn insert_task(pool: &SqlitePool, task_key: &str, provider: &str) {
    sqlx::query(
        "INSERT INTO pm_tasks
           (task_key, provider, title, description_text, status_category,
            issue_type, project_key, url, updated_at, fetched_at)
         VALUES (?, ?, 'T', '', 'in_progress', 'Issue', 'acme/api', '',
                 strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                 strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .bind(task_key)
    .bind(provider)
    .execute(pool)
    .await
    .unwrap();
}

fn narrative(task_key: &str) -> GroundedNarrative {
    GroundedNarrative {
        update: JiraUpdate {
            task_key: task_key.to_string(),
            window_start: "2026-06-01T09:00:00+00:00".to_string(),
            window_end: "2026-06-01T10:00:00+00:00".to_string(),
            cycle_index: 9,
            time_spent_seconds: 1800,
            summary: "Did work".to_string(),
            what_shipped: vec![],
            in_progress: vec![],
            blockers: vec![],
            decisions: vec![],
            next_steps: vec![],
            risk_flags: vec![],
            confidence: 0.9,
            reasoning: String::new(),
        },
        coverage: 1.0,
        dropped_bullets: vec![],
    }
}

async fn provider_of(pool: &SqlitePool, id: i64) -> String {
    let row = sqlx::query("SELECT provider FROM pm_worklogs WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    row.get::<String, _>("provider")
}

#[tokio::test]
async fn snapshots_provider_from_pm_tasks() {
    let pool = make_db().await;
    insert_task(&pool, "acme/api#1", "github").await;

    let id = upsert_pm_worklog(
        &pool,
        &narrative("acme/api#1"),
        UpdateState::Drafted,
        "2026-06-01",
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(provider_of(&pool, id).await, "github");
}

#[tokio::test]
async fn defaults_to_jira_when_task_absent() {
    let pool = make_db().await;
    // No pm_tasks row for this key — the COALESCE must fall back to 'jira'.
    let id = upsert_pm_worklog(
        &pool,
        &narrative("KAN-1"),
        UpdateState::Drafted,
        "2026-06-01",
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(provider_of(&pool, id).await, "jira");
}

#[tokio::test]
async fn snapshots_trello_provider() {
    let pool = make_db().await;
    insert_task(&pool, "HSkL1pnj", "trello").await;

    let id = upsert_pm_worklog(
        &pool,
        &narrative("HSkL1pnj"),
        UpdateState::Drafted,
        "2026-06-01",
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(provider_of(&pool, id).await, "trello");
}

#[tokio::test]
async fn approved_worklogs_carry_provider_for_routing() {
    let pool = make_db().await;
    insert_task(&pool, "ENG-7", "linear").await;

    // Insert straight as approved (what the dashboard's approve action produces).
    upsert_pm_worklog(
        &pool,
        &narrative("ENG-7"),
        UpdateState::Approved,
        "2026-06-01",
        None,
        None,
    )
    .await
    .unwrap();

    let approved = fetch_approved_worklogs(&pool).await.unwrap();
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0].provider, "linear");
    assert_eq!(approved[0].task_key, "ENG-7");
    assert_eq!(approved[0].comment, "Did work");
    assert_eq!(approved[0].time_spent_seconds, 1800);
}
