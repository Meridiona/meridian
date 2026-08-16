//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! "What's New" auto-open — once per app version, open the dashboard on the
//! What's New modal so a fresh update doesn't land silently.
//!
//! Runs every poll tick (30 s), same trigger choice as [`crate::poll::plan_auto_open`]:
//! tick 0 fires at launch, which is exactly when a version bump would show up
//! (the updater relaunches into the new binary), and the recurring check also
//! catches an `enforce_minimum_version` forced install without a full restart
//! of the poll loop.
//!
//! Gates, cheapest first:
//! 1. `~/.meridian/last_seen_version` already matches the running version —
//!    one file stat + read on the common path.
//! 2. not onboarded (no `~/.meridian/onboarded`) — don't open over the wizard.
//! 3. the dashboard window is already open — never yank the user's current
//!    view (whatever it is: the Plan modal from the sibling auto-open just
//!    above it in the poll loop, or anything they opened themselves) over to
//!    What's New. Deferred, not skipped: the marker write only happens on the
//!    fire path below, so this retries every tick until the window is closed
//!    and this fires on a fresh open. Worst case for a user who never closes
//!    the dashboard: the auto-open never fires for them, but the toolbar's
//!    "What's New" nav pill is always there as a manual fallback.
//!
//! The marker is written BEFORE the window opens, same crash-safety trade-off
//! as `plan_auto_open`: worst case of a crash between write and open is one
//! missed popup, never a repeat-every-relaunch loop under launchd `KeepAlive`.
//!
//! # Who calls this
//! [`crate::poll::run_poll_loop`], every tick the DB pool is open.
//!
//! # Related
//! - [`crate::commands::whats_new`] — the marker read/write + the modal's data command.
//! - [`crate::deep_link`] — how the freshly opened window learns to show the
//!   What's New modal.

use crate::commands::whats_new;
use tauri::Manager;

/// Once per app version, open the dashboard on the What's New modal. See the
/// module docs for the gate order and rationale.
#[tracing::instrument(skip(app))]
pub(crate) async fn maybe_auto_open_whats_new(app: &tauri::AppHandle) {
    let Some(home) = meridian_core::paths::home_dir() else {
        return;
    };
    let home = home.as_path();
    let current_version = app.package_info().version.to_string();

    // 1. Already seen this version — the common case on every tick after the
    //    first since the last update.
    if !whats_new::has_unseen_release_notes(home, &current_version) {
        return;
    }

    // 2. Don't open over (or instead of) the first-run wizard.
    if !home.join(".meridian").join("onboarded").exists() {
        return;
    }

    // 2b. …or over the WALKTHROUGH. Same reasoning as the sibling plan
    //     auto-open: `onboarded` marks the end of the WIZARD, and the tour runs
    //     after it, so this gate stops covering onboarding at the moment the
    //     tour starts. Deferred, not skipped - the marker below is only written
    //     on the fire path, so this retries once the tour is finished.
    //
    //     THIS one has to read `tour_owed`, not `walkthrough_in_progress`: this
    //     function is what OPENS the window the tour mounts in, so "is the tour
    //     on screen" is necessarily false at the moment it is asked here. It was
    //     measured losing that race by 2.1 s and drawing the changelog under a
    //     live tour. The entitlement marker is on disk before the window exists.
    if crate::commands::walkthrough::tour_owed(home) {
        tracing::debug!("whats-new auto-open: walkthrough owed or on screen - deferring");
        return;
    }

    // 3. Don't yank an already-open dashboard to a different view — covers
    //    both the sibling Plan auto-open (same tick) and any manual open.
    if app.get_webview_window("dashboard").is_some() {
        return;
    }

    // Fire. Marker first (crash-safe vs KeepAlive relaunch loops), then hand
    // the target to the window (parked for a fresh window's mount-time pull,
    // or emitted to an already-open one), then open/focus the window.
    if let Err(e) = whats_new::mark_whats_new_seen(home, &current_version) {
        tracing::warn!(error = %e, "whats-new auto-open: marker write failed");
    }
    crate::deep_link::navigate_dashboard(app, meridian_core::notifications::deep_links::WHATS_NEW);
    crate::tray::open_native_dashboard(app);
    tracing::info!(version = %current_version, "whats-new auto-open: opened dashboard on the What's New view");
}
