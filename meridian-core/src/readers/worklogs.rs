//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/worklogs?day=YYYY-MM-DD` ported to Rust — a faithful port of
//! `ui/app/api/worklogs/route.ts`.
//!
//! # What this is
//! A day's drafted/approved/posted worklogs for review: the editable Jira
//! comment (payload `summary`), supporting bullets/next-steps for context,
//! confidence/coverage, risk flags, and post status. The review *writes*
//! ([`edit_worklog`], [`worklog_action`]) are faithful ports of `worklogs/[id]`
//! PATCH/POST: they record the human-in-the-loop decision in the DB (the daemon's
//! ~60s approved-sweep is what actually posts to Jira) and append an immutable
//! `pm_worklog_feedback` row per action (the eval signal). A `posted` worklog is
//! immutable here.
//!
//! # Who calls this
//! The tray `get_worklogs` / `edit_worklog` / `worklog_action` commands → the
//! dashboard `WorklogsView` (read list + the approve/reject/edit actions).
//!
//! # Related
//! - [`crate::tasks`] joins the same `pm_tasks` for per-ticket time.
//! - Bullets/next-steps are parsed out of the row's `payload_json` blob below.

use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;
use std::collections::BTreeMap;
use tracing::Instrument;

/// One supporting bullet on a worklog, tagged by kind (shipped / in progress /
/// blocker / decision).
#[derive(Debug, Clone, Serialize)]
pub struct WorklogBullet {
    pub kind: String,
    pub text: String,
}

/// One reviewable worklog: the editable comment (`summary`) + context (bullets,
/// next steps, risk flags, reasoning) + post state. `edited` reflects a manual
/// edit on the row.
#[derive(Debug, Clone, Serialize)]
pub struct WorklogItem {
    pub id: i64,
    pub task_key: String,
    pub task_title: Option<String>,
    pub task_url: Option<String>,
    pub provider: String,
    pub window_start: String,
    pub window_end: Option<String>,
    pub state: String,
    pub confidence: f64,
    pub coverage: f64,
    pub time_spent_seconds: i64,
    pub summary: String,
    pub bullets: Vec<WorklogBullet>,
    pub next_steps: Vec<String>,
    pub risk_flags: Vec<String>,
    pub reasoning: String,
    pub posted_worklog_id: Option<String>,
    pub last_post_error: Option<String>,
    pub edited: bool,
    /// True when this entry is a tier-3 PROPOSED new ticket (from
    /// `pm_proposed_tasks`), not a real worklog. The UI renders it inline in the
    /// day timeline with an editable title + worklog body and Approve/Dismiss
    /// actions that route to the `*_proposed` tray commands (vs the worklog
    /// edit/approve commands). `false` for ordinary worklogs.
    #[serde(default)]
    pub is_proposed: bool,
    /// `pm_proposed_tasks.id` when `is_proposed` — the key the proposed-ticket
    /// edit/approve/dismiss commands take. `None` for ordinary worklogs.
    #[serde(default)]
    pub proposed_id: Option<i64>,
    /// The ticket's issue type (`Task` / `Bug` / `Story`, etc). For a real
    /// worklog, pulled from the joined `pm_tasks.issue_type` (empty if the task
    /// row is missing or its type was never fetched from the tracker). For a
    /// proposed ticket (`is_proposed`), the drafted type (migration 051) —
    /// surfaced as a chip on the proposal card and used when the ticket is
    /// created. The dashboard falls back to a generic label only when this is
    /// empty, never hardcodes "Work log".
    #[serde(default)]
    pub issue_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorklogsResponse {
    pub day: String,
    pub items: Vec<WorklogItem>,
    /// state → count. Key order differs from the route's insertion order
    /// (sorted here); consumers read by key, so this is functionally identical.
    pub counts: BTreeMap<String, i64>,
}

#[derive(FromRow)]
struct RawRow {
    id: i64,
    task_key: String,
    task_title: Option<String>,
    task_url: Option<String>,
    provider: String,
    window_start: String,
    window_end: Option<String>,
    state: String,
    confidence: Option<f64>,
    coverage: Option<f64>,
    time_spent_seconds: Option<i64>,
    payload_json: Option<String>,
    posted_worklog_id: Option<String>,
    last_post_error: Option<String>,
    edited_at: Option<String>,
    issue_type: String,
}

/// payload_json bullet groups → display kind (order matches the route).
const BULLET_GROUPS: [(&str, &str); 4] = [
    ("what_shipped", "shipped"),
    ("in_progress", "in progress"),
    ("blockers", "blocker"),
    ("decisions", "decision"),
];

/// Pull `[{text}]` style bullets out of a payload array field.
fn bullets_from(payload: &Value, field: &str, kind: &str, out: &mut Vec<WorklogBullet>) {
    if let Some(arr) = payload.get(field).and_then(|v| v.as_array()) {
        for b in arr {
            if let Some(text) = b.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    out.push(WorklogBullet {
                        kind: kind.to_string(),
                        text: text.to_string(),
                    });
                }
            }
        }
    }
}

/// A `["a","b"]` string array from a payload field, else empty.
fn str_array(payload: &Value, field: &str) -> Vec<String> {
    payload
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tracing::instrument(skip(pool))]
pub async fn get_worklogs(pool: &SqlitePool, day: &str) -> anyhow::Result<WorklogsResponse> {
    let rows: Vec<RawRow> = sqlx::query_as::<_, RawRow>(
        r#"
        SELECT w.id, w.task_key, t.title AS task_title, t.url AS task_url,
               COALESCE(w.provider, 'jira') AS provider, w.window_start, w.window_end,
               w.state, w.confidence, w.coverage,
               w.time_spent_seconds, w.payload_json, w.posted_worklog_id,
               w.last_post_error, w.edited_at, COALESCE(t.issue_type, '') AS issue_type
        FROM pm_worklogs w
        LEFT JOIN pm_tasks t ON t.task_key = w.task_key
        WHERE w.day_utc = ?
        ORDER BY w.window_start, w.task_key
        "#,
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("worklogs.read.pm_worklogs"))
    .await
    .context("worklogs: fetch pm_worklogs")?;
    tracing::debug!(rows = rows.len(), "worklogs.read.pm_worklogs");

    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut items: Vec<WorklogItem> = Vec::with_capacity(rows.len());

    for r in rows {
        *counts.entry(r.state.clone()).or_insert(0) += 1;

        let payload: Value = r
            .payload_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);

        let mut bullets = Vec::new();
        for (field, kind) in BULLET_GROUPS {
            bullets_from(&payload, field, kind, &mut bullets);
        }

        // The route keeps task_url only when it's an https URL.
        let task_url = r.task_url.filter(|u| u.starts_with("https://"));

        items.push(WorklogItem {
            id: r.id,
            task_key: r.task_key,
            task_title: r.task_title,
            task_url,
            provider: r.provider,
            window_start: r.window_start,
            window_end: r.window_end,
            state: r.state,
            confidence: r.confidence.unwrap_or(0.0),
            coverage: r.coverage.unwrap_or(0.0),
            time_spent_seconds: r.time_spent_seconds.unwrap_or(0),
            summary: payload
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            bullets,
            next_steps: str_array(&payload, "next_steps"),
            risk_flags: str_array(&payload, "risk_flags"),
            reasoning: payload
                .get("reasoning")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            posted_worklog_id: r.posted_worklog_id,
            last_post_error: r.last_post_error,
            edited: r.edited_at.is_some(),
            is_proposed: false,
            proposed_id: None,
            issue_type: r.issue_type,
        });
    }

    // Tier-3 proposed new tickets for the day, rendered INLINE in the same
    // timeline (a continuation of the real worklogs). Each carries its drafted
    // worklog in `worklog_payload_json` (migration 050) so the approval surface
    // shows an editable title + worklog body + the reasoning together. Only
    // still-`proposed` rows surface; approved/dismissed ones drop out.
    append_proposed_items(pool, day, &mut counts, &mut items).await?;

    // Order the merged list by window so proposals slot in by their source hour
    // (e.g. 19:00 worklog → 20:00 proposal), then by task_key for stability.
    items.sort_by(|a, b| {
        a.window_start
            .cmp(&b.window_start)
            .then_with(|| a.task_key.cmp(&b.task_key))
    });

    tracing::info!(day, items = items.len(), "worklogs computed");
    Ok(WorklogsResponse {
        day: day.to_string(),
        items,
        counts,
    })
}

/// Raw `pm_proposed_tasks` row for the day's not-yet-created proposals
/// (`proposed` or `approved` awaiting ticket creation).
#[derive(FromRow)]
struct RawProposedRow {
    id: i64,
    source_hour: String,
    title: String,
    reasoning: String,
    issue_type: String,
    state: String,
    confidence: f64,
    time_spent_seconds: i64,
    window_start: Option<String>,
    window_end: Option<String>,
    worklog_payload_json: Option<String>,
}

/// Read `pm_proposed_tasks` for `day` and push each not-yet-created proposal
/// (`state IN ('proposed', 'approved')`, no real ticket minted yet) as a
/// `WorklogItem` with `is_proposed = true`. An approved proposal stays visible
/// here — carrying its real `approved` state — until the daemon's proposal
/// sweep creates the ticket and inserts the real `pm_worklogs` row (at which
/// point `created_task_key` is set and this row drops out, so it isn't shown
/// twice); without the `approved` branch an approved-but-not-yet-swept
/// proposal would vanish from the timeline for the length of the sweep gap.
/// Degrades gracefully if the table or the migration-050 columns are absent
/// (older DB) — returns without erroring.
#[tracing::instrument(skip(pool, counts, items))]
async fn append_proposed_items(
    pool: &SqlitePool,
    day: &str,
    counts: &mut BTreeMap<String, i64>,
    items: &mut Vec<WorklogItem>,
) -> anyhow::Result<()> {
    let rows = sqlx::query_as::<_, RawProposedRow>(
        r#"
        SELECT id, source_hour, title, reasoning, issue_type, state, confidence,
               time_spent_seconds, window_start, window_end, worklog_payload_json
        FROM pm_proposed_tasks
        WHERE day_utc = ?
          AND state IN ('proposed', 'approved')
          AND (created_task_key IS NULL OR created_task_key = '')
        ORDER BY source_hour
        "#,
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("worklogs.read.pm_proposed_tasks"))
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            // Missing table/columns on an un-migrated DB is not fatal — the day
            // simply has no proposals to show.
            tracing::warn!(error = %e, "worklogs: pm_proposed_tasks read skipped");
            return Ok(());
        }
    };
    tracing::debug!(rows = rows.len(), "worklogs.read.pm_proposed_tasks");

    for r in rows {
        // Bucket by the row's real state (proposed/approved) so an approved
        // proposal counts alongside real approved worklogs, not under a
        // separate always-"proposed" key.
        *counts.entry(r.state.clone()).or_insert(0) += 1;

        let payload: Value = r
            .worklog_payload_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);

        let mut bullets = Vec::new();
        for (field, kind) in BULLET_GROUPS {
            bullets_from(&payload, field, kind, &mut bullets);
        }

        // Fall back to the source hour when window_start wasn't stored (rows
        // proposed before migration 050) so ordering still works.
        let window_start = r
            .window_start
            .unwrap_or_else(|| format!("{}:00:00+00:00", r.source_hour));

        items.push(WorklogItem {
            id: r.id,
            task_key: String::new(),
            // The proposed (editable) ticket title shows where the task title goes.
            task_title: Some(r.title),
            task_url: None,
            provider: String::new(),
            window_start,
            window_end: r.window_end,
            state: r.state,
            confidence: r.confidence,
            coverage: 0.0,
            time_spent_seconds: r.time_spent_seconds,
            summary: payload
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            bullets,
            next_steps: str_array(&payload, "next_steps"),
            risk_flags: str_array(&payload, "risk_flags"),
            reasoning: r.reasoning,
            posted_worklog_id: None,
            last_post_error: None,
            edited: false,
            is_proposed: true,
            proposed_id: Some(r.id),
            issue_type: r.issue_type,
        });
    }
    Ok(())
}

// ── Writes (the worklogs/[id] PATCH + POST) ───────────────────────────────────

/// A worklog review action (the route's POST `action` allow-set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorklogAction {
    Approve,
    Reject,
    Unapprove,
}

impl WorklogAction {
    /// The stored `feedback_kind` string (also the route's action value).
    pub fn as_str(&self) -> &'static str {
        match self {
            WorklogAction::Approve => "approve",
            WorklogAction::Reject => "reject",
            WorklogAction::Unapprove => "unapprove",
        }
    }

    /// Parse a request string; `None` outside approve/reject/unapprove.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approve" => Some(WorklogAction::Approve),
            "reject" => Some(WorklogAction::Reject),
            "unapprove" => Some(WorklogAction::Unapprove),
            _ => None,
        }
    }

    /// The `state` this action transitions the worklog into.
    fn next_state(&self) -> &'static str {
        match self {
            WorklogAction::Approve => "approved",
            WorklogAction::Reject => "skipped",
            WorklogAction::Unapprove => "drafted",
        }
    }
}

/// A worklog write rejected by a business rule (the route's 404/409).
#[derive(Debug)]
pub enum WorklogWriteError {
    /// No `pm_worklogs` row with that id (the route's 404).
    NotFound(i64),
    /// The worklog is already `posted` to Jira → immutable (the route's 409).
    AlreadyPosted(i64),
}

impl std::fmt::Display for WorklogWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "worklog {id} not found"),
            Self::AlreadyPosted(id) => write!(f, "worklog {id} already posted to Jira"),
        }
    }
}
impl std::error::Error for WorklogWriteError {}

#[derive(FromRow)]
struct StateRow {
    state: String,
    payload_json: String,
}

/// Edit a worklog's Jira comment (port of `worklogs/[id]` PATCH). Sets the
/// payload `summary`, pulls an `approved` row back to `drafted` (content changed
/// → must be re-approved), clears any post error, and records an `edit` feedback
/// row (old → new). Returns the resulting state. Errors [`WorklogWriteError`] for
/// a missing (404) or already-posted (409) worklog.
#[tracing::instrument(skip(pool, summary))]
pub async fn edit_worklog(
    pool: &SqlitePool,
    id: i64,
    summary: &str,
    now: &str,
) -> anyhow::Result<String> {
    let row: Option<StateRow> =
        sqlx::query_as::<_, StateRow>("SELECT state, payload_json FROM pm_worklogs WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .instrument(tracing::debug_span!("worklogs.read.state"))
            .await?;
    let Some(row) = row else {
        return Err(WorklogWriteError::NotFound(id).into());
    };
    if row.state == "posted" {
        return Err(WorklogWriteError::AlreadyPosted(id).into());
    }

    // Capture the pre-edit summary for the feedback row (parse failure → "").
    let original = serde_json::from_str::<Value>(&row.payload_json)
        .ok()
        .and_then(|p| p.get("summary").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default();
    // Editing an approved row pulls it back to drafted (re-approval required).
    let next_state = if row.state == "approved" {
        "drafted"
    } else {
        row.state.as_str()
    };

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE pm_worklogs \
         SET payload_json = json_set(payload_json, '$.summary', ?), \
             state = ?, edited_at = ?, last_post_error = NULL \
         WHERE id = ?",
    )
    .bind(summary)
    .bind(next_state)
    .bind(now)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO pm_worklog_feedback (pm_worklog_id, feedback_kind, original_text, edited_text) \
         VALUES (?, 'edit', ?, ?)",
    )
    .bind(id)
    .bind(&original)
    .bind(summary)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    tracing::info!(id, state = next_state, "worklog edited");
    Ok(next_state.to_string())
}

/// Transition a worklog's review state (port of `worklogs/[id]` POST). Applies
/// the state change and appends an immutable feedback row carrying the
/// `state→next` transition (and, for `reject` only, the attribution correction:
/// where the time should have gone). Returns the resulting state. Errors
/// [`WorklogWriteError`] for a missing (404) or already-posted (409) worklog.
///
/// `corrected_task_key` / `corrected_to_untracked` are the reviewer's optional
/// reject-time attribution; the caller has already gated them to `reject`
/// (matching the route, which ignores them for other actions).
#[tracing::instrument(skip(pool), fields(action = action.as_str()))]
pub async fn worklog_action(
    pool: &SqlitePool,
    id: i64,
    action: WorklogAction,
    corrected_task_key: Option<&str>,
    corrected_to_untracked: bool,
    now: &str,
) -> anyhow::Result<String> {
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM pm_worklogs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .instrument(tracing::debug_span!("worklogs.read.state"))
        .await?;
    let Some(state) = state else {
        return Err(WorklogWriteError::NotFound(id).into());
    };
    if state == "posted" {
        return Err(WorklogWriteError::AlreadyPosted(id).into());
    }

    let next = action.next_state();
    let transition = format!("{state}→{next}");

    let mut tx = pool.begin().await?;
    match action {
        WorklogAction::Approve => {
            sqlx::query(
                "UPDATE pm_worklogs SET state = 'approved', approved_at = ?, last_post_error = NULL WHERE id = ?",
            )
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        WorklogAction::Reject => {
            sqlx::query("UPDATE pm_worklogs SET state = 'skipped' WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        WorklogAction::Unapprove => {
            sqlx::query(
                "UPDATE pm_worklogs SET state = 'drafted', approved_at = NULL WHERE id = ?",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "INSERT INTO pm_worklog_feedback \
            (pm_worklog_id, feedback_kind, note, corrected_task_key, corrected_to_untracked) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(action.as_str())
    .bind(&transition)
    .bind(corrected_task_key)
    .bind(corrected_to_untracked as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    tracing::info!(
        id,
        action = action.as_str(),
        state = next,
        "worklog action recorded"
    );
    Ok(next.to_string())
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_worklogs() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE pm_worklogs (id INTEGER PRIMARY KEY, state TEXT, payload_json TEXT, \
                approved_at TEXT, edited_at TEXT, last_post_error TEXT)",
            "CREATE TABLE pm_worklog_feedback (id INTEGER PRIMARY KEY AUTOINCREMENT, \
                pm_worklog_id INTEGER, feedback_kind TEXT, original_text TEXT, edited_text TEXT, \
                note TEXT, corrected_task_key TEXT, corrected_to_untracked INTEGER)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn seed(pool: &SqlitePool, id: i64, state: &str, summary: &str) {
        sqlx::query("INSERT INTO pm_worklogs (id, state, payload_json) VALUES (?, ?, ?)")
            .bind(id)
            .bind(state)
            .bind(format!(r#"{{"summary":"{summary}"}}"#))
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn edit_pulls_approved_back_to_drafted_and_logs_feedback() {
        let pool = pool_with_worklogs().await;
        seed(&pool, 1, "approved", "old text").await;

        let next = edit_worklog(&pool, 1, "new text", "2026-06-18T10:00:00Z")
            .await
            .unwrap();
        assert_eq!(next, "drafted", "editing an approved row re-drafts it");

        let (state, payload): (String, String) =
            sqlx::query_as("SELECT state, payload_json FROM pm_worklogs WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "drafted");
        assert!(payload.contains("new text"));
        // An immutable edit-feedback row captured old → new.
        let (kind, orig, edited): (String, String, String) = sqlx::query_as(
            "SELECT feedback_kind, original_text, edited_text FROM pm_worklog_feedback WHERE pm_worklog_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (kind.as_str(), orig.as_str(), edited.as_str()),
            ("edit", "old text", "new text")
        );
    }

    #[tokio::test]
    async fn posted_worklog_is_immutable() {
        let pool = pool_with_worklogs().await;
        seed(&pool, 1, "posted", "shipped").await;
        let e = edit_worklog(&pool, 1, "x", "2026-06-18T10:00:00Z")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("already posted"));
        let e = worklog_action(
            &pool,
            1,
            WorklogAction::Approve,
            None,
            false,
            "2026-06-18T10:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("already posted"));
        // Missing id → 404-equivalent.
        let e = worklog_action(
            &pool,
            999,
            WorklogAction::Approve,
            None,
            false,
            "2026-06-18T10:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn reject_records_correction_attribution() {
        let pool = pool_with_worklogs().await;
        seed(&pool, 1, "drafted", "draft").await;
        let next = worklog_action(
            &pool,
            1,
            WorklogAction::Reject,
            Some("PROJ-9"),
            false,
            "2026-06-18T10:00:00Z",
        )
        .await
        .unwrap();
        assert_eq!(next, "skipped");
        let (kind, note, corrected): (String, String, Option<String>) = sqlx::query_as(
            "SELECT feedback_kind, note, corrected_task_key FROM pm_worklog_feedback WHERE pm_worklog_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kind, "reject");
        assert_eq!(note, "drafted→skipped");
        assert_eq!(corrected.as_deref(), Some("PROJ-9"));
    }
}
