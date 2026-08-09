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
//! - [`meridian::notices`] — the fault-bus `refresh_health` raises/clears
//!   `tray.daemon_quiet` through (event_key `system.health`).
//! - [`crate::commands::health::check_health`] — the direct health check.

use crate::commands::health::check_health;
use crate::state::{ActiveSession, AppState, HealthStatus, TodayBreakdown};
use meridian_core::SqlitePool;
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tracing::Instrument;

/// Run the local health check, fold it into [`AppState`], and raise/clear the
/// went-quiet / back-online notice (debounced to the 2nd consecutive failure).
/// Also reconciles a `tray.daemon_quiet` notice left stale by a previous
/// process instance on this process's first healthy tick — see
/// `startup_health_reconciled` on [`AppState`].
pub(super) async fn refresh_health(
    app: &tauri::AppHandle,
    state: &Arc<Mutex<AppState>>,
    pool: Option<&SqlitePool>,
) {
    let hr = check_health().await;

    // Surface the in-use LLM provider's health on the poll path too (the `get_health` command
    // logs it on demand; this catches the background 60 s cadence). A warn when it's unavailable
    // is genuinely worth having in the log - it means hourly summaries are paused.
    if hr.llm_provider_ok == Some(false) {
        tracing::warn!(
            provider = ?hr.llm_provider_name,
            detail = ?hr.llm_provider_detail,
            "in-use LLM provider is unavailable — hourly summaries paused"
        );
    } else if hr.llm_provider_rate_limited == Some(true) {
        tracing::info!(
            provider = ?hr.llm_provider_name,
            "in-use LLM provider is rate-limited — summaries will catch up when the limit clears"
        );
    }

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

    let (attempt_restart, notify_down, notify_back, reconcile_stale) = {
        let Ok(mut s) = state.lock() else {
            tracing::warn!("refresh_health: state lock poisoned");
            return;
        };
        let now_healthy = new_health == HealthStatus::Healthy;
        // One explicit deref first: disjoint field borrows aren't provable
        // across separate `MutexGuard::deref_mut` calls, only through a
        // single plain `&mut AppState`.
        let s = &mut *s;
        let decision = decide_health_notice(
            now_healthy,
            &mut s.consecutive_health_failures,
            &mut s.daemon_was_healthy,
            &mut s.startup_health_reconciled,
        );
        s.ui_reachable = true; // health checks are now direct (no HTTP); always reachable
        s.health = new_health;

        (
            decision.attempt_restart,
            decision.notify_down,
            decision.notify_back,
            decision.reconcile_stale,
        )
    };

    // A confirmed outage (2nd consecutive failed poll, not a transient blip) is
    // worth one automatic recovery attempt — most causes (a crashed process, a
    // machine where the scheduled task/launchd agent lost track of it) self-heal
    // from a plain restart, and someone who isn't watching the tray shouldn't
    // have to notice the banner and click "Restart daemon" for that.
    //
    // Crucially this is NOT gated on `daemon_was_healthy`: the daemon being
    // already down when this tray process started (machine resumed from sleep,
    // the login launcher never fired, an earlier crash) is exactly when it has
    // no other path back up. On machines where the scheduled task couldn't be
    // created (`schtasks` blocked by policy — the Startup-folder launcher only
    // fires once at login) the tray is the only supervisor there is, so it must
    // recover a cold-down daemon too, not just one it watched go down. The
    // restart needs no DB pool, so it runs even when `pool` is None; only the
    // user-facing notice below does. Best-effort: the result feeds the notice
    // detail when we also notify.
    let restart_result = if attempt_restart {
        // Wrap the restart attempt in a span so the health-path recovery is
        // traceable end-to-end, matching the fast watchdog's span.
        async {
            // `start_if_stopped`, never `restart`: this path fires on a health
            // *report* (`database_ready` / `daemon_running` read from the DB),
            // which a daemon that is alive and merely busy can fail. Restarting
            // on that signal means SIGTERM to a live process mid-write, which is
            // the second, slower instance of what corrupted `meridian.db` — see
            // [`super::watchdog`]. Starting a stopped daemon still works; a
            // running one is left alone.
            //
            // Deliberately still returns a `Result` from the same shape of call,
            // so the `notify_down` ⇒ `restart_result.is_some()` invariant
            // asserted below continues to hold.
            let r = crate::commands::daemon_control::start_if_stopped().await;
            if r.is_err() {
                // Span status ERROR as well as the log below - an error-only
                // telemetry query filters on the span, and a failed automatic
                // recovery must not be invisible to it.
                tracing::Span::current().record("otel.status_code", "ERROR");
            }
            match &r {
                // ERROR (not WARN) so this line crosses the error-only central
                // telemetry filter. The went-quiet *notice* is a local DB row
                // and the watchdog's per-tick retries are WARN, so a
                // paused/offline daemon the tray also couldn't restart was
                // previously invisible in central OO — you could see the banner
                // on the machine but nothing shipped. Edge-triggered
                // (`attempt_restart` fires once on the 2nd consecutive failure),
                // so it's one event per down-episode, not per poll. The fields
                // are what you debug from: `daemon_running` vs `db_ready`
                // pinpoints which subsystem is down, and `cold_start` (never seen
                // healthy this run) distinguishes an install/autostart gap from a
                // crash of a daemon that had been up.
                Err(e) => tracing::error!(
                    error = %e,
                    daemon_running,
                    db_ready,
                    cold_start = !notify_down,
                    "daemon offline and the automatic start failed — the daemon is down with no recovery this episode"
                ),
                Ok(()) => tracing::info!(
                    daemon_running,
                    db_ready,
                    "daemon offline — start attempted"
                ),
            }
            Some(r)
        }
        // Renamed from `daemon_watchdog.restart`: this is the health path, not
        // the watchdog, and it no longer restarts anything. Sharing the old name
        // would have made a central-OO query for restarts silently match only
        // this span post-#678 and read as "kills stopped" whether or not they
        // had — a blind spot in the exact signal used to verify that fix.
        .instrument(tracing::info_span!(
            "daemon_health.start",
            otel.status_code = tracing::field::Empty
        ))
        .await
    } else {
        None
    };

    let Some(pool) = pool else { return };
    if notify_down {
        // `notify_down` implies `attempt_restart` (both require the 2nd
        // consecutive failure), so `restart_result` is always `Some` here.
        // Assert it rather than only documenting it, so a future refactor that
        // breaks the invariant trips a test instead of silently rendering the
        // failed-restart case as the success message.
        debug_assert!(
            restart_result.is_some(),
            "notify_down implies attempt_restart, so restart_result must be Some"
        );
        let detail = match &restart_result {
            Some(Err(_)) => {
                "Tried to start it automatically and that failed too. Tap to check what happened."
            }
            _ => "Tried starting it automatically - give it a moment.",
        };
        if let Err(e) = meridian::notices::raise_typed(
            pool,
            meridian::notices::Notice {
                id: "tray.daemon_quiet",
                severity: "warning",
                title: "Meridian went quiet.",
                detail,
                remedy: None,
                event_key: "system.health",
                deep_link: Some(meridian_core::notifications::deep_links::LOGS),
            },
        )
        .await
        {
            tracing::warn!(error = %e, "daemon-health notice raise failed");
        }
    } else if attempt_restart && matches!(restart_result, Some(Err(_))) {
        // Cold-start path: `daemon_was_healthy == false` suppressed `notify_down`
        // above, AND the one automatic restart failed (e.g. `schtasks /Run`
        // failed and `spawn_staged_daemon()` found no staged binary). Left alone
        // this strands the user silently — `attempt_restart` fires only once per
        // episode, so nothing else surfaces for this process lifetime. A failed
        // recovery is news in its own right, independent of the went-quiet
        // debounce the cold-start silence exists for, so raise the notice here
        // regardless of `daemon_was_healthy`. Same `tray.daemon_quiet` id, so it
        // dedupes with the warm-path notice and is cleared by the same
        // `notify_back` / `reconcile_stale` paths once the daemon recovers. (The
        // 5 s `run_daemon_watchdog` keeps retrying the restart in the meantime —
        // this only fixes the *notification* gap, not the retry.)
        if let Err(e) = meridian::notices::raise_typed(
            pool,
            meridian::notices::Notice {
                id: "tray.daemon_quiet",
                severity: "warning",
                title: "Meridian went quiet.",
                detail: "Couldn't start it automatically. Tap to check what happened.",
                remedy: None,
                event_key: "system.health",
                deep_link: Some(meridian_core::notifications::deep_links::LOGS),
            },
        )
        .await
        {
            tracing::warn!(error = %e, "daemon-health cold-start failure notice raise failed");
        }
    } else if notify_back {
        if let Err(e) =
            meridian::notices::clear_typed(pool, "tray.daemon_quiet", "system.health").await
        {
            tracing::warn!(error = %e, "daemon-health notice clear failed");
        }
        send_back_online_toast(pool).await;
    } else if reconcile_stale {
        match meridian::notices::clear_typed_reporting(pool, "tray.daemon_quiet", "system.health")
            .await
        {
            // A notice really was sitting there from a prior process instance —
            // confirm the recovery the same way the normal path does.
            Ok(true) => send_back_online_toast(pool).await,
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, "daemon-health stale notice reconcile failed"),
        }
    }
}

/// "Back online" is a discrete confirmation, not a state to leave sitting as a
/// banner once the fault it answers is already cleared.
async fn send_back_online_toast(pool: &SqlitePool) {
    let dedup = format!(
        "system.health:back_online:{}",
        chrono::Utc::now().timestamp()
    );
    let n = meridian::notifications::NewNotification::event(
        &dedup,
        "system.health",
        "Back online.",
        "Picking up where you left off.",
    )
    .via(meridian::notifications::CHANNEL_NATIVE);
    if let Err(e) = meridian::notifications::enqueue(pool, n).await {
        tracing::warn!(error = %e, "back-online notification enqueue failed");
    }
}

/// The actions a health tick can trigger. Among the three *notice* actions
/// (`notify_down` / `notify_back` / `reconcile_stale`) at most one is ever true.
/// `attempt_restart` is orthogonal — it is the recovery *action*, decoupled
/// from the notification debounce so a cold-down daemon still gets restarted.
struct HealthNoticeDecision {
    /// Fire exactly one automatic daemon restart on a confirmed outage (the 2nd
    /// consecutive failed poll). Deliberately NOT gated on `daemon_was_healthy`:
    /// a daemon already down when this process started has no other path back
    /// up (see `refresh_health`). Fires once per down-episode — the failure
    /// counter only equals 2 on a single tick.
    attempt_restart: bool,
    notify_down: bool,
    notify_back: bool,
    reconcile_stale: bool,
}

/// Pure decision over one health tick's state transition, extracted out of
/// `refresh_health` so the debounce/reconcile rules are unit-testable without
/// a DB, `AppState`'s `Mutex`, or `check_health()`'s IO. Mutates the three
/// counters exactly as `refresh_health` used to inline; keep this in sync
/// with that call site.
fn decide_health_notice(
    now_healthy: bool,
    consecutive_health_failures: &mut u32,
    daemon_was_healthy: &mut bool,
    startup_health_reconciled: &mut bool,
) -> HealthNoticeDecision {
    // The 2nd consecutive failure is the "confirmed outage" edge — one miss is a
    // transient blip. Both the recovery attempt and the down-notice key off it,
    // but they diverge on `daemon_was_healthy`: recovery always fires (a
    // never-yet-healthy daemon still needs bringing back), the notice stays
    // silent on a cold start (a daemon that hasn't come up yet isn't news).
    let second_failure = if !now_healthy {
        *consecutive_health_failures += 1;
        *consecutive_health_failures == 2
    } else {
        false
    };
    let attempt_restart = second_failure;
    let notify_down = second_failure && *daemon_was_healthy;
    // Fire "back online" only when we had previously sent a "gone quiet" notification
    // (consecutive_health_failures reached 2), so a brief outage during startup is silent.
    let notify_back = now_healthy && *consecutive_health_failures >= 2;

    // One-shot, and mutually exclusive with notify_back: if THIS process's own
    // counter already reached 2 failures and recovered, notify_back above is
    // the right (and already correct) path. reconcile_stale only fires on
    // this process's first-ever healthy tick when that didn't happen — e.g.
    // the daemon was already healthy again by the time this fresh tray
    // process started, so a `tray.daemon_quiet` notice raised by the PREVIOUS
    // instance is sitting in system_notices with no process left able to
    // observe the transition that would clear it.
    let reconcile_stale = now_healthy && !*startup_health_reconciled && !notify_back;

    if now_healthy {
        *consecutive_health_failures = 0;
        *daemon_was_healthy = true;
        *startup_health_reconciled = true;
    }

    HealthNoticeDecision {
        attempt_restart,
        notify_down,
        notify_back,
        reconcile_stale,
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
    let Ok(mut s) = state.lock() else {
        tracing::warn!("refresh_active: state lock poisoned");
        return;
    };
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
            let Ok(mut s) = state.lock() else {
                tracing::warn!("refresh_current_task: state lock poisoned");
                return;
            };
            s.current_task_key = ct.as_ref().map(|c| c.key.clone());
            s.task_percent = ct.as_ref().and_then(|c| c.percent);
            s.task_title = ct.as_ref().and_then(|c| c.title.clone());
            s.task_status_category = ct.as_ref().and_then(|c| c.status_category.clone());
            s.task_priority = ct.as_ref().and_then(|c| c.priority.clone());
            s.task_spent_today_s = ct
                .as_ref()
                .map(|c| c.spent_today_s.max(0) as u64)
                .unwrap_or(0);
            s.task_estimate_s = ct
                .as_ref()
                .and_then(|c| c.estimate_s.map(|e| e.max(0) as u64));
        }
        Err(e) => tracing::warn!(error = %e, "refresh_current_task failed"),
    }
}

/// First foreground window title from the active session's `window_titles` JSON.
/// Tolerates both shapes the column has carried — `["title", …]` and
/// `[{"title": "…", "count": n}, …]` — and drops empties.
fn top_title(titles: &serde_json::Value) -> Option<String> {
    titles.as_array()?.iter().find_map(|e| {
        e.as_str()
            .map(str::to_string)
            .or_else(|| e.get("title").and_then(|t| t.as_str()).map(str::to_string))
            .filter(|s| !s.is_empty())
    })
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
            let Ok(mut s) = state.lock() else {
                tracing::warn!("refresh_today: state lock poisoned");
                return;
            };
            s.focus_s = t.focus_s.max(0) as u64;
            s.switch_count = t.switch_count.max(0) as u32;
            s.today = bd;
        }
        Err(e) => tracing::warn!(error = %e, "refresh_today failed"),
    }
}

/// Track the drafted-worklog count and today's logged time for the popover
/// (direct DB). "Logged" sums `time_spent_seconds` across today's
/// approved/posted REAL work logs — the same definition the dashboard's
/// Overview panel uses for its "Logged" stat. `is_proposed` items (tier-3
/// proposed tickets — `meridian_core::worklogs::get_worklogs` folds them in
/// with a real `approved`/`posted` state before the daemon's sweep has
/// actually created the ticket + posted a worklog) are excluded — they
/// aren't logged work yet, just an approved intent. The "worklog ready"
/// notification itself is emitted by the daemon's worklog scheduler into the
/// notification outbox and delivered via `drain_notifications` — not here.
pub(super) async fn refresh_worklogs(pool: &SqlitePool, state: &Arc<Mutex<AppState>>) {
    let today = meridian_core::date::today_string();
    match meridian_core::worklogs::get_worklogs(pool, &today).await {
        Ok(w) => {
            let count = w.items.iter().filter(|i| i.state == "drafted").count() as u32;
            let logged_s: u64 = w
                .items
                .iter()
                .filter(|i| !i.is_proposed && (i.state == "approved" || i.state == "posted"))
                .map(|i| i.time_spent_seconds.max(0) as u64)
                .sum();
            let Ok(mut s) = state.lock() else {
                tracing::warn!("refresh_worklogs: state lock poisoned");
                return;
            };
            s.drafts_count = count;
            s.logged_s = logged_s;
        }
        Err(e) => tracing::warn!(error = %e, "refresh_worklogs failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bug this fix targets: a fresh tray process (all counters at
    /// their `AppState::default()` values) whose very first health check is
    /// already healthy — e.g. the daemon recovered from an overnight sleep
    /// gap before this process even started. The normal down→up transition
    /// can never be observed by this process, so reconciliation must fire
    /// directly instead.
    #[test]
    fn reconciles_stale_notice_on_first_healthy_tick_after_restart() {
        let mut failures = 0u32;
        let mut was_healthy = false;
        let mut reconciled = false;

        let d = decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);

        assert!(!d.attempt_restart, "a healthy tick must not restart");
        assert!(!d.notify_down);
        assert!(!d.notify_back);
        assert!(
            d.reconcile_stale,
            "must reconcile on the first healthy tick"
        );
        assert!(reconciled, "startup_health_reconciled must be latched true");
        assert!(was_healthy);
        assert_eq!(failures, 0);
    }

    /// The one-shot must not re-fire on every subsequent healthy tick — only
    /// the first one this process observes.
    #[test]
    fn does_not_reconcile_again_after_the_first_healthy_tick() {
        let mut failures = 0u32;
        let mut was_healthy = false;
        let mut reconciled = false;

        decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);
        let second = decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);

        assert!(!second.reconcile_stale);
        assert!(!second.notify_back);
    }

    /// Normal case, unchanged by this fix: this process itself observes 2
    /// consecutive failures (after having been healthy at least once), then
    /// recovers — notify_back handles it, not the startup reconciler.
    #[test]
    fn normal_down_then_up_uses_notify_back_not_reconcile() {
        let mut failures = 0u32;
        let mut was_healthy = true; // already established healthy earlier
        let mut reconciled = true; // startup reconciliation already happened

        let first_fail =
            decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(!first_fail.notify_down); // only 1 consecutive failure so far
        assert!(!first_fail.attempt_restart); // one blip is not yet an outage

        let second_fail =
            decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(second_fail.notify_down); // 2nd consecutive failure
        assert!(second_fail.attempt_restart); // …and the recovery attempt fires with it

        let recovered =
            decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(recovered.notify_back);
        assert!(!recovered.reconcile_stale);
        assert!(!recovered.attempt_restart);
    }

    /// The bug this change fixes: a tray process that starts (or is already
    /// running) while the daemon is down and was never observed healthy this
    /// session. `daemon_was_healthy` stays false, so no down-*notice* fires —
    /// but the automatic *restart* must still fire on the 2nd consecutive
    /// failure, because on a machine with no scheduled task the tray is the
    /// daemon's only supervisor. Before the fix, `notify_down` gated both and
    /// the daemon stayed dead until a manual "Restart daemon" click.
    #[test]
    fn cold_down_daemon_is_restarted_even_though_notice_stays_silent() {
        let mut failures = 0u32;
        let mut was_healthy = false; // never seen healthy this session
        let mut reconciled = false;

        let f1 = decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(
            !f1.attempt_restart,
            "one failure is a blip, not yet a restart"
        );
        assert!(!f1.notify_down);

        let f2 = decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(
            f2.attempt_restart,
            "a cold-down daemon MUST be auto-restarted on the 2nd failure"
        );
        assert!(
            !f2.notify_down,
            "…while the went-quiet notice stays silent on a cold start"
        );

        // It fires exactly once per episode — no restart loop while it stays down.
        let f3 = decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(
            !f3.attempt_restart,
            "must not re-fire every subsequent failed tick"
        );
    }

    /// Review follow-up: when a cold-down daemon's one restart attempt FAILS,
    /// `refresh_health` raises `tray.daemon_quiet` regardless of
    /// `daemon_was_healthy` (a failed recovery is news). This pins the invariant
    /// that lets that notice be cleared by the ordinary recovery path rather
    /// than leaking: `notify_back` keys off `consecutive_health_failures >= 2`,
    /// NOT `daemon_was_healthy`, so a cold-start episode that reached the restart
    /// (failures == 2) still fires `notify_back` on recovery.
    #[test]
    fn cold_start_failure_notice_is_cleared_on_recovery() {
        let mut failures = 0u32;
        let mut was_healthy = false; // cold start — never healthy this session
        let mut reconciled = false;

        decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled); // 1
        let f2 = decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled); // 2
        assert!(f2.attempt_restart, "restart attempted at the 2nd failure");
        assert!(
            !f2.notify_down,
            "cold start stays silent on the went-quiet notice"
        );
        // Daemon stays down (restart failed); the counter keeps climbing.
        decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled); // 3

        // Recovery: notify_back must fire so the cold-start-failure notice
        // (raised with the same `tray.daemon_quiet` id) gets cleared.
        let recovered =
            decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(
            recovered.notify_back,
            "recovery must clear the cold-start failure notice"
        );
    }

    /// Preserves existing (pre-fix) behavior: a cold-start outage that never
    /// establishes healthy first stays silent — `daemon_was_healthy` gates
    /// `notify_down` off — but recovery still clears via `notify_back` once
    /// the failure count has reached 2, same as before this change.
    #[test]
    fn cold_start_outage_is_silent_until_recovery() {
        let mut failures = 0u32;
        let mut was_healthy = false;
        let mut reconciled = false;

        let f1 = decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        let f2 = decide_health_notice(false, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(!f1.notify_down);
        assert!(
            !f2.notify_down,
            "never-yet-healthy daemon must not notify down"
        );
        // Silence is about the *notification*; recovery still happens (see
        // `cold_down_daemon_is_restarted_even_though_notice_stays_silent`).
        assert!(f2.attempt_restart);

        let recovered =
            decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(recovered.notify_back);
        assert!(!recovered.reconcile_stale);
    }

    async fn fresh_db() -> meridian_core::SqlitePool {
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = meridian_core::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("../../src/migrations")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    /// End-to-end DB check for the scenario the code review flagged as
    /// hardest to unit-test: a `tray.daemon_quiet` notice raised by one
    /// process instance, still sitting in `system_notices` when a FRESH
    /// process's first-ever health check comes back healthy. Composes
    /// `decide_health_notice` with the real `meridian::notices` DB calls
    /// against an in-memory schema-migrated database — exactly the sequence
    /// `refresh_health`'s `reconcile_stale` branch runs — rather than a mock.
    /// Short of driving an actual packaged tray process through a real
    /// sleep/restart cycle, this is the strongest verification available
    /// from an automated test.
    #[tokio::test]
    async fn stale_notice_from_a_prior_process_instance_is_actually_cleared() {
        let pool = fresh_db().await;

        // The PREVIOUS process instance raised the fault before it exited.
        meridian::notices::raise_typed(
            &pool,
            meridian::notices::Notice {
                id: "tray.daemon_quiet",
                severity: "warning",
                title: "Meridian went quiet.",
                detail: "Tap to check what happened.",
                remedy: None,
                event_key: "system.health",
                deep_link: Some(meridian_core::notifications::deep_links::LOGS),
            },
        )
        .await
        .unwrap();

        // A FRESH process's AppState defaults, observing its first-ever health
        // tick as healthy (the daemon recovered before this process started).
        let mut failures = 0u32;
        let mut was_healthy = false;
        let mut reconciled = false;
        let decision = decide_health_notice(true, &mut failures, &mut was_healthy, &mut reconciled);
        assert!(
            decision.reconcile_stale,
            "a fresh process's first healthy tick must trigger reconciliation"
        );
        assert!(!decision.notify_back);

        // Exactly what refresh_health's reconcile_stale branch does.
        let cleared =
            meridian::notices::clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
                .await
                .unwrap();
        assert!(
            cleared,
            "a real stale notice must report as actually cleared"
        );
        send_back_online_toast(&pool).await;

        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM system_notices WHERE notice_id = 'tray.daemon_quiet'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "the banner's backing row must be gone");

        let toast_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE event_key = 'system.health' AND title = 'Back online.'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            toast_count, 1,
            "a back-online toast must confirm the recovery"
        );
    }
}
