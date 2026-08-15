//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notification delivery policy + the native-channel queue (read + delivery
//! writes) — the consumer half of the outbox, ported from `ui/lib/notifications.ts`.
//!
//! The Rust daemon (`src/notifications.rs`) ENQUEUES into the `notifications`
//! table; this module is the single place the *delivery* decision lives:
//! master switch ([`event_allowed`]) and quiet hours ([`in_quiet_hours`]) —
//! deliberately the ONLY two knobs, no per-category toggles. Producers always
//! enqueue; only the user's settings decide whether an event actually
//! surfaces. The two delivery writes
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
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tracing::Instrument;

// Interactive-notification response leg (migration 057) lives in `responses.rs`
// to keep each file under the repo's 500-line cap; re-exported so the public
// path stays `meridian_core::notifications::*` (callers are unchanged).
mod responses;
pub use responses::{
    expired_unanswered, mark_response_consumed, notification_deep_link, record_response,
    unconsumed_responses, NotificationResponse,
};

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

/// The fixed `deep_link` vocabulary — every value a producer may put on a
/// notification or notice, and nothing else.
///
/// These are NOT URLs. The dashboard is a static export whose only real routes
/// are `/`, `/setup` and `/uninstall`; a deep link is a *navigation intent*
/// the shell resolves to a modal or a settings section
/// (`MeridianTimelineShell.tsx`'s `navigate`). The path-like spelling is
/// historical — they are the former Next route paths, kept because rows
/// carrying them are already in users' outboxes.
///
/// Why this module exists: `navigate` used to handle three values with no
/// `else` arm, while producers emitted seven. The other four — including the
/// `[View]` button on every fault toast — silently opened the dashboard's
/// default view instead of the thing the notification was about, and
/// `/tasks?integrations=1` went on being emitted for months after the `/tasks`
/// route was deleted. Nothing failed, because nothing checked. [`ALL`] is what
/// the two guard tests check against: `deep_link_literals_are_all_known`
/// (Rust, every literal in a producer is listed here) and
/// `deep-links.test.ts` (bun, `navigate` handles every entry here).
///
/// **Adding one means updating `navigate` in the same change** — the bun guard
/// fails otherwise. Legacy spellings stay handled by `navigate` forever (old
/// rows never rewrite themselves) but must not be used by new producers, which
/// is why they live in [`LEGACY`] rather than here.
pub mod deep_links {
    /// The daily planner.
    pub const PLAN: &str = "/plan";
    /// Worklog review.
    pub const WORKLOGS: &str = "/worklogs";
    /// The What's New modal.
    pub const WHATS_NEW: &str = "/whats-new";
    /// The dashboard's own default view (dismisses any open modal).
    pub const TODAY: &str = "/today";
    /// Settings, at its default section.
    pub const SETTINGS: &str = "/settings";
    /// Settings → Integrations — where a tracker is (re)connected.
    pub const INTEGRATIONS: &str = "/settings/integrations";
    /// Settings → Account, which holds the Support ID and Export Diagnostics.
    /// This is where a fault sends the user: there is no in-app log viewer, so
    /// the actionable response to "something broke" is exporting diagnostics
    /// and quoting the Support ID. The `/logs` spelling predates that being
    /// true and is kept only because outbox rows carry it.
    pub const LOGS: &str = "/logs";
    /// The end-of-day summary, for TODAY. There is no day parameter in this
    /// vocabulary, so the shell's arm resets the viewed day rather than opening
    /// the summary over whatever day the dashboard happened to be showing. The
    /// only producer (`day_summary.ready`) fires for the current local day and
    /// expires at local midnight, so "today" is always the day it meant.
    pub const SUMMARY: &str = "/summary";

    /// Every link a producer may emit.
    pub const ALL: [&str; 8] = [
        PLAN,
        WORKLOGS,
        WHATS_NEW,
        TODAY,
        SETTINGS,
        INTEGRATIONS,
        LOGS,
        SUMMARY,
    ];

    /// Spellings retired from the producer side but still resolvable, because
    /// undelivered rows and older installs carry them. `navigate` must keep
    /// handling these; new code must not emit them.
    ///
    /// - `/tasks?integrations=1` — the pre-fold Next route for the tracker
    ///   panel, emitted by every `pm.*` sync fault until it moved to
    ///   [`INTEGRATIONS`]. The `/tasks` route itself no longer exists.
    /// - `/health` — an early daemon-health target; observed on rows in the
    ///   wild, no longer emitted anywhere.
    pub const LEGACY: [&str; 2] = ["/tasks?integrations=1", "/health"];

    /// Whether `link` is a value the shell is expected to resolve — current
    /// vocabulary or legacy.
    ///
    /// Used by `tests/deep_links.rs`. Deliberately NOT called by producers: a
    /// runtime check would fire on the user's machine long after the mistake
    /// was made, whereas the static scan fails the build. If a producer ever
    /// takes a caller-supplied link (none does today), this is the gate for it.
    pub fn is_known(link: &str) -> bool {
        ALL.contains(&link) || LEGACY.contains(&link)
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

/// Whether an event may surface at all: the master switch, full stop. Every
/// event type is treated the same — there is no per-category toggle.
pub fn event_allowed(s: &RuntimeSettings) -> bool {
    s.notifications_enabled
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

/// Pools already observed to carry the 057 interactive columns, keyed by pool
/// handle address. The columns are monotonic — a migration only ever ADDS them
/// and this process never drops them — so a `true` observation is permanent.
fn action_columns_cache() -> &'static Mutex<HashSet<usize>> {
    static CACHE: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether the interactive-notification columns (migration 057) exist. The tray
/// can run against a DB whose daemon predates 057 (overlapping installs), so
/// every read/write touching the new columns degrades gracefully instead of
/// erroring the whole queue. Errors count as "missing" — same fail-soft posture
/// as the pending read.
///
/// The result is cached per pool so the steady state skips the pragma round trip
/// every read/write otherwise pays on each tick, forever. Only a `true` is
/// latched: columns are monotonic (see [`action_columns_cache`]), and a `false`
/// covers both a genuine pre-057 schema AND a transient probe error, so caching
/// it would permanently disable the response leg on one bad read. Keyed
/// per-pool, not a process-global flag, because one process can legitimately
/// hold both a post-057 and a pre-057 pool.
async fn has_action_columns(pool: &SqlitePool) -> bool {
    let key = pool as *const SqlitePool as usize;
    if action_columns_cache().lock().unwrap().contains(&key) {
        return true;
    }
    let present = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('notifications') WHERE name = 'category'",
    )
    .fetch_one(pool)
    .await
    .map(|n| n > 0)
    .unwrap_or(false);
    if present {
        action_columns_cache().lock().unwrap().insert(key);
    }
    present
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
        r#"SELECT id, severity, title, body, deep_link, channels, {action_cols}
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
        .filter(|r| has_channel(&r.channels, "native") && event_allowed(s))
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
        .filter(|r| has_channel(&r.channels, "banner") && event_allowed(s))
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
mod tests;
