//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray's background poll loop.
//!
//! Every 30 s tick refreshes a slice of [`AppState`] (active session each tick;
//! health + today every 2nd; worklog drafts every 10th), drains the daemon's
//! notification outbox, pushes the live notice/banner sets to the dashboard
//! webview, then syncs the tray (event emit + tooltip + menu).
//!
//! The health tick also keeps the MLX classifier alive ([`supervise_mlx`]) and,
//! on a slow ~6 h cadence ([`MLX_RUNTIME_CHECK_TICKS`]), swaps in a newer
//! published runtime in the background ([`maybe_upgrade_runtime`]).
//!
//! - [`refresh`] — the per-tick fetch-and-store functions (also emits
//!   `health-update`, the ported `/api/health/stream`).
//! - [`notifications`] — outbox drain + the quiet-hours policy check
//!   ([`notifications_allowed`], re-exported here for [`crate::commands::daemon`]).
//! - [`live`] — the live data → Tauri events that replace the dashboard's SSE
//!   streams: `notices-update`, `notifications-update`, and the `log-tail`
//!   tailer ([`spawn_log_tailer`], started from `lib.rs`, runs at ~1 s
//!   independent of this 30 s loop).
//!
//! The tray-sync helpers (emit / tooltip / menu) stay here, coupled to the loop.

mod live;
mod notifications;
mod refresh;

pub(crate) use live::spawn_log_tailer;
pub(crate) use notifications::notifications_allowed;

use crate::mlx_server::{self, SharedMlxManager, SuperviseOutcome};
use crate::state::{AppState, HealthStatus, PauseSource};
use chrono::{Datelike, Local, Timelike};
use notifications::drain_notifications;
use refresh::{
    refresh_active, refresh_current_task, refresh_health, refresh_today, refresh_worklogs,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

const TICK: Duration = Duration::from_secs(30);

/// Cap on consecutive automatic MLX restarts before the tray stops spawning and
/// just watches for recovery — prevents a server that won't come up from
/// spawn-storming. Reset to 0 the moment `/health` answers.
const MLX_MAX_RESTARTS: u32 = 5;

/// After exhausting [`MLX_MAX_RESTARTS`], how many additional health ticks to
/// wait before allowing another restart cycle (~10 min at 60 s / tick). Using
/// the single `attempts` counter (letting it grow past `MLX_MAX_RESTARTS`)
/// avoids a second mutable state variable. Once the window closes, `attempts`
/// resets to 0 and supervision resumes — preventing a permanent wedge after an
/// OOM / crash that keeps `/health` silent.
const MLX_COOLING_TICKS: u32 = 10;

/// How often (in ticks) to check for a newer published MLX runtime and upgrade in
/// the background. `720 * 30 s = 6 h`. Also fires at tick 0, so a relaunch checks
/// immediately. The check is a single small manifest GET when already current.
const MLX_RUNTIME_CHECK_TICKS: u32 = 720;

pub async fn run_poll_loop(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let mut tick: u32 = 0;
    // Last-emitted JSON snapshots for the live events — emit only on change
    // (mirrors the SSE stores' change-only broadcast).
    let mut last_notices = String::new();
    let mut last_banners = String::new();
    // Consecutive automatic MLX restart attempts (bounded by MLX_MAX_RESTARTS).
    let mut mlx_restart_attempts: u32 = 0;
    // True while a background runtime upgrade is downloading/swapping. Supervision
    // stands down while set so it doesn't fight the upgrade over the server +
    // runtime dir; the upgrade itself restarts the server when it's done.
    let mlx_upgrade_inflight = Arc::new(AtomicBool::new(false));

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
            refresh_health(&app, &state).await;
            // Keep the MLX classifier alive. The daemon *detects + surfaces*
            // "Classifier offline"; the tray is what *fixes* it — restart on
            // death, bounded so a server that won't start can't spawn-storm.
            if let Some(mlx) = app
                .try_state::<SharedMlxManager>()
                .map(|s| s.inner().clone())
            {
                // Pause supervision while an upgrade owns the server (else it would
                // restart the very server the upgrade just stopped to swap).
                if mlx_upgrade_inflight.load(Ordering::SeqCst) {
                    tracing::trace!("mlx: supervision paused — runtime upgrade in flight");
                } else {
                    supervise_mlx(&mlx, &mut mlx_restart_attempts).await;
                }
                // Check for a newer runtime on a slow cadence (and at tick 0).
                // Runs AFTER supervise so a healthy old server is observed first.
                if tick.is_multiple_of(MLX_RUNTIME_CHECK_TICKS) {
                    maybe_upgrade_runtime(mlx, mlx_upgrade_inflight.clone());
                }
            }
        }
        if let Some(pool) = &pool {
            refresh_active(pool, &state).await;
            refresh_current_task(pool, &state).await;
            if do_health {
                refresh_today(pool, &state).await;
            }
            if do_worklogs {
                refresh_worklogs(pool, &state).await;
            }
        }
        // Work-hours schedule enforcement: auto-pause capture outside the
        // configured window, auto-resume when entering it. Only fires when the
        // feature is enabled; never overrides a user-initiated timed pause.
        if let Some(pool) = &pool {
            check_work_hours(&app, &state, pool).await;
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

/// One bounded MLX supervision step (called every health tick).
///
/// Delegates the decision to [`mlx_server::supervise`] and maintains the
/// consecutive-restart budget: reset on health, increment on each (re)start,
/// and once the budget is spent stop spawning — keep cheaply probing `/health`
/// so the tray resumes automatically if the server recovers (self-heal or a
/// human fix). User-facing "offline" messaging stays with the daemon's notice;
/// this loop only logs (and remediates).
async fn supervise_mlx(mlx: &SharedMlxManager, attempts: &mut u32) {
    // After the restart budget is exhausted, let `attempts` grow past
    // MLX_MAX_RESTARTS to count out the cooling window. Once it reaches
    // MLX_MAX_RESTARTS + MLX_COOLING_TICKS, reset to 0 so supervise() runs
    // again and can kill any zombie on port 7823 before attempting a fresh
    // restart cycle. This prevents a permanent wedge after an OOM / crash
    // that keeps `/health` silent indefinitely.
    if *attempts >= MLX_MAX_RESTARTS + MLX_COOLING_TICKS {
        tracing::info!("mlx: cooling period elapsed — resetting restart budget for a new cycle");
        *attempts = 0;
    }

    if *attempts >= MLX_MAX_RESTARTS {
        let port = mlx.lock().await.port;
        if mlx_server::health_check(port).await {
            *attempts = 0;
            mlx.lock().await.status = mlx_server::MlxStatus::Running;
            tracing::info!("mlx: recovered after restart budget exhausted");
        } else {
            *attempts += 1;
            tracing::warn!(
                attempts = *attempts,
                cooling_remaining = MLX_MAX_RESTARTS + MLX_COOLING_TICKS - *attempts,
                "mlx: restart budget exhausted — cooling before next cycle",
            );
        }
        return;
    }

    match mlx_server::supervise(mlx).await {
        SuperviseOutcome::Healthy => *attempts = 0,
        SuperviseOutcome::Restarted | SuperviseOutcome::KilledWedged => {
            *attempts += 1;
            tracing::info!(attempt = *attempts, "mlx: supervision (re)start");
        }
        SuperviseOutcome::RestartFailed(e) => {
            *attempts += 1;
            tracing::warn!(error = %e, attempt = *attempts, "mlx: restart failed");
        }
        SuperviseOutcome::PortHeldForeign => {
            tracing::debug!("mlx: port held by an unmanaged process — leaving it")
        }
        SuperviseOutcome::NoRuntime => tracing::trace!("mlx: no runtime to supervise"),
    }
}

/// Spawn a background runtime auto-upgrade unless one is already running.
///
/// Detached so a multi-minute download never stalls the 30 s poll cadence. The
/// in-flight flag (set synchronously before the spawn) prevents the next
/// scheduled check from starting a second upgrade, and a drop guard clears it on
/// completion, early return, AND panic — so a crashing upgrade can never latch
/// the flag and silently disable the feature (which would also leave supervision
/// permanently paused).
///
/// Deliberately **not** wrapped in an outer timeout: cancelling the upgrade
/// future mid-swap could leave the machine with no live runtime, then clear the
/// flag and resume supervision against a missing install. Instead
/// [`mlx_server::auto_upgrade_runtime`] bounds only its network download phase
/// internally and runs the stop→swap→restart to completion (all bounded,
/// non-network steps), so the unbounded part is capped without risking the swap.
fn maybe_upgrade_runtime(mlx: SharedMlxManager, inflight: Arc<AtomicBool>) {
    if inflight.swap(true, Ordering::SeqCst) {
        return; // a previous upgrade is still running
    }
    tauri::async_runtime::spawn(async move {
        struct ResetOnDrop(Arc<AtomicBool>);
        impl Drop for ResetOnDrop {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _reset = ResetOnDrop(inflight);

        mlx_server::auto_upgrade_runtime(&mlx).await;
    });
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
            capture_paused_flag.store(true, Ordering::Relaxed);
            {
                let mut s = state.lock().unwrap();
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

            capture_paused_flag.store(false, Ordering::Relaxed);
            {
                let mut s = state.lock().unwrap();
                s.pause_source = None;
                s.pause_started_at = None;
                s.schedule_resume_at = None;
                s.pause_until = None;
            }
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
