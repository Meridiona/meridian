//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Assembly of the `daily_usage` event — one per completed local calendar day.
//!
//! # What the event carries
//! Three groups, each from its own source, all describing the SAME closed day:
//!
//! 1. **Engagement hours** (`focus_hours`, `coding_hours`, `logged_hours`,
//!    `drafts_count`) — the four numbers on the dashboard's Today card,
//!    unchanged since the event was introduced, read through the very same
//!    readers the card uses so they cannot drift apart.
//! 2. **Product actions** ([`meridian_core::usage_rollup`]) — tickets updated,
//!    posts by provider, plan ritual, day-task corrections, notification
//!    delivery. Counts of what Meridian DID; no captured content.
//! 3. **Install health** ([`super::health`]) — the positive "this install is
//!    fine" signal that WARN+-only error telemetry structurally cannot provide.
//!
//! # Failure semantics (load-bearing — do not soften)
//! Returns `false` on ANY read or send failure, and the caller then leaves its
//! `last_sent_day_by_email` cursor untouched so the exact same day is retried
//! next tick. This module must therefore **never substitute a zero for a read
//! that failed**: a fabricated zero would be sent, the cursor would advance
//! past it, and that day would be permanently recorded as idle. A genuinely
//! idle day's zeros come only from reads that succeeded.
//!
//! # Who calls this
//! [`super::maybe_send_daily_tick`], on the first poll tick after a local-day
//! boundary.
//!
//! # Related
//! - [`meridian_core::usage_rollup`] — the per-day product-action counters.
//! - [`super::health`] — the point-in-time health snapshot.

use meridian_core::SqlitePool;

/// Build + send the `daily_usage` event for an already-completed local day.
///
/// Queried at the day's own closing instant (its local-day upper bound) so a
/// past day's still-open active session cannot leak into the totals.
///
/// Returns `false` — without sending anything — on a DB read failure, and
/// `false` when the HTTP POST itself fails. Either way the caller must not
/// advance its cursor; see the module doc.
pub(super) async fn send_daily_usage(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    email: &str,
    device_id: &str,
    day: &str,
) -> bool {
    let (_, day_end) = meridian_core::date::local_day_bounds(day);
    let today = match meridian_core::today::get_today(pool, day, &day_end).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: today read failed for daily_usage — will retry");
            return false;
        }
    };
    // Mirrors OverviewPanel.tsx's "Focus" tile exactly (focus + autonomous
    // agent time), not the raw `focus_s` field.
    let focus_s = today.engaged_s;
    // Mirrors TimeByCategory.tsx's coding-category fold: session time
    // classified `cat == "coding"` plus coding-agent tool time (`agent_s`).
    let coding_s: i64 = today
        .sessions
        .iter()
        .filter(|s| s.cat == "coding")
        .map(|s| s.dur)
        .sum::<i64>()
        + today.agent_s;

    let worklogs = match meridian_core::worklogs::get_worklogs(pool, day).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: worklogs read failed for daily_usage — will retry");
            return false;
        }
    };
    // Mirrors OverviewPanel.tsx's "Logged" tile: real (non-proposed) worklogs
    // that are approved or posted, summed by their actual time spent.
    let logged_s: i64 = worklogs
        .items
        .iter()
        .filter(|i| !i.is_proposed && (i.state == "approved" || i.state == "posted"))
        .map(|i| i.time_spent_seconds)
        .sum();
    // Mirrors OverviewPanel.tsx's "Drafts" tile (`isPending`, types.ts):
    // proposed tickets still `proposed`, or real worklogs still `drafted`.
    let drafts_count = worklogs
        .items
        .iter()
        .filter(|i| {
            if i.is_proposed {
                i.state == "proposed"
            } else {
                i.state == "drafted"
            }
        })
        .count();

    // The product-action counters. A failure here is a real read failure, so
    // it retries rather than shipping a partially-zeroed day.
    let rollup = match meridian_core::usage_rollup::usage_rollup(pool, day).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: usage rollup failed for daily_usage — will retry");
            return false;
        }
    };

    let hours = |s: i64| (s.max(0) as f64 / 3600.0 * 100.0).round() / 100.0;
    let mut props = super::base_properties(app, device_id);
    props.insert(
        "date".to_string(),
        serde_json::Value::String(day.to_string()),
    );

    // ── Group 1: engagement hours (unchanged) ────────────────────────────────
    props.insert("focus_hours".to_string(), serde_json::json!(hours(focus_s)));
    props.insert(
        "coding_hours".to_string(),
        serde_json::json!(hours(coding_s)),
    );
    props.insert(
        "logged_hours".to_string(),
        serde_json::json!(hours(logged_s)),
    );
    props.insert("drafts_count".to_string(), serde_json::json!(drafts_count));

    // ── Group 2: product actions ─────────────────────────────────────────────
    write_rollup_properties(&rollup, &mut props);

    // ── Group 3: install health ──────────────────────────────────────────────
    super::health::snapshot(pool)
        .await
        .write_properties(&mut props);

    tracing::info!(
        day,
        focus_s,
        coding_s,
        logged_s,
        drafts_count,
        tickets_updated = rollup.tickets_updated,
        "analytics: sending daily_usage"
    );
    super::capture("daily_usage", email, props).await
}

/// Fold [`meridian_core::usage_rollup::UsageRollup`] into the event properties.
///
/// Split out so the mapping is readable and unit-testable without a live DB.
/// Map-valued counters (`posts_by_provider`, notification outcomes) ship as
/// nested JSON objects rather than being flattened into one key per provider,
/// so a new tracker never silently needs a schema change here.
pub(super) fn write_rollup_properties(
    r: &meridian_core::usage_rollup::UsageRollup,
    props: &mut serde_json::Map<String, serde_json::Value>,
) {
    // Tickets — the headline. `tickets_updated` is the per-user, attributable
    // version of the public counter in `crate::counter_ping`.
    props.insert(
        "tickets_updated".to_string(),
        serde_json::json!(r.tickets_updated),
    );
    props.insert(
        "worklogs_posted".to_string(),
        serde_json::json!(r.worklogs_posted),
    );
    props.insert(
        "posts_by_provider".to_string(),
        serde_json::json!(r.posts_by_provider),
    );
    // Named for state, not action — see `UsageRollup::worklogs_with_post_error`.
    // It counts worklogs CREATED on this day that are currently stuck, which is
    // not the same population as "posts that failed today", and the key says so
    // because nobody reading a chart will go check the module doc.
    props.insert(
        "worklogs_with_post_error".to_string(),
        serde_json::json!(r.worklogs_with_post_error),
    );
    props.insert(
        "worklogs_drafted".to_string(),
        serde_json::json!(r.worklogs_drafted),
    );
    props.insert(
        "worklogs_approved".to_string(),
        serde_json::json!(r.worklogs_approved),
    );
    props.insert(
        "proposals_pending".to_string(),
        serde_json::json!(r.proposals_pending),
    );

    // Daily plan ritual. `plan_state` carries the distinction the booleans
    // cannot: "never saw it" vs "saw it and did neither" are both false/false.
    props.insert("plan_state".to_string(), serde_json::json!(r.plan_state));
    props.insert(
        "plan_confirmed".to_string(),
        serde_json::json!(r.plan_confirmed),
    );
    props.insert(
        "plan_skipped".to_string(),
        serde_json::json!(r.plan_skipped),
    );
    props.insert("plan_items".to_string(), serde_json::json!(r.plan_items));
    props.insert(
        "plan_items_by_origin".to_string(),
        serde_json::json!(r.plan_items_by_origin),
    );

    // Day tasks + the correction (quality) signal.
    props.insert("day_tasks".to_string(), serde_json::json!(r.day_tasks));
    props.insert(
        "day_task_corrections".to_string(),
        serde_json::json!(r.day_task_corrections),
    );
    props.insert(
        "day_summary_generated".to_string(),
        serde_json::json!(r.day_summary_generated),
    );

    // Personal tasks + coding agents.
    props.insert(
        "personal_tasks_touched".to_string(),
        serde_json::json!(r.personal_tasks_touched),
    );
    props.insert(
        "agent_sessions_summarised".to_string(),
        serde_json::json!(r.agent_sessions_summarised),
    );

    // Notifications: per-event detail plus flat totals, because the totals are
    // what a trend chart needs and digging them out of a nested object in
    // HogQL is needless friction.
    props.insert(
        "notifications_enqueued".to_string(),
        serde_json::json!(r.notifications_enqueued()),
    );
    props.insert(
        "notifications_delivered".to_string(),
        serde_json::json!(r.notifications_delivered()),
    );
    props.insert(
        "notifications_failed".to_string(),
        serde_json::json!(r.notifications_failed()),
    );
    props.insert(
        "notifications_by_event".to_string(),
        serde_json::json!(r.notifications),
    );
}

#[cfg(test)]
mod tests {
    use meridian_core::usage_rollup::{NotificationOutcome, UsageRollup};

    /// The headline ticket number and the notification totals must be present
    /// as flat, top-level properties — a PostHog trend or a HogQL fleet query
    /// reads these directly, and burying them inside a nested object would
    /// make the whole event awkward to chart.
    #[test]
    fn headline_counters_are_flat_top_level_properties() {
        let mut r = UsageRollup {
            tickets_updated: 3,
            worklogs_posted: 5,
            ..Default::default()
        };
        r.notifications.insert(
            "plan.nudge".to_string(),
            NotificationOutcome {
                enqueued: 2,
                delivered: 1,
                failed: 1,
                dismissed: 0,
            },
        );

        let mut props = serde_json::Map::new();
        super::write_rollup_properties(&r, &mut props);

        assert_eq!(props.get("tickets_updated"), Some(&serde_json::json!(3)));
        assert_eq!(props.get("worklogs_posted"), Some(&serde_json::json!(5)));
        assert_eq!(
            props.get("notifications_delivered"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            props.get("notifications_failed"),
            Some(&serde_json::json!(1))
        );
        // …and the per-event breakdown still rides along for drill-down.
        assert!(props.contains_key("notifications_by_event"));
    }

    /// A genuinely idle day must still emit every counter as an explicit zero.
    /// Omitting them would make "idle" and "this build doesn't send that
    /// property yet" the same thing in PostHog, which silently breaks any
    /// average or funnel computed over the fleet.
    #[test]
    fn an_idle_day_emits_explicit_zeros() {
        let mut props = serde_json::Map::new();
        super::write_rollup_properties(&UsageRollup::default(), &mut props);

        for key in [
            "tickets_updated",
            "worklogs_posted",
            "worklogs_with_post_error",
            "plan_items",
            "day_tasks",
            "notifications_delivered",
        ] {
            assert_eq!(
                props.get(key),
                Some(&serde_json::json!(0)),
                "{key} must ship as an explicit 0 on an idle day"
            );
        }
        assert_eq!(
            props.get("plan_confirmed"),
            Some(&serde_json::json!(false)),
            "booleans too - absent must never stand in for false"
        );
    }
}
