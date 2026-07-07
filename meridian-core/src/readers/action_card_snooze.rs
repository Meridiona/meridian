//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Per-kind dismiss/snooze state for the standardized action-card stack
//! (must-fix / cleanup / drafts). No route to port — new work, migration 057.
//!
//! # What this is
//! One row per `card_kind` in `action_card_snooze`; dismissing a card in the
//! UI upserts `snoozed_until` (the tray command resolves the per-kind window —
//! drafts ~4h, cleanup ~3d, must-fix ~1d — mirroring
//! [`crate::triage::record_decision`]'s snooze-expiry pattern). The read side
//! returns only STILL-ACTIVE snoozes (`snoozed_until > now`) so the frontend
//! can filter with a simple presence check, no client-side date math.
//!
//! # Who calls this
//! The tray `get_action_card_snoozes` / `snooze_action_card` commands →
//! `useTimelineData`'s `actionCardSnoozes` state + `dismissActionCard` action
//! → `useActionItems` filters a kind out of the stack while its snooze is
//! active, and `ActionCard`'s dismiss (×) button triggers the write.
//!
//! # Related
//! - [`crate::triage`] — the sibling ticket-level snooze concept; a distinct
//!   table, same shape (`snoozed_until` NULL/expired = visible).

use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use sqlx::FromRow;
use tracing::Instrument;

/// One card kind's active dismiss state.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ActionCardSnooze {
    pub card_kind: String,
    pub snoozed_until: String,
}

/// Every card_kind currently snoozed into the future (`snoozed_until >
/// now_iso`). An expired or absent row means the card is visible again —
/// filtered out here so the frontend only ever sees active snoozes (a simple
/// presence check, no date math on that side).
#[tracing::instrument(skip(pool))]
pub async fn get_active_snoozes(
    pool: &SqlitePool,
    now_iso: &str,
) -> anyhow::Result<Vec<ActionCardSnooze>> {
    let rows = sqlx::query_as::<_, ActionCardSnooze>(
        "SELECT card_kind, snoozed_until FROM action_card_snooze WHERE snoozed_until > ?",
    )
    .bind(now_iso)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("action_card_snooze.read.active"))
    .await
    .context("reading active action-card snoozes")?;
    tracing::debug!(rows = rows.len(), "action_card_snooze.read.active");
    Ok(rows)
}

/// Upsert a card kind's snooze expiry (the caller/tray command resolves the
/// per-kind window). Idempotent — re-dismissing the same kind just replaces
/// its expiry rather than erroring or duplicating.
#[tracing::instrument(skip(pool))]
pub async fn snooze_card(
    pool: &SqlitePool,
    card_kind: &str,
    snoozed_until: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO action_card_snooze (card_kind, snoozed_until) VALUES (?, ?) \
         ON CONFLICT(card_kind) DO UPDATE SET snoozed_until = excluded.snoozed_until",
    )
    .bind(card_kind)
    .bind(snoozed_until)
    .execute(pool)
    .instrument(tracing::debug_span!("action_card_snooze.write.upsert"))
    .await
    .with_context(|| format!("snoozing action card {card_kind}"))?;
    tracing::info!(card_kind, snoozed_until, "action card snoozed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_table() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE action_card_snooze (card_kind TEXT PRIMARY KEY, snoozed_until TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn active_snoozes_exclude_expired() {
        let pool = pool_with_table().await;
        snooze_card(&pool, "drafts", "2026-01-01T04:00:00Z")
            .await
            .unwrap();
        snooze_card(&pool, "cleanup", "2020-01-01T00:00:00Z")
            .await
            .unwrap();

        let active = get_active_snoozes(&pool, "2026-01-01T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].card_kind, "drafts");
    }

    #[tokio::test]
    async fn re_snoozing_replaces_not_duplicates() {
        let pool = pool_with_table().await;
        snooze_card(&pool, "mustfix", "2026-01-01T00:00:00Z")
            .await
            .unwrap();
        snooze_card(&pool, "mustfix", "2026-01-02T00:00:00Z")
            .await
            .unwrap();

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM action_card_snooze")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);
        let active = get_active_snoozes(&pool, "2025-12-31T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(active[0].snoozed_until, "2026-01-02T00:00:00Z");
    }
}
