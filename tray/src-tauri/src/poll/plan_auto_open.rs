//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Daily "Plan your day" auto-open — once per local day, open the dashboard
//! window on the Plan modal so the user starts the day by planning it.
//!
//! Runs every poll tick (30 s). The tick — not a launch hook — is the trigger
//! because on most days the machine just wakes from sleep with the tray
//! already running for days; tick 0 fires at launch, so one check covers both
//! "logged in this morning" and "new day started while the tray kept running".
//!
//! Gates, cheapest first:
//! 1. marker file `~/.meridian/plan_auto_opened` already records today (it
//!    holds a timestamp — `meridian_core::plan_marker` — refreshed when the
//!    user dismisses the planner, which the daemon reads to hold its plan
//!    nudge back until an hour after the open/dismissal)
//! 2. not onboarded (no `~/.meridian/onboarded` — don't open over the wizard)
//! 3. today's plan already confirmed/skipped (`daily_plan_meta`) — the marker
//!    is written WITHOUT opening, so the day stays settled
//!
//! The marker is written BEFORE the window opens: under launchd `KeepAlive` a
//! crash-relaunch loop would otherwise re-open the planner on every relaunch.
//! Worst case of the inverse (marker written, open failed) is one missed day.
//!
//! Deliberately NO hour gate (product call: "first activity of the day, any
//! time"). An unattended overnight wake (Power Nap) can therefore fire under
//! the lock screen — the window simply greets the user at unlock.
//!
//! # Who calls this
//! [`crate::poll::run_poll_loop`], every tick, once the DB pool is open.
//!
//! # Related
//! - [`crate::deep_link`] — how the freshly opened window learns to show the
//!   Plan modal.
//! - `src/daily_plan.rs` (daemon) — the sibling notification nudge; both read
//!   the same `daily_plan_meta` "already handled" state.

use meridian_core::plan_marker;
use std::path::Path;

/// Once per local day, open the dashboard on the Plan modal. See the module
/// docs for the gate order and rationale.
#[tracing::instrument(skip(app, pool))]
pub(crate) async fn maybe_auto_open_plan(app: &tauri::AppHandle, pool: &meridian_core::SqlitePool) {
    let Some(home) = meridian_core::paths::home_dir() else {
        return;
    };
    let meridian_dir = home.join(".meridian");
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();

    // 1. Already fired today — one file stat on the common path. The marker
    //    holds the open's RFC3339 timestamp (see `meridian_core::plan_marker`);
    //    the daemon reads the same file to hold its plan nudge back for an
    //    hour after the auto-open.
    let marker = plan_marker::marker_path(&meridian_dir);
    let marker_contents = std::fs::read_to_string(&marker).unwrap_or_default();
    if plan_marker::opened_today(&marker_contents, &today) {
        return;
    }

    // 2. Don't open the planner over (or instead of) the first-run wizard.
    if !meridian_dir.join("onboarded").exists() {
        return;
    }

    // 3. Day already planned (confirmed or skipped, e.g. via the notification
    //    nudge) — settle the day without opening.
    if meridian_core::plan::plan_handled(pool, &today).await {
        write_marker(&marker, &now);
        tracing::info!(%today, "plan auto-open: plan already handled — marking day settled");
        return;
    }

    // Fire. Marker first (crash-safe vs KeepAlive relaunch loops), then hand
    // the target to the window — parked for a fresh window's mount-time pull,
    // or emitted to an already-open one (`deep_link::navigate_dashboard`
    // picks; parking both ways would leave a stale target for a later manual
    // open) — then open/focus the window.
    write_marker(&marker, &now);
    crate::deep_link::navigate_dashboard(app, meridian_core::notifications::deep_links::PLAN);
    crate::tray::open_native_dashboard(app);
    tracing::info!(%today, "plan auto-open: opened dashboard on the Plan view");
}

/// Persist the open's timestamp into the marker. A failure is logged but not
/// fatal: the worst case is the open firing again after a tray restart —
/// annoying, not incorrect.
fn write_marker(marker: &Path, now: &chrono::DateTime<chrono::Local>) {
    if let Err(e) = std::fs::write(marker, plan_marker::stamp(now)) {
        tracing::warn!(error = %e, path = %marker.display(), "plan auto-open: marker write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Format/date semantics are covered in `meridian_core::plan_marker`; this
    // exercises the tray's write half against the real fs.
    #[test]
    fn marker_round_trips_through_the_fs() {
        let dir = std::env::temp_dir().join(format!("meridian-plan-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = plan_marker::marker_path(&dir);
        assert_eq!(marker.file_name().unwrap(), plan_marker::MARKER_FILE);

        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        assert!(!plan_marker::opened_today(
            &std::fs::read_to_string(&marker).unwrap_or_default(),
            &today
        ));
        write_marker(&marker, &now);
        let contents = std::fs::read_to_string(&marker).unwrap();
        assert!(plan_marker::opened_today(&contents, &today));
        // The daemon's hold-back can recover the instant from what we wrote.
        assert_eq!(
            plan_marker::opened_at(&contents).map(|t| t.timestamp()),
            Some(now.timestamp())
        );
        // A different day no longer matches (stale marker → fire again).
        assert!(!plan_marker::opened_today(&contents, "1999-01-01"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
