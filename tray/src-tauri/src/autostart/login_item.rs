//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! macOS login item via **SMAppService** — the sanctioned API since macOS 13,
//! and the only one that puts a NAMED, user-togglable "Meridian" entry in
//! System Settings → General → Login Items & Extensions.
//!
//! # Why this exists at all
//! Writing a plist into `~/Library/LaunchAgents/` does register with macOS's
//! Background Task Management database — measured, not assumed: a probe plist
//! that was never `launchctl bootstrap`ped still appeared in `sfltool dumpbtm`.
//! So the raw-plist approach is not invisible. What it cannot do is present
//! itself as the APP: a legacy agent shows up under "Allow in the Background"
//! keyed on the plist, whereas `SMAppService.mainApp` produces an "Open at
//! Login" entry attributed to the signed bundle. That difference is the whole
//! point — a user looking for "Meridian" in that pane should find "Meridian".
//!
//! # Why it is only for the LOGIN half
//! `SMAppService.mainApp` registers "launch this app when the user logs in" and
//! nothing more: there is no way to attach a `StartCalendarInterval` to it. The
//! morning relaunch therefore stays on our own plist, which is written with
//! **`RunAtLoad` false** on macOS 13+ precisely so the two mechanisms cannot
//! both start a tray at login. See [`super::macos`] for that coordination —
//! getting it wrong is a double-launch, i.e. two processes writing one SQLite
//! file.
//!
//! # Version gating is mandatory, not defensive
//! The app declares `LSMinimumSystemVersion` 10.13 while `SMAppService` is
//! macOS 13+. Sending these selectors on an older system is an
//! unrecognized-selector crash, not a graceful failure, so **every** entry
//! point here goes through [`available`] first.
//!
//! # Signing
//! SMAppService validates the caller's code signature and refuses an unsigned
//! or ad-hoc-signed bundle. Meridian's release builds are Developer ID signed
//! and notarized (`release-build.yml` imports the certificate and notarizes both
//! channels; verified on a shipped `.app` with `spctl -a -vvv` reporting
//! `source=Notarized Developer ID`), so the precondition
//! `backend_install.rs` documented for adopting SMAppService is already met.
//! An unsigned local `cargo build` will get [`RegisterOutcome::Failed`] — which
//! is why this is bundled-only and why the fallback path still exists.
//!
//! # Who calls this
//! [`super::macos::ensure_registered`] / [`super::macos::unregister`], and
//! [`super::status`] via [`status`].

// The ObjC bindings are a macOS-only dependency (see this crate's
// `[target.'cfg(target_os = "macos")'.dependencies]`), so the imports — and only
// the four functions that actually message Objective-C — are gated. Everything
// else in this module is pure and compiles on every target ON PURPOSE, for the
// same reason `super::macos` and `super::windows` do: CI runs on macOS and
// Linux, and a type whose wire names are a contract with saved fleet queries
// should be linted and tested everywhere, not only where it can run.
//
// This gate is also what broke the Windows build once: `super::macos` is
// compiled on every target and imports this module, so making the WHOLE module
// macOS-only left Windows with an unresolved import.
#[cfg(target_os = "macos")]
use objc2_foundation::NSProcessInfo;
#[cfg(target_os = "macos")]
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// Lowest macOS major version that has `SMAppService`.
///
/// `isize` to match `NSOperatingSystemVersion`'s field type. Read by
/// `available()` on macOS and pinned by a test on every target, so it is never
/// dead code.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MIN_MACOS_MAJOR: isize = 13;

/// Whether `SMAppService` can be called on this system at all.
///
/// Checked through `NSProcessInfo`'s structured version rather than by probing
/// for the class, because the answer is also worth logging and reporting: an
/// install stuck on the fallback path is a fact about the machine, not a
/// mystery.
#[cfg(target_os = "macos")]
pub(crate) fn available() -> bool {
    // `processInfo` is a singleton accessor and `operatingSystemVersion` a
    // plain struct read; objc2-foundation exposes both as safe, and they exist
    // on every macOS version this app can run on.
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    version.majorVersion >= MIN_MACOS_MAJOR
}

/// Off macOS there is no ServiceManagement framework at all, so the honest
/// answer is a flat no — and every entry point below short-circuits on it,
/// which is what lets the rest of this module stay platform-neutral.
#[cfg(not(target_os = "macos"))]
pub(crate) fn available() -> bool {
    false
}

/// What the system currently thinks of our login-item registration.
///
/// Deliberately mirrors `SMAppServiceStatus` rather than collapsing to a bool:
/// `RequiresApproval` is the one state that looks like failure and is not — the
/// registration succeeded and macOS is waiting on the user in System Settings.
/// Treating it as "broken" would make the app re-register in a loop and never
/// tell the user the one thing that would fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginItemStatus {
    /// SMAppService is not available on this macOS version.
    Unavailable,
    /// Registered and eligible to launch at login.
    Enabled,
    /// Registered, but the user has to allow it in System Settings.
    RequiresApproval,
    /// Never registered, or unregistered since.
    NotRegistered,
    /// The system could not find a matching service.
    NotFound,
}

impl LoginItemStatus {
    /// Stable wire name for analytics — kept separate from the `Debug`
    /// spelling so renaming a variant cannot break a saved query.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires_approval",
            Self::NotRegistered => "not_registered",
            Self::NotFound => "not_found",
        }
    }

    /// True when nothing more needs doing: either it is working, or the only
    /// remaining step belongs to the user.
    ///
    /// `RequiresApproval` counts as settled ON PURPOSE. Re-registering does not
    /// clear it — only the user can, in System Settings — so treating it as
    /// unsettled would mean writing the same registration on every launch
    /// forever and reporting a repair every time.
    pub(crate) fn is_settled(self) -> bool {
        matches!(self, Self::Enabled | Self::RequiresApproval)
    }
}

/// Current registration status. `Unavailable` below macOS 13, and off macOS.
#[cfg(target_os = "macos")]
pub(crate) fn status() -> LoginItemStatus {
    if !available() {
        return LoginItemStatus::Unavailable;
    }
    // SAFETY: guarded by `available()`; `mainAppService` is a class accessor and
    // `status` a property read, neither of which can fail or block.
    let raw = unsafe { SMAppService::mainAppService().status() };
    if raw == SMAppServiceStatus::Enabled {
        LoginItemStatus::Enabled
    } else if raw == SMAppServiceStatus::RequiresApproval {
        LoginItemStatus::RequiresApproval
    } else if raw == SMAppServiceStatus::NotRegistered {
        LoginItemStatus::NotRegistered
    } else {
        // Covers NotFound and any status a future macOS adds — reported as
        // `not_found` rather than guessed at, so a new state shows up in the
        // fleet as itself instead of being silently folded into "fine".
        LoginItemStatus::NotFound
    }
}

/// No ServiceManagement framework off macOS.
#[cfg(not(target_os = "macos"))]
pub(crate) fn status() -> LoginItemStatus {
    LoginItemStatus::Unavailable
}

/// Result of a registration attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegisterOutcome {
    /// Not attempted — SMAppService needs macOS 13+.
    Unavailable,
    /// Already `Enabled` or `RequiresApproval`; nothing was written.
    AlreadySettled,
    /// Newly registered by this call.
    Registered,
    /// The call failed. The error is logged at WARN with its description.
    Failed,
}

impl RegisterOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::AlreadySettled => "already_settled",
            Self::Registered => "registered",
            Self::Failed => "failed",
        }
    }
}

/// Register the app as a login item, idempotently.
///
/// Checks [`status`] first and returns early when already settled: Apple's API
/// can error on a redundant `register` of an enabled service, so "register
/// unconditionally and ignore the error" would turn a healthy install into a
/// WARN on every launch and make the real failures unfindable.
pub(crate) fn register() -> RegisterOutcome {
    if !available() {
        tracing::debug!(
            min_macos = MIN_MACOS_MAJOR,
            "autostart: SMAppService needs a newer macOS - using the LaunchAgent fallback"
        );
        return RegisterOutcome::Unavailable;
    }
    let current = status();
    if current.is_settled() {
        tracing::debug!(
            status = current.as_str(),
            "autostart: login item already set"
        );
        return RegisterOutcome::AlreadySettled;
    }
    register_main_app()
}

/// The one line that actually messages Objective-C, isolated so the policy above
/// it (availability gate, idempotence check, logging) stays platform-neutral and
/// reviewable in one place.
#[cfg(target_os = "macos")]
fn register_main_app() -> RegisterOutcome {
    // SAFETY: callers gate on `available()`. `registerAndReturnError` maps the
    // ObjC out-error to a Rust `Result`, so there is no raw pointer handling.
    match unsafe { SMAppService::mainAppService().registerAndReturnError() } {
        Ok(()) => {
            // Re-read rather than assuming `Enabled`: a first registration on a
            // modern macOS commonly lands on `RequiresApproval`, and reporting
            // it as enabled would hide the one thing the user must do.
            let after = status();
            tracing::info!(
                status = after.as_str(),
                "autostart: registered as a login item via SMAppService"
            );
            RegisterOutcome::Registered
        }
        Err(e) => {
            let reason = e.localizedDescription();
            // WARN, not ERROR: the LaunchAgent fallback still covers the user,
            // so this is degraded rather than broken. WARN is also the lowest
            // severity that leaves the machine, and "SMAppService refused" is
            // exactly the fleet signal worth having — an unsigned or relocated
            // bundle is the usual cause.
            tracing::warn!(
                error = %reason,
                "autostart: SMAppService registration failed - falling back to the LaunchAgent"
            );
            RegisterOutcome::Failed
        }
    }
}

/// Unreachable off macOS: `available()` is a constant `false` there, so
/// `register` returns before this. Present only so the module compiles on every
/// target — see the import gate at the top for why that matters.
#[cfg(not(target_os = "macos"))]
fn register_main_app() -> RegisterOutcome {
    RegisterOutcome::Unavailable
}

/// Unregister the login item, honouring a user who turned autostart off.
///
/// Unlike `launchctl bootout`, this does NOT terminate the running app — it only
/// removes the launch-at-login registration — so it is safe to call from the
/// Settings toggle while the user is looking at the window.
pub(crate) fn unregister() {
    if !available() || status() == LoginItemStatus::NotRegistered {
        return;
    }
    unregister_main_app();
}

#[cfg(target_os = "macos")]
fn unregister_main_app() {
    // SAFETY: callers gate on `available()`; same shape as `register_main_app`.
    match unsafe { SMAppService::mainAppService().unregisterAndReturnError() } {
        Ok(()) => tracing::info!("autostart: login item unregistered"),
        Err(e) => {
            let reason = e.localizedDescription();
            tracing::warn!(error = %reason, "autostart: SMAppService unregistration failed");
        }
    }
}

/// Unreachable off macOS — see [`register_main_app`]'s non-macOS twin.
#[cfg(not(target_os = "macos"))]
fn unregister_main_app() {}

/// Open System Settings → Login Items, for a UI affordance that walks a user to
/// the approval toggle instead of describing where it is.
///
/// Unused today; kept because `RequiresApproval` is a real, reachable state that
/// only the user can clear, and this is the one-call way to take them there.
#[allow(dead_code)]
pub(crate) fn open_system_settings() {
    #[cfg(target_os = "macos")]
    if available() {
        // SAFETY: guarded by `available()`; takes no arguments, returns nothing.
        unsafe { SMAppService::openSystemSettingsLoginItems() };
    }
}

/// The plist name is unused for `mainApp` registration, but the constant
/// documents the relationship for anyone reaching for `agentServiceWithPlistName`
/// later. See the module doc for why the agent variant was not used: its plist
/// must live inside the signed bundle at `Contents/Library/LaunchAgents/`, which
/// Tauri's `bundle.resources` cannot target (it maps into `Contents/Resources/`)
/// and which would have to be injected before the CI signing step.
#[allow(dead_code)]
pub(crate) const BUNDLED_AGENT_PLIST: &str = "com.meridiona.tray.plist";

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire names are a contract with saved fleet queries.
    #[test]
    fn status_and_outcome_wire_names_are_stable() {
        assert_eq!(LoginItemStatus::Unavailable.as_str(), "unavailable");
        assert_eq!(LoginItemStatus::Enabled.as_str(), "enabled");
        assert_eq!(
            LoginItemStatus::RequiresApproval.as_str(),
            "requires_approval"
        );
        assert_eq!(LoginItemStatus::NotRegistered.as_str(), "not_registered");
        assert_eq!(LoginItemStatus::NotFound.as_str(), "not_found");

        assert_eq!(RegisterOutcome::Unavailable.as_str(), "unavailable");
        assert_eq!(RegisterOutcome::AlreadySettled.as_str(), "already_settled");
        assert_eq!(RegisterOutcome::Registered.as_str(), "registered");
        assert_eq!(RegisterOutcome::Failed.as_str(), "failed");
    }

    /// `RequiresApproval` must count as settled: re-registering cannot clear it
    /// (only the user can, in System Settings), so treating it as unsettled
    /// would re-register on every launch and report a repair every time.
    #[test]
    fn requires_approval_is_settled_but_not_registered_is_not() {
        assert!(LoginItemStatus::Enabled.is_settled());
        assert!(LoginItemStatus::RequiresApproval.is_settled());
        assert!(!LoginItemStatus::NotRegistered.is_settled());
        assert!(!LoginItemStatus::NotFound.is_settled());
        assert!(!LoginItemStatus::Unavailable.is_settled());
    }

    /// Calling SMAppService below macOS 13 is an unrecognized-selector crash,
    /// and the app ships with LSMinimumSystemVersion 10.13 - so this constant
    /// is load-bearing, not documentation.
    #[test]
    fn the_version_gate_matches_smappservice_availability() {
        assert_eq!(MIN_MACOS_MAJOR, 13);
    }

    /// Runs on whatever macOS the test host has. Asserts only the invariant that
    /// holds either way: `available()` and `status()` must agree about whether
    /// SMAppService can be used, so a gated call site can trust either one.
    #[test]
    fn status_reports_unavailable_exactly_when_the_api_is_unavailable() {
        assert_eq!(status() == LoginItemStatus::Unavailable, !available());
    }
}
