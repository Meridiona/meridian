//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray's background poll loop.
//!
//! Every 30 s tick refreshes a slice of [`AppState`] (active session each tick;
//! health + today every 2nd; worklog drafts every 10th), drains the daemon's
//! notification outbox, pushes the live notice/banner sets to the dashboard
//! webview, then syncs the tray (event emit + tooltip + menu).
//!
//! - [`refresh`] — the per-tick fetch-and-store functions (also emits
//!   `health-update`, the ported `/api/health/stream`).
//! - [`notifications`] — the daemon's outbox drain (own policy check happens
//!   server-side per row).
//! - [`live`] — the live data → Tauri events that replace the dashboard's SSE
//!   streams: `notices-update`, `notifications-update`.
//!
//! The tray-sync helpers (emit / tooltip / menu) stay here, coupled to the loop.

mod live;
mod notifications;
mod permissions;
mod plan_auto_open;
mod refresh;
mod whats_new_auto_open;

use crate::state::{AppState, HealthStatus, PauseSource};
use chrono::{Datelike, Local, Timelike};
use notifications::drain_notifications;
use refresh::{
    refresh_active, refresh_current_task, refresh_health, refresh_today, refresh_worklogs,
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tracing::Instrument;

const TICK: Duration = Duration::from_secs(30);

/// How often the daemon watchdog probes the IPC endpoint. Much tighter than the
/// 60 s health cadence in [`run_poll_loop`] because recovery latency, unlike the
/// popover's status readout, is felt directly — a dead daemon means no capture.
const WATCHDOG_TICK: Duration = Duration::from_secs(5);
/// Consecutive missed probes before the watchdog treats the daemon as down. Two
/// (≈10 s) filters a single transient blip without waiting on the slow path.
const WATCHDOG_STRIKES: u32 = 2;
/// Quiet window after a restart attempt before the watchdog will fire again. The
/// daemon needs a few seconds to come up and start serving its endpoint; without
/// this the next probes would spawn a second (and third…) instance on top of one
/// that is merely mid-startup. Comfortably longer than a cold start, so a
/// genuinely crash-looping daemon still retries — just not in a tight loop. (The
/// daemon's single-instance guard makes an overlapping spawn a harmless no-op
/// regardless; this simply avoids the churn.)
const WATCHDOG_COOLDOWN: Duration = Duration::from_secs(45);

/// A fast, self-contained daemon supervisor: probe every [`WATCHDOG_TICK`], and
/// once the daemon has missed [`WATCHDOG_STRIKES`] probes in a row (~10 s),
/// restart it — then hold off for [`WATCHDOG_COOLDOWN`] before considering
/// another attempt.
///
/// This is deliberately separate from [`run_poll_loop`]'s `refresh_health`,
/// which owns the user-facing went-quiet/back-online *notices* on a slower,
/// debounced cadence. Recovery is split out here so it can react in seconds
/// without dragging the whole 30 s UI-refresh loop (and its DB reads) down to a
/// 5 s beat. Both may call `restart()` in the same outage; the daemon's
/// single-instance guard makes that safe, and the cooldown keeps this side from
/// stacking attempts. Not gated on any "was healthy" flag — a daemon already
/// down at startup is exactly the case that most needs bringing back.
pub async fn run_daemon_watchdog() {
    let down_after_s = WATCHDOG_STRIKES as u64 * WATCHDOG_TICK.as_secs();
    let mut consecutive_down: u32 = 0;
    let mut cooldown_until: Option<Instant> = None;

    loop {
        tokio::time::sleep(WATCHDOG_TICK).await;

        if crate::commands::daemon_control::probe().await.running {
            consecutive_down = 0;
            cooldown_until = None;
            continue;
        }

        consecutive_down += 1;
        if consecutive_down < WATCHDOG_STRIKES {
            continue;
        }
        // Confirmed down. Respect the post-restart quiet window so a daemon
        // that's still starting up isn't restarted on top of itself.
        if cooldown_until.is_some_and(|until| Instant::now() < until) {
            continue;
        }

        // Wrap the discrete recovery operation in a span so the whole attempt is
        // traceable end-to-end; `down_after_s` rides on the span as a structured
        // field (queryable in JSON sinks) rather than baked into a message.
        async {
            tracing::info!("daemon watchdog: endpoint down, restarting");
            if let Err(e) = crate::commands::daemon_control::restart().await {
                tracing::warn!(error = %e, "daemon watchdog: restart attempt failed");
            }
        }
        .instrument(tracing::info_span!("daemon_watchdog.restart", down_after_s))
        .await;
        cooldown_until = Some(Instant::now() + WATCHDOG_COOLDOWN);
        consecutive_down = 0;
    }
}

pub async fn run_poll_loop(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let mut tick: u32 = 0;
    // Last-emitted JSON snapshots for the live events — emit only on change
    // (mirrors the SSE stores' change-only broadcast).
    let mut last_notices = String::new();
    let mut last_banners = String::new();

    loop {
        // Tick 0, 1, 2… every 30s.
        // Active: every tick (30s)
        // Health + today: every 2nd tick (60s)
        // Worklogs: every 10th tick (5 min)
        let do_health = tick.is_multiple_of(2);
        let do_worklogs = tick.is_multiple_of(10);

        // The tray's own DB pool (opened at startup) — every read is now a direct
        // DB read through it, so the loop has no HTTP dependency on the Next
        // server. `None` only before the DB is first opened.
        let pool = app
            .try_state::<Option<meridian_core::SqlitePool>>()
            .and_then(|s| s.inner().clone());

        if do_health {
            refresh_health(&app, &state, pool.as_ref()).await;
        }
        if let Some(pool) = &pool {
            refresh_active(pool, &state).await;
            refresh_current_task(pool, &state).await;
            if do_health {
                refresh_today(pool, &state).await;
                // Spawned off, not awaited inline: on the rare tick where this
                // actually fires (≤1 install event ever + ≤1/day), a slow or
                // hanging PostHog response (5s timeout) must not delay the
                // more user-visible steps below (notification drain, live
                // notices/banners, tray icon/menu refresh).
                let analytics_app = app.clone();
                let analytics_pool = pool.clone();
                tauri::async_runtime::spawn(async move {
                    crate::analytics::maybe_send_daily_tick(&analytics_app, &analytics_pool).await;
                });
            }
            if do_worklogs {
                refresh_worklogs(pool, &state).await;
                // Spawned off, not awaited inline: this can make one HTTP call
                // per newly-posted worklog this tick, and a slow/hanging
                // counter response (5s timeout each) must not delay the
                // more user-visible steps below.
                let counter_pool = pool.clone();
                tauri::async_runtime::spawn(async move {
                    crate::counter_ping::check_worklog_posts(&counter_pool).await;
                });
            }
            if do_health {
                permissions::check_permissions(&app, pool).await;
            }
        }
        // Work-hours schedule enforcement: auto-pause capture outside the
        // configured window, auto-resume when entering it. Only fires when the
        // feature is enabled; never overrides a user-initiated timed pause.
        if let Some(pool) = &pool {
            check_work_hours(&app, &state, pool).await;
            // Daily "Plan your day" auto-open — at most once per local day
            // (marker-file gated; a single file stat on the common path). If
            // the pool isn't open yet this tick, it simply retries next tick.
            plan_auto_open::maybe_auto_open_plan(&app, pool).await;
            // "What's New" auto-open — at most once per app version. Defers
            // itself (see its own module docs) whenever the dashboard window
            // is already open — which also naturally avoids colliding with
            // the Plan auto-open just above on the same tick.
            whats_new_auto_open::maybe_auto_open_whats_new(&app).await;
        }

        // Drain the daemon's notification outbox every tick — this is the single
        // delivery path for all daemon-originated notifications (plan nudge,
        // worklog ready, promoted faults). The tray is a dumb delivery agent;
        // preference + quiet-hours filtering already happened server-side.
        drain_notifications(&app).await;

        // Push live notices + banner notifications to the dashboard webview
        // (the ported SSE streams). Skipped silently when the DB isn't open.
        if let Some(pool) = &pool {
            live::emit_notices(&app, pool, &mut last_notices).await;
            live::emit_banners(&app, pool, &mut last_banners).await;
        }

        {
            let mut s = state.lock().unwrap();
            s.last_poll = Some(Instant::now());
        }

        emit_update(&app, &state);
        update_tray_icon(&app, &state);
        update_toggle_menu(&app, &state);

        tokio::time::sleep(TICK).await;
        tick = tick.wrapping_add(1);
    }
}

fn emit_update(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let payload = state.lock().unwrap().to_payload();
    let _ = app.emit("status-update", payload);
}

fn update_tray_icon(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let (health, drafts, tray_id) = {
        let s = state.lock().unwrap();
        (s.health.clone(), s.drafts_count, s.tray_id.clone())
    };

    let tooltip = match &health {
        HealthStatus::Healthy if drafts > 0 => {
            format!(
                "Meridian — {} draft{} waiting",
                drafts,
                if drafts == 1 { "" } else { "s" }
            )
        }
        HealthStatus::Healthy => "Meridian — everything's running.".to_string(),
        HealthStatus::Unhealthy => "Meridian — gone quiet.".to_string(),
        HealthStatus::Unknown => "Meridian".to_string(),
    };

    if let Some(id) = tray_id {
        if let Some(tray) = app.tray_by_id(&id) {
            let _ = tray.set_tooltip(Some(&tooltip));
        }
    }
}

/// Enforce the work-hours schedule: auto-pause outside the window, auto-resume
/// inside it. Runs every poll tick (30 s). The state machine is:
///   - Outside hours + not schedule-paused → start a schedule pause
///   - Inside hours + schedule-paused → end the schedule pause, write the gap
///   - User is in a timed pause → leave it alone (don't override)
async fn check_work_hours(
    app: &tauri::AppHandle,
    state: &Arc<Mutex<AppState>>,
    pool: &meridian_core::SqlitePool,
) {
    let settings = meridian_core::settings::load_runtime_settings();
    if !settings.work_hours_enabled {
        return;
    }

    let in_hours = is_within_work_hours(&settings);

    let (pause_source, started_at, capture_paused_flag) = {
        let s = state.lock().unwrap();
        (
            s.pause_source.clone(),
            s.pause_started_at,
            s.capture_paused.clone(),
        )
    };

    match (in_hours, &pause_source) {
        (false, None) => {
            // Outside work hours, not currently paused → begin schedule pause.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            // Drop cancel senders → stops engine + UI consumer, halting capture.
            {
                let mut s = state.lock().unwrap();
                drop(s.engine_cancel.take());
                drop(s.ui_consumer_cancel.take());
                capture_paused_flag.store(true, Ordering::Relaxed);
                s.pause_source = Some(PauseSource::Schedule);
                s.pause_started_at = Some(now);
                s.schedule_resume_at = Some(settings.work_hours_start.clone());
            }
            tracing::info!(resume_at = %settings.work_hours_start, "work-hours: schedule pause started");
        }
        (true, Some(PauseSource::Schedule)) => {
            // Back inside work hours → end the schedule pause and write the gap.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let duration_s = started_at
                .map(|s| now.saturating_sub(s) as i64)
                .unwrap_or(0);

            if let Some(started_secs) = started_at {
                if duration_s > 0 {
                    use chrono::{DateTime, SecondsFormat, Utc};
                    let from = DateTime::<Utc>::from_timestamp(started_secs as i64, 0)
                        .unwrap_or_else(Utc::now)
                        .to_rfc3339_opts(SecondsFormat::Millis, true);
                    let to = DateTime::<Utc>::from_timestamp(now as i64, 0)
                        .unwrap_or_else(Utc::now)
                        .to_rfc3339_opts(SecondsFormat::Millis, true);
                    if let Err(e) = meridian_core::insert_pause_gap(
                        pool,
                        &from,
                        &to,
                        duration_s,
                        "schedule_paused",
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "work-hours: failed to write schedule_paused gap");
                    }
                }
            }

            {
                let mut s = state.lock().unwrap();
                capture_paused_flag.store(false, Ordering::Relaxed);
                s.pause_source = None;
                s.pause_started_at = None;
                s.schedule_resume_at = None;
                s.pause_until = None;
            }
            // Restart engine so screen recording resumes.
            #[cfg(feature = "capture")]
            crate::start_capture(state.clone(), Some(pool.clone()));
            tracing::info!(
                duration_s,
                "work-hours: schedule pause ended — capture resumed"
            );
            let _ = app;
        }
        _ => {
            // Timed pause, or both already correct — nothing to do.
        }
    }
}

/// Returns `true` when the current local time falls within the configured work
/// hours on a configured work day. Handles same-day ranges ("09:00"–"18:00").
/// Does NOT handle overnight ranges (end < start); those are a quiet-hours
/// pattern — work hours are always a same-day window.
fn is_within_work_hours(settings: &meridian_core::settings::RuntimeSettings) -> bool {
    let now = Local::now();
    // ISO weekday: Mon=1 … Sun=7 — matches the "1,2,3,4,5" work_days convention.
    let weekday_num = now.weekday().number_from_monday();
    let active_day = settings
        .work_days
        .split(',')
        .filter_map(|d| d.trim().parse::<u32>().ok())
        .any(|d| d == weekday_num);
    if !active_day {
        return false;
    }

    let now_mins = now.hour() * 60 + now.minute();
    let start = hhmm_to_minutes(&settings.work_hours_start);
    let end = hhmm_to_minutes(&settings.work_hours_end);
    match (start, end) {
        (Some(s), Some(e)) if e > s => now_mins >= s && now_mins < e,
        _ => false, // malformed config → treat as "always outside"
    }
}

/// Parse "HH:MM" → minutes from midnight. Returns `None` on malformed input.
fn hhmm_to_minutes(hhmm: &str) -> Option<u32> {
    let hhmm = hhmm.trim();
    let (h, m) = hhmm.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m_str = m;
    if m_str.len() != 2 {
        return None;
    }
    let m: u32 = m_str.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

fn update_toggle_menu(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let (health, tray_id, last_menu_state) = {
        let s = state.lock().unwrap();
        (
            s.health.clone(),
            s.tray_id.clone(),
            s.last_menu_state.clone(),
        )
    };

    if health == last_menu_state {
        return;
    }

    // Rebuild via the single source of truth in lib.rs so this health-driven
    // refresh always carries the full item set (it used to hardcode a 5-item
    // menu here and silently drop "Open Dashboard (native)").
    if let Some(id) = tray_id {
        if let Some(tray) = app.tray_by_id(&id) {
            if let Ok(menu) = crate::tray::build_tray_menu(app, &health) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    }

    {
        let mut s = state.lock().unwrap();
        s.last_menu_state = health;
    }
}
