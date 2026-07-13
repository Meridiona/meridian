//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Pending deep-link handoff — how tray-side code steers a dashboard window
//! that may not exist yet to a specific in-app view (e.g. the Plan modal).
//!
//! A freshly built `dashboard` webview can't receive a Tauri event emitted
//! before its JS has mounted (no listener yet — the emit is lost), so pushing
//! the target at the window races window creation. This module inverts it into
//! a pull: the opener stores the target in managed state BEFORE opening the
//! window, and `MeridianTimelineShell` fetches-and-clears it on mount via the
//! `take_pending_deep_link` command. For a window that is ALREADY open (no
//! remount, so no pull), openers additionally emit a `dashboard-navigate`
//! event; double delivery is harmless — opening the same modal twice is
//! idempotent.
//!
//! Targets are the former route paths the notification producers already use
//! as `deep_link`s (`/plan`, `/worklogs`, …); the shell owns the mapping to a
//! modal.
//!
//! # Who calls this
//! - Setters: the poll loop's daily plan auto-open
//!   ([`crate::poll`]'s `plan_auto_open`) and the notification tap handler
//!   (`commands::record_notification_response`).
//! - Taker: the `take_pending_deep_link` command (`commands/system.rs`),
//!   invoked once by `ui/components/timeline/MeridianTimelineShell.tsx` on
//!   mount.
//!
//! # Related
//! - [`crate::tray`] — `open_native_dashboard`, the window the target lands in.

use tauri::Manager;

/// Managed slot holding at most one pending navigation target. Registered in
/// `lib.rs` via `.manage(...)`; a plain `std::sync::Mutex` (never held across
/// an await).
pub struct PendingDeepLink(pub std::sync::Mutex<Option<String>>);

/// Store `target` as the pending navigation for the next dashboard mount,
/// replacing any previous (undelivered) one — last writer wins, matching how
/// a user would perceive two rapid-fire opens.
pub fn set_pending(app: &tauri::AppHandle, target: &str) {
    let slot = app.state::<PendingDeepLink>();
    let mut guard = slot.0.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(prev) = guard.replace(target.to_string()) {
        tracing::debug!(prev, target, "pending deep link replaced before delivery");
    }
    tracing::debug!(target, "pending deep link set");
}

/// Fetch-and-clear the pending target. `None` on a plain dashboard open (the
/// common case) — the shell then just shows the default timeline.
pub fn take_pending(app: &tauri::AppHandle) -> Option<String> {
    let slot = app.state::<PendingDeepLink>();
    let taken = slot.0.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(t) = &taken {
        tracing::debug!(target = %t, "pending deep link taken");
    }
    taken
}
