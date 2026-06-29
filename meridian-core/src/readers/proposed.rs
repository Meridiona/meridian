//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Writes for tier-3 PROPOSED tickets (`pm_proposed_tasks`).
//!
//! # What this is
//! The human-in-the-loop edits/decisions on a proposed new ticket, mirroring the
//! worklog review writes in [`crate::worklogs`]. A proposal carries an editable
//! title and a drafted worklog (in `worklog_payload_json`, migration 050); the
//! user can edit either, then **approve** (→ the daemon's proposal sweep creates
//! the real ticket via the provider write-back path and posts the worklog) or
//! **dismiss**. These functions only mutate rows still in `state='proposed'`, so
//! a resolved proposal is immutable here — the same draft-immutability rule the
//! worklog surface uses.
//!
//! # Who calls this
//! The tray `edit_proposed_title` / `edit_proposed_worklog` / `proposed_action`
//! commands → the dashboard `WorklogsView` (proposals render inline; see
//! [`crate::worklogs::get_worklogs`] for the read side).
//!
//! # Related
//! - [`crate::worklogs`] — reads proposals into the day timeline and owns the
//!   real-worklog review writes.

use crate::SqlitePool;
use anyhow::Context;
use sqlx::Row;
use tracing::Instrument;

/// A decision on a proposed ticket (the allow-set the tray command accepts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposedAction {
    /// Accept the proposal — the daemon sweep then creates the real ticket and
    /// posts its worklog. Sets `state='approved'`.
    Approve,
    /// Reject the proposal — it stops surfacing and is never created. Sets
    /// `state='dismissed'`.
    Dismiss,
}

impl ProposedAction {
    /// The stored `state` value this action transitions the row to.
    pub fn target_state(self) -> &'static str {
        match self {
            ProposedAction::Approve => "approved",
            ProposedAction::Dismiss => "dismissed",
        }
    }

    /// Parse the wire string (`"approve"` / `"dismiss"`) from the tray command.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approve" => Some(ProposedAction::Approve),
            "dismiss" => Some(ProposedAction::Dismiss),
            _ => None,
        }
    }
}

/// Edit a proposed ticket's title. No-op (returns `false`) once the proposal is
/// approved/dismissed. Returns `true` when a `proposed` row was updated.
#[tracing::instrument(skip(pool))]
pub async fn edit_proposed_title(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    now_iso: &str,
) -> anyhow::Result<bool> {
    let res =
        sqlx::query("UPDATE pm_proposed_tasks SET title = ? WHERE id = ? AND state = 'proposed'")
            .bind(title)
            .bind(id)
            .execute(pool)
            .instrument(tracing::debug_span!("proposed.write.edit_title"))
            .await
            .context("proposed: edit title")?;
    let _ = now_iso;
    let updated = res.rows_affected() > 0;
    tracing::info!(id, updated, "proposed title edited");
    Ok(updated)
}

/// Edit the drafted worklog's `summary` (the editable comment body) on a
/// proposed ticket, in place inside `worklog_payload_json`. Uses SQLite
/// `json_set` so the other payload fields (bullets, next steps) are preserved.
/// No-op once approved/dismissed.
#[tracing::instrument(skip(pool))]
pub async fn edit_proposed_worklog(
    pool: &SqlitePool,
    id: i64,
    summary: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        r#"
        UPDATE pm_proposed_tasks
        SET worklog_payload_json =
            json_set(COALESCE(worklog_payload_json, '{}'), '$.summary', ?)
        WHERE id = ? AND state = 'proposed'
        "#,
    )
    .bind(summary)
    .bind(id)
    .execute(pool)
    .instrument(tracing::debug_span!("proposed.write.edit_worklog"))
    .await
    .context("proposed: edit worklog summary")?;
    let updated = res.rows_affected() > 0;
    tracing::info!(id, updated, "proposed worklog edited");
    Ok(updated)
}

/// Apply a [`ProposedAction`] (approve/dismiss) to a proposed ticket, stamping
/// `resolved_at`. Only transitions a row still in `state='proposed'`; returns
/// the new state on success, or `None` if the row was missing or already
/// resolved. The actual ticket creation + worklog post happen later in the
/// daemon's proposal sweep (this only records the decision).
#[tracing::instrument(skip(pool))]
pub async fn proposed_action(
    pool: &SqlitePool,
    id: i64,
    action: ProposedAction,
    now_iso: &str,
) -> anyhow::Result<Option<String>> {
    let target = action.target_state();
    let res = sqlx::query(
        r#"
        UPDATE pm_proposed_tasks
        SET state = ?, resolved_at = ?
        WHERE id = ? AND state = 'proposed'
        "#,
    )
    .bind(target)
    .bind(now_iso)
    .bind(id)
    .execute(pool)
    .instrument(tracing::debug_span!("proposed.write.action"))
    .await
    .context("proposed: apply action")?;

    if res.rows_affected() == 0 {
        tracing::warn!(
            id,
            ?action,
            "proposed action no-op (missing or already resolved)"
        );
        return Ok(None);
    }
    tracing::info!(id, state = target, "proposed action applied");
    Ok(Some(target.to_string()))
}

/// Fetch the connected PM provider id for routing a proposal's ticket creation.
/// Returns the first provider in `pm_sync_state` (the tracker the user linked),
/// defaulting to `"jira"` when none is recorded.
#[tracing::instrument(skip(pool))]
pub async fn connected_provider(pool: &SqlitePool) -> anyhow::Result<String> {
    let row = sqlx::query("SELECT provider FROM pm_sync_state ORDER BY rowid LIMIT 1")
        .fetch_optional(pool)
        .await
        .context("proposed: read connected provider")?;
    Ok(row
        .and_then(|r| r.try_get::<String, _>("provider").ok())
        .unwrap_or_else(|| "jira".to_string()))
}
