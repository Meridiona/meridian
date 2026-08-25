//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Launch-at-login registration for the tray, owned outright rather than
//! delegated to `tauri-plugin-autostart`.
//!
//! # Login, restart, or a manual start — never a mid-day resurrection
//! Meridian must come back after the device is turned on or logged into. It
//! must NOT come back on its own after the user deliberately quits it, before
//! then — an earlier version of this module got that wrong by relaunching on
//! OS wake/unlock events (macOS `LaunchEvents`, Windows
//! `SessionStateChangeTrigger`), which fire many times in an ordinary day, so
//! Quit did not stick: a user closed Meridian and had it back within minutes,
//! the next time their screen so much as locked and unlocked. See
//! [`macos::plist_body`] and the `windows` module docs for the removal and
//! [`macos::disarm_relaunch`] for clearing an already-armed wake job left by
//! an older build.
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
//! 1. **It cannot coordinate with `SMAppService`.** Verified against
//!    `auto-launch-0.5.0/src/macos.rs`: `enable()` always writes `RunAtLoad
//!    true` with no way to turn it off, so on macOS 13+ it would double-launch
//!    a tray alongside SMAppService's own login item — two processes writing
//!    one SQLite file.
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
//!
//! - **macOS 13+** — `SMAppService.mainApp` owns LOGIN, and login alone (see
//!   [`login_item`]), which is what produces a named, user-togglable
//!   "Meridian" entry in System Settings → General → Login Items &
//!   Extensions. This module writes NO plist at all in this case — there is
//!   nothing left for one to do — and clears any left by an older build (see
//!   [`macos::ensure_registered`]).
//! - **Below macOS 13** — there is no SMAppService, so
//!   `~/Library/LaunchAgents/com.meridiona.tray.plist` carries the login
//!   trigger itself, with `RunAtLoad` true. Deliberately **no `KeepAlive`**
//!   either way: the user asked for Quit to stick, and `KeepAlive` would make
//!   quitting impossible.
//! - **Windows** — one scheduled task with a `LogonTrigger`, registered from
//!   XML rather than `schtasks`' command form so it can also carry the
//!   battery-defaults and execution-time overrides (see the `windows` module
//!   docs). Its `MultipleInstancesPolicy` is `IgnoreNew`, a defensive dedupe
//!   against a logon race rather than a second trigger to reconcile.
//!
//! There is **no clock in this module** — see the note beside
//! [`launched_by_autostart`] for the removed hardcoded hour, and the `windows`
//! /  [`macos::plist_body`] docs for the later removal of the wake/unlock
//! triggers that replaced it.
//!
//! # Duplicates are still guarded against
//! `tauri-plugin-single-instance` is load-bearing, not a nicety: it is
//! registered FIRST in [`crate::run`]'s builder so a doomed second process
//! (a logon race, a stray manual launch while the tray is already running)
//! exits before it can create a tray icon or a capture writer. Two processes
//! writing one `meridian.db` is the double-writer condition behind the
//! `database disk image is malformed` incidents in [`crate::backend_install`].
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
/// SMAppService login-item registration.
///
/// Compiled on every target, like the two platform modules and for the same
/// reason — but here it is also required, not merely preferable: [`macos`] is
/// itself compiled everywhere and imports this, so gating the DECLARATION is
/// what broke the Windows build with `no login_item in autostart`. Gating only
/// the module's *contents* was not enough; the `mod` item has to exist.
///
/// The ObjC calls inside are individually `cfg`-gated and `available()` is a
/// constant `false` off macOS, so nothing here can message a framework that
/// isn't there.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) mod login_item;
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

/// True when this process was started by the login job rather than by the
/// user. Consulted before anything that puts a window on screen: an
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

// NOTE: there is no MORNING_HOUR any more, and that removal is the point.
//
// A hardcoded relaunch hour was the wrong shape for the requirement ("Meridian is
// running after the device is turned on or woken"): it fires when the user is not
// there and misses them when they are.
//
// The wake/unlock triggers that briefly replaced it are ALSO gone - launchd
// `LaunchEvents` on wake notifications, and Windows'
// `SessionStateChangeTrigger` on `SessionUnlock`/`ConsoleConnect`. They made
// Quit not stick: the app the user had just closed came back on the next unlock.
// See this module's doc header for that history, and
// `windows::task_carries_only_the_logon_trigger`, which asserts the Windows
// trigger is absent.
//
// What remains is login and nothing else: SMAppService on macOS 13+, a
// `RunAtLoad` plist below it, and a single logon trigger on Windows. There is no
// clock and no session-event trigger left in this module.

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
    /// Registered, but missing an expected element of the definition (e.g.
    /// Windows' `LogonTrigger`). The Windows variant name is a PostHog wire
    /// contract ([`RegistrationAction::as_str`]) predating the removal of the
    /// wake-relaunch trigger this originally detected — kept rather than
    /// renamed so saved fleet queries do not silently break.
    RepairedMissingMorningTrigger,
    /// Registered at the right path, but the definition is not what this build
    /// renders — a field changed that no substring probe would have noticed.
    /// The case that motivated it: an older build's plist carried
    /// `RunAtLoad true`, which would double-launch alongside the SMAppService
    /// login item on macOS 13+.
    RepairedStaleDefinition,
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
            Self::RepairedStaleDefinition => "repaired_stale_definition",
            Self::Failed => "failed",
        }
    }

    /// True when this launch had to write a registration that should already
    /// have been there — i.e. autostart was broken until now.
    pub(crate) fn is_repair(self) -> bool {
        matches!(
            self,
            Self::RegisteredMissing
                | Self::RepairedPathDrift
                | Self::RepairedMissingMorningTrigger
                | Self::RepairedStaleDefinition
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
            Self::RepairedStaleDefinition => 8,
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
            8 => Self::RepairedStaleDefinition,
            _ => return None,
        })
    }
}

/// Whether the plugin-era registration (Windows' `HKCU\…\Run` value, macOS'
/// `Meridian.plist`) may be deleted, given the verdict and whether a
/// replacement actually got registered.
///
/// # Why this is shared, and why it is a pure predicate
/// On an install upgrading across the move off `tauri-plugin-autostart`, the
/// plugin-era registration IS the working autostart. Deleting it before a
/// replacement exists leaves the user with none at all — and because capture
/// runs in-process in the tray, a tray that never starts records nothing, so
/// the cost is every day's data until the user happens to open the app by
/// hand. The bug has no local symptom; it surfaces only as someone saying
/// Meridian stopped starting.
///
/// Windows encoded this rule and macOS did not: `macos::ensure_registered`
/// called `migrate_off_plugin()` unconditionally and FIRST, ahead of the
/// transient-path skip and both hard-failure returns. The two platforms
/// disagreeing is what made it invisible, so the rule now lives in ONE place
/// that both call rather than in a comment each side restates differently.
///
/// Keeping it a pure predicate is deliberate: the sequencing inside each
/// `ensure_registered` is the whole safety property and is invisible at the
/// call site, so the table is what the tests below pin.
pub(crate) fn may_drop_legacy(action: RegistrationAction, registered: bool) -> bool {
    match action {
        // Removing it IS honouring the user's "no": left behind, the stale
        // registration keeps starting the tray and the setting reads as ignored.
        RegistrationAction::SkippedDisabledByUser => true,
        // Something correct is already registered, so it is redundant.
        RegistrationAction::AlreadyCorrect => true,
        // A repair was attempted — only safe if it actually took.
        a if a.is_repair() => registered,
        // Transient path, or a hard failure: nothing took over, so the legacy
        // registration is the only autostart the user has. Keep it.
        _ => false,
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

/// Whether registration should be skipped outright, before either platform
/// gets to its own job-definition check — shared because both skip conditions
/// are platform-independent facts (a setting, and where the binary is
/// running from), not anything about the job definition itself.
///
/// Returns `None` when neither applies, meaning the caller should proceed to
/// its own platform-specific repair check ([`macos::ensure_registered`]'s
/// exact-body compare, `windows`'s own private `decide_task`).
///
/// # History
/// This used to be one shared `decide()` that also matched an "expected
/// executable path" and a "morning trigger marker" substring against the
/// registered job text, and both platforms called it for their full repair
/// decision. Windows still needs a marker-based check (Task Scheduler
/// reformats the XML it hands back, so an exact-body compare there would
/// report drift on every launch) but no longer expresses it through a shared
/// helper: a stale task can contain BOTH the expected `LogonTrigger` marker
/// AND a retired `SessionStateChangeTrigger` that needs stripping, which a
/// presence-only check cannot see. macOS dropped the marker entirely in
/// favour of comparing the whole rendered body. So the only still-shared
/// decision is the skip check below.
pub(crate) fn decide_skip(disabled_by_user: bool, stable_path: bool) -> Option<RegistrationAction> {
    if disabled_by_user {
        return Some(RegistrationAction::SkippedDisabledByUser);
    }
    if !stable_path {
        return Some(RegistrationAction::SkippedTransientPath);
    }
    None
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

/// Verify the tray's login registration and repair it if it has drifted.
/// Safe to call on every launch; idempotent when everything is already
/// correct.
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
    } else if matches!(
        action,
        RegistrationAction::SkippedTransientPath | RegistrationAction::Failed
    ) {
        // WARN, because these two mean AUTOSTART IS NOT IN PLACE and the user
        // does not know it.
        //
        // They were previously DEBUG, lumped in with "no change needed" — which
        // is how an install that silently never registers looked identical, in
        // the fleet and in `meridian logs`, to one that was working perfectly.
        // The requirement this module exists to satisfy is "the tray comes back
        // at login and after a restart, until uninstalled"; an install sitting
        // on either of these branches satisfies none of it, and
        // that has to be visible without asking the user to run anything.
        //
        // Volume is bounded: a transient path resolves as soon as the app is in
        // a stable location (`crate::relocate`), and `Failed` is a real I/O or
        // API error rather than a routine state. `SkippedDisabledByUser` stays
        // quiet on purpose — that one is the user's decision, not a fault.
        tracing::warn!(
            action = action.as_str(),
            "autostart: NOT registered - the tray will not come back on its own"
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
    /// macOS only: what SMAppService says about the LOGIN-item registration —
    /// `enabled`, `requires_approval`, `not_registered`, `not_found`, or
    /// `unavailable` below macOS 13. `None` off macOS.
    ///
    /// Distinct from [`Self::registered`], which on macOS 13+ describes a
    /// LaunchAgent this module deliberately no longer writes (SMAppService
    /// owns login there — see [`macos::ensure_registered`]) and below 13
    /// describes the fallback LaunchAgent that DOES carry login. The two are
    /// separate mechanisms and can disagree, and the whole reason this field
    /// exists is that "is Meridian actually in Login Items & Extensions" was
    /// unanswerable from the fleet — which is exactly the question a user
    /// asking "why doesn't it start" is really asking.
    pub(crate) login_item: Option<&'static str>,
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

/// Does a registered job definition reference `exe`?
///
/// # Why this is not `text.contains(exe)`
/// Both writers put the path through [`xml_escape`] - it is element text in a
/// plist or a scheduled-task XML, and a user directory can legitimately contain
/// `&`. Every comparison site then searched for the RAW `current_exe()` string,
/// so for a path containing any of the five XML predefined entities the two
/// forms never matched and the job read as drifted forever.
///
/// On Windows that was a loop rather than a cosmetic error: `decide_task` has no
/// byte-equality fast path, so an affected install returned
/// `RepairedPathDrift` and re-registered its scheduled task on EVERY launch.
///
/// Both spellings are accepted on purpose. A definition written by a build from
/// before the escaping was added carries the raw path, and rejecting those would
/// make this fix itself report drift for one upgrade cycle.
pub(crate) fn job_references_exe(text: &str, exe: &str) -> bool {
    text.contains(exe) || text.contains(&xml_escape(exe))
}

/// Shared by both platform modules: compare a job definition against the
/// running executable, tolerating the "could not tell" cases.
pub(crate) fn status_from(registered: Option<&str>, expected_exe: Option<&str>) -> Status {
    match (registered, expected_exe) {
        (None, _) => Status {
            registered: Some(false),
            path_ok: None,
            login_item: None,
        },
        (Some(text), Some(exe)) => Status {
            registered: Some(true),
            path_ok: Some(job_references_exe(text, exe)),
            login_item: None,
        },
        // Registered, but we cannot resolve our own path to compare against.
        (Some(_), None) => Status {
            registered: Some(true),
            path_ok: None,
            login_item: None,
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

    /// A path containing an XML metacharacter must not read as permanently
    /// drifted.
    ///
    /// `task_xml` and the macOS plist body both write the executable path
    /// through [`xml_escape`] - `xml_escape_covers_a_path_with_an_ampersand`
    /// pins that. Every comparison site then searched the registered job text
    /// for the RAW `current_exe()` string. For a path containing `&`, `<`, `>`,
    /// `"` or `'` the two forms differ, `contains` never matches, and the job
    /// reads as drifted forever.
    ///
    /// Windows profile directories can legally contain `&`, and `decide_task`
    /// has no byte-equality fast path - so an affected install re-registered its
    /// scheduled task on EVERY launch, and reported
    /// `health_autostart_path_ok: false` for good measure.
    #[test]
    fn a_path_with_xml_metacharacters_is_not_read_as_drifted() {
        for exe in [
            r"C:\Users\A&B\AppData\Local\Meridian\Meridian.exe",
            "/Users/a&b/Applications/Meridian.app/Contents/MacOS/Meridian",
            "/Users/o'brien/Meridian.app/Contents/MacOS/Meridian",
            "/Users/a<b>c/Meridian",
        ] {
            let registered = format!("<string>{}</string>", xml_escape(exe));
            assert!(
                job_references_exe(&registered, exe),
                "escaped-on-write must match raw-on-read for {exe:?}"
            );
        }
    }

    /// A definition written by an older build carries the RAW path. Those must
    /// keep matching too, or this fix would itself register drift on every
    /// launch for exactly one upgrade cycle.
    #[test]
    fn a_pre_escape_definition_still_matches() {
        let exe = "/Users/a&b/Meridian";
        assert!(job_references_exe(&format!("<string>{exe}</string>"), exe));
    }

    /// A genuinely moved app must still be detected - the tolerance above must
    /// not become "always matches".
    #[test]
    fn a_genuinely_different_path_is_still_drift() {
        let registered = format!(
            "<string>{}</string>",
            xml_escape("/Applications/Meridian.app/Contents/MacOS/Meridian")
        );
        assert!(!job_references_exe(
            &registered,
            "/Users/tester/Downloads/Meridian.app/Contents/MacOS/Meridian"
        ));
    }

    /// The analytics half of the same defect: `status_from` reported
    /// `path_ok: false` for a correctly registered job.
    #[test]
    fn status_reports_path_ok_for_an_escaped_registration() {
        let exe = "/Users/a&b/Meridian.app/Contents/MacOS/Meridian";
        let registered = format!("<string>{}</string>", xml_escape(exe));
        assert_eq!(
            status_from(Some(&registered), Some(exe)).path_ok,
            Some(true)
        );
    }
    use super::*;

    /// The rule both platforms now share. It lived in `windows.rs` and applied
    /// to Windows alone, while `macos::ensure_registered` deleted the
    /// plugin-era plist unconditionally and FIRST - ahead of the transient-path
    /// skip and both hard-failure returns - so an upgrading Mac could finish a
    /// launch with no autostart at all. Pinned here, once, for both.
    #[test]
    fn the_legacy_registration_survives_until_something_replaces_it() {
        // The three paths that used to lose a working autostart outright.
        assert!(
            !may_drop_legacy(RegistrationAction::RegisteredMissing, false),
            "a failed registration must not delete the only autostart that works"
        );
        assert!(
            !may_drop_legacy(RegistrationAction::Failed, false),
            "a hard failure must not delete the only autostart that works"
        );
        assert!(
            !may_drop_legacy(RegistrationAction::SkippedTransientPath, false),
            "deferring must not delete the only autostart that works"
        );

        // The paths where dropping it is correct.
        assert!(may_drop_legacy(RegistrationAction::RegisteredMissing, true));
        assert!(may_drop_legacy(RegistrationAction::RepairedPathDrift, true));
        assert!(may_drop_legacy(
            RegistrationAction::RepairedStaleDefinition,
            true
        ));
        assert!(may_drop_legacy(RegistrationAction::AlreadyCorrect, false));
        assert!(
            may_drop_legacy(RegistrationAction::SkippedDisabledByUser, false),
            "a user who turned autostart off must not keep being started by the stale registration"
        );

        // Every repair variant follows the same "only if it took" rule, so a
        // new one cannot silently default to dropping the legacy registration.
        for a in [
            RegistrationAction::RegisteredMissing,
            RegistrationAction::RepairedPathDrift,
            RegistrationAction::RepairedMissingMorningTrigger,
            RegistrationAction::RepairedStaleDefinition,
        ] {
            assert!(a.is_repair());
            assert!(!may_drop_legacy(a, false), "{a:?} must wait for a success");
            assert!(may_drop_legacy(a, true), "{a:?} may drop once registered");
        }
    }

    /// A deliberate opt-out outranks everything - otherwise "turn autostart
    /// off" would be undone by the very next launch's repair pass, which is
    /// the failure mode that made the old marker-based gate feel necessary in
    /// the first place.
    #[test]
    fn user_opt_out_wins_the_skip_check() {
        assert_eq!(
            decide_skip(true, true),
            Some(RegistrationAction::SkippedDisabledByUser)
        );
        // And it wins even when the path is ALSO unstable - either fact alone
        // is enough to skip, so the check must not require both.
        assert_eq!(
            decide_skip(true, false),
            Some(RegistrationAction::SkippedDisabledByUser)
        );
    }

    /// A DMG-mounted or translocated launch must not be pinned: the path is
    /// gone once the image ejects, and `relocate` is what fixes that case.
    #[test]
    fn transient_path_defers_without_writing() {
        assert_eq!(
            decide_skip(false, false),
            Some(RegistrationAction::SkippedTransientPath)
        );
    }

    /// Neither skip condition applies - the caller must proceed to its own
    /// platform-specific check rather than treating `None` as any particular
    /// registration state.
    #[test]
    fn neither_skip_condition_defers_to_the_caller() {
        assert_eq!(decide_skip(false, true), None);
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
            RegistrationAction::RepairedStaleDefinition,
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

    /// The severity split in `ensure_registered`: exactly the actions that mean
    /// "autostart is not in place" must be loud, because the user cannot tell.
    ///
    /// Pinned as a test because it is a one-line `matches!` that reads like
    /// logging trivia and is not. It was DEBUG once, which made an install that
    /// silently never registered indistinguishable — in the fleet AND in
    /// `meridian logs` — from one working perfectly. `SkippedDisabledByUser` is
    /// deliberately NOT in the loud set: that is the user's decision, not a
    /// fault, and warning on it would train everyone to ignore the warning.
    #[test]
    fn only_the_not_registered_outcomes_are_loud() {
        let loud = |a: RegistrationAction| {
            matches!(
                a,
                RegistrationAction::SkippedTransientPath | RegistrationAction::Failed
            )
        };
        assert!(loud(RegistrationAction::SkippedTransientPath));
        assert!(loud(RegistrationAction::Failed));
        assert!(!loud(RegistrationAction::SkippedDisabledByUser));
        assert!(!loud(RegistrationAction::AlreadyCorrect));
        // Repairs are loud too, via `is_repair` on the branch above.
        assert!(RegistrationAction::RegisteredMissing.is_repair());
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
            RegistrationAction::RepairedStaleDefinition,
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
        const EXE: &str = "/Applications/Meridian.app/Contents/MacOS/Meridian";

        let unknown_exe = status_from(Some("<plist/>"), None);
        assert_eq!(unknown_exe.registered, Some(true));
        assert_eq!(unknown_exe.path_ok, None);

        let absent = status_from(None, Some(EXE));
        assert_eq!(absent.registered, Some(false));
        assert_eq!(absent.path_ok, None);

        let ok = status_from(
            Some(&format!("<array><string>{EXE}</string></array>")),
            Some(EXE),
        );
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
