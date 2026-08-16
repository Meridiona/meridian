//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Periodic re-checks of the OS permissions the notification system and
//! capture pipeline depend on.
//!
//! Each of these is checked exactly once during onboarding
//! ([`crate::commands::setup`]) and never again — if the user revokes one
//! afterward (System Settings, or a macOS reset), the app degrades silently:
//! capture starts producing thin/no frames, or every native toast just stops
//! firing with nothing to say why. This module closes that gap by raising a
//! `system_notices` row per permission on loss, cleared on restore. The
//! dashboard banner this produces is the resilient path by construction — it
//! reads `system_notices` independent of the toast channel, so it's the one
//! notice guaranteed to reach the user even when the thing it's reporting is
//! "toasts are broken."
//!
//! The notification leg is the one check that is genuinely cross-platform:
//! [`crate::sys::notification_permission_state`] reads a real
//! `UNUserNotificationCenter` authorization on macOS and a real WinRT
//! `ToastNotifier::Setting()` on Windows, so this raises `system.notif_permission`
//! identically on either OS. Accessibility and Screen Recording stay
//! macOS-only checks in practice — Windows has no TCC analogue, so
//! [`crate::sys::accessibility_trusted`] / [`crate::sys::screen_recording_trusted`]
//! report `true` there and their `check_bool_permission` calls never fire.
//!
//! # Why a reading has to persist before we believe it ([`PermissionDebounce`])
//!
//! **A raised notice can be withdrawn. A delivered toast cannot.**
//! [`meridian::notices::clear_typed`] deletes the `system_notices` row AND the
//! `notifications` outbox row (so a re-occurrence toasts again), but nothing in
//! macOS lets us un-deliver a banner the user has already been shown. So the
//! cost of the two error directions is wildly asymmetric: reporting a real
//! revocation a few minutes late costs nothing (the user cannot act in seconds
//! anyway), while reporting a phantom one costs a permanently-parked "tracking
//! has effectively stopped" notification that no later recovery can retract.
//!
//! That is not hypothetical. On 2026-08-16 a freshly-built (Developer-ID
//! signed) `Meridian.app` raised BOTH `Accessibility access is off` and
//! `Screen Recording access is off` on the first check after launch, while
//! capture was demonstrably healthy the entire time — frames landing 5s ago,
//! a11y text at 100% on the focused app, OCR text present on the apps without
//! an a11y tree. Capture runs **in-process in this very tray**, so a genuinely
//! false [`crate::sys::accessibility_trusted`] could not have coexisted with
//! it. The notices self-cleared on a later tick and `system_notices` was empty
//! by the time anyone looked; the toasts stayed on screen for hours. Code
//! identity was NOT the cause and is worth ruling out explicitly before
//! reaching for that explanation — `codesign -d -r-` produced byte-identical
//! designated requirements for the new build and the installed one.
//!
//! Hence [`REVOKE_HOLD`]: a permission must read off *continuously* for five
//! minutes before we tell the user. A fresh process starts with an empty map,
//! so this doubles as a launch grace period — which is exactly when the
//! phantom readings were observed — without needing a second mechanism, or a
//! theory about which macOS internal is warming up.
//!
//! # Related
//! - [`crate::sys::notification_permission_state`] / [`crate::sys::accessibility_trusted`] /
//!   [`crate::sys::screen_recording_trusted`] — the shared probes, also used by the setup wizard.
//! - [`crate::commands::setup`] — the one-shot onboarding checks this periodically re-runs.
//! - [`meridian::notices`] — the fault-bus this writes into.

use meridian_core::SqlitePool;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a permission must read "off" *without interruption* before it is
/// reported to the user.
///
/// Five minutes at the ~60s check cadence means six consecutive off readings.
/// Sized against the cost of being wrong in each direction (see the module
/// docs): late is free, early is permanent.
const REVOKE_HOLD: Duration = Duration::from_secs(300);

/// One permission's notice copy — grouped so [`check_bool_permission`] stays
/// under clippy's argument limit now that it also takes the debounce and a
/// clock (same convention as `BlockBounds` in `src/etl/runner.rs`).
struct Permission {
    /// `system_notices.notice_id`, also the debounce key.
    id: &'static str,
    event_key: &'static str,
    title: &'static str,
    detail: &'static str,
    remedy: &'static str,
}

const NOTIFICATIONS: Permission = Permission {
    id: "tray.notifications_denied",
    event_key: "system.notif_permission",
    title: "Notifications are off",
    detail: "Meridian can't show reminders or alerts without them.",
    remedy: NOTIFICATION_REMEDY,
};

const ACCESSIBILITY: Permission = Permission {
    id: "tray.accessibility_revoked",
    event_key: "system.capture_permission",
    title: "Accessibility access is off",
    detail: "Meridian can't see window and app activity without it, so tracking has effectively stopped.",
    remedy: "Open System Settings \u{2192} Privacy & Security to re-grant it.",
};

const SCREEN_RECORDING: Permission = Permission {
    id: "tray.screen_recording_revoked",
    event_key: "system.capture_permission",
    title: "Screen Recording access is off",
    detail: "Meridian can't read on-screen text without it, so tracking has effectively stopped.",
    remedy: "Open System Settings \u{2192} Privacy & Security to re-grant it.",
};

/// What to do with a permission this tick, once [`REVOKE_HOLD`] is applied.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) enum Verdict {
    /// Granted — clear any raised notice (idempotent when there is none).
    Clear,
    /// Off, and it has been off long enough to be believed.
    Raise,
    /// Off, but not for long enough yet. Touch nothing: do not raise (the
    /// reading may be phantom) and do not clear (it may be real, and an
    /// already-raised notice from before a restart must survive).
    Hold,
}

/// Per-permission "how long has this been reading off" tracker.
///
/// Owned by the poll loop and threaded through [`check_permissions`] rather
/// than kept in a static, so the state dies with the loop and the decision
/// stays a pure function of (previous state, reading, clock) — which is what
/// makes it testable without a live Tauri app or a real TCC grant.
#[derive(Default)]
pub(super) struct PermissionDebounce {
    /// When each id started its current unbroken run of "off" readings.
    /// Absent → the last reading was granted (or none has been taken yet).
    first_off: HashMap<&'static str, Instant>,
}

impl PermissionDebounce {
    /// Fold one reading into the debounce and say what to do about it.
    ///
    /// `now` is a parameter rather than an `Instant::now()` call so tests can
    /// drive the clock; production passes one `Instant` per tick so all three
    /// permissions are judged against the same instant.
    pub(super) fn observe(&mut self, id: &'static str, granted: bool, now: Instant) -> Verdict {
        if granted {
            self.first_off.remove(id);
            return Verdict::Clear;
        }
        // First off-reading of this run starts the clock and, by definition,
        // has not elapsed yet — so a lone blip can never raise.
        let started = *self.first_off.entry(id).or_insert(now);
        if now.duration_since(started) >= REVOKE_HOLD {
            Verdict::Raise
        } else {
            Verdict::Hold
        }
    }
}

/// Re-check notification, accessibility, and screen-recording permission and
/// raise/clear the matching `system_notices` row for each. Runs on the same
/// cadence as [`super::refresh::refresh_health`] (every 2nd tick, ~60s) — a
/// TCC read is cheap, no need for the full 30s tick. Skipped entirely in
/// unbundled runs (`tauri dev`): the notification plugin never registers
/// there and TCC prompts behave differently outside a signed `.app`, so
/// there's nothing meaningful to police.
///
/// All three raw readings are recorded on this span. They are the whole
/// diagnostic: without them the spool shows an unbroken row of bare
/// `check_permissions` spans and cannot answer "was it actually off?" — which
/// is exactly why the 2026-08-16 phantom notices had to be chased through the
/// encrypted DB instead of `meridian logs`.
#[tracing::instrument(
    skip(app, pool, debounce),
    fields(
        notifications = tracing::field::Empty,
        accessibility = tracing::field::Empty,
        screen_recording = tracing::field::Empty,
    )
)]
pub(super) async fn check_permissions(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    debounce: &mut PermissionDebounce,
) {
    if !crate::sys::is_bundled() {
        return;
    }

    let notifications = notification_permission_granted(app).await;
    let accessibility = crate::sys::accessibility_trusted();
    let screen_recording = crate::sys::screen_recording_trusted();

    let span = tracing::Span::current();
    span.record("notifications", notifications);
    span.record("accessibility", accessibility);
    span.record("screen_recording", screen_recording);

    // One instant for all three, so a slow DB write between them can't make
    // two permissions disagree about when "now" was.
    let now = Instant::now();
    check_bool_permission(pool, debounce, now, &NOTIFICATIONS, notifications).await;
    check_bool_permission(pool, debounce, now, &ACCESSIBILITY, accessibility).await;
    check_bool_permission(pool, debounce, now, &SCREEN_RECORDING, screen_recording).await;
}

/// Notifications-specific remedy text — the one `check_bool_permission` caller
/// that now fires on both platforms. macOS keeps the exact original copy
/// unchanged (Accessibility/Screen Recording still use the same string
/// inline); only Windows — new behaviour, nothing to stay compatible with —
/// gets an OS-accurate string instead of the macOS pane name.
#[cfg(target_os = "windows")]
const NOTIFICATION_REMEDY: &str =
    "Open Settings \u{2192} System \u{2192} Notifications to re-grant it.";
#[cfg(not(target_os = "windows"))]
const NOTIFICATION_REMEDY: &str =
    "Open System Settings \u{2192} Privacy & Security to re-grant it.";

/// `true` when notifications are usable. Only an explicit `Denied` counts as
/// off — an unknown/undetermined state is not evidence of revocation.
async fn notification_permission_granted(app: &tauri::AppHandle) -> bool {
    !matches!(
        crate::sys::notification_permission_state(app).await,
        Some(tauri_plugin_notifications::PermissionState::Denied)
    )
}

/// Raise `perm` when it has read off for [`REVOKE_HOLD`], clear it when
/// granted, and do nothing while the reading is too young to trust.
///
/// Raise/clear are both idempotent — [`meridian::notices::raise_typed`] /
/// [`meridian::notices::clear_typed`] upsert/delete — so a steady-state tick
/// that finds nothing changed is a cheap no-op (same pattern as the daemon's
/// own `etl.failed`/`pm.*` checks).
async fn check_bool_permission(
    pool: &SqlitePool,
    debounce: &mut PermissionDebounce,
    now: Instant,
    perm: &Permission,
    granted: bool,
) {
    let verdict = debounce.observe(perm.id, granted, now);
    let result = match verdict {
        Verdict::Hold => {
            // Logged at debug, not warn: this is the *expected* state for the
            // first few minutes of every process, and the whole point is that
            // it is not yet news.
            tracing::debug!(
                id = perm.id,
                "permission reads off but has not held long enough to report"
            );
            return;
        }
        Verdict::Clear => meridian::notices::clear_typed(pool, perm.id, perm.event_key).await,
        Verdict::Raise => {
            meridian::notices::raise_typed(
                pool,
                meridian::notices::Notice {
                    id: perm.id,
                    severity: "warning",
                    title: perm.title,
                    detail: perm.detail,
                    remedy: Some(perm.remedy),
                    event_key: perm.event_key,
                    deep_link: Some(meridian_core::notifications::deep_links::SETTINGS),
                },
            )
            .await
        }
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, id = perm.id, "permission notice write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::{PermissionDebounce, Verdict, ACCESSIBILITY, NOTIFICATION_REMEDY, REVOKE_HOLD};
    use std::time::{Duration, Instant};

    /// THE regression. On 2026-08-16 both capture permissions read off on the
    /// first check after a rebuilt `.app` launched, raised, toasted, and then
    /// self-cleared — leaving two "tracking has effectively stopped"
    /// notifications parked in Notification Center that no recovery could
    /// retract (`clear_typed` deletes the outbox row, not the delivered
    /// banner).
    ///
    /// A fresh `PermissionDebounce` is precisely the state a just-launched
    /// tray is in, so this reproduces that launch exactly.
    #[test]
    fn a_permission_that_reads_off_at_launch_does_not_notify() {
        let mut d = PermissionDebounce::default();
        assert_eq!(
            d.observe(ACCESSIBILITY.id, false, Instant::now()),
            Verdict::Hold,
            "a single off-reading raised a notice - the toast it produces cannot be un-delivered"
        );
    }

    /// The other half of that incident: it recovered. Nothing may have been
    /// said in the meantime, and the recovery must reset the clock so the next
    /// blip starts from zero rather than inheriting the earlier run's age.
    #[test]
    fn an_off_reading_that_recovers_never_notifies() {
        let mut d = PermissionDebounce::default();
        let t0 = Instant::now();

        assert_eq!(d.observe(ACCESSIBILITY.id, false, t0), Verdict::Hold);
        assert_eq!(
            d.observe(ACCESSIBILITY.id, true, t0 + Duration::from_secs(60)),
            Verdict::Clear
        );

        // Long past REVOKE_HOLD measured from t0 - but the run was broken, so
        // this is a brand-new one and must hold again.
        assert_eq!(
            d.observe(
                ACCESSIBILITY.id,
                false,
                t0 + REVOKE_HOLD + Duration::from_secs(60)
            ),
            Verdict::Hold,
            "a recovery did not reset the clock - the next blip inherits the old run's age"
        );
    }

    /// A genuine revocation still has to reach the user. The boundary is
    /// inclusive, and once raised it stays raised (the notice upsert refreshes
    /// `raised_at` every tick, as before).
    #[test]
    fn a_sustained_off_reading_is_reported_once_the_hold_elapses() {
        let mut d = PermissionDebounce::default();
        let t0 = Instant::now();

        assert_eq!(d.observe(ACCESSIBILITY.id, false, t0), Verdict::Hold);
        assert_eq!(
            d.observe(
                ACCESSIBILITY.id,
                false,
                t0 + REVOKE_HOLD - Duration::from_secs(1)
            ),
            Verdict::Hold,
            "raised one second early - the hold boundary is off by one"
        );
        assert_eq!(
            d.observe(ACCESSIBILITY.id, false, t0 + REVOKE_HOLD),
            Verdict::Raise
        );
        assert_eq!(
            d.observe(ACCESSIBILITY.id, false, t0 + REVOKE_HOLD * 2),
            Verdict::Raise,
            "stopped raising while still off - the notice would stop being refreshed"
        );
    }

    /// Each permission ages on its own clock. Accessibility and Screen
    /// Recording share an `event_key`, so keying the debounce on that instead
    /// of the notice id would let one permission's long outage instantly raise
    /// the other on its first off-reading — the exact bug, reintroduced.
    #[test]
    fn permissions_do_not_share_a_clock() {
        let mut d = PermissionDebounce::default();
        let t0 = Instant::now();

        assert_eq!(d.observe(super::ACCESSIBILITY.id, false, t0), Verdict::Hold);
        assert_eq!(
            d.observe(super::ACCESSIBILITY.id, false, t0 + REVOKE_HOLD),
            Verdict::Raise
        );
        assert_eq!(
            d.observe(super::SCREEN_RECORDING.id, false, t0 + REVOKE_HOLD),
            Verdict::Hold,
            "one permission's outage aged another - they share an event_key, not a clock"
        );
    }

    /// Granted is believed immediately and unconditionally: the asymmetry runs
    /// one way only. Delaying a *clear* would leave a stale banner up after the
    /// user has already fixed the thing it complains about.
    #[test]
    fn a_granted_reading_clears_without_waiting() {
        let mut d = PermissionDebounce::default();
        assert_eq!(
            d.observe(ACCESSIBILITY.id, true, Instant::now()),
            Verdict::Clear
        );
    }

    /// The debounce is only worth anything if the notice writer actually goes
    /// through it. Source-scanned because `check_bool_permission` needs a live
    /// pool and a real TCC grant to drive — the same reason `reopen.rs`'s gate
    /// tests scan rather than call.
    #[test]
    fn the_notice_writer_routes_through_the_debounce() {
        let src = include_str!("permissions.rs");
        assert!(
            src.contains("let verdict = debounce.observe(perm.id, granted, now);"),
            "check_bool_permission no longer consults the debounce - a single \
             phantom reading can toast again, permanently"
        );
        assert!(
            src.contains("Verdict::Hold => {"),
            "the Hold verdict is no longer handled - it must write nothing at all"
        );
    }

    /// Windows-only behaviour: `system.notif_permission` can now actually
    /// fire on Windows (see `sys::notification_permission_state`), so its
    /// remedy needs to point somewhere that exists on Windows — the macOS
    /// "Privacy & Security" pane name would be actively wrong there.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_remedy_points_to_windows_settings() {
        assert_eq!(
            NOTIFICATION_REMEDY,
            "Open Settings \u{2192} System \u{2192} Notifications to re-grant it."
        );
    }

    /// Regression guard for the explicit "don't touch macOS behaviour"
    /// requirement: this must stay byte-for-byte the string that shipped
    /// before the Windows notification-permission probe existed.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_remedy_is_unchanged_from_the_original_copy() {
        assert_eq!(
            NOTIFICATION_REMEDY,
            "Open System Settings \u{2192} Privacy & Security to re-grant it."
        );
    }
}
