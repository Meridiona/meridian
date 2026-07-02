//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use anyhow::{Context, Result};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;

use crate::config::LinearConfig;

use super::*;

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

async fn make_pool() -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .context("parse in-memory sqlite DSN")?
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts)
        .await
        .context("open in-memory sqlite pool")?;
    sqlx::migrate!("src/migrations")
        .run(&pool)
        .await
        .context("run linear test migrations")?;
    Ok(pool)
}

fn make_issue(identifier: &str, state_type: &str) -> LinearIssue {
    LinearIssue {
        identifier: identifier.into(),
        title: format!("Issue {identifier}"),
        description: None,
        updated_at: "2026-06-01T00:00:00.000Z".into(),
        url: Some(format!("https://linear.app/t/issue/{identifier}")),
        state: Some(WorkflowState {
            name: state_type.into(),
            type_: state_type.into(),
        }),
        team: Some(Team {
            id: "t1".into(),
            key: "ENG".into(),
        }),
        parent: None,
        assignee: None,
        due_date: None,
        started_at: None,
        labels: None,
        cycle: None,
        project: None,
    }
}

fn linear_cfg(team_ids: &[&str]) -> LinearConfig {
    LinearConfig {
        api_key: "lin_api_test".into(),
        team_ids: team_ids.iter().map(|s| s.to_string()).collect(),
    }
}

/// Pairs a hand-built [`LinearIssue`] with a minimal raw payload for `upsert`'s
/// new `(LinearIssue, Value)` shape. These non-CDM-focused tests only need the
/// tuple shape to satisfy `upsert`'s signature — `cdm_columns` degrades to all-
/// `None` on a payload this sparse, which doesn't affect the non-CDM columns
/// under test. Dedicated CDM tests below use a fuller payload.
fn with_raw(issue: LinearIssue) -> (LinearIssue, serde_json::Value) {
    let raw = serde_json::json!({ "id": issue.identifier });
    (issue, raw)
}

// ---------------------------------------------------------------------------
// Unit tests (pure logic — no DB)
// ---------------------------------------------------------------------------

#[test]
fn state_terminality() {
    // completed / canceled are terminal; everything else is open.
    assert!(native_terminal("completed"));
    assert!(native_terminal("canceled"));
    assert!(!native_terminal("started"));
    assert!(!native_terminal("backlog"));
    assert!(!native_terminal("triage"));
}

fn issue_with(team_id: &str, team_key: &str, state: &str) -> LinearIssue {
    LinearIssue {
        identifier: "ENG-1".into(),
        title: "t".into(),
        description: None,
        updated_at: "2026-06-01T00:00:00.000Z".into(),
        url: None,
        state: Some(WorkflowState {
            name: state.into(),
            type_: state.into(),
        }),
        team: Some(Team {
            id: team_id.into(),
            key: team_key.into(),
        }),
        parent: None,
        assignee: None,
        due_date: None,
        started_at: None,
        labels: None,
        cycle: None,
        project: None,
    }
}

#[test]
fn finished_issues_detected() {
    assert!(is_finished(&issue_with("u", "ENG", "completed")));
    assert!(is_finished(&issue_with("u", "ENG", "canceled")));
    assert!(!is_finished(&issue_with("u", "ENG", "started")));
}

#[test]
fn missing_state_is_not_finished() {
    let mut i = issue_with("u", "ENG", "started");
    i.state = None;
    assert!(
        !is_finished(&i),
        "None state should not be treated as finished"
    );
}

#[test]
fn team_filter_matches_id_or_key() {
    let i = issue_with("uuid-123", "ENG", "started");
    assert!(team_allowed(&i, &[])); // empty = all
    assert!(team_allowed(&i, &["uuid-123".into()])); // by id
    assert!(team_allowed(&i, &["ENG".into()])); // by key
    assert!(!team_allowed(&i, &["OTHER".into()]));
}

#[test]
fn team_filter_no_team_excluded_when_filter_set() {
    let mut i = issue_with("uuid-123", "ENG", "started");
    i.team = None;
    // A filter is set but the issue has no team → rejected.
    assert!(!team_allowed(&i, &["ENG".into()]));
    // No filter → team-less issues pass through.
    assert!(team_allowed(&i, &[]));
}

#[test]
fn parses_assigned_issues_response() {
    let raw = r#"{"data":{"viewer":{"assignedIssues":{"nodes":[
        {"identifier":"ENG-12","title":"Fix bug","description":"d","updatedAt":"2026-06-01T00:00:00.000Z",
         "url":"https://linear.app/x/issue/ENG-12","state":{"type":"started"},
         "team":{"id":"t1","key":"ENG"},"project":{"name":"P"},
         "parent":{"identifier":"ENG-1","title":"Epic"},"assignee":{"name":"Sam"}}
    ]}}}}"#;
    let parsed: GqlResponse = serde_json::from_str(raw).unwrap();
    let nodes = parsed.data.unwrap().viewer.unwrap().assigned_issues.nodes;
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].identifier, "ENG-12");
    assert_eq!(nodes[0].state.as_ref().unwrap().type_, "started");
    assert_eq!(nodes[0].parent.as_ref().unwrap().identifier, "ENG-1");
}

#[test]
fn parses_labels_and_cycle() {
    let raw = r#"{"data":{"viewer":{"assignedIssues":{"nodes":[
        {"identifier":"ENG-5","title":"T","updatedAt":"2026-06-01T00:00:00.000Z",
         "state":{"name":"In Progress","type":"started"},
         "team":{"id":"t1","key":"ENG"},
         "labels":{"nodes":[{"name":"bug"},{"name":"P1"}]},
         "cycle":{"name":"Sprint 7"}}
    ]}}}}"#;
    let parsed: GqlResponse = serde_json::from_str(raw).unwrap();
    let node = &parsed.data.unwrap().viewer.unwrap().assigned_issues.nodes[0];
    let label_names: Vec<&str> = node
        .labels
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(label_names, ["bug", "P1"]);
    assert_eq!(
        node.cycle.as_ref().unwrap().name.as_deref(),
        Some("Sprint 7")
    );
}

// ---------------------------------------------------------------------------
// DB integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upsert_writes_basic_row() -> Result<()> {
    let pool = make_pool().await?;
    let issue = make_issue("ENG-1", "started");
    let kept = upsert(&pool, &[with_raw(issue)], &linear_cfg(&[]))
        .await
        .context("upsert linear task")?;
    assert_eq!(kept, ["ENG-1"]);
    let (title, provider): (String, String) =
        sqlx::query_as("SELECT title, provider FROM pm_tasks WHERE task_key = 'ENG-1'")
            .fetch_one(&pool)
            .await
            .context("fetch persisted linear task")?;
    assert_eq!(title, "Issue ENG-1");
    assert_eq!(provider, "linear");
    Ok(())
}

#[tokio::test]
async fn upsert_done_canceled_excluded() -> Result<()> {
    // Regression: completed/canceled issues must NOT land in pm_tasks.
    let pool = make_pool().await?;
    let done = make_issue("ENG-2", "completed");
    let canceled = make_issue("ENG-3", "canceled");
    let kept = upsert(
        &pool,
        &[with_raw(done), with_raw(canceled)],
        &linear_cfg(&[]),
    )
    .await
    .context("upsert done/canceled")?;
    assert!(kept.is_empty(), "completed/canceled should be excluded");
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pm_tasks WHERE provider = 'linear'")
            .fetch_one(&pool)
            .await
            .context("count linear tasks after exclusion")?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn upsert_skips_team_filtered_issues() -> Result<()> {
    let pool = make_pool().await?;
    let mut outside = make_issue("ENG-4", "started");
    outside.team = Some(Team {
        id: "other-uuid".into(),
        key: "OTHER".into(),
    });
    let kept = upsert(&pool, &[with_raw(outside)], &linear_cfg(&["ALLOWED"]))
        .await
        .context("upsert team-filtered issue")?;
    assert!(kept.is_empty());
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pm_tasks WHERE provider = 'linear'")
            .fetch_one(&pool)
            .await
            .context("count linear tasks after team filter")?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn upsert_maps_parent_to_epic() -> Result<()> {
    // Regression: parent.identifier → parent_key, parent.title → epic_title.
    let pool = make_pool().await?;
    let mut issue = make_issue("ENG-5", "started");
    issue.parent = Some(Parent {
        identifier: "ENG-1".into(),
        title: Some("Big Epic".into()),
    });
    upsert(&pool, &[with_raw(issue)], &linear_cfg(&[]))
        .await
        .context("upsert issue with parent")?;
    let (parent_key, epic_title): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT parent_key, epic_title FROM pm_tasks WHERE task_key = 'ENG-5'")
            .fetch_one(&pool)
            .await
            .context("fetch parent_key/epic_title")?;
    assert_eq!(parent_key.as_deref(), Some("ENG-1"));
    assert_eq!(epic_title.as_deref(), Some("Big Epic"));
    Ok(())
}

#[tokio::test]
async fn upsert_uses_project_as_epic_when_no_parent() -> Result<()> {
    // Regression: Linear tasks without a parent issue must group by project name
    // (stored as "project:<name>" in parent_key) so the UI shows named groups
    // instead of collapsing everything into "No epic".
    let pool = make_pool().await?;
    let mut issue = make_issue("ENG-9", "started");
    issue.project = Some(Project {
        name: Some("Meridian Core".into()),
    });
    upsert(&pool, &[with_raw(issue)], &linear_cfg(&[]))
        .await
        .context("upsert issue with project")?;
    let (parent_key, epic_title): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT parent_key, epic_title FROM pm_tasks WHERE task_key = 'ENG-9'")
            .fetch_one(&pool)
            .await
            .context("fetch parent_key/epic_title")?;
    assert_eq!(parent_key.as_deref(), Some("project:Meridian Core"));
    assert_eq!(epic_title.as_deref(), Some("Meridian Core"));
    Ok(())
}

#[tokio::test]
async fn upsert_joins_labels_as_tags() -> Result<()> {
    let pool = make_pool().await?;
    let mut issue = make_issue("ENG-6", "started");
    issue.labels = Some(LabelConnection {
        nodes: vec![
            LabelNode { name: "bug".into() },
            LabelNode { name: "P1".into() },
        ],
    });
    upsert(&pool, &[with_raw(issue)], &linear_cfg(&[]))
        .await
        .context("upsert issue with labels")?;
    let (tags,): (Option<String>,) =
        sqlx::query_as("SELECT tags FROM pm_tasks WHERE task_key = 'ENG-6'")
            .fetch_one(&pool)
            .await
            .context("fetch tags")?;
    assert_eq!(tags.as_deref(), Some("bug, P1"));
    Ok(())
}

#[tokio::test]
async fn upsert_stores_sprint_name() -> Result<()> {
    let pool = make_pool().await?;
    let mut issue = make_issue("ENG-7", "started");
    issue.cycle = Some(Cycle {
        name: Some("Sprint 7".into()),
    });
    upsert(&pool, &[with_raw(issue)], &linear_cfg(&[]))
        .await
        .context("upsert issue with cycle")?;
    let (sprint_name,): (Option<String>,) =
        sqlx::query_as("SELECT sprint_name FROM pm_tasks WHERE task_key = 'ENG-7'")
            .fetch_one(&pool)
            .await
            .context("fetch sprint_name")?;
    assert_eq!(sprint_name.as_deref(), Some("Sprint 7"));
    Ok(())
}

#[tokio::test]
async fn upsert_idempotent_on_conflict() -> Result<()> {
    let pool = make_pool().await?;
    let cfg = linear_cfg(&[]);
    upsert(&pool, &[with_raw(make_issue("ENG-8", "started"))], &cfg)
        .await
        .context("first upsert")?;
    let mut updated = make_issue("ENG-8", "started");
    updated.title = "Updated title".into();
    upsert(&pool, &[with_raw(updated)], &cfg)
        .await
        .context("second upsert")?;
    let (count, title): (i64, String) =
        sqlx::query_as("SELECT COUNT(*), title FROM pm_tasks WHERE task_key = 'ENG-8'")
            .fetch_one(&pool)
            .await
            .context("fetch conflict row")?;
    assert_eq!(count, 1, "ON CONFLICT must not duplicate the row");
    assert_eq!(title, "Updated title");
    Ok(())
}

#[tokio::test]
async fn prune_removes_stale_linear_tasks() -> Result<()> {
    let pool = make_pool().await?;
    let cfg = linear_cfg(&[]);
    upsert(&pool, &[with_raw(make_issue("ENG-10", "started"))], &cfg)
        .await
        .context("upsert ENG-10")?;
    upsert(&pool, &[with_raw(make_issue("ENG-11", "started"))], &cfg)
        .await
        .context("upsert ENG-11")?;
    // ENG-11 is still live; ENG-10 is stale.
    let pruned = prune(&pool, &["ENG-11".to_owned()])
        .await
        .context("prune stale tasks")?;
    assert_eq!(pruned, 1);
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT task_key FROM pm_tasks WHERE provider = 'linear'")
            .fetch_all(&pool)
            .await
            .context("fetch remaining keys")?;
    assert_eq!(keys, ["ENG-11"]);
    Ok(())
}

#[tokio::test]
async fn prune_leaves_other_providers_intact() -> Result<()> {
    let pool = make_pool().await?;
    // Seed a Jira task directly — prune('linear') must not touch it.
    sqlx::query(
        "INSERT INTO pm_tasks (task_key, provider, title, description_text, issue_type,
         project_key, url, updated_at)
         VALUES ('JRA-1', 'jira', 'Jira task', '', '', '', '', '2026-06-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .context("seed jira task")?;
    // Also add a linear task, then prune it away.
    upsert(
        &pool,
        &[with_raw(make_issue("ENG-12", "started"))],
        &linear_cfg(&[]),
    )
    .await
    .context("upsert ENG-12")?;
    // 'ENG-GONE' is not in the DB so ENG-12 gets pruned; JRA-1 must survive.
    prune(&pool, &["ENG-GONE".to_owned()])
        .await
        .context("prune linear tasks")?;
    let providers: Vec<String> =
        sqlx::query_scalar("SELECT provider FROM pm_tasks ORDER BY provider")
            .fetch_all(&pool)
            .await
            .context("fetch surviving providers")?;
    assert_eq!(providers, ["jira"]);
    Ok(())
}

// -----------------------------------------------------------------------
// CDM (Stage 3b): the new pm_tasks columns are derived from the raw issue
// through the shared adapter. This locks the daemon-side glue; the mapping
// itself is tested in meridian_core::adapters::linear.
// -----------------------------------------------------------------------

#[test]
fn cdm_columns_derives_from_raw_issue() {
    let raw = serde_json::json!({
        "id": "11111111-2222-3333-4444-555555555555",
        "identifier": "ENG-42",
        "state": {"name": "In Review", "type": "started"},
        "creator": {"id": "usr-2", "displayName": "Lead"},
        "parent": {"id": "uuid-parent"},
        "project": {"id": "proj-uuid"},
        "completedAt": null,
        "canceledAt": null
    });
    let cdm = super::cdm_columns(&raw);
    // Stable key is the UUID, namespaced.
    assert_eq!(
        cdm.canonical_id.as_deref(),
        Some("linear:11111111-2222-3333-4444-555555555555")
    );
    // "In Review" (state.type=started) → snake_case canonical category.
    assert_eq!(cdm.status_category.as_deref(), Some("in_progress"));
    assert_eq!(cdm.reporter_name.as_deref(), Some("Lead"));
    assert_eq!(cdm.completed_at, None); // completedAt/canceledAt both null
    assert_eq!(
        cdm.ancestor_path.as_deref(),
        Some(r#"["linear:uuid-parent"]"#)
    );
    assert_eq!(cdm.project_ids.as_deref(), Some(r#"["linear:proj-uuid"]"#));
    assert!(cdm.raw_payload.is_some());
}

#[test]
fn cdm_columns_empty_on_unusable_payload() {
    // No `id` → adapter errors → all columns NULL, never blocks the upsert.
    let cdm = super::cdm_columns(&serde_json::json!({"identifier": "ENG-1"}));
    assert!(cdm.canonical_id.is_none());
    assert!(cdm.raw_payload.is_none());
    assert!(cdm.status_category.is_none());
}
