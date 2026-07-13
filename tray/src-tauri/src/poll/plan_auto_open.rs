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
//! 1. marker file `~/.meridian/plan_auto_opened` already holds today's date
//! 2. `auto_open_plan` disabled in settings
//! 3. not onboarded (no `~/.meridian/onboarded` — don't open over the wizard)
//! 4. today's plan already confirmed/skipped (`daily_plan_meta`) — the marker
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

use std::path::{Path, PathBuf};
use tauri::Emitter;

/// Marker file under `~/.meridian` holding the local date (`YYYY-MM-DD`) of
/// the last auto-open. Same convention as the `onboarded` marker.
const MARKER_FILE: &str = "plan_auto_opened";

/// Once per local day, open the dashboard on the Plan modal. See the module
/// docs for the gate order and rationale.
#[tracing::instrument(skip(app, pool))]
pub(crate) async fn maybe_auto_open_plan(app: &tauri::AppHandle, pool: &meridian_core::SqlitePool) {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return;
    }
    let meridian_dir = Path::new(&home).join(".meridian");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // 1. Already fired today — one file stat on the common path.
    let marker = marker_path(&meridian_dir);
    let marker_contents = std::fs::read_to_string(&marker).unwrap_or_default();
    if already_opened(&marker_contents, &today) {
        return;
    }

    // 2. Feature toggle.
    if !meridian_core::settings::load_runtime_settings().auto_open_plan {
        return;
    }

    // 3. Don't open the planner over (or instead of) the first-run wizard.
    if !meridian_dir.join("onboarded").exists() {
        return;
    }

    // 4. Day already planned (confirmed or skipped, e.g. via the notification
    //    nudge) — settle the day without opening.
    if meridian_core::plan::plan_handled(pool, &today).await {
        write_marker(&marker, &today);
        tracing::info!(%today, "plan auto-open: plan already handled — marking day settled");
        return;
    }

    // Fire. Marker first (crash-safe vs KeepAlive relaunch loops), then hand
    // the target to the (possibly not-yet-existing) window via the pending
    // deep link, open/focus the window, and also emit for an already-open
    // window (which won't remount and thus never pulls). Double delivery is
    // idempotent — the shell opens the same modal.
    write_marker(&marker, &today);
    crate::deep_link::set_pending(app, "/plan");
    crate::tray::open_native_dashboard(app);
    let _ = app.emit_to("dashboard", "dashboard-navigate", "/plan");
    tracing::info!(%today, "plan auto-open: opened dashboard on the Plan view");
}

/// `~/.meridian/plan_auto_opened`. Split out (and pure over the dir) so tests
/// can point it at a temp dir.
fn marker_path(meridian_dir: &Path) -> PathBuf {
    meridian_dir.join(MARKER_FILE)
}

/// True when the marker already records `today`. Pure so the date comparison
/// (whitespace tolerance, stale dates, empty/missing file) is unit-testable.
fn already_opened(marker_contents: &str, today: &str) -> bool {
    marker_contents.trim() == today
}

/// Persist today's date into the marker. A failure is logged but not fatal:
/// the worst case is the open firing again after a tray restart — annoying,
/// not incorrect.
fn write_marker(marker: &Path, today: &str) {
    if let Err(e) = std::fs::write(marker, today) {
        tracing::warn!(error = %e, path = %marker.display(), "plan auto-open: marker write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_opened_matches_today_only() {
        assert!(already_opened("2026-07-13", "2026-07-13"));
        assert!(
            already_opened("2026-07-13\n", "2026-07-13"),
            "trailing newline tolerated"
        );
        assert!(
            already_opened("  2026-07-13  ", "2026-07-13"),
            "surrounding whitespace tolerated"
        );
        assert!(
            !already_opened("", "2026-07-13"),
            "missing/empty marker → not opened"
        );
        assert!(
            !already_opened("2026-07-12", "2026-07-13"),
            "stale marker → fire again"
        );
        assert!(!already_opened("garbage", "2026-07-13"));
    }

    #[test]
    fn marker_round_trips_through_the_fs() {
        let dir = std::env::temp_dir().join(format!("meridian-plan-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = marker_path(&dir);
        assert_eq!(marker.file_name().unwrap(), MARKER_FILE);

        assert!(!already_opened(
            &std::fs::read_to_string(&marker).unwrap_or_default(),
            "2026-07-13"
        ));
        write_marker(&marker, "2026-07-13");
        assert!(already_opened(
            &std::fs::read_to_string(&marker).unwrap(),
            "2026-07-13"
        ));
        // Next day: stale marker no longer counts, and a rewrite wins.
        assert!(!already_opened(
            &std::fs::read_to_string(&marker).unwrap(),
            "2026-07-14"
        ));
        write_marker(&marker, "2026-07-14");
        assert!(already_opened(
            &std::fs::read_to_string(&marker).unwrap(),
            "2026-07-14"
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
