//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The poll loop's per-tick refreshers — each pulls one slice of state and
//! writes it into the shared [`AppState`].
//!
//! All reads are now direct DB reads through [`meridian_core`] (the same readers
//! the dashboard commands call) — the tray no longer round-trips the Next server
//! over HTTP, so it keeps working after the export cutover removes that server.
//!
//! # Related
//! - [`super`] — the loop that schedules these and the tray-sync that follows.
//! - [`super::notifications_allowed`] — the quiet-hours gate `refresh_health` consults.
//! - [`crate::commands::health::check_health`] — the direct health check.

use crate::commands::health::check_health;
use crate::state::{ActiveSession, AppState, HealthStatus, TodayBreakdown};
use crate::sys::notify;
use meridian_core::SqlitePool;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// Run the local health check, fold it into [`AppState`], and fire the
/// went-quiet / back-online toasts (debounced to the 2nd consecutive failure).
pub(super) async fn refresh_health(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let hr = check_health().await;

    // Push the health detail to the dashboard webview (the ported
    // `/api/health/stream`). HealthResponse is a superset of the route's payload
    // (it also carries `daemon_running`, which the banner ignores).
    let _ = app.emit("health-update", &hr);

    // db_ready and daemon_running both default true when absent (older schema compat).
    let db_ready = hr.database_ready.unwrap_or(false);
    let daemon_running = hr.daemon_running.unwrap_or(true);

    let new_health = if db_ready && daemon_running {
        HealthStatus::Healthy
    } else {
        HealthStatus::Unhealthy
    };

    let (notify_down, notify_back) = {
        let mut s = state.lock().unwrap();
        let now_healthy = new_health == HealthStatus::Healthy;

        let notify_down = if !now_healthy {
            s.consecutive_health_failures += 1;
            // Notify only on the second consecutive failure — one miss is a transient blip.
            s.consecutive_health_failures == 2 && s.daemon_was_healthy
        } else {
            false
        };
        // Fire "back online" only when we had previously sent a "gone quiet" notification
        // (consecutive_health_failures reached 2), so a brief outage during startup is silent.
        let notify_back = now_healthy && s.consecutive_health_failures >= 2;

        if now_healthy {
            s.consecutive_health_failures = 0;
            s.daemon_was_healthy = true;
        }
        s.ui_reachable = true; // health checks are now direct (no HTTP); always reachable
        s.health = new_health;

        (notify_down, notify_back)
    };

    if notify_down && super::notifications_allowed("system.health").await {
        notify(app, "Meridian went quiet.", "Tap to check what happened.");
    } else if notify_back && super::notifications_allowed("system.health").await {
        notify(app, "Back online.", "Picking up where you left off.");
    }
}

/// Read the active session (direct DB) and store the app name + elapsed seconds.
/// On a read error we keep the previous value rather than clearing the pill on a
/// transient blip.
pub(super) async fn refresh_active(pool: &SqlitePool, state: &Arc<Mutex<AppState>>) {
    let now = chrono::Utc::now().to_rfc3339();
    let session = match meridian_core::active::get_active_view(pool, &now).await {
        Ok(Some(v)) => Some(ActiveSession {
            app_name: v.app_name,
            elapsed_s: v.elapsed_s.max(0) as u64,
            title: top_title(&v.window_titles),
            category: v.category,
            confidence: v.confidence,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "refresh_active failed");
            return;
        }
    };
    let mut s = state.lock().unwrap();
    // Stamp the refresh time only while a session is live, so the tray-title
    // ticker can extrapolate the running timer between polls.
    s.active_set_at = session.as_ref().map(|_| std::time::Instant::now());
    s.active_session = session;
}

/// Resolve the menu-bar pill's "current task" (most recently classified task
/// today) and its progress-ring fill, storing both in [`AppState`]. On a read
/// error we keep the previous value rather than blanking the pill on a blip.
pub(super) async fn refresh_current_task(pool: &SqlitePool, state: &Arc<Mutex<AppState>>) {
    let today = meridian_core::date::today_string();
    match meridian_core::current_task::get_current_task(pool, &today).await {
        Ok(ct) => {
            let mut s = state.lock().unwrap();
            s.current_task_key = ct.as_ref().map(|c| c.key.clone());
            s.task_percent = ct.and_then(|c| c.percent);
        }
        Err(e) => tracing::warn!(error = %e, "refresh_current_task failed"),
    }
}

/// First foreground window title from the active session's `window_titles` JSON.
/// Tolerates both shapes the column has carried — `["title", …]` and
/// `[{"title": "…", "count": n}, …]` — and drops empties.
fn top_title(titles: &serde_json::Value) -> Option<String> {
    titles
        .as_array()?
        .first()
        .and_then(|e| {
            e.as_str()
                .map(str::to_string)
                .or_else(|| e.get("title").and_then(|t| t.as_str()).map(str::to_string))
        })
        .filter(|s| !s.is_empty())
}

/// Read today's totals into [`AppState`]: the headline focus seconds + switch
/// count, plus the per-category split (Coding / Review / Comms) and autonomous
/// agent time that drive the popover's Time Tracker tiles. The split sums the
/// closed sessions and folds in the live one, so it tracks `focus_s`.
pub(super) async fn refresh_today(pool: &SqlitePool, state: &Arc<Mutex<AppState>>) {
    let date = meridian_core::date::today_string();
    let now = chrono::Utc::now().to_rfc3339();
    match meridian_core::today::get_today(pool, &date, &now).await {
        Ok(t) => {
            let mut bd = TodayBreakdown {
                autonomous_s: t.autonomous_s.max(0) as u64,
                ..TodayBreakdown::default()
            };
            let mut add = |cat: &str, dur: i64| {
                let d = dur.max(0) as u64;
                match cat {
                    "coding" => bd.coding_s += d,
                    "code_review" => bd.review_s += d,
                    "communication" => bd.comms_s += d,
                    _ => {}
                }
            };
            for sess in &t.sessions {
                add(&sess.cat, sess.dur);
            }
            if let Some(a) = &t.active {
                add(&a.cat, a.elapsed_s);
            }
            let mut s = state.lock().unwrap();
            s.focus_s = t.focus_s.max(0) as u64;
            s.switch_count = t.switch_count.max(0) as u32;
            s.today = bd;
        }
        Err(e) => tracing::warn!(error = %e, "refresh_today failed"),
    }
}

/// Track the drafted-worklog count for the tray tooltip/badge only (direct DB).
/// The "worklog ready" notification itself is emitted by the daemon's worklog
/// scheduler into the notification outbox and delivered via `drain_notifications`
/// — not here.
pub(super) async fn refresh_worklogs(pool: &SqlitePool, state: &Arc<Mutex<AppState>>) {
    let today = meridian_core::date::today_string();
    match meridian_core::worklogs::get_worklogs(pool, &today).await {
        Ok(w) => {
            let count = w.items.iter().filter(|i| i.state == "drafted").count() as u32;
            state.lock().unwrap().drafts_count = count;
        }
        Err(e) => tracing::warn!(error = %e, "refresh_worklogs failed"),
    }
}
