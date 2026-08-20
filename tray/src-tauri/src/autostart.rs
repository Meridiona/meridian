//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Launch-at-login **and** morning relaunch registration for the tray, owned
//! outright rather than delegated to `tauri-plugin-autostart`.
//!
//! # Why the tray specifically
//! Capture runs IN-PROCESS in the tray (`crate::capture`). A tray that isn't
//! running produces no `capture_frames`, so the daemon's ETL has nothing to
//! read and the user's timeline is simply empty. "Meridian didn't start" is
//! therefore total data loss for that period, not a cosmetic annoyance — and in
//! PostHog it is indistinguishable from a churned user, because `app_active`
//! (`crate::analytics`) can only fire from a running tray.
//!
//! # Why not `tauri-plugin-autostart`
//! Two reasons, both structural:
//!
//! 1. **It cannot express a morning relaunch.** Verified against
//!    `auto-launch-0.5.0/src/macos.rs`: `enable()` writes a plist carrying
//!    `Label` + `ProgramArguments` + `RunAtLoad` and nothing else. There is no
//!    hook for `StartCalendarInterval`, which is the only thing that brings the
//!    tray back after the user quits it.
//! 2. **It named the plist after `productName`** — `~/Library/LaunchAgents/Meridian.plist`,
//!    not `com.meridiona.*`. `src/uninstall.rs` asserted in a comment that the
//!    macOS login item was swept up by its `com.meridiona.*.plist` glob. It was
//!    not, so every uninstall left the login item behind, pointed at a deleted
//!    app. Owning the plist under the conventional label fixes that by
//!    construction.
//!
//! # The bug this module exists to end
//! The previous implementation registered **once ever**, gated on a
//! `~/.meridian/autostart_configured` marker, and never asked the OS whether the
//! registration still existed. That marker lives under `~/.meridian`, which
//! survives the app being trashed, reinstalled, or moved — so the marker and
//! the reality routinely diverged, and nothing in the product could notice.
//! Measured on a developer machine: marker present, and **no Meridian login
//! item at all** in `~/Library/LaunchAgents/`. That install could never
//! autostart again, and no code path could ever repair or report it.
//!
//! So this verifies and repairs on EVERY launch ([`ensure_registered`]), and the
//! marker's only remaining job is to record a **deliberate** user opt-out
//! (`autostart_disabled`).
//!
//! # What gets registered
//! One job per platform, carrying both triggers, so the OS's own
//! single-job-instance semantics do the de-duplication:
//!
//! - **macOS** — `~/Library/LaunchAgents/com.meridiona.tray.plist` with
//!   `RunAtLoad` (login) + `StartCalendarInterval` at [`MORNING_HOUR`] (the
//!   morning relaunch), and deliberately **no `KeepAlive`**: the user asked for
//!   Quit to stick for the rest of the day and be undone the next morning, and
//!   `KeepAlive` would make quitting impossible.
//! - **Windows** — one scheduled task with a `LogonTrigger` *and* a daily
//!   `CalendarTrigger`, registered from XML because `schtasks`' command form
//!   cannot express two triggers. Its `MultipleInstancesPolicy` is `IgnoreNew`,
//!   which is what stops the 09:00 trigger starting a second tray on top of the
//!   one the logon trigger already started.
//!
//! # Deliberately not bootstrapped on macOS
//! [`macos::ensure_registered`] writes the plist but does not
//! `launchctl bootstrap` it. `bootstrap` honours `RunAtLoad`, so bootstrapping
//! our own job from inside the running tray would immediately start a SECOND
//! tray — two processes writing `capture_frames` to one SQLite file. launchd
//! loads `~/Library/LaunchAgents` at session start, so login coverage and the
//! calendar trigger both work from the next login onward. The cost is precise
//! and bounded: in the single session where the plist was first written, the
//! calendar trigger is not yet live, so a user who installs and then quits on
//! the same day waits until their next login rather than until 09:00. Every
//! later session behaves fully.
//!
//! # Who calls this
//! [`crate::run`]'s `setup()` hook, once per launch, bundled runs only (see
//! [`crate::sys::is_bundled`]) — an unbundled `cargo run` lives under `target/`
//! and must never be pinned as a login item.
//! [`launched_by_autostart`] is read by [`crate::poll::whats_new_auto_open`]
//! (to stay silent on an unattended launch) and by [`crate::analytics::health`].
//!
//! # Related
//! - [`crate::backend_install`] — the DAEMON's equivalent registration, whose
//!   `launchctl` helpers this reuses. Note its plist DOES carry `KeepAlive`:
//!   the daemon is headless and has no Quit, so the trade-off there is the
//!   opposite one.
//! - [`crate::relocate`] — runs earlier in `setup()`; moves a DMG/translocated
//!   launch into `/Applications` so there is a stable path to register at all.

// BOTH platform modules are compiled on EVERY target, and only
// [`register_platform`] below is `cfg`-gated. That is deliberate and it is the
// only thing giving the Windows path any test coverage at all: CI and every
// developer machine here are macOS/Linux, so `#[cfg(target_os = "windows")]` on
// the module would mean its plist/XML builders were never compiled, never
// linted, and never tested by anyone until a Windows user reported that
// Meridian does not start. The generators are pure string functions with no
// platform APIs, so there is nothing to stop them building anywhere — the same
// reasoning (and the same `cfg_attr(allow(dead_code))` shape) as
// [`crate::backend_install`]'s `wait_until_gone`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) mod macos;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) mod windows;

use std::sync::atomic::{AtomicU8, Ordering};

/// Argument the registered job passes so the tray can tell an unattended
/// OS-initiated launch from one the user performed.
///
/// A **flag, not an environment variable**, because it has to work on both
/// platforms: a launchd plist can set `EnvironmentVariables`, but a Windows
/// scheduled task has no equivalent, so an env var would leave every Windows
/// autostart looking like a manual launch. `Arguments`/`ProgramArguments` exist
/// on both.
///
/// Nothing parses tray argv (there is no `tauri-plugin-cli`), so an extra flag
/// is inert. It is deliberately absent from every self-relaunch
/// ([`crate::relocate`], the updater), because those ARE user-initiated
/// continuations and should behave like one.
pub(crate) const AUTOSTART_FLAG: &str = "--autostart";

/// True when this process was started by the login/morning job rather than by
/// the user. Consulted before anything that puts a window on screen: an
/// unattended launch must stay in the menu bar and nothing else.
pub(crate) fn launched_by_autostart() -> bool {
    args_indicate_autostart(std::env::args())
}

/// The pure half of [`launched_by_autostart`], split out because a test cannot
/// set the running process's own argv — and this decides whether a window
/// appears, which is the behaviour the user specifically asked for.
pub(crate) fn args_indicate_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|a| a.as_ref() == AUTOSTART_FLAG)
}

/// Local hour the morning relaunch fires, on both platforms.
///
/// A constant, not a setting: a settings field would need a UI, a migration and
/// a re-registration path on change, to serve a preference nobody has asked
/// for. Widening this to several times a day is one more entry in the plist's
/// `StartCalendarInterval` array / the task's `Triggers`.
pub(crate) const MORNING_HOUR: u32 = 9;

/// Marker recording that the user deliberately turned autostart OFF.
///
/// This replaces the old `autostart_configured` marker as the gate, and the
/// inversion is the whole point: "we already did it once" is a claim about the
/// past that decays silently, whereas "the user said no" is a decision that
/// stays true until they change it. Absent (the default) means register.
pub(crate) const DISABLED_MARKER: &str = "autostart_disabled";

/// The retired one-shot marker. Still named here only so
/// [`clear_legacy_marker`] can remove it: left on disk it is harmless, but it
/// is also actively misleading to anyone debugging an install, since it reads
/// as "autostart is configured" when that is exactly what could not be relied
/// on.
pub(crate) const LEGACY_MARKER: &str = "autostart_configured";

/// What [`ensure_registered`] did, and why.
///
/// The distinctions matter downstream: [`crate::analytics::health`] ships this
/// so the fleet can answer "how many installs had a broken login item", which
/// is unanswerable today, and each repair variant names a different root cause
/// so the answer is actionable rather than just alarming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationAction {
    /// The user turned autostart off. Nothing written, nothing repaired.
    SkippedDisabledByUser,
    /// Running from a mounted DMG or Gatekeeper translocation — the path would
    /// be gone by the next login, so registering it is worse than deferring.
    SkippedTransientPath,
    /// What is registered already matches what this launch would write.
    AlreadyCorrect,
    /// Nothing was registered at all. A fresh install, or the divergence
    /// described in the module docs.
    RegisteredMissing,
    /// Registered, but pointing at a different executable than the one running
    /// — the app was moved after being registered.
    RepairedPathDrift,
    /// Registered, but with no morning trigger. This is the upgrade path for
    /// every install that was registered by `tauri-plugin-autostart`.
    RepairedMissingMorningTrigger,
    /// The registration could not be read or written (permissions, missing
    /// home directory, `schtasks` refused). Retried on the next launch.
    Failed,
}

impl RegistrationAction {
    /// Stable wire name for analytics. Kept separate from the `Debug` spelling
    /// so renaming a variant cannot silently break a saved PostHog query.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SkippedDisabledByUser => "skipped_disabled_by_user",
            Self::SkippedTransientPath => "skipped_transient_path",
            Self::AlreadyCorrect => "already_correct",
            Self::RegisteredMissing => "registered_missing",
            Self::RepairedPathDrift => "repaired_path_drift",
            Self::RepairedMissingMorningTrigger => "repaired_missing_morning_trigger",
            Self::Failed => "failed",
        }
    }

    /// True when this launch had to write a registration that should already
    /// have been there — i.e. autostart was broken until now.
    pub(crate) fn is_repair(self) -> bool {
        matches!(
            self,
            Self::RegisteredMissing | Self::RepairedPathDrift | Self::RepairedMissingMorningTrigger
        )
    }

    fn code(self) -> u8 {
        match self {
            Self::SkippedDisabledByUser => 1,
            Self::SkippedTransientPath => 2,
            Self::AlreadyCorrect => 3,
            Self::RegisteredMissing => 4,
            Self::RepairedPathDrift => 5,
            Self::RepairedMissingMorningTrigger => 6,
            Self::Failed => 7,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::SkippedDisabledByUser,
            2 => Self::SkippedTransientPath,
            3 => Self::AlreadyCorrect,
            4 => Self::RegisteredMissing,
            5 => Self::RepairedPathDrift,
            6 => Self::RepairedMissingMorningTrigger,
            7 => Self::Failed,
            _ => return None,
        })
    }
}

/// [`RegistrationAction::code`] of the last [`ensure_registered`] call, `0`
/// until it has run.
///
/// An atomic rather than a field on `AppState` because the one consumer
/// ([`crate::analytics::health`]) runs on the poll loop and only needs a
/// snapshot; threading it through app state would mean a lock for a single byte
/// that is written once per process.
static LAST_ACTION: AtomicU8 = AtomicU8::new(0);

/// What [`ensure_registered`] decided this launch, or `None` if it has not run
/// yet (an unbundled run, or the poll loop winning a race against `setup()`).
pub(crate) fn last_action() -> Option<RegistrationAction> {
    RegistrationAction::from_code(LAST_ACTION.load(Ordering::Relaxed))
}

/// Decide what to do, given only the facts — no filesystem, no launchctl.
///
/// `registered` is the current job definition as text (plist XML on macOS, task
/// XML on Windows) or `None` when nothing is registered. `expected_exe` is the
/// running executable's path and `morning_trigger_marker` the element name that
/// proves the definition carries a morning trigger (`StartCalendarInterval` /
/// `CalendarTrigger`).
///
/// Text `contains` rather than a parsed comparison is deliberate. This decides
/// only whether to REWRITE, and the rewrite is unconditional and idempotent, so
/// the cost of a false "needs repair" is one file write; a parser would add a
/// dependency and a new failure mode to answer the same question. Both needles
/// are specific enough not to collide: an absolute executable path, and an XML
/// element name that appears nowhere else in either format.
pub(crate) fn decide(
    disabled_by_user: bool,
    stable_path: bool,
    registered: Option<&str>,
    expected_exe: &str,
    morning_trigger_marker: &str,
) -> RegistrationAction {
    if disabled_by_user {
        return RegistrationAction::SkippedDisabledByUser;
    }
    if !stable_path {
        return RegistrationAction::SkippedTransientPath;
    }
    let Some(text) = registered else {
        return RegistrationAction::RegisteredMissing;
    };
    if !text.contains(expected_exe) {
        return RegistrationAction::RepairedPathDrift;
    }
    if !text.contains(morning_trigger_marker) {
        return RegistrationAction::RepairedMissingMorningTrigger;
    }
    RegistrationAction::AlreadyCorrect
}

/// True when the user has explicitly turned autostart off.
///
/// Two independent sources, either of which counts:
/// - `settings.json`'s `autostart_enabled` — the switch in Settings, and the
///   supported way to say no. It is opt-OUT (default true), so an unreadable or
///   absent file reads as "leave it on", which is the right default for a
///   component whose absence means capturing nothing.
/// - a `~/.meridian/autostart_disabled` marker — a file-only escape hatch for
///   support and for a machine whose settings write is failing, which is
///   precisely the situation where telling someone to use a GUI toggle does not
///   help.
pub(crate) fn disabled_by_user() -> bool {
    if !meridian_core::settings::load_runtime_settings().autostart_enabled {
        return true;
    }
    meridian_core::paths::meridian_dir().is_some_and(|d| d.join(DISABLED_MARKER).exists())
}

/// Remove the retired one-shot marker. Best-effort and idempotent; see
/// [`LEGACY_MARKER`] for why it is worth removing rather than ignoring.
async fn clear_legacy_marker() {
    if let Some(dir) = meridian_core::paths::meridian_dir() {
        let _ = tokio::fs::remove_file(dir.join(LEGACY_MARKER)).await;
    }
}

/// Verify the tray's login + morning registration and repair it if it has
/// drifted. Safe to call on every launch; idempotent when everything is
/// already correct.
///
/// Never fails: autostart is important but it is not worth crashing the tray
/// for, so every error path logs and leaves the next launch to retry.
#[tracing::instrument]
pub async fn ensure_registered() {
    let action = register_platform().await;
    LAST_ACTION.store(action.code(), Ordering::Relaxed);

    if action.is_repair() {
        // WARN, not INFO, on purpose: WARN+ is the only severity that leaves
        // the machine (`src/telemetry_spool/redact.rs`), and "this install's
        // autostart was broken until now" is precisely the fact that was
        // invisible for the entire life of the previous implementation. Volume
        // is bounded by construction — a repair happens once, after which
        // every later launch reports `already_correct` at DEBUG.
        tracing::warn!(
            action = action.as_str(),
            "autostart: registration was missing or stale - repaired"
        );
    } else {
        tracing::debug!(action = action.as_str(), "autostart: no change needed");
    }

    if !matches!(action, RegistrationAction::Failed) {
        clear_legacy_marker().await;
    }
}

/// Apply a change to the `autostart_enabled` setting NOW, rather than at the
/// next launch.
///
/// Both directions matter and neither is a no-op. Turning it ON must register,
/// or the switch reads as done while nothing has changed. Turning it OFF must
/// actively REMOVE the job — [`ensure_registered`] merely declining to write is
/// not enough, because the job already on disk is what the OS acts on, so the
/// user's "no" would be ignored until they uninstalled.
///
/// Called from `crate::commands::settings::update_settings`. Bundled-only for
/// the same reason as [`ensure_registered`].
#[tracing::instrument]
pub async fn apply_setting_change(enabled: bool) {
    if enabled {
        ensure_registered().await;
        return;
    }
    unregister_platform().await;
    LAST_ACTION.store(
        RegistrationAction::SkippedDisabledByUser.code(),
        Ordering::Relaxed,
    );
    tracing::info!("autostart: removed at the user's request - Meridian will not start itself");
}

#[cfg(target_os = "macos")]
async fn unregister_platform() {
    macos::unregister().await;
}

#[cfg(target_os = "windows")]
async fn unregister_platform() {
    windows::unregister().await;
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn unregister_platform() {}

#[cfg(target_os = "macos")]
async fn register_platform() -> RegistrationAction {
    macos::ensure_registered().await
}

#[cfg(target_os = "windows")]
async fn register_platform() -> RegistrationAction {
    windows::ensure_registered().await
}

/// Linux has no packaged install shape yet, so there is nothing to register.
/// Reported as `Failed` rather than `AlreadyCorrect` so a fleet query can never
/// read an unimplemented platform as a healthy one.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn register_platform() -> RegistrationAction {
    RegistrationAction::Failed
}

/// The registration as it exists on disk right now, for
/// [`crate::analytics::health`].
///
/// Read live rather than cached from [`ensure_registered`]: the point of
/// shipping it is to catch drift, and drift that happened after startup is
/// exactly the case a cached value would miss.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Status {
    /// A job is registered at all. `None` when the check itself could not run.
    pub(crate) registered: Option<bool>,
    /// The registered job points at the running executable. `None` when
    /// nothing is registered or the check could not run — never `false` for
    /// "we could not tell", per the rule in [`crate::analytics::health`].
    pub(crate) path_ok: Option<bool>,
}

/// Probe the current registration. Best-effort; every failure degrades to
/// `None` rather than to `false`.
///
/// `async` for the Windows path's sake: reading a scheduled task means shelling
/// out to `schtasks`, and its only caller ([`crate::analytics::health::snapshot`])
/// runs on the tray's poll loop. A blocking `std::process::Command` there would
/// stall a runtime worker for the length of a process spawn — briefly, once a
/// day, but the poll loop is also what drives the tray icon, the health banner
/// and the notification drain, and "the menu bar froze for a moment" is not a
/// trade worth making for a telemetry field. macOS only reads a file, but
/// matches the signature so there is one shape to call.
pub(crate) async fn status() -> Status {
    #[cfg(target_os = "macos")]
    {
        macos::status().await
    }
    #[cfg(target_os = "windows")]
    {
        windows::status().await
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Status::default()
    }
}

/// Shared by both platform modules: compare a job definition against the
/// running executable, tolerating the "could not tell" cases.
pub(crate) fn status_from(registered: Option<&str>, expected_exe: Option<&str>) -> Status {
    match (registered, expected_exe) {
        (None, _) => Status {
            registered: Some(false),
            path_ok: None,
        },
        (Some(text), Some(exe)) => Status {
            registered: Some(true),
            path_ok: Some(text.contains(exe)),
        },
        // Registered, but we cannot resolve our own path to compare against.
        (Some(_), None) => Status {
            registered: Some(true),
            path_ok: None,
        },
    }
}

/// Minimal XML text escape for values interpolated into a plist or a task
/// definition — an executable path, which can legitimately contain `&` in a
/// user's directory name.
///
/// Escaping only the five XML predefined entities is sufficient here because
/// every interpolation site is element text, never an attribute value or a
/// comment.
pub(crate) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKER: &str = "StartCalendarInterval";
    const EXE: &str = "/Applications/Meridian.app/Contents/MacOS/Meridian";

    fn plist_with(exe: &str, morning: bool) -> String {
        let trigger = if morning { MARKER } else { "" };
        format!("<array><string>{exe}</string></array>{trigger}")
    }

    /// A deliberate opt-out outranks everything, including a missing
    /// registration - otherwise "turn autostart off" would be undone by the
    /// very next launch's repair pass, which is the failure mode that made the
    /// old marker-based gate feel necessary in the first place.
    #[test]
    fn user_opt_out_wins_over_every_repair() {
        for registered in [None, Some(""), Some(plist_with(EXE, true).as_str())] {
            assert_eq!(
                decide(true, true, registered, EXE, MARKER),
                RegistrationAction::SkippedDisabledByUser
            );
        }
    }

    /// A DMG-mounted or translocated launch must not be pinned: the path is
    /// gone once the image ejects, and `relocate` is what fixes that case.
    #[test]
    fn transient_path_defers_without_writing() {
        assert_eq!(
            decide(false, false, None, EXE, MARKER),
            RegistrationAction::SkippedTransientPath
        );
    }

    #[test]
    fn absent_registration_is_registered_fresh() {
        assert_eq!(
            decide(false, true, None, EXE, MARKER),
            RegistrationAction::RegisteredMissing
        );
    }

    /// The defect that made a moved app permanently unable to autostart: the
    /// old code wrote a marker, never compared paths, and never looked again.
    #[test]
    fn a_moved_app_is_detected_and_repointed() {
        let stale = plist_with(
            "/Users/x/Downloads/Meridian.app/Contents/MacOS/Meridian",
            true,
        );
        assert_eq!(
            decide(false, true, Some(&stale), EXE, MARKER),
            RegistrationAction::RepairedPathDrift
        );
    }

    /// The upgrade path for every install registered by
    /// `tauri-plugin-autostart`: right path, no morning trigger.
    #[test]
    fn a_plugin_era_registration_gains_the_morning_trigger() {
        let old = plist_with(EXE, false);
        assert_eq!(
            decide(false, true, Some(&old), EXE, MARKER),
            RegistrationAction::RepairedMissingMorningTrigger
        );
    }

    #[test]
    fn a_correct_registration_is_left_alone() {
        let good = plist_with(EXE, true);
        assert_eq!(
            decide(false, true, Some(&good), EXE, MARKER),
            RegistrationAction::AlreadyCorrect
        );
    }

    /// Only the variants that mean "autostart was broken until this launch"
    /// count as repairs - a skip or a no-op must not inflate the fleet's
    /// repair rate.
    #[test]
    fn only_the_writing_variants_count_as_repairs() {
        for a in [
            RegistrationAction::RegisteredMissing,
            RegistrationAction::RepairedPathDrift,
            RegistrationAction::RepairedMissingMorningTrigger,
        ] {
            assert!(a.is_repair(), "{a:?} should count as a repair");
        }
        for a in [
            RegistrationAction::SkippedDisabledByUser,
            RegistrationAction::SkippedTransientPath,
            RegistrationAction::AlreadyCorrect,
            RegistrationAction::Failed,
        ] {
            assert!(!a.is_repair(), "{a:?} should not count as a repair");
        }
    }

    /// The analytics wire names are a contract with saved PostHog queries, so
    /// they are pinned here rather than derived from the variant spelling.
    #[test]
    fn action_codes_and_names_round_trip() {
        for a in [
            RegistrationAction::SkippedDisabledByUser,
            RegistrationAction::SkippedTransientPath,
            RegistrationAction::AlreadyCorrect,
            RegistrationAction::RegisteredMissing,
            RegistrationAction::RepairedPathDrift,
            RegistrationAction::RepairedMissingMorningTrigger,
            RegistrationAction::Failed,
        ] {
            assert_eq!(RegistrationAction::from_code(a.code()), Some(a));
            assert!(!a.as_str().is_empty());
        }
        // 0 is "never ran", which must not decode to a real action.
        assert_eq!(RegistrationAction::from_code(0), None);
    }

    /// "Could not tell" must never serialise as `false` - a health dashboard
    /// cannot distinguish it from "broken" once it does.
    #[test]
    fn status_never_reports_unknown_as_broken() {
        let unknown_exe = status_from(Some("<plist/>"), None);
        assert_eq!(unknown_exe.registered, Some(true));
        assert_eq!(unknown_exe.path_ok, None);

        let absent = status_from(None, Some(EXE));
        assert_eq!(absent.registered, Some(false));
        assert_eq!(absent.path_ok, None);

        let ok = status_from(Some(&plist_with(EXE, true)), Some(EXE));
        assert_eq!(ok.registered, Some(true));
        assert_eq!(ok.path_ok, Some(true));
    }

    /// The flag is what tells the tray to stay windowless. A manual launch must
    /// never look like an autostart (the user would lose the changelog they were
    /// meant to see) and an autostart must never look manual (a window appears
    /// on a machine nobody touched, which is the annoyance that makes people
    /// disable autostart altogether).
    #[test]
    fn only_the_autostart_flag_marks_a_launch_unattended() {
        let exe = "/Applications/Meridian.app/Contents/MacOS/Meridian";
        assert!(args_indicate_autostart([exe, AUTOSTART_FLAG]));
        assert!(!args_indicate_autostart([exe]));
        // A near-miss must not count - a prefix match would make any future
        // flag starting with these characters silently suppress windows.
        assert!(!args_indicate_autostart([exe, "--autostart-later"]));
        assert!(!args_indicate_autostart([exe, "autostart"]));
        assert!(!args_indicate_autostart(Vec::<&str>::new()));
    }

    #[test]
    fn xml_escape_covers_a_path_with_an_ampersand() {
        assert_eq!(
            xml_escape("/Users/a&b/Meridian.app"),
            "/Users/a&amp;b/Meridian.app"
        );
        assert_eq!(xml_escape("<>\"'"), "&lt;&gt;&quot;&apos;");
        assert_eq!(
            xml_escape("/Applications/Meridian.app"),
            "/Applications/Meridian.app"
        );
    }
}
