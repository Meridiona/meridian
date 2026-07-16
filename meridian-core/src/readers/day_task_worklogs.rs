//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The read/write side of the generated-worklog ledger (`day_task_worklogs`,
//! migration 060) behind the "Generate worklog" action on a day-task card.
//!
//! # What this is
//! Each row is one AI-drafted status update for a day-task (T1/T2…): a MATCH to an
//! existing PM ticket XOR a PROPOSE of a new one, plus a rich status update to post
//! as a comment. This module owns the DB shape and the guarded transitions; the
//! LLM call, provider dispatch, and the actual posting live daemon-side in
//! `src/pm_worklog/generate.rs` (which reuses these types so the CLI/tray/UI all
//! serialize the identical wire contract).
//!
//! # Who calls this
//! - [`get_day_task_worklog`] → the `worklog-generate-get` CLI → the tray
//!   `get_day_task_worklog` command → the day-task detail panel (shows an existing
//!   draft on reopen). Degrades to `None` on a pre-060 DB.
//! - [`upsert_draft`] / [`mark_approved`] / [`mark_posted`] / [`mark_error`] → the
//!   daemon `generate`/`approve` flow.
//!
//! # Related
//! - [`crate::day_tasks`] — the day-task cards this drafts a worklog for; its
//!   `linked_ticket` is set once a draft is approved+posted.

use crate::SqlitePool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::Instrument;

/// The matched-ticket branch of a draft (mutually exclusive with [`GeneratedWorklogPropose`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogMatch {
    pub task_key: String,
    pub confidence: f64,
}

/// The proposed-new-ticket branch of a draft (mutually exclusive with [`GeneratedWorklogMatch`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogPropose {
    pub issue_type: String,
    pub title: String,
    pub description: String,
}

/// The rich status update — always present. `decisions`/`architecture` are bullet
/// lists; `status` is the current-state one-liner. This is what gets posted as a
/// comment (rendered to plain text at post time).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneratedWorklogUpdate {
    pub summary: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub architecture: Vec<String>,
    #[serde(default)]
    pub status: String,
}

/// One generated-worklog draft, in the exact shape the CLI prints and the tray/UI
/// consume. `match`/`propose` are mutually exclusive; `update` is always present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayTaskWorklogDraft {
    /// `drafted` | `approved` | `posted`.
    pub state: String,
    /// The tracker this update posts to.
    pub provider: String,
    #[serde(rename = "match")]
    pub match_: Option<GeneratedWorklogMatch>,
    pub propose: Option<GeneratedWorklogPropose>,
    pub update: GeneratedWorklogUpdate,
    pub reasoning: String,
    /// Matched key OR created key; `None` until known.
    pub target_key: Option<String>,
    pub created_task_key: Option<String>,
    pub posted_comment_id: Option<String>,
    pub browse_url: Option<String>,
    pub error: Option<String>,
}

/// The raw DB row (migration 060).
#[derive(FromRow)]
struct RawWorklog {
    provider: String,
    match_task_key: Option<String>,
    match_confidence: Option<f64>,
    propose_issue_type: Option<String>,
    propose_title: Option<String>,
    propose_description: Option<String>,
    update_summary: String,
    update_json: String,
    reasoning: String,
    state: String,
    target_key: Option<String>,
    created_task_key: Option<String>,
    posted_comment_id: Option<String>,
    browse_url: Option<String>,
    last_error: Option<String>,
}

impl RawWorklog {
    fn into_draft(self) -> DayTaskWorklogDraft {
        // Prefer the full JSON object; fall back to the denormalised summary so a
        // partially-written / legacy row still renders.
        let update: GeneratedWorklogUpdate = serde_json::from_str(&self.update_json)
            .unwrap_or_else(|_| GeneratedWorklogUpdate {
                summary: self.update_summary.clone(),
                ..Default::default()
            });
        let match_ = self.match_task_key.map(|task_key| GeneratedWorklogMatch {
            task_key,
            confidence: self.match_confidence.unwrap_or(0.0),
        });
        let propose = match (self.propose_title, self.propose_description) {
            (Some(title), Some(description)) => Some(GeneratedWorklogPropose {
                issue_type: self
                    .propose_issue_type
                    .unwrap_or_else(|| "Task".to_string()),
                title,
                description,
            }),
            _ => None,
        };
        DayTaskWorklogDraft {
            state: self.state,
            provider: self.provider,
            match_,
            propose,
            update,
            reasoning: self.reasoning,
            target_key: self.target_key,
            created_task_key: self.created_task_key,
            posted_comment_id: self.posted_comment_id,
            browse_url: self.browse_url,
            error: self.last_error,
        }
    }
}

/// Read the current draft for `(day_local, task_id)`, or `None` if there is none.
/// A missing table (pre-060 DB) degrades to `None`, never an error.
#[tracing::instrument(skip(pool))]
pub async fn get_day_task_worklog(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> anyhow::Result<Option<DayTaskWorklogDraft>> {
    let row = sqlx::query_as::<_, RawWorklog>(
        "SELECT provider, match_task_key, match_confidence, propose_issue_type, \
                propose_title, propose_description, update_summary, update_json, \
                reasoning, state, target_key, created_task_key, posted_comment_id, \
                browse_url, last_error \
         FROM day_task_worklogs WHERE day_local = ? AND task_id = ?",
    )
    .bind(day_local)
    .bind(task_id)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.read.day_task_worklogs"
    ))
    .await;

    match row {
        Ok(Some(r)) => {
            tracing::debug!(rows = 1, "day_task_worklogs.read.day_task_worklogs");
            Ok(Some(r.into_draft()))
        }
        Ok(None) => {
            tracing::debug!(rows = 0, "day_task_worklogs.read.day_task_worklogs");
            Ok(None)
        }
        Err(e) => {
            tracing::warn!(error = %e, "day_task_worklogs: read skipped (pre-060 DB?)");
            Ok(None)
        }
    }
}

/// The fields an [`upsert_draft`] writes. Kept as a struct to stay under clippy's
/// argument limit and to read at the call site.
#[derive(Debug, Clone)]
pub struct DraftUpsert {
    pub provider: String,
    pub match_: Option<GeneratedWorklogMatch>,
    pub propose: Option<GeneratedWorklogPropose>,
    pub update: GeneratedWorklogUpdate,
    pub reasoning: String,
    /// The matched key at draft time (known immediately); `None` for a proposal.
    pub target_key: Option<String>,
}

/// UPSERT a `drafted` row for `(day_local, task_id)`, overwriting only a row that
/// is still `drafted`. An `approved`/`posted` row is human-owned and left intact
/// (the `WHERE state = 'drafted'` guard) — this is "regenerate overwrites the
/// draft, never approved/posted work". Returns the freshly-read draft.
#[tracing::instrument(skip(pool, upsert))]
pub async fn upsert_draft(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    upsert: DraftUpsert,
    now: &str,
) -> anyhow::Result<DayTaskWorklogDraft> {
    let update_json = serde_json::to_string(&upsert.update).unwrap_or_else(|_| "{}".to_string());
    let (match_key, match_conf) = match &upsert.match_ {
        Some(m) => (Some(m.task_key.clone()), Some(m.confidence)),
        None => (None, None),
    };
    let (issue_type, title, desc) = match &upsert.propose {
        Some(p) => (
            Some(p.issue_type.clone()),
            Some(p.title.clone()),
            Some(p.description.clone()),
        ),
        None => (None, None, None),
    };

    sqlx::query(
        "INSERT INTO day_task_worklogs \
            (day_local, task_id, provider, match_task_key, match_confidence, \
             propose_issue_type, propose_title, propose_description, update_summary, \
             update_json, reasoning, state, target_key, created_task_key, \
             posted_comment_id, browse_url, last_error, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'drafted', ?, NULL, NULL, NULL, NULL, ?, ?) \
         ON CONFLICT(day_local, task_id) DO UPDATE SET \
            provider = excluded.provider, \
            match_task_key = excluded.match_task_key, \
            match_confidence = excluded.match_confidence, \
            propose_issue_type = excluded.propose_issue_type, \
            propose_title = excluded.propose_title, \
            propose_description = excluded.propose_description, \
            update_summary = excluded.update_summary, \
            update_json = excluded.update_json, \
            reasoning = excluded.reasoning, \
            state = 'drafted', \
            target_key = excluded.target_key, \
            created_task_key = NULL, \
            posted_comment_id = NULL, \
            browse_url = NULL, \
            last_error = NULL, \
            updated_at = excluded.updated_at \
         WHERE day_task_worklogs.state = 'drafted'",
    )
    .bind(day_local)
    .bind(task_id)
    .bind(&upsert.provider)
    .bind(&match_key)
    .bind(match_conf)
    .bind(&issue_type)
    .bind(&title)
    .bind(&desc)
    .bind(&upsert.update.summary)
    .bind(&update_json)
    .bind(&upsert.reasoning)
    .bind(&upsert.target_key)
    .bind(now)
    .bind(now)
    .execute(pool)
    .instrument(tracing::debug_span!("day_task_worklogs.write.upsert"))
    .await?;

    // Read back whatever now owns the key — either the fresh draft, or the
    // preserved approved/posted row the guard protected.
    let draft = get_day_task_worklog(pool, day_local, task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("draft vanished immediately after upsert"))?;
    Ok(draft)
}

/// Move a `drafted` row to `approved` (idempotent — an already-approved/posted row
/// is untouched). Clears any prior `last_error` so a retry starts clean.
#[tracing::instrument(skip(pool))]
pub async fn mark_approved(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs SET state = 'approved', last_error = NULL, updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'drafted'",
    )
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Persist a created ticket's key BEFORE its comment is posted, so an approve
/// retry never re-creates the ticket. Sets both `created_task_key` and
/// `target_key`.
#[tracing::instrument(skip(pool))]
pub async fn mark_created(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    created_task_key: &str,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs \
         SET created_task_key = ?, target_key = ?, updated_at = ? \
         WHERE day_local = ? AND task_id = ?",
    )
    .bind(created_task_key)
    .bind(created_task_key)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Move a row to `posted` with the created comment id + browse url. Terminal.
#[tracing::instrument(skip(pool))]
pub async fn mark_posted(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    target_key: &str,
    comment_id: &str,
    browse_url: Option<&str>,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs \
         SET state = 'posted', target_key = ?, posted_comment_id = ?, browse_url = ?, \
             last_error = NULL, updated_at = ? \
         WHERE day_local = ? AND task_id = ?",
    )
    .bind(target_key)
    .bind(comment_id)
    .bind(browse_url)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failure onto the row without changing its lifecycle state — leaves it
/// retry-safe (a `drafted` row stays drafted, an `approved` row stays approved).
#[tracing::instrument(skip(pool))]
pub async fn mark_error(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    error: &str,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs SET last_error = ?, updated_at = ? \
         WHERE day_local = ? AND task_id = ?",
    )
    .bind(error)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn seeded() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE day_task_worklogs (\
                day_local TEXT NOT NULL, task_id TEXT NOT NULL, provider TEXT NOT NULL, \
                match_task_key TEXT, match_confidence REAL, propose_issue_type TEXT, \
                propose_title TEXT, propose_description TEXT, \
                update_summary TEXT NOT NULL DEFAULT '', update_json TEXT NOT NULL DEFAULT '{}', \
                reasoning TEXT NOT NULL DEFAULT '', state TEXT NOT NULL DEFAULT 'drafted', \
                target_key TEXT, created_task_key TEXT, posted_comment_id TEXT, browse_url TEXT, \
                last_error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, \
                PRIMARY KEY (day_local, task_id))",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn match_upsert() -> DraftUpsert {
        DraftUpsert {
            provider: "jira".into(),
            match_: Some(GeneratedWorklogMatch {
                task_key: "KAN-12".into(),
                confidence: 0.86,
            }),
            propose: None,
            update: GeneratedWorklogUpdate {
                summary: "Wired the thing".into(),
                decisions: vec!["Chose X".into()],
                architecture: vec!["Y talks to Z".into()],
                status: "In progress".into(),
            },
            reasoning: "clear advance".into(),
            target_key: Some("KAN-12".into()),
        }
    }

    #[tokio::test]
    async fn none_when_absent_and_pre_060() {
        // Absent row.
        let pool = seeded().await;
        assert!(get_day_task_worklog(&pool, "2026-07-16", "T1")
            .await
            .unwrap()
            .is_none());
        // No table at all (pre-060 DB) degrades to None, not an error.
        let bare = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        assert!(get_day_task_worklog(&bare, "2026-07-16", "T1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn upsert_roundtrips_a_match_draft() {
        let pool = seeded().await;
        let d = upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
            .await
            .unwrap();
        assert_eq!(d.state, "drafted");
        assert_eq!(d.provider, "jira");
        assert_eq!(d.match_.as_ref().unwrap().task_key, "KAN-12");
        assert!(d.propose.is_none());
        assert_eq!(d.update.decisions, vec!["Chose X"]);
        assert_eq!(d.target_key.as_deref(), Some("KAN-12"));
        assert!(d.error.is_none());
    }

    #[tokio::test]
    async fn upsert_overwrites_drafted_only() {
        let pool = seeded().await;
        upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
            .await
            .unwrap();

        // A regenerate over a still-drafted row overwrites it (flip to a proposal).
        let mut prop = match_upsert();
        prop.match_ = None;
        prop.propose = Some(GeneratedWorklogPropose {
            issue_type: "Bug".into(),
            title: "Fix it".into(),
            description: "It is broken".into(),
        });
        prop.target_key = None;
        let d = upsert_draft(&pool, "2026-07-16", "T1", prop.clone(), "t1")
            .await
            .unwrap();
        assert!(d.match_.is_none());
        assert_eq!(d.propose.as_ref().unwrap().title, "Fix it");

        // Now approve+post the row, then attempt a regenerate — it MUST be preserved.
        mark_approved(&pool, "2026-07-16", "T1", "t2")
            .await
            .unwrap();
        mark_posted(
            &pool,
            "2026-07-16",
            "T1",
            "KAN-99",
            "c1",
            Some("http://x"),
            "t3",
        )
        .await
        .unwrap();
        let d = upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t4")
            .await
            .unwrap();
        assert_eq!(d.state, "posted", "posted row must not be clobbered");
        assert_eq!(d.posted_comment_id.as_deref(), Some("c1"));
        assert_eq!(d.browse_url.as_deref(), Some("http://x"));
    }

    #[tokio::test]
    async fn mark_error_is_state_preserving() {
        let pool = seeded().await;
        upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
            .await
            .unwrap();
        mark_error(&pool, "2026-07-16", "T1", "boom", "t1")
            .await
            .unwrap();
        let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(d.state, "drafted");
        assert_eq!(d.error.as_deref(), Some("boom"));
    }
}
