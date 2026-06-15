//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Notification outbox — the single emit point for discrete, user-facing
// notification events (plan nudge, worklog ready, a fault promoted to an OS
// toast). Mirrors the `notices` fault-bus shape but with different semantics:
//
//   notices       — STATEFUL conditions, raised then cleared; shown as a banner
//                    while active. Use for faults that come and go.
//   notifications — DISCRETE events delivered once per `dedup_key`, drained by
//                    the tray (native macOS toast) and/or the dashboard (banner).
//
// Features call `enqueue` with a `dedup_key` that encodes the once-only scope
// (e.g. `plan.nudge:2026-06-15`); re-enqueuing the same key is a no-op, so the
// event fires exactly once no matter how often the producing loop runs. Delivery
// state (per channel) and preference/quiet-hours filtering live with the
// consumers (the UI API routes and the tray relay), keeping this a thin,
// centralised write surface.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// macOS desktop toast, delivered by the tray relay.
pub const CHANNEL_NATIVE: &str = "native";
/// In-dashboard banner, surfaced by the UI.
pub const CHANNEL_BANNER: &str = "banner";
/// Both channels — the redundant default for important events.
pub const CHANNELS_BOTH: &str = "native,banner";

/// A notification to enqueue. Grouped into a struct so producers read clearly and
/// to stay under clippy's argument limit.
pub struct NewNotification<'a> {
    /// Once-only key — usually `<event_key>:<scope>`. Re-enqueuing is a no-op.
    pub dedup_key: &'a str,
    /// Event type, used for preference lookup (e.g. `plan.nudge`).
    pub event_key: &'a str,
    /// `info` | `warning` | `error`.
    pub severity: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    /// Optional dashboard route to open on click (e.g. `/plan`).
    pub deep_link: Option<&'a str>,
    /// CSV of [`CHANNEL_NATIVE`] / [`CHANNEL_BANNER`].
    pub channels: &'a str,
    /// ISO8601 UTC; the notification is withheld until this time (NULL = now).
    pub scheduled_for: Option<&'a str>,
    /// ISO8601 UTC; the notification is suppressed after this time (NULL = never).
    pub expires_at: Option<&'a str>,
}

impl<'a> NewNotification<'a> {
    /// A simple info-level event delivered on both channels immediately.
    pub fn event(dedup_key: &'a str, event_key: &'a str, title: &'a str, body: &'a str) -> Self {
        Self {
            dedup_key,
            event_key,
            severity: "info",
            title,
            body,
            deep_link: None,
            channels: CHANNELS_BOTH,
            scheduled_for: None,
            expires_at: None,
        }
    }

    /// Set the click-through deep link (builder style).
    pub fn link(mut self, deep_link: &'a str) -> Self {
        self.deep_link = Some(deep_link);
        self
    }

    /// Restrict delivery to specific channels (builder style).
    pub fn via(mut self, channels: &'a str) -> Self {
        self.channels = channels;
        self
    }

    /// Set the expiry (builder style).
    pub fn expiring(mut self, expires_at: &'a str) -> Self {
        self.expires_at = Some(expires_at);
        self
    }
}

/// Enqueue a notification. Idempotent on `dedup_key` — repeated calls are
/// no-ops, so a producing loop can call this every tick without spamming.
pub async fn enqueue(pool: &SqlitePool, n: NewNotification<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO notifications
            (dedup_key, event_key, severity, title, body, deep_link, channels, scheduled_for, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(dedup_key) DO NOTHING",
    )
    .bind(n.dedup_key)
    .bind(n.event_key)
    .bind(n.severity)
    .bind(n.title)
    .bind(n.body)
    .bind(n.deep_link)
    .bind(n.channels)
    .bind(n.scheduled_for)
    .bind(n.expires_at)
    .execute(pool)
    .await
    .context("enqueueing notification")?;
    Ok(())
}

/// Drop any pending notification with this `dedup_key`. Used when a stateful
/// condition recovers (e.g. a fault clears), so a later re-occurrence of the same
/// condition re-enqueues and notifies again instead of being deduped away.
pub async fn retract(pool: &SqlitePool, dedup_key: &str) -> Result<()> {
    sqlx::query("DELETE FROM notifications WHERE dedup_key = ?")
        .bind(dedup_key)
        .execute(pool)
        .await
        .context("retracting notification")?;
    Ok(())
}
