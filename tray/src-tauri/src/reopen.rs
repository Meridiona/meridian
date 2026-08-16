//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Where an external re-activation lands, and whether onboarding is done.
//!
//! Split out of `lib.rs` because the routing decision is a pure function and the
//! `RunEvent::Reopen` handler that consumes it is not - keeping them apart is what
//! makes the decision unit-testable without a live Tauri app.
//!
//! # Who calls this
//! [`crate::run`]'s `RunEvent::Reopen` arm routes with [`reopen_target`];
//! [`onboarding_complete`] is the gate every dashboard-opening path consults
//! (`tray::open_native_dashboard`, `commands::system::open_dashboard`), re-exported
//! from the crate root as `crate::onboarding_complete` so those call sites read the
//! same in source-scanning tests.
//!
//! # Related
//! - [`crate::tray`] - the menu handlers that open the windows this routes to.
//! - [`crate::commands::setup`] - `is_first_run`, the same marker read from the
//!   setup wizard's side.

/// Which window an external re-activation (Spotlight, dock, `open -a Meridian`)
/// opens. Split out of the `RunEvent::Reopen` handler so the routing decision is
/// unit-testable without a live Tauri app.
///
/// Defined for every target (not just macOS) so its tests run in CI, which
/// builds the workspace on Linux; `allow(dead_code)` off-macOS keeps that from
/// tripping `-D warnings` where the reopen handler is compiled out.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReopenTarget {
    Dashboard,
    Wizard,
}

/// True when onboarding has completed — the `~/.meridian/onboarded` marker
/// exists under `home`. The inverse of [`commands::setup::is_first_run`]'s
/// check, kept a pure filesystem read so it can be tested against a temp dir.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn is_onboarded(home: &std::path::Path) -> bool {
    home.join(".meridian/onboarded").exists()
}

/// True when onboarding has completed, resolved against the real home directory.
///
/// The single gate every dashboard-opening path consults, so a fresh install
/// cannot reach the timeline before setup. [`is_onboarded`] above is the same
/// check as a pure function of a path (kept separate so it stays testable
/// against a temp dir); this is the one production code calls.
///
/// Uses [`meridian_core::paths::meridian_dir`] rather than joining onto a raw
/// `HOME` read — `HOME` is unset on Windows, where that resolves to a bogus
/// cwd-relative path and the marker can never be found. Same reasoning as
/// [`commands::setup::is_first_run`], and the same helper, so the two can never
/// disagree about whether this install is onboarded.
///
/// Unresolvable home → `false` → the wizard. Being wrong in that direction shows
/// setup to someone who has already done it, which is recoverable in one click;
/// the other direction hands a brand-new install a dashboard with nothing in it.
pub(crate) fn onboarding_complete() -> bool {
    meridian_core::paths::meridian_dir().is_some_and(|dir| dir.join("onboarded").exists())
}

/// Route an external re-activation: finished onboarding → the dashboard;
/// otherwise the wizard, so a partial onboard can be resumed.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn reopen_target(onboarded: bool) -> ReopenTarget {
    if onboarded {
        ReopenTarget::Dashboard
    } else {
        ReopenTarget::Wizard
    }
}

#[cfg(test)]
mod reopen_tests {
    use super::{is_onboarded, reopen_target, ReopenTarget};

    #[test]
    fn reopen_target_routes_by_onboarded() {
        // Onboarded users land on the dashboard; everyone else resumes the wizard.
        assert_eq!(reopen_target(true), ReopenTarget::Dashboard);
        assert_eq!(reopen_target(false), ReopenTarget::Wizard);
    }

    /// Every path that can open the dashboard window must be behind the
    /// onboarding gate, or a fresh install reaches the timeline before setup and
    /// sees an empty product that reads as broken rather than unconfigured.
    ///
    /// Source-scanned via `include_str!` (compile-time, no I/O) because the thing
    /// being protected is a call site, not a value: both functions build a real
    /// webview and cannot be driven from a unit test. A behavioural test would be
    /// better; this is what is actually available, and it fails loudly if either
    /// gate is deleted or renamed.
    #[test]
    fn every_dashboard_path_waits_for_onboarding() {
        for (name, src) in [
            ("tray.rs", include_str!("tray.rs")),
            ("commands/system.rs", include_str!("commands/system.rs")),
        ] {
            assert!(
                src.contains("if !crate::onboarding_complete() {"),
                "{name} opens the dashboard without checking onboarding_complete() - \
                 a first-run install would reach the timeline before setup"
            );
        }
        // And the gate must send them somewhere, not just refuse.
        assert!(include_str!("tray.rs").contains("open_wizard_window(app);"));
        assert!(include_str!("commands/system.rs").contains("open_wizard_window(&app);"));
    }

    /// Both dashboard openers must take the window full-screen after building
    /// it.
    ///
    /// `.maximized(true)` is on both builders and is NOT enough on macOS: tao's
    /// `set_maximized` early-returns when `is_zoomed()` already equals the
    /// requested state, so the zoom is skipped with no error, the flag still
    /// set, and `is_maximized()` still reporting `true`. That is why this
    /// shipped looking correct. `sys::open_full_screen` is the explicit half;
    /// the builder flag alone must never be trusted to satisfy this.
    ///
    /// Source-scanned for the same reason as the test above - both functions
    /// build a real webview and cannot be driven from a unit test.
    #[test]
    fn every_dashboard_path_opens_full_screen() {
        for (name, src) in [
            ("tray.rs", include_str!("tray.rs")),
            ("commands/system.rs", include_str!("commands/system.rs")),
        ] {
            assert!(
                src.contains("crate::sys::open_full_screen(&win);"),
                "{name} opens the dashboard without calling open_full_screen - \
                 a builder flag alone is a silent no-op on macOS"
            );
        }
    }

    /// …but `set_fullscreen` itself must be macOS-only.
    ///
    /// THE ONE THIS EXISTS FOR. On macOS full-screen is a first-class window
    /// mode with its own exit: the menu bar drops down on hover, the green
    /// button is there, `Ctrl+Cmd+F` works. On Windows the same call means
    /// `Fullscreen::Borderless`, and tao does three separate things with it
    /// (`platform_impl/windows/window.rs`): sets `MARKER_BORDERLESS_FULLSCREEN`,
    /// which strips the title bar and with it minimize/maximize/close; sizes to
    /// `monitor.size()`, the FULL monitor rect rather than the work area, so the
    /// taskbar is covered; and calls `taskbar_mark_fullscreen(hwnd, true)` to ask
    /// the shell to yield. Nothing in this app binds F11 or Escape on that
    /// window, so the result is a dashboard a Windows user cannot leave except
    /// with Alt+F4.
    ///
    /// `fill_work_area` stays on both platforms and is already right for
    /// Windows - it reads `work_area()`, which excludes the taskbar.
    ///
    /// Source-scanned rather than unit-tested because the defect is a window
    /// STATE on another OS: it compiles cleanly, passes every macOS test, and is
    /// invisible until someone runs the build on Windows.
    #[test]
    fn full_screen_is_requested_on_macos_only() {
        let src = include_str!("sys.rs");
        // Anchor on the CALL (`win.` prefixed), not the bare name: the first
        // bare occurrence in the file is a doc comment explaining what the call
        // means on Windows, so scanning from there measured the distance from a
        // paragraph of prose and passed or failed on whichever functions
        // happened to sit in between.
        let at = src
            .find("win.set_fullscreen(true)")
            .expect("sys.rs no longer requests full-screen at all");
        // The gate must be the nearest thing above the call, not merely present
        // somewhere in the file.
        let before = &src[..at];
        let gate = before
            .rfind("#[cfg(target_os = \"macos\")]")
            .expect("set_fullscreen(true) is not behind a macOS cfg gate");
        assert!(
            !before[gate..].contains("\nfn ")
                && !before[gate..].contains("\npub fn ")
                && !before[gate..].contains("\npub(crate) fn "),
            "the nearest macOS cfg gate is on a different item, so set_fullscreen \
             still runs on Windows and traps the user in a borderless window"
        );
        // And the geometry that Windows relies on is NOT gated with it.
        assert!(
            src.contains("fn open_full_screen")
                && src[src.find("fn open_full_screen").unwrap()..].contains("fill_work_area(win);"),
            "open_full_screen must still size the window on every platform"
        );
    }

    /// Nothing may resize the dashboard window without checking `is_fullscreen`
    /// first.
    ///
    /// The dashboard now carries the full-screen style mask, and tao's
    /// `set_inner_size` calls `NSWindow::setContentSize:` unconditionally
    /// against it - shrinking the rendered content while the Space stays
    /// screen-sized and leaving a black surround. That is exactly the setup
    /// wizard bug, and `resize_setup_window` guards against it by name.
    ///
    /// This pins that the guard is still there, because the wizard's guard is
    /// now also the pattern the dashboard depends on being copied.
    #[test]
    fn window_resizes_check_for_full_screen_first() {
        let src = include_str!("commands/system.rs");
        assert!(
            src.contains("win.is_fullscreen().unwrap_or(false)"),
            "resize_setup_window lost its full-screen guard - a setContentSize: \
             against a full-screen style mask renders black around the content"
        );
    }

    /// `lib.rs`'s import of these three must stay `#[cfg(target_os = "macos")]`.
    ///
    /// Their only use site is the `RunEvent::Reopen` arm, which is macOS-only
    /// because no other platform emits Reopen. The definitions above already
    /// carry `cfg_attr(not(target_os = "macos"), allow(dead_code))`, so they are
    /// covered - but an *import* has no such escape hatch: unused, it is an
    /// ERROR under `-D warnings`, and it broke the Windows clippy AND test jobs
    /// while every macOS gate stayed green.
    ///
    /// That asymmetry is the whole reason this test exists. A macOS developer
    /// running the full local suite cannot see this class of mistake; it
    /// surfaces only after a push, on a runner, ~15 minutes later. Scanning the
    /// source makes it fail here instead.
    ///
    /// Checks the preceding line rather than a fixed block of text, so
    /// reordering the symbols or rewriting the comment above it does not
    /// produce a spurious failure.
    #[test]
    fn the_macos_only_reopen_import_stays_gated() {
        let lines: Vec<&str> = include_str!("lib.rs").lines().collect();
        let idx = lines
            .iter()
            .position(|l| l.trim_start().starts_with("use reopen::{"))
            .expect(
                "lib.rs no longer imports symbols from `reopen` - if the Reopen \
                     handler moved, move this guard with it",
            );
        assert_eq!(
            lines[idx - 1].trim(),
            "#[cfg(target_os = \"macos\")]",
            "lib.rs:{} imports macOS-only `reopen` symbols without a cfg gate - \
             this compiles on macOS and fails the Windows build with \
             `unused imports`",
            idx + 1
        );
    }

    #[test]
    fn is_onboarded_reflects_the_marker_file() {
        // `tempfile` rather than a PID-named directory. A PID is unique among
        // LIVE processes, not over time: if a run panicked between the two
        // assertions below, the `onboarded` marker it wrote survived, and the
        // next run that happened to draw the same PID failed on the first
        // assertion - because `create_dir_all` is happy with an existing
        // directory and does not clear what is in it. `tempdir()` removes the
        // whole tree on drop, panics included.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let meridian = dir.join(".meridian");
        std::fs::create_dir_all(&meridian).unwrap();

        // No marker yet → not onboarded → the reopen handler would open the wizard.
        assert!(!is_onboarded(dir));
        assert_eq!(reopen_target(is_onboarded(dir)), ReopenTarget::Wizard);

        // Marker present (mark_setup_complete writes an RFC-3339 stamp here) →
        // onboarded → the handler would open the dashboard.
        std::fs::write(meridian.join("onboarded"), "2026-07-07T00:00:00Z").unwrap();
        assert!(is_onboarded(dir));
        assert_eq!(reopen_target(is_onboarded(dir)), ReopenTarget::Dashboard);
    }
}
