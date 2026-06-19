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
use crate::state::{AppState, HealthStatus};
use notifications::drain_notifications;
use refresh::{refresh_active, refresh_health, refresh_today, refresh_worklogs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

const TICK: Duration = Duration::from_secs(30);

/// Cap on consecutive automatic MLX restarts before the tray stops spawning and
/// just watches for recovery — prevents a server that won't come up from
/// spawn-storming. Reset to 0 the moment `/health` answers.
const MLX_MAX_RESTARTS: u32 = 5;

pub async fn run_poll_loop(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let mut tick: u32 = 0;
    // Last-emitted JSON snapshots for the live events — emit only on change
    // (mirrors the SSE stores' change-only broadcast).
    let mut last_notices = String::new();
    let mut last_banners = String::new();
    // Consecutive automatic MLX restart attempts (bounded by MLX_MAX_RESTARTS).
    let mut mlx_restart_attempts: u32 = 0;

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
                supervise_mlx(&mlx, &mut mlx_restart_attempts).await;
            }
        }
        if let Some(pool) = &pool {
            refresh_active(pool, &state).await;
            if do_health {
                refresh_today(pool, &state).await;
            }
            if do_worklogs {
                refresh_worklogs(pool, &state).await;
            }
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
    if *attempts >= MLX_MAX_RESTARTS {
        let port = mlx.lock().await.port;
        if mlx_server::health_check(port).await {
            *attempts = 0;
            mlx.lock().await.status = mlx_server::MlxStatus::Running;
            tracing::info!("mlx: recovered after restart budget exhausted");
        } else {
            tracing::warn!(
                attempts = *attempts,
                "mlx: restart budget exhausted — not restarting (see daemon's mlx.down notice)"
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
