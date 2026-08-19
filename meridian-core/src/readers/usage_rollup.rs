//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Per-day **product-action** rollup — how much work Meridian actually did for
//! one user on one already-closed local day.
//!
//! # What this is (and deliberately is not)
//! Every counter here describes an action **Meridian took, or the user took
//! inside Meridian**: a worklog posted to a tracker, a plan confirmed, a day
//! task corrected, a notification delivered. None of it describes what the user
//! was *working on* — no window titles, no app names, no OCR, no ticket keys,
//! no summary text. That split is the whole point: this rollup exists to answer
//! "is the product working and being used", and it is designed so the answer
//! can be shipped to product analytics without carrying captured content along
//! with it. Adding a field that names the user's work would break that
//! property, so don't.
//!
//! The headline number is [`UsageRollup::tickets_updated`] — distinct tracker
//! tickets Meridian posted to on the day. It is the per-user, attributable
//! version of the global vanity counter the tray already pings
//! (`tray/src-tauri/src/counter_ping.rs`), which carries no payload and so can
//! never be broken down by user, provider, or day.
//!
//! # Two different day keys, on purpose
//! Meridian stores local-day-keyed tables (`daily_plan.plan_date`,
//! `day_tasks.day_local`, `day_summaries.day_local` — already 'YYYY-MM-DD'
//! local) alongside UTC-timestamp tables (`pm_worklogs.posted_at`,
//! `notifications.created_at`, `pm_tasks.updated_at`). The former are compared
//! against `day` directly; the latter against [`meridian_core::date::local_day_bounds`]
//! — the SAME bounds `analytics::daily` uses for `focus_hours`, so a counter
//! here can never disagree with the number rendered next to it.
//!
//! `posted_at` (not `day_utc`) is what scopes the ticket counters: a worklog
//! covering Monday's work but posted Tuesday is a Tuesday *action*, and it is
//! actions this module counts. The pipeline-state counters
//! ([`UsageRollup::worklogs_drafted`] and friends) come from
//! [`crate::worklogs::get_worklogs`] instead — the very reader the dashboard's
//! Today card uses — so "drafts waiting" here means exactly what the user sees.
//!
//! # Who calls this
//! `tray/src-tauri/src/analytics/daily.rs` — once per completed local day, to
//! assemble the `daily_usage` PostHog event. Nothing else; it is not a
//! dashboard reader and has no `#[tauri::command]` of its own.
//!
//! # Related
//! - [`crate::worklogs`] — the worklog pipeline-state reader reused here.
//! - [`crate::date::local_day_bounds`] — the shared local-day bound math.
//! - `tray/src-tauri/src/analytics/health.rs` — the sibling *point-in-time*
//!   health snapshot; this module is strictly per-day counters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::Instrument;

/// Notification delivery outcomes for one `event_key` on the day.
///
/// Split per event key rather than totalled so a delivery regression can be
/// attributed to the feature that produced it (a broken `plan.nudge` looks
/// nothing like a broken `system.fault`) without ever carrying the
/// notification's title or body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationOutcome {
    /// Rows enqueued on the day, whatever their channel.
    pub enqueued: i64,
    /// Native-channel rows that reached the OS (`delivered_native_at` set).
    pub delivered: i64,
    /// Native-channel rows the drain *tried* and never landed
    /// (`attempts > 0` with no `delivered_native_at`). This is the real
    /// delivery-failure signal — a row that was simply never due yet has
    /// `attempts = 0` and must not be counted as a failure.
    pub failed: i64,
    /// Banner-channel rows the user actively dismissed — the only engagement
    /// signal the outbox records.
    pub dismissed: i64,
}

/// Every product-action counter for one already-closed local day.
///
/// Field groups map 1:1 onto the questions this exists to answer: did Meridian
/// update their tickets, did they run the daily rituals, did we classify their
/// day correctly, and did our notifications actually arrive.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageRollup {
    // ── Tickets: what Meridian wrote back to the user's tracker ──────────────
    /// Distinct tracker tickets posted to on the day. The headline metric.
    pub tickets_updated: i64,
    /// Individual worklog posts that succeeded on the day (≥ `tickets_updated`,
    /// since one ticket can take several posts across the day).
    pub worklogs_posted: i64,
    /// Successful posts broken down by tracker provider (`jira`, `linear`, …).
    pub posts_by_provider: BTreeMap<String, i64>,
    /// Worklogs carrying a `last_post_error` — posting was attempted and
    /// failed. The counterpart to `worklogs_posted` and a direct "is the
    /// integration healthy" signal.
    pub post_failures: i64,

    // ── Worklog pipeline state, as the dashboard shows it ────────────────────
    /// Real worklogs still sitting in `drafted` at day close.
    pub worklogs_drafted: i64,
    /// Real worklogs the user approved.
    pub worklogs_approved: i64,
    /// Proposed (not-yet-real) tickets still awaiting a decision.
    pub proposals_pending: i64,

    // ── Daily plan ritual ────────────────────────────────────────────────────
    /// The user confirmed the day's plan.
    pub plan_confirmed: bool,
    /// The user explicitly dismissed the plan ritual (distinct from never
    /// having seen it — both `plan_confirmed` and this are false then).
    pub plan_skipped: bool,
    /// Tasks committed to the day's plan.
    pub plan_items: i64,
    /// Plan items by how they got there (`manual`, `carryover`, `in_progress`,
    /// `recent`, `due_soon`) — shows whether our suggestions are being taken
    /// or overridden.
    pub plan_items_by_origin: BTreeMap<String, i64>,

    // ── Day tasks + the correction signal ────────────────────────────────────
    /// Day tasks Meridian folded for the day.
    pub day_tasks: i64,
    /// Corrections the user applied to those tasks, by action (`dismissed`,
    /// `merged`). The best available *quality* signal: a day with many
    /// corrections is a day we classified badly.
    pub day_task_corrections: BTreeMap<String, i64>,
    /// A day summary was generated for the day.
    pub day_summary_generated: bool,

    // ── Personal tasks + coding agents ───────────────────────────────────────
    /// Personal (`provider = 'local'`) tasks created or edited on the day.
    pub personal_tasks_touched: i64,
    /// Coding-agent segments that reached `summarised` on the day — proves the
    /// ingest → seal → summarise pipeline ran end to end.
    pub agent_sessions_summarised: i64,

    // ── Notifications ────────────────────────────────────────────────────────
    /// Delivery outcomes keyed by `event_key`.
    pub notifications: BTreeMap<String, NotificationOutcome>,
}

impl UsageRollup {
    /// Total notifications enqueued across all event keys.
    pub fn notifications_enqueued(&self) -> i64 {
        self.notifications.values().map(|o| o.enqueued).sum()
    }

    /// Total native notifications delivered across all event keys.
    pub fn notifications_delivered(&self) -> i64 {
        self.notifications.values().map(|o| o.delivered).sum()
    }

    /// Total native notifications that were attempted and never landed.
    pub fn notifications_failed(&self) -> i64 {
        self.notifications.values().map(|o| o.failed).sum()
    }
}

/// Read every product-action counter for `day` (a local `'YYYY-MM-DD'` that has
/// already closed).
///
/// Returns `Err` only when a query genuinely fails — never a partially-zeroed
/// rollup, because the caller advances its "already reported" cursor on
/// success and a fabricated zero would be permanently mistaken for a genuinely
/// idle day.
#[tracing::instrument(skip(pool))]
pub async fn usage_rollup(pool: &SqlitePool, day: &str) -> anyhow::Result<UsageRollup> {
    let (day_start, day_end) = crate::date::local_day_bounds(day);
    let mut r = UsageRollup::default();

    read_ticket_actions(pool, &day_start, &day_end, &mut r).await?;
    read_worklog_states(pool, day, &mut r).await?;
    read_plan(pool, day, &mut r).await?;
    read_day_tasks(pool, day, &mut r).await?;
    read_personal_and_agents(pool, day, &day_start, &day_end, &mut r).await?;
    read_notifications(pool, &day_start, &day_end, &mut r).await?;

    tracing::info!(
        day,
        tickets_updated = r.tickets_updated,
        worklogs_posted = r.worklogs_posted,
        post_failures = r.post_failures,
        plan_items = r.plan_items,
        day_tasks = r.day_tasks,
        notifications_delivered = r.notifications_delivered(),
        "usage_rollup: assembled"
    );
    Ok(r)
}

/// Posts that actually landed on a tracker, scoped by `posted_at` — see the
/// module doc on why that column and not `day_utc`.
async fn read_ticket_actions(
    pool: &SqlitePool,
    day_start: &str,
    day_end: &str,
    r: &mut UsageRollup,
) -> anyhow::Result<()> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT COALESCE(provider, 'jira') AS provider,
                COUNT(*) AS posts,
                COUNT(DISTINCT task_key) AS tickets
         FROM pm_worklogs
         WHERE posted_at IS NOT NULL AND posted_at >= ? AND posted_at <= ?
         GROUP BY provider",
    )
    .bind(day_start)
    .bind(day_end)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("usage_rollup.read.pm_worklogs_posted"))
    .await?;
    tracing::debug!(
        rows = rows.len(),
        "usage_rollup: posted worklogs by provider"
    );

    for (provider, posts, tickets) in rows {
        r.worklogs_posted += posts;
        // `pm_tasks.task_key` is UNIQUE, so a task_key belongs to exactly one
        // provider and summing the per-provider distinct counts cannot
        // double-count a ticket.
        r.tickets_updated += tickets;
        *r.posts_by_provider.entry(provider).or_insert(0) += posts;
    }

    // Attempted-and-failed posts. Scoped by created_at rather than posted_at,
    // which is NULL precisely because the post never succeeded.
    r.post_failures = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pm_worklogs
         WHERE last_post_error IS NOT NULL AND last_post_error <> ''
           AND created_at >= ? AND created_at <= ?",
    )
    .bind(day_start)
    .bind(day_end)
    .fetch_one(pool)
    .instrument(tracing::debug_span!("usage_rollup.read.pm_worklogs_failed"))
    .await?;

    Ok(())
}

/// Pipeline state via the dashboard's own reader, so "drafts waiting" here
/// means exactly what the user saw on the Today card.
async fn read_worklog_states(
    pool: &SqlitePool,
    day: &str,
    r: &mut UsageRollup,
) -> anyhow::Result<()> {
    let w = crate::worklogs::get_worklogs(pool, day).await?;
    for item in &w.items {
        match (item.is_proposed, item.state.as_str()) {
            (true, "proposed") => r.proposals_pending += 1,
            (false, "drafted") => r.worklogs_drafted += 1,
            (false, "approved") => r.worklogs_approved += 1,
            _ => {}
        }
    }
    tracing::debug!(rows = w.items.len(), "usage_rollup: worklog states");
    Ok(())
}

/// The daily-plan ritual: whether it was acted on, and what went into it.
async fn read_plan(pool: &SqlitePool, day: &str, r: &mut UsageRollup) -> anyhow::Result<()> {
    let meta: Option<(Option<String>, i64)> =
        sqlx::query_as("SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?")
            .bind(day)
            .fetch_optional(pool)
            .instrument(tracing::debug_span!("usage_rollup.read.daily_plan_meta"))
            .await?;
    if let Some((confirmed_at, skipped)) = meta {
        r.plan_confirmed = confirmed_at.is_some();
        r.plan_skipped = skipped != 0;
    }

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT origin, COUNT(*) FROM daily_plan WHERE plan_date = ? GROUP BY origin",
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("usage_rollup.read.daily_plan"))
    .await?;
    tracing::debug!(rows = rows.len(), "usage_rollup: plan items by origin");
    for (origin, n) in rows {
        r.plan_items += n;
        *r.plan_items_by_origin.entry(origin).or_insert(0) += n;
    }
    Ok(())
}

/// Day tasks, the corrections applied to them, and whether a summary was made.
async fn read_day_tasks(pool: &SqlitePool, day: &str, r: &mut UsageRollup) -> anyhow::Result<()> {
    r.day_tasks = sqlx::query_scalar("SELECT COUNT(*) FROM day_tasks WHERE day_local = ?")
        .bind(day)
        .fetch_one(pool)
        .instrument(tracing::debug_span!("usage_rollup.read.day_tasks"))
        .await?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action, COUNT(*) FROM day_task_corrections WHERE day_local = ? GROUP BY action",
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!(
        "usage_rollup.read.day_task_corrections"
    ))
    .await?;
    tracing::debug!(rows = rows.len(), "usage_rollup: day task corrections");
    for (action, n) in rows {
        r.day_task_corrections.insert(action, n);
    }

    let summaries: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM day_summaries WHERE day_local = ?")
            .bind(day)
            .fetch_one(pool)
            .instrument(tracing::debug_span!("usage_rollup.read.day_summaries"))
            .await?;
    r.day_summary_generated = summaries > 0;
    Ok(())
}

/// Personal-task edits and coding-agent summarisation — both UTC-timestamped,
/// so both use the day bounds.
async fn read_personal_and_agents(
    pool: &SqlitePool,
    day: &str,
    day_start: &str,
    day_end: &str,
    r: &mut UsageRollup,
) -> anyhow::Result<()> {
    r.personal_tasks_touched = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pm_tasks
         WHERE provider = 'local' AND updated_at >= ? AND updated_at <= ?",
    )
    .bind(day_start)
    .bind(day_end)
    .fetch_one(pool)
    .instrument(tracing::debug_span!("usage_rollup.read.pm_tasks_local"))
    .await?;

    // `started_at` is the segment's own clock, so a segment summarised later
    // still counts against the day it happened on — matching how every other
    // session-derived number in the product is attributed.
    r.agent_sessions_summarised = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_sessions
         WHERE task_method = 'summarised' AND started_at >= ? AND started_at <= ?",
    )
    .bind(day_start)
    .bind(day_end)
    .fetch_one(pool)
    .instrument(tracing::debug_span!(
        "usage_rollup.read.app_sessions_summarised"
    ))
    .await?;

    tracing::debug!(
        day,
        personal = r.personal_tasks_touched,
        agents = r.agent_sessions_summarised,
        "usage_rollup: personal tasks and agent sessions"
    );
    Ok(())
}

/// Notification delivery outcomes per `event_key`.
///
/// The native counters are restricted to rows whose `channels` actually
/// include `native`: a banner-only row has `delivered_native_at` NULL forever
/// by design, and counting it as undelivered would invent a permanent
/// delivery failure that never existed.
async fn read_notifications(
    pool: &SqlitePool,
    day_start: &str,
    day_end: &str,
    r: &mut UsageRollup,
) -> anyhow::Result<()> {
    let rows: Vec<(String, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT event_key,
                COUNT(*),
                SUM(CASE WHEN channels LIKE '%native%'
                          AND delivered_native_at IS NOT NULL THEN 1 ELSE 0 END),
                SUM(CASE WHEN channels LIKE '%native%'
                          AND delivered_native_at IS NULL
                          AND attempts > 0 THEN 1 ELSE 0 END),
                SUM(CASE WHEN banner_dismissed_at IS NOT NULL THEN 1 ELSE 0 END)
         FROM notifications
         WHERE created_at >= ? AND created_at <= ?
         GROUP BY event_key",
    )
    .bind(day_start)
    .bind(day_end)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("usage_rollup.read.notifications"))
    .await?;
    tracing::debug!(rows = rows.len(), "usage_rollup: notification outcomes");

    for (event_key, enqueued, delivered, failed, dismissed) in rows {
        r.notifications.insert(
            event_key,
            NotificationOutcome {
                enqueued,
                delivered,
                failed,
                dismissed,
            },
        );
    }
    Ok(())
}
