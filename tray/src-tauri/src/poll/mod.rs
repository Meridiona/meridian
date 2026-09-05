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
//! - [`startup_health`] — a separate, faster one-shot loop (spawned
//!   alongside this one, not called from inside it) that repaints the
//!   displayed health status the moment the daemon+DB are ready, instead of
//!   waiting for this loop's next 30/60 s-cadenced health tick.
//!
//! The tray-sync helpers (emit / tooltip / menu) stay here, coupled to the loop.

mod live;
mod notifications;
mod permissions;
mod plan_auto_open;
mod refresh;
mod startup_health;
mod watchdog;
mod whats_new_auto_open;

use crate::state::{AppState, HealthStatus, PauseSource};
use chrono::{Datelike, Local, Timelike};
use notifications::drain_notifications;
use refresh::{
    refresh_active, refresh_current_task, refresh_health, refresh_today, refresh_worklogs,
};
pub use startup_health::fast_poll_until_healthy;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
pub use watchdog::run_daemon_watchdog;

const TICK: Duration = Duration::from_secs(30);

pub async fn run_poll_loop(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let mut tick: u32 = 0;
    // Last-emitted JSON snapshots for the live events — emit only on change
    // (mirrors the SSE stores' change-only broadcast).
    let mut last_notices = String::new();
    let mut last_banners = String::new();
    // How long each OS permission has been reading "off". Lives with the loop
    // rather than in a static so it dies with it — and so a fresh process
    // starts with an empty map, which is what gives every launch its grace
    // period (see `permissions`' module docs).
    let mut permission_debounce = permissions::PermissionDebounce::default();

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
            .try_state::<crate::db_pool::DbPool>()
            .and_then(|s| s.get());

        if do_health {
            refresh_health(&app, &state, pool.as_ref()).await;

            // Awaited inline rather than spawned, unlike the analytics send
            // below, for one reason: ORDERING. This call is what rolls the
            // perf window at a local-day boundary, and `maybe_send_daily_tick`
            // (spawned further down this same tick) reads the window it
            // closes. Spawning both would race, and the loser would ship a
            // `daily_usage` with no perf data on exactly the tick that had it.
            //
            // Safe to await: it is a socket probe for the daemon pid plus two
            // process refreshes — no network, no DB, single-digit ms.
            crate::analytics::perf::sample().await;
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
                permissions::check_permissions(&app, pool, &mut permission_debounce).await;
            }
        }
        // Disk-space guard: auto-pause capture when free disk space on the
        // meridian.db volume drops below the low-disk threshold, auto-resume
        // once it recovers. Checked before work-hours so a low-disk pause
        // always wins the race to claim `pause_source` first; see
        // check_disk_space's doc comment for why writing into a nearly-full
        // disk must stop rather than degrade silently.
        if let Some(pool) = &pool {
            check_disk_space(&app, &state, pool).await;
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

/// Auto-pause capture when free disk space on the `meridian.db` volume drops
/// below the low-disk threshold, auto-resume once it recovers. Runs every
/// poll tick (30 s), before [`check_work_hours`] so a low-disk pause always
/// wins the race to claim `pause_source` first.
///
/// Writing into a nearly-full disk is exactly how a real `meridian.db` got
/// corrupted in practice: SQLite/SQLCipher torn page allocations under WAL
/// when a write landed with the disk critically full, scrambling the
/// page-allocation bookkeeping for `capture_frames`/`capture_ui_events` and
/// one `app_sessions` row's overflow chain. The daemon already raises a
/// `system.disk_low` notice every tick once space runs low
/// (`meridian::health::daemon`) — this only makes the tray *act* on the same
/// condition instead of writing blind.
///
/// The state machine mirrors [`check_work_hours`]:
///   - Low disk + not disk-paused → start a disk-low pause (unless a Timed or
///     Indefinite user pause is already active — never override those; a
///     schedule pause is preempted, since disk safety is the higher-priority
///     reason once space runs low and check_disk_space runs first each tick).
///   - Disk recovered + disk-paused → end the pause, write the gap, resume.
///
/// Every OTHER resume path (manual "Resume now", a timed pause's own expiry
/// timer) is separately gated in [`crate::commands::pause::resume_capture`],
/// so a still-low disk can't be resumed into from any direction — this
/// function only owns the disk-low pause's own start/end transition.
async fn check_disk_space(
    app: &tauri::AppHandle,
    state: &Arc<Mutex<AppState>>,
    pool: &meridian_core::SqlitePool,
) {
    let low = meridian::health::platform::meridian_data_low_gb().is_some();

    let (pause_source, started_at, capture_paused_flag) = {
        // Degrade gracefully on a poisoned mutex rather than unwrap — this
        // runs synchronously inside run_poll_loop's loop body (not isolated
        // in its own task), so a panic here would kill the entire poll loop
        // for the rest of the process's life (health checks, notifications,
        // tray icon updates, and this same guard, all silently dead), not
        // just this one check. Same pattern refresh_health/refresh_active use.
        let Ok(s) = state.lock() else {
            tracing::warn!("check_disk_space: state lock poisoned");
            return;
        };
        (
            s.pause_source.clone(),
            s.pause_started_at,
            s.capture_paused.clone(),
        )
    };

    match (low, &pause_source) {
        (true, None) | (true, Some(PauseSource::Schedule)) => {
            // Disk low, not paused (or only schedule-paused, which this
            // preempts) → begin a disk-low pause.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Preempting a Schedule pause must close out its own interval
            // first — same as pause.rs::transition_pause does for the
            // command-driven pause paths. Without this, the time already
            // spent Schedule-paused before disk-low kicked in silently
            // vanishes from the gaps table, and downstream worklog/analytics
            // logic (which relies on gaps to exclude paused time) would
            // treat it as tracked/active time instead.
            if let (Some(PauseSource::Schedule), Some(prev_started)) = (&pause_source, started_at) {
                let duration_s = now.saturating_sub(prev_started) as i64;
                if duration_s > 0 {
                    use chrono::{DateTime, SecondsFormat, Utc};
                    let from = DateTime::<Utc>::from_timestamp(prev_started as i64, 0)
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
                        tracing::warn!(error = %e, "disk-space guard: failed to write preempted schedule_paused gap");
                    }
                }
            }

            {
                let Ok(mut s) = state.lock() else {
                    tracing::warn!("check_disk_space: state lock poisoned");
                    return;
                };
                drop(s.engine_cancel.take());
                drop(s.ui_consumer_cancel.take());
                capture_paused_flag.store(true, Ordering::Relaxed);
                s.pause_source = Some(PauseSource::DiskLow);
                s.pause_started_at = Some(now);
                s.schedule_resume_at = None;
            }
            tracing::warn!("disk-space guard: capture paused — free disk space is low");
        }
        (false, Some(PauseSource::DiskLow)) => {
            // Disk recovered → end the pause and write the gap.
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
                        "disk_low_paused",
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "disk-space guard: failed to write disk_low_paused gap");
                    }
                }
            }

            {
                let Ok(mut s) = state.lock() else {
                    tracing::warn!("check_disk_space: state lock poisoned");
                    return;
                };
                capture_paused_flag.store(false, Ordering::Relaxed);
                s.pause_source = None;
                s.pause_started_at = None;
                s.pause_until = None;
            }
            // Restart engine so capture resumes. If this happens to also be
            // outside work hours, check_work_hours (running right after this,
            // same tick) immediately re-pauses with Schedule — a harmless,
            // rare blip (engine starts and stops within the same tick,
            // capturing nothing) rather than a bug worth cross-checking
            // schedule state here too.
            #[cfg(feature = "capture")]
            crate::restart_capture(app, state, "disk space recovered");
            tracing::info!(duration_s, "disk-space guard: capture resumed");
        }
        _ => {
            // Still low and already disk-paused, still fine and untouched, or
            // a Timed/Indefinite user pause is active — leave it alone.
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
            crate::restart_capture(app, state, "work hours started");
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
