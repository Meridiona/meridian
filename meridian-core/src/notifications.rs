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
//! Interactive notifications (migration 057) extend the outbox into a
//! round-trip mailbox: a row may carry a [`categories`] id + action buttons,
//! the tray records the user's answer via [`record_response`], and the daemon
//! consumes answers via [`unconsumed_responses`] + [`mark_response_consumed`].
//!
//! # Who calls this
//! - [`pending_native`] + [`mark_native_delivered`] — the tray poll loop's
//!   `drain_notifications` (replaces its `/api/notifications/pending` fetch AND
//!   its `/api/notifications/:id/delivered` ack — the loop is now HTTP-free).
//! - [`event_allowed`] + [`in_quiet_hours`] — the tray poll loop's
//!   `notifications_allowed` (replaces its `/api/notifications/allowed` fetch).
//! - [`dismiss_banner`] — the tray `dismiss_notification` command (ported
//!   `/api/notifications/:id/dismiss`), from the dashboard banner.
//! - [`record_response`] — the tray `record_notification_response` command
//!   (the plugin's `actionPerformed` listener lands there).
//! - [`unconsumed_responses`] + [`mark_response_consumed`] — the daemon's
//!   response consumer (`src/notification_responses.rs`).
//! - [`categories`] — the tray's startup category registration AND the daemon
//!   producers, so the button sets can't drift between the two.
//!
//! # Related
//! - [`crate::settings::RuntimeSettings`] — the preference fields these read.

use crate::settings::RuntimeSettings;
use crate::SqlitePool;
use sqlx::FromRow;
use tracing::Instrument;

/// The fixed interactive-category set. One source of truth for BOTH sides of
/// the wire: the tray registers these as `UNNotificationCategory`s at startup,
/// and the daemon producers stamp the ids onto outbox rows. Action descriptors
/// are JSON `[{id,title,input?,destructive?,foreground?,inputButtonTitle?,
/// inputPlaceholder?}]` — the exact shape the notification plugin's
/// `ActionType.actions` deserializes, so registration parses these verbatim.
pub mod categories {
    /// Single \[Open\] button; the generic click-through category.
    pub const GENERIC_LINK: &str = "generic_link";
    /// Morning plan nudge: \[Open Plan\] \[Snooze 1h\].
    pub const PLAN_NUDGE: &str = "plan_nudge";
    /// Worklog drafts ready: \[Open Worklogs\] \[Snooze 1h\].
    pub const WORKLOG_READY: &str = "worklog_ready";
    /// A fault promoted to a toast: \[View\].
    pub const SYSTEM_FAULT: &str = "system_fault";
    /// Task-switch verification (flagship nudge, producer lands in PR 2):
    /// \[Yes, new task\] \[No, same task\] \[Reply…\] with inline text input.
    pub const VERIFY_SWITCH: &str = "verify_switch";

    /// Every category the tray must register at startup.
    pub const ALL: [&str; 5] = [
        GENERIC_LINK,
        PLAN_NUDGE,
        WORKLOG_READY,
        SYSTEM_FAULT,
        VERIFY_SWITCH,
    ];

    /// The category's action descriptors (JSON). `foreground: true` actions
    /// bring the app forward — used for every open/navigate button.
    pub fn actions_json(category: &str) -> Option<&'static str> {
        match category {
            GENERIC_LINK => Some(r#"[{"id":"open","title":"Open","foreground":true}]"#),
            PLAN_NUDGE => Some(
                r#"[{"id":"open","title":"Open Plan","foreground":true},{"id":"snooze","title":"Snooze 1h"}]"#,
            ),
            WORKLOG_READY => Some(
                r#"[{"id":"open","title":"Open Worklogs","foreground":true},{"id":"snooze","title":"Snooze 1h"}]"#,
            ),
            SYSTEM_FAULT => Some(r#"[{"id":"view","title":"View","foreground":true}]"#),
            VERIFY_SWITCH => Some(
                r#"[{"id":"yes","title":"Yes, new task"},{"id":"no","title":"No, same task"},{"id":"reply","title":"Reply…","input":true,"inputButtonTitle":"Send","inputPlaceholder":"What are you working on?"}]"#,
            ),
            _ => None,
        }
    }
}

/// A native notification ready to fire (the shape the tray delivers + the route
/// returns). `category`/`actions` are `None` on plain rows AND on a pre-057 DB
/// (the columns are selected only when present), so the tray falls back to a
/// plain toast in both cases.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingNotification {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub deep_link: Option<String>,
    pub severity: String,
    pub category: Option<String>,
    pub actions: Option<String>,
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
    category: Option<String>,
    actions: Option<String>,
}

fn has_channel(channels: &str, channel: &str) -> bool {
    channels.split(',').any(|c| c.trim() == channel)
}

/// Whether the interactive-notification columns (migration 057) exist. The tray
/// can run against a DB whose daemon predates 057 (overlapping installs), so
/// every read/write touching the new columns degrades gracefully instead of
/// erroring the whole queue. Errors count as "missing" — same fail-soft posture
/// as the pending read.
async fn has_action_columns(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('notifications') WHERE name = 'category'",
    )
    .fetch_one(pool)
    .await
    .map(|n| n > 0)
    .unwrap_or(false)
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
    // Pre-057 DB → NULL literals so one query shape serves both schemas.
    let action_cols = if has_action_columns(pool).await {
        "category, actions"
    } else {
        "NULL AS category, NULL AS actions"
    };
    let rows: Vec<NotifRow> = sqlx::query_as::<_, NotifRow>(&format!(
        r#"SELECT id, event_key, severity, title, body, deep_link, channels, {action_cols}
           FROM notifications
           WHERE delivered_native_at IS NULL
             AND (scheduled_for IS NULL OR scheduled_for <= ?)
             AND (expires_at IS NULL OR expires_at > ?)
           ORDER BY id ASC"#
    ))
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
            category: r.category,
            actions: r.actions,
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

// ── Response leg (interactive notifications, migration 057) ─────────────────

/// Record the user's answer to an interactive notification: stamp
/// `responded_at` + the pressed `action` (an action id, `'tap'`, or
/// `'dismiss'`) + any inline-reply `text`. First answer wins — the
/// `AND responded_at IS NULL` guard makes a duplicate (or late) answer a no-op,
/// mirroring [`mark_native_delivered`]. `now` is the caller-resolved stamp.
/// No-op on a pre-057 DB (the columns don't exist — nothing to record).
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
    .await?;
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
/// consumer just idles, same fail-soft posture as [`pending_native`].
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

    /// In-memory pool with the FULL post-057 column set the interactive reads
    /// touch (042 outbox columns + 057 response columns).
    async fn interactive_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE notifications (\
                id INTEGER PRIMARY KEY, dedup_key TEXT NOT NULL UNIQUE, event_key TEXT, \
                severity TEXT DEFAULT 'info', title TEXT, body TEXT, deep_link TEXT, \
                channels TEXT DEFAULT 'native', scheduled_for TEXT, expires_at TEXT, \
                delivered_native_at TEXT, banner_dismissed_at TEXT, \
                attempts INTEGER NOT NULL DEFAULT 0, created_at TEXT, \
                category TEXT, actions TEXT, responded_at TEXT, response_action TEXT, \
                response_text TEXT, response_consumed_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn record_response_first_answer_wins() {
        let pool = interactive_pool().await;
        sqlx::query(
            "INSERT INTO notifications (id, dedup_key, event_key, title, body, category) \
             VALUES (1, 'k1', 'plan.nudge', 't', 'b', 'plan_nudge')",
        )
        .execute(&pool)
        .await
        .unwrap();
        record_response(&pool, 1, "snooze", None, "2026-07-06T10:00:00Z")
            .await
            .unwrap();
        // A second (late) answer must be a no-op — first answer wins.
        record_response(&pool, 1, "open", Some("late"), "2026-07-06T11:00:00Z")
            .await
            .unwrap();
        let (at, action, text): (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT responded_at, response_action, response_text FROM notifications WHERE id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(at.as_deref(), Some("2026-07-06T10:00:00Z"));
        assert_eq!(action.as_deref(), Some("snooze"));
        assert_eq!(text, None);
    }

    #[tokio::test]
    async fn unconsumed_responses_drain_and_consume() {
        let pool = interactive_pool().await;
        for (id, key) in [(1, "k1"), (2, "k2")] {
            sqlx::query(
                "INSERT INTO notifications (id, dedup_key, event_key, title, body) \
                 VALUES (?, ?, 'worklog.ready', 't', 'b')",
            )
            .bind(id)
            .bind(key)
            .execute(&pool)
            .await
            .unwrap();
        }
        // Nothing answered yet → empty queue.
        assert!(unconsumed_responses(&pool).await.is_empty());
        record_response(&pool, 2, "reply", Some("hi"), "2026-07-06T10:00:00Z")
            .await
            .unwrap();
        let q = unconsumed_responses(&pool).await;
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].id, 2);
        assert_eq!(q[0].response_action, "reply");
        assert_eq!(q[0].response_text.as_deref(), Some("hi"));
        // Consume → leaves the queue; a duplicate consume is a no-op.
        mark_response_consumed(&pool, 2, "2026-07-06T10:01:00Z")
            .await
            .unwrap();
        mark_response_consumed(&pool, 2, "2026-07-06T11:00:00Z")
            .await
            .unwrap();
        assert!(unconsumed_responses(&pool).await.is_empty());
        let stamp: Option<String> =
            sqlx::query_scalar("SELECT response_consumed_at FROM notifications WHERE id = 2")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stamp.as_deref(), Some("2026-07-06T10:01:00Z"));
    }

    #[tokio::test]
    async fn pending_native_carries_category_and_survives_pre_057() {
        // Post-057 schema: category/actions ride along.
        let pool = interactive_pool().await;
        sqlx::query(
            "INSERT INTO notifications (id, dedup_key, event_key, title, body, category, actions) \
             VALUES (1, 'k1', 'plan.nudge', 't', 'b', 'plan_nudge', '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = "2026-07-06T10:00:00Z";
        let pending = pending_native(&pool, now, &settings()).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].category.as_deref(), Some("plan_nudge"));

        // Pre-057 schema (042 columns only): the read degrades to category=None
        // instead of erroring the queue empty, and the response leg no-ops.
        let old = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE notifications (\
                id INTEGER PRIMARY KEY, dedup_key TEXT NOT NULL UNIQUE, event_key TEXT, \
                severity TEXT DEFAULT 'info', title TEXT, body TEXT, deep_link TEXT, \
                channels TEXT DEFAULT 'native', scheduled_for TEXT, expires_at TEXT, \
                delivered_native_at TEXT, banner_dismissed_at TEXT, \
                attempts INTEGER NOT NULL DEFAULT 0, created_at TEXT)",
        )
        .execute(&old)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO notifications (id, dedup_key, event_key, title, body) \
             VALUES (1, 'k1', 'plan.nudge', 't', 'b')",
        )
        .execute(&old)
        .await
        .unwrap();
        let pending = pending_native(&old, now, &settings()).await;
        assert_eq!(pending.len(), 1, "pre-057 read must still deliver");
        assert_eq!(pending[0].category, None);
        record_response(&old, 1, "tap", None, now).await.unwrap(); // must not error
        assert!(unconsumed_responses(&old).await.is_empty());
    }

    #[tokio::test]
    async fn expired_unanswered_selects_only_delivered_unanswered_past_expiry() {
        let pool = interactive_pool().await;
        let now = "2026-07-06T12:00:00Z";
        // (id, delivered, responded, expires) — only id 1 qualifies.
        let rows: [(i64, Option<&str>, Option<&str>, Option<&str>); 5] = [
            (1, Some("t"), None, Some("2026-07-06T11:00:00Z")), // expired, unanswered → in
            (2, Some("t"), None, Some("2026-07-06T13:00:00Z")), // not yet expired → out
            (3, Some("t"), Some("t"), Some("2026-07-06T11:00:00Z")), // answered → out
            (4, None, None, Some("2026-07-06T11:00:00Z")),      // never delivered → out
            (5, Some("t"), None, None),                         // no expiry → out
        ];
        for (id, delivered, responded, expires) in rows {
            sqlx::query(
                "INSERT INTO notifications (id, dedup_key, event_key, title, body, \
                 delivered_native_at, responded_at, expires_at) VALUES (?, ?, 'e', 't', 'b', ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("k{id}"))
            .bind(delivered)
            .bind(responded)
            .bind(expires)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(expired_unanswered(&pool, now).await, vec![1]);
        // The 'expired' stamp (record_response) removes it from the queue.
        record_response(&pool, 1, "expired", None, now)
            .await
            .unwrap();
        assert!(expired_unanswered(&pool, now).await.is_empty());
    }

    #[test]
    fn every_category_has_valid_actions_json() {
        for cat in categories::ALL {
            let json = categories::actions_json(cat)
                .unwrap_or_else(|| panic!("category {cat} missing actions"));
            let parsed: serde_json::Value = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("category {cat} actions not valid JSON: {e}"));
            let arr = parsed.as_array().expect("actions must be a JSON array");
            assert!(!arr.is_empty(), "category {cat} has no actions");
            for a in arr {
                assert!(a.get("id").is_some() && a.get("title").is_some());
            }
        }
        assert_eq!(categories::actions_json("nope"), None);
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
