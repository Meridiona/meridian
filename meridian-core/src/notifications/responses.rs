//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Interactive-notification response leg (migration 057) — split out of
//! [`super`] to keep each file under the repo's 500-line cap.
//!
//! A row may carry a category + action buttons; the tray records the user's
//! answer via [`record_response`], and the daemon consumes answers via
//! [`unconsumed_responses`] + [`mark_response_consumed`]. Every read/write here
//! degrades gracefully on a pre-057 DB (overlapping installs) through
//! [`super::has_action_columns`], the same fail-soft posture as the delivery
//! reads in [`super`].

use super::has_action_columns;
use crate::SqlitePool;
use anyhow::Context;
use sqlx::FromRow;
use tracing::Instrument;

/// Record the user's answer to an interactive notification: stamp
/// `responded_at` + the pressed `action` (an action id, `'tap'`, or
/// `'dismiss'`) + any inline-reply `text`. First answer wins — the
/// `AND responded_at IS NULL` guard makes a duplicate (or late) answer a no-op,
/// mirroring [`super::mark_native_delivered`]. `now` is the caller-resolved
/// stamp. No-op on a pre-057 DB (the columns don't exist — nothing to record).
#[tracing::instrument(skip(pool, text))]
pub async fn record_response(
    pool: &SqlitePool,
    id: i64,
    action: &str,
    text: Option<&str>,
    now: &str,
) -> anyhow::Result<()> {
    if !has_action_columns(pool).await {
        tracing::warn!(id, "notifications: response ignored — pre-057 schema");
        return Ok(());
    }
    sqlx::query(
        "UPDATE notifications
         SET responded_at = ?, response_action = ?, response_text = ?
         WHERE id = ? AND responded_at IS NULL",
    )
    .bind(now)
    .bind(action)
    .bind(text)
    .bind(id)
    .execute(pool)
    .instrument(tracing::debug_span!("notifications.write.response"))
    .await
    .context("recording notification response")?;
    Ok(())
}

/// An answered-but-unconsumed notification, everything the daemon's response
/// consumer needs to dispatch on the answer and (for snooze) re-enqueue the
/// original event.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NotificationResponse {
    pub id: i64,
    pub dedup_key: String,
    pub event_key: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub deep_link: Option<String>,
    pub channels: String,
    pub category: Option<String>,
    pub actions: Option<String>,
    pub response_action: String,
    pub response_text: Option<String>,
    pub responded_at: String,
}

/// Rows the user has answered that no consumer has acted on yet (FIFO by id).
/// The daemon's response consumer drains these each tick and stamps
/// [`mark_response_consumed`]. Empty on a pre-057 DB or read error — the
/// consumer just idles, same fail-soft posture as [`super::pending_native`].
#[tracing::instrument(skip(pool))]
pub async fn unconsumed_responses(pool: &SqlitePool) -> Vec<NotificationResponse> {
    if !has_action_columns(pool).await {
        return Vec::new();
    }
    let rows: Vec<NotificationResponse> = sqlx::query_as::<_, NotificationResponse>(
        r#"SELECT id, dedup_key, event_key, severity, title, body, deep_link, channels,
                  category, actions, response_action, response_text, responded_at
           FROM notifications
           WHERE responded_at IS NOT NULL AND response_consumed_at IS NULL
           ORDER BY id ASC"#,
    )
    .fetch_all(pool)
    .instrument(tracing::debug_span!("notifications.read.responses"))
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "notifications: responses read failed, treating as empty");
        Vec::new()
    });
    tracing::debug!(rows = rows.len(), "notifications.read.responses");
    rows
}

/// Outbox rows whose toast should be withdrawn: delivered, unanswered, past
/// `expires_at`. The tray retracts each from the screen/Notification Center
/// (macOS Alerts style persists until acted on — expiry is how a "fleeting"
/// notification self-clears) and stamps it via [`record_response`] with
/// action `'expired'`, which removes it from this queue (idempotent).
/// Empty on a pre-057 DB or read error.
#[tracing::instrument(skip(pool))]
pub async fn expired_unanswered(pool: &SqlitePool, now_iso: &str) -> Vec<i64> {
    if !has_action_columns(pool).await {
        return Vec::new();
    }
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT id FROM notifications
           WHERE delivered_native_at IS NOT NULL
             AND responded_at IS NULL
             AND expires_at IS NOT NULL AND expires_at <= ?
           ORDER BY id ASC"#,
    )
    .bind(now_iso)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("notifications.read.expired"))
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "notifications: expired read failed, treating as empty");
        Vec::new()
    });
    tracing::debug!(rows = ids.len(), "notifications.read.expired");
    ids
}

/// The row's `deep_link`, if any. The tray's response command resolves
/// click-through navigation from the DB row rather than from the toast payload
/// (the notification plugin's macOS layer doesn't round-trip attached extras —
/// the notification id is the only correlation that survives).
#[tracing::instrument(skip(pool))]
pub async fn notification_deep_link(pool: &SqlitePool, id: i64) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT deep_link FROM notifications WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .instrument(tracing::debug_span!("notifications.read.deep_link"))
        .await
        .ok()
        .flatten()
        .flatten()
}

/// Ack that a consumer acted on a response: stamp `response_consumed_at` so the
/// row leaves the [`unconsumed_responses`] queue. Idempotent via the `IS NULL`
/// guard. `now` is the caller-resolved stamp.
#[tracing::instrument(skip(pool))]
pub async fn mark_response_consumed(pool: &SqlitePool, id: i64, now: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE notifications SET response_consumed_at = ? \
         WHERE id = ? AND response_consumed_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .instrument(tracing::debug_span!("notifications.write.consumed"))
    .await
    .context("marking notification response consumed")?;
    Ok(())
}
