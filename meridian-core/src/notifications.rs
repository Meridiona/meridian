//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notification delivery policy + the native-channel pending queue — the read
//! half of the outbox, ported from `ui/lib/notifications.ts`.
//!
//! The Rust daemon (`src/notifications.rs`) ENQUEUES into the `notifications`
//! table; this module is the single place the *delivery* decision lives:
//! master switch + per-type toggle ([`event_allowed`]) and quiet hours
//! ([`in_quiet_hours`]). Producers always enqueue; only the user's settings
//! decide whether an event actually surfaces.
//!
//! # Who calls this
//! - [`crate::notifications::pending_native`] — the tray poll loop's
//!   `drain_notifications` (replaces its `/api/notifications/pending` fetch).
//! - [`event_allowed`] + [`in_quiet_hours`] — the tray poll loop's
//!   `notifications_allowed` (replaces its `/api/notifications/allowed` fetch).
//!
//! # Related
//! - [`crate::settings::RuntimeSettings`] — the preference fields these read.

use crate::settings::RuntimeSettings;
use crate::SqlitePool;
use sqlx::FromRow;
use tracing::Instrument;

/// A native notification ready to fire (the shape the tray delivers + the route
/// returns).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingNotification {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub deep_link: Option<String>,
    pub severity: String,
}

/// Per-type preference for an `event_key`. Unknown keys default to enabled (a new
/// producer is visible until the user opts out), gated only by the master switch.
fn type_enabled(event_key: &str, s: &RuntimeSettings) -> bool {
    match event_key {
        "plan.nudge" => s.notify_plan_nudge,
        "worklog.ready" => s.notify_worklog_ready,
        "system.fault" => s.notify_system_fault,
        _ => true,
    }
}

/// Whether `event_key` may surface at all: master switch AND per-type toggle.
/// Mirrors `eventAllowed` in ui/lib/notifications.ts.
pub fn event_allowed(event_key: &str, s: &RuntimeSettings) -> bool {
    s.notifications_enabled && type_enabled(event_key, s)
}

/// Minutes since midnight for an 'HH:MM' string, or `None` if malformed.
fn hhmm_to_minutes(hhmm: &str) -> Option<i64> {
    let (h, m) = hhmm.trim().split_once(':')?;
    let h: i64 = h.parse().ok()?;
    let m: i64 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// True if `cur_minutes` (minutes since local midnight) falls inside the quiet-
/// hours window. Pure + testable; handles windows that wrap past midnight
/// (22:00→08:00). Fails open (false) when disabled or bounds malformed — better
/// to notify than to silently swallow. Mirrors `inQuietHours`.
pub fn in_quiet_hours_at(s: &RuntimeSettings, cur_minutes: i64) -> bool {
    if !s.quiet_hours_enabled {
        return false;
    }
    let (Some(start), Some(end)) = (
        hhmm_to_minutes(&s.quiet_hours_start),
        hhmm_to_minutes(&s.quiet_hours_end),
    ) else {
        return false;
    };
    if start == end {
        return false;
    }
    if start < end {
        cur_minutes >= start && cur_minutes < end // same-day window
    } else {
        cur_minutes >= start || cur_minutes < end // wraps past midnight
    }
}

/// [`in_quiet_hours_at`] evaluated against the current local wall clock.
pub fn in_quiet_hours(s: &RuntimeSettings) -> bool {
    use chrono::Timelike;
    let now = chrono::Local::now();
    in_quiet_hours_at(s, now.hour() as i64 * 60 + now.minute() as i64)
}

#[derive(FromRow)]
struct NotifRow {
    id: i64,
    event_key: String,
    severity: String,
    title: String,
    body: String,
    deep_link: Option<String>,
    channels: String,
}

fn has_channel(channels: &str, channel: &str) -> bool {
    channels.split(',').any(|c| c.trim() == channel)
}

/// Native-channel rows ready to fire: undelivered, due, unexpired, channel
/// includes 'native', allowed by prefs + quiet hours. FIFO by id. `now_iso` is
/// the comparison instant (UTC ISO, no millis — matches the route). Mirrors
/// `pendingNative`. Returns empty on a pre-migration-042 DB (no table).
#[tracing::instrument(skip(pool, s))]
pub async fn pending_native(
    pool: &SqlitePool,
    now_iso: &str,
    s: &RuntimeSettings,
) -> Vec<PendingNotification> {
    // Quiet hours gate the whole native channel — short-circuit before querying.
    if in_quiet_hours(s) {
        return Vec::new();
    }
    let rows: Vec<NotifRow> = sqlx::query_as::<_, NotifRow>(
        r#"SELECT id, event_key, severity, title, body, deep_link, channels
           FROM notifications
           WHERE delivered_native_at IS NULL
             AND (scheduled_for IS NULL OR scheduled_for <= ?)
             AND (expires_at IS NULL OR expires_at > ?)
           ORDER BY id ASC"#,
    )
    .bind(now_iso)
    .bind(now_iso)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("notifications.read.pending"))
    .await
    .unwrap_or_else(|e| {
        // Pre-migration-042 DB (no notifications table) or transient read error —
        // empty queue rather than erroring the tray's poll loop (matches the route).
        tracing::warn!(error = %e, "notifications: pending read failed, treating as empty");
        Vec::new()
    });

    let out: Vec<PendingNotification> = rows
        .into_iter()
        .filter(|r| has_channel(&r.channels, "native") && event_allowed(&r.event_key, s))
        .map(|r| PendingNotification {
            id: r.id,
            title: r.title,
            body: r.body,
            deep_link: r.deep_link,
            severity: r.severity,
        })
        .collect();
    tracing::debug!(rows = out.len(), "notifications.read.pending");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> RuntimeSettings {
        RuntimeSettings::default()
    }

    #[test]
    fn event_allowed_respects_master_and_type() {
        let mut s = settings();
        assert!(event_allowed("plan.nudge", &s));
        assert!(event_allowed("unknown.event", &s)); // unknown → enabled
        s.notify_plan_nudge = false;
        assert!(!event_allowed("plan.nudge", &s));
        s.notify_plan_nudge = true;
        s.notifications_enabled = false; // master off → nothing
        assert!(!event_allowed("plan.nudge", &s));
        assert!(!event_allowed("unknown.event", &s));
    }

    #[test]
    fn quiet_hours_same_day_and_wraparound() {
        let mut s = settings();
        // disabled → never quiet
        assert!(!in_quiet_hours_at(&s, 23 * 60));
        s.quiet_hours_enabled = true;
        // default 22:00–08:00 wraps midnight
        assert!(in_quiet_hours_at(&s, 23 * 60)); // 23:00 inside
        assert!(in_quiet_hours_at(&s, 2 * 60)); // 02:00 inside
        assert!(!in_quiet_hours_at(&s, 12 * 60)); // noon outside
        assert!(!in_quiet_hours_at(&s, 8 * 60)); // 08:00 end-exclusive → outside
        assert!(in_quiet_hours_at(&s, 22 * 60)); // 22:00 start-inclusive → inside
                                                 // same-day window 09:00–17:00
        s.quiet_hours_start = "09:00".into();
        s.quiet_hours_end = "17:00".into();
        assert!(in_quiet_hours_at(&s, 12 * 60));
        assert!(!in_quiet_hours_at(&s, 8 * 60));
        // malformed bounds → fail open (not quiet)
        s.quiet_hours_start = "nope".into();
        assert!(!in_quiet_hours_at(&s, 12 * 60));
    }
}
