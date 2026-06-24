//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notification delivery policy + the native-channel queue (read + delivery
//! writes) — the consumer half of the outbox, ported from `ui/lib/notifications.ts`.
//!
//! The Rust daemon (`src/notifications.rs`) ENQUEUES into the `notifications`
//! table; this module is the single place the *delivery* decision lives:
//! master switch + per-type toggle ([`event_allowed`]) and quiet hours
//! ([`in_quiet_hours`]). Producers always enqueue; only the user's settings
//! decide whether an event actually surfaces. The two delivery writes
//! ([`mark_native_delivered`], [`dismiss_banner`]) ack a row so it isn't
//! re-delivered / re-shown — idempotent, mirroring the same-named TS helpers.
//!
//! # Who calls this
//! - [`pending_native`] + [`mark_native_delivered`] — the tray poll loop's
//!   `drain_notifications` (replaces its `/api/notifications/pending` fetch AND
//!   its `/api/notifications/:id/delivered` ack — the loop is now HTTP-free).
//! - [`event_allowed`] + [`in_quiet_hours`] — the tray poll loop's
//!   `notifications_allowed` (replaces its `/api/notifications/allowed` fetch).
//! - [`dismiss_banner`] — the tray `dismiss_notification` command (ported
//!   `/api/notifications/:id/dismiss`), from the dashboard banner.
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
///
/// Strict by design: mirrors the original route's `/^(\d{1,2}):(\d{2})$/` —
/// 1–2 ASCII-digit hours, exactly 2 ASCII-digit minutes, nothing else. A plain
/// `split_once(':') + parse` would be more lenient (accepting `"8:5"`, `"+8:00"`,
/// `"8:00:00"`) and diverge from the dashboard's silence/notify decision.
fn hhmm_to_minutes(hhmm: &str) -> Option<i64> {
    let (h, m) = hhmm.trim().split_once(':')?;
    let valid_digits = |s: &str, max_len: usize| -> bool {
        (1..=max_len).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
    };
    if !valid_digits(h, 2) || m.len() != 2 || !m.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
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

/// An in-app banner notification (the shape `NotificationBanner.tsx` renders).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BannerNotification {
    pub id: i64,
    pub event_key: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub deep_link: Option<String>,
    pub created_at: String,
}

#[derive(FromRow)]
struct BannerRow {
    id: i64,
    event_key: String,
    severity: String,
    title: String,
    body: String,
    deep_link: Option<String>,
    created_at: String,
    channels: String,
}

/// Banner-channel rows the dashboard should show: not dismissed, unexpired,
/// channel includes 'banner', allowed by prefs. NEWEST first (id DESC). Unlike
/// [`pending_native`], banners are NOT gated by quiet hours — they're passive
/// (the user dismisses them), so quiet hours only silences the interruptive
/// native channel. `now_iso` is the expiry comparison instant. Mirrors
/// `activeBanners` in ui/lib/notifications.ts. Empty on a pre-migration DB.
#[tracing::instrument(skip(pool, s))]
pub async fn active_banners(
    pool: &SqlitePool,
    now_iso: &str,
    s: &RuntimeSettings,
) -> Vec<BannerNotification> {
    let rows: Vec<BannerRow> = sqlx::query_as::<_, BannerRow>(
        r#"SELECT id, event_key, severity, title, body, deep_link, created_at, channels
           FROM notifications
           WHERE banner_dismissed_at IS NULL
             AND (expires_at IS NULL OR expires_at > ?)
           ORDER BY id DESC"#,
    )
    .bind(now_iso)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("notifications.read.banners"))
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "notifications: banner read failed, treating as empty");
        Vec::new()
    });

    let out: Vec<BannerNotification> = rows
        .into_iter()
        .filter(|r| has_channel(&r.channels, "banner") && event_allowed(&r.event_key, s))
        .map(|r| BannerNotification {
            id: r.id,
            event_key: r.event_key,
            severity: r.severity,
            title: r.title,
            body: r.body,
            deep_link: r.deep_link,
            created_at: r.created_at,
        })
        .collect();
    tracing::debug!(rows = out.len(), "notifications.read.banners");
    out
}

// ── Delivery writes ───────────────────────────────────────────────────────────

/// Ack native delivery of a notification (port of `/api/notifications/:id/delivered`):
/// stamp `delivered_native_at` + bump `attempts`, so the tray's poll loop never
/// re-toasts it. The `AND delivered_native_at IS NULL` guard makes it idempotent
/// (a duplicate ack is a no-op). `now` is the caller-resolved stamp.
#[tracing::instrument(skip(pool))]
pub async fn mark_native_delivered(pool: &SqlitePool, id: i64, now: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE notifications SET delivered_native_at = ?, attempts = attempts + 1 \
         WHERE id = ? AND delivered_native_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .instrument(tracing::debug_span!("notifications.write.delivered"))
    .await?;
    Ok(())
}

/// Dismiss an in-app banner (port of `/api/notifications/:id/dismiss`): stamp
/// `banner_dismissed_at` so the dashboard banner set drops it. Idempotent via the
/// `AND banner_dismissed_at IS NULL` guard. `now` is the caller-resolved stamp.
#[tracing::instrument(skip(pool))]
pub async fn dismiss_banner(pool: &SqlitePool, id: i64, now: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE notifications SET banner_dismissed_at = ? \
         WHERE id = ? AND banner_dismissed_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .instrument(tracing::debug_span!("notifications.write.dismiss"))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    fn settings() -> RuntimeSettings {
        RuntimeSettings::default()
    }

    /// In-memory pool with the columns the delivery writes touch.
    async fn notif_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE notifications (\
                id INTEGER PRIMARY KEY, delivered_native_at TEXT, banner_dismissed_at TEXT, \
                attempts INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn mark_native_delivered_is_idempotent() {
        let pool = notif_pool().await;
        sqlx::query("INSERT INTO notifications (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        mark_native_delivered(&pool, 1, "2026-06-18T10:00:00Z")
            .await
            .unwrap();
        // Second ack is a no-op (the IS NULL guard) — attempts must NOT bump again.
        mark_native_delivered(&pool, 1, "2026-06-18T11:00:00Z")
            .await
            .unwrap();
        let (delivered, attempts): (Option<String>, i64) =
            sqlx::query_as("SELECT delivered_native_at, attempts FROM notifications WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(delivered.as_deref(), Some("2026-06-18T10:00:00Z"));
        assert_eq!(attempts, 1, "duplicate ack must not re-bump attempts");
    }

    #[tokio::test]
    async fn dismiss_banner_stamps_once() {
        let pool = notif_pool().await;
        sqlx::query("INSERT INTO notifications (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        dismiss_banner(&pool, 1, "2026-06-18T10:00:00Z")
            .await
            .unwrap();
        dismiss_banner(&pool, 1, "2026-06-18T11:00:00Z")
            .await
            .unwrap();
        let stamp: Option<String> =
            sqlx::query_scalar("SELECT banner_dismissed_at FROM notifications WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stamp.as_deref(), Some("2026-06-18T10:00:00Z"));
    }

    #[tokio::test]
    async fn active_banners_filters_channel_dismissed_expired_and_prefs() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE notifications (\
                id INTEGER PRIMARY KEY, event_key TEXT, severity TEXT, title TEXT, body TEXT, \
                deep_link TEXT, created_at TEXT, channels TEXT, \
                banner_dismissed_at TEXT, expires_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = "2026-06-18T10:00:00Z";
        // id1: banner, live → shown.  id2: native-only → excluded.
        // id3: dismissed → excluded.  id4: expired → excluded.
        // id5: banner but newer id → must sort BEFORE id1 (id DESC).
        let rows = [
            (1, "plan.nudge", "banner", None::<&str>, None::<&str>),
            (2, "plan.nudge", "native", None, None),
            (3, "plan.nudge", "banner", Some(now), None),
            (
                4,
                "plan.nudge",
                "banner",
                None,
                Some("2026-06-18T09:00:00Z"),
            ),
            (5, "plan.nudge", "banner,native", None, None),
        ];
        for (id, ek, ch, dismissed, expires) in rows {
            sqlx::query(
                "INSERT INTO notifications (id, event_key, severity, title, body, created_at, channels, banner_dismissed_at, expires_at) \
                 VALUES (?, ?, 'info', 't', 'b', '2026-06-18T08:00:00Z', ?, ?, ?)",
            )
            .bind(id)
            .bind(ek)
            .bind(ch)
            .bind(dismissed)
            .bind(expires)
            .execute(&pool)
            .await
            .unwrap();
        }

        let banners = active_banners(&pool, now, &settings()).await;
        let ids: Vec<i64> = banners.iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            vec![5, 1],
            "id DESC; native/dismissed/expired excluded"
        );

        // Master switch off → nothing surfaces.
        let mut off = settings();
        off.notifications_enabled = false;
        assert!(active_banners(&pool, now, &off).await.is_empty());
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

    #[test]
    fn hhmm_parser_is_strict_like_the_route_regex() {
        // Accepts 1–2 digit hours, exactly 2 digit minutes.
        assert_eq!(hhmm_to_minutes("8:00"), Some(8 * 60));
        assert_eq!(hhmm_to_minutes("08:00"), Some(8 * 60));
        assert_eq!(hhmm_to_minutes("23:59"), Some(23 * 60 + 59));
        assert_eq!(hhmm_to_minutes(" 09:30 "), Some(9 * 60 + 30)); // outer trim only
                                                                   // Rejects everything the original /^(\d{1,2}):(\d{2})$/ rejected.
        assert_eq!(hhmm_to_minutes("8:5"), None); // 1-digit minutes
        assert_eq!(hhmm_to_minutes("8:0"), None);
        assert_eq!(hhmm_to_minutes("+8:00"), None); // sign
        assert_eq!(hhmm_to_minutes("8:00:00"), None); // trailing seconds
        assert_eq!(hhmm_to_minutes("24:00"), None); // hour out of range
        assert_eq!(hhmm_to_minutes("8:60"), None); // minute out of range
        assert_eq!(hhmm_to_minutes("nope"), None);
    }
}
