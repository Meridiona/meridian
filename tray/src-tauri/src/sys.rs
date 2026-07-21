//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Small shared runtime helpers used across the tray: the current uid, the
//! bundle probe, and the native-notification facade.
//!
//! These each had 2–3 copy-pasted definitions scattered across `lib.rs`,
//! `commands`, `poll`, and `health` before this module consolidated them.
//!
//! The notification facade fronts the community `tauri-plugin-notifications`
//! (real `UNUserNotificationCenter` on macOS: action buttons + inline reply).
//! Everything above this module talks only to [`notify`] /
//! [`notify_outbox`], so if the plugin disappoints it can be swapped for
//! in-house `objc2-user-notifications` bindings without touching callers.
//! The plugin requires a packaged `.app` bundle ([`is_bundled`]); in an
//! unbundled run (`tauri dev`, `cargo run`) it is not registered at all and
//! both facades degrade to a logged no-op — interactive toasts are verified on
//! packaged builds only.
//!
//! # Related
//! - [`crate::poll`] — the loop that toasts via [`notify`] / [`notify_outbox`].
//! - [`crate::commands::notifications`] — records the user's answer
//!   (`record_notification_response`) that [`notify_outbox`]'s buttons produce.
//! - [`crate::install`] — install-mode + path resolution (a separate concern from these).

use tauri::Manager;
use tauri_plugin_notifications::Notifications;

/// The current user's numeric uid as a string (for `launchctl gui/<uid>/…`
/// domain targets). Falls back to `"501"` (the first macOS user) if `id -u`
/// can't be read — better than failing the whole launchctl call.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}

/// True when running from a packaged install rather than a dev build.
///
/// Two callers depend on this and both mean the same thing by it: `lib.rs`
/// gates notification-plugin registration on it, and `update.rs` gates the
/// auto-updater on it. They deliberately share one detector so they can never
/// disagree.
///
/// **macOS** asks the literal question — is the executable inside
/// `…/Foo.app/Contents/MacOS/`? — because there the answer is load-bearing
/// beyond packaging: `UNUserNotificationCenter`, and therefore the
/// notifications plugin whose init hard-fails unbundled, only works from a
/// real bundle. This mirrors the plugin's own `require_bundle` check.
///
/// **Windows** has no bundle concept — an NSIS install is a plain directory of
/// files — so there is nothing equivalent to test for. What the callers
/// actually need to know is "am I an installed app or a dev build", and the
/// dev case is the one with a reliable signature: `cargo run` and `tauri dev`
/// both produce an executable under `target\debug\` or `target\release\`.
/// Anything else is treated as installed.
///
/// That heuristic errs toward `true`, which is the safer direction here: a
/// false negative silently disables toasts and updates on a real install,
/// while a false positive merely means a dev build also shows toasts and
/// checks for updates — both harmless, and arguably what a Windows dev
/// expects anyway.
pub fn is_bundled() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };

    #[cfg(target_os = "macos")]
    {
        (|| {
            let macos = exe.parent()?;
            let contents = macos.parent()?;
            let bundle = contents.parent()?;
            Some(
                macos.ends_with("MacOS")
                    && contents.ends_with("Contents")
                    && bundle.to_string_lossy().ends_with(".app"),
            )
        })()
        .unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Walk for a `target/{debug,release}` pair, the cargo layout every dev
        // run sits in. Matching the pair rather than either name alone avoids
        // false-flagging an install that merely happens to live under a
        // directory called "release".
        let looks_like_dev_build = exe.ancestors().any(|a| {
            a.file_name()
                .is_some_and(|n| n == "debug" || n == "release")
                && a.parent()
                    .and_then(|p| p.file_name())
                    .is_some_and(|n| n == "target")
        });
        !looks_like_dev_build
    }
}

// NOTE: a process-spawning `open_url_detached` (+ its `is_web_url` scheme gate)
// used to live here for the GitHub device flow's auto-open. That was removed:
// the device-flow checklist now opens GitHub only when the user clicks its
// "Open GitHub" button, and the frontend routes that through the Tauri opener
// plugin (`openExternal`) - the established webview external-link path - so the
// backend spawn helper had no remaining caller.

/// The plugin state, if the plugin was registered (bundled runs only).
/// `pub(crate)` so the setup wizard's permission probes
/// ([`crate::commands::setup::check_notifications`]) share the one lookup.
pub(crate) fn notifier(app: &tauri::AppHandle) -> Option<&Notifications<tauri::Wry>> {
    let state = app.try_state::<Notifications<tauri::Wry>>()?;
    Some(state.inner())
}

/// Show a plain native macOS toast (title + body, no buttons).
///
/// Fire-and-forget: the plugin's `show()` is async, so delivery is spawned and
/// a failure is logged, never surfaced — no caller has a meaningful recovery.
/// Unbundled runs (no plugin) log at debug and drop the toast.
pub fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    let Some(n) = notifier(app) else {
        tracing::debug!(
            title,
            "notify: plugin absent (unbundled run) — toast dropped"
        );
        return;
    };
    let builder = n.builder().title(title).body(body);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = builder.show().await {
            tracing::warn!(error = %e, "notify: toast delivery failed");
        }
    });
}

/// Show the toast for an outbox row — plain (title+body) or interactive (the
/// row's `category` selects the button set registered at startup, `lib.rs`).
///
/// The notification identifier IS the outbox row id, for EVERY outbox toast:
/// that id is the ONLY correlation that survives the plugin round-trip (its
/// Swift `ActiveNotification` has no `extra` field, so attached extras never
/// come back on `actionPerformed`). It's what lets the listener (popover JS →
/// `record_notification_response`) stamp taps/buttons/replies onto the row,
/// and what [`retract_toast`] targets when the row expires. The command
/// resolves everything else (deep_link) from the DB row. It also means the OS
/// collapses a re-delivered row onto the same toast instead of duplicating it.
///
/// Known limitation: the plugin's `actionPerformed` only fires for toasts shown
/// by the CURRENT tray process (its notification map is in-memory), so an
/// answer given after a tray restart is dropped. Expiries keep stale
/// interactive toasts rare; a v2 could reconcile via `notificationClicked`.
pub fn notify_outbox(
    app: &tauri::AppHandle,
    n: &meridian_core::notifications::PendingNotification,
) {
    let Some(nf) = notifier(app) else {
        tracing::debug!(
            id = n.id,
            "notify_outbox: plugin absent (unbundled run) — toast dropped"
        );
        return;
    };
    // The id is the response/retraction correlation (see above) — a row whose
    // id can't be represented losslessly must NOT deliver correlated (the
    // answer would stamp the wrong row). Degrade to an uncorrelated plain
    // toast; ids are AUTOINCREMENT so this is theoretical.
    let Ok(toast_id) = i32::try_from(n.id) else {
        tracing::warn!(id = n.id, "outbox id exceeds i32 — delivering uncorrelated");
        notify(app, &n.title, &n.body);
        return;
    };
    let mut builder = nf.builder().id(toast_id).title(&n.title).body(&n.body);
    if let Some(category) = &n.category {
        builder = builder.action_type_id(category);
    }
    let id = n.id;
    let category = n.category.clone();
    tauri::async_runtime::spawn(async move {
        match builder.show().await {
            Ok(()) => {
                tracing::info!(id, category = ?category, "outbox toast delivered")
            }
            Err(e) => {
                tracing::warn!(error = %e, id, "notify_outbox: toast delivery failed")
            }
        }
    });
}

/// Withdraw a delivered outbox toast from the screen and Notification Center —
/// the delivery-side half of expiry (`expires_at`): under the persistent
/// Alerts style an ignored question would otherwise sit forever, so the poll
/// loop retracts expired rows and stamps them `response_action = 'expired'`.
/// Same effect as the user pressing ✕, minus the user. Best-effort: a failure
/// leaves a stale toast on screen but the row is stamped regardless, so it is
/// never re-delivered.
pub fn retract_toast(app: &tauri::AppHandle, id: i64) {
    let Some(nf) = notifier(app) else {
        return; // unbundled run — nothing was ever shown
    };
    let Ok(toast_id) = i32::try_from(id) else {
        return; // uncorrelated delivery (see notify_outbox) — nothing to target
    };
    if let Err(e) = nf.remove_active(vec![toast_id]) {
        tracing::warn!(error = %e, id, "toast retraction failed");
    } else {
        tracing::info!(id, "expired toast retracted");
    }
}

/// Current notification authorization, shared by the setup wizard's probe
/// ([`crate::commands::setup::check_notifications`]) and the background
/// re-check ([`crate::poll::permissions`]) so the two never drift. `None`
/// covers both "plugin absent" (unbundled run) and a probe error — callers
/// that need to distinguish those log separately; both mean "can't toast".
///
/// **macOS** asks the plugin, which is a real `UNUserNotificationCenter`
/// authorization read. **Windows** does NOT ask the plugin: its notify-rust
/// backend has no permission API of its own — `permission_state()` is
/// hardcoded to always return `Granted` (see
/// `tauri-plugin-notifications-0.4.6/src/desktop.rs`) — so this instead reads
/// [`windows_notification_setting`], the real WinRT read.
pub async fn notification_permission_state(
    app: &tauri::AppHandle,
) -> Option<tauri_plugin_notifications::PermissionState> {
    let _nf = notifier(app)?; // plugin absent (unbundled run) — same "can't toast" gate on every OS

    #[cfg(target_os = "windows")]
    {
        match windows_notification_setting(app) {
            Ok(setting) => Some(windows_permission_state(setting)),
            Err(e) => {
                tracing::warn!(error = %e, "windows notification setting probe failed");
                None
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        match _nf.permission_state().await {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::warn!(error = %e, "notification permission probe failed");
                None
            }
        }
    }
}

/// Read the Windows toast-notification setting for this app via WinRT —
/// `Settings > System > Notifications`, not TCC (Windows has no such
/// authorization dialog; see [`is_bundled`]'s doc comment on the platform gap
/// this mirrors for Accessibility/Screen Recording). A pure status read, no
/// prompt: WinRT exposes no "request" API for toast permission, only the
/// Settings page itself (see [`crate::commands::system::open_permission_pane`]
/// `"notifications"`).
///
/// Queried under the SAME app identity notify-rust delivers toasts under —
/// `ToastNotificationManager::CreateToastNotifierWithId` with the Tauri
/// bundle identifier as AUMID — because that's the only identity Windows has
/// a setting for; reading a different one would silently answer the wrong
/// question. `tauri-winrt-notification`'s `Toast::show()` creates its
/// notifier the identical way, so the two calls are guaranteed to agree.
#[cfg(target_os = "windows")]
fn windows_notification_setting(
    app: &tauri::AppHandle,
) -> windows::core::Result<windows::UI::Notifications::NotificationSetting> {
    use windows::UI::Notifications::ToastNotificationManager;
    use windows::core::HSTRING;
    let aumid = HSTRING::from(&app.config().identifier);
    ToastNotificationManager::CreateToastNotifierWithId(&aumid)?.Setting()
}

/// Map WinRT's `NotificationSetting` to the plugin's `PermissionState` so the
/// wizard and the background re-check both branch on the same tri-state
/// vocabulary as macOS. `Enabled` is the only variant that means the user can
/// see a toast; every `Disabled*` variant (for-this-app, for-the-user, by
/// group policy, by manifest) is a dead end the wizard can't distinguish
/// between anyway — there is exactly one recovery action for all of them
/// (`open_permission_pane` `"notifications"`), so collapsing them to `Denied`
/// loses no actionable information. Pure and free-standing so it's testable
/// without a live `AppHandle` / real WinRT call.
#[cfg(target_os = "windows")]
fn windows_permission_state(
    setting: windows::UI::Notifications::NotificationSetting,
) -> tauri_plugin_notifications::PermissionState {
    use tauri_plugin_notifications::PermissionState;
    use windows::UI::Notifications::NotificationSetting;
    if setting == NotificationSetting::Enabled {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

/// Whether the tray process holds macOS Accessibility trust
/// (`AXIsProcessTrusted`) — a pure status read, no prompt. Shared by the setup
/// wizard's probe ([`crate::commands::setup::check_accessibility`]) and the
/// background re-check ([`crate::poll::permissions`]).
pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "ApplicationServices", kind = "framework")]
        #[allow(improper_ctypes)]
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        // Safety: AXIsProcessTrusted is a pure status read with no side effects or UB.
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Whether the tray process holds macOS Screen Recording permission
/// (`CGPreflightScreenCaptureAccess`) — a pure status read, no prompt. Shared
/// by the setup wizard's probe
/// ([`crate::commands::setup::check_screen_recording`]) and the background
/// re-check ([`crate::poll::permissions`]).
pub fn screen_recording_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        // Safety: preflight is a pure status read — no prompt, no side effects.
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Register the fixed interactive-category set (`meridian_core`'s
/// [`meridian_core::notifications::categories`]) with the OS. Called once at
/// startup from `lib.rs` on bundled runs; a failure downgrades every
/// interactive toast to plain title+body (macOS ignores an unknown
/// `action_type_id`), so it's logged loudly but never fatal.
pub fn register_notification_categories(app: &tauri::AppHandle) {
    use meridian_core::notifications::categories;
    let Some(nf) = notifier(app) else {
        tracing::debug!("category registration skipped — plugin absent (unbundled run)");
        return;
    };
    // ActionType's fields are private but it derives Deserialize, so the
    // categories are built from the shared JSON descriptors — the same bytes
    // the daemon stamps into the outbox `actions` column, one source of truth.
    // customDismissAction makes macOS report the user closing the toast (the
    // X) as an action event, so a dismissal is captured on the outbox row
    // (response_action = 'dismiss') instead of vanishing silently.
    let types: Vec<tauri_plugin_notifications::ActionType> = categories::ALL
        .iter()
        .filter_map(|id| {
            let actions: serde_json::Value =
                serde_json::from_str(categories::actions_json(id)?).ok()?;
            serde_json::from_value(serde_json::json!({
                "id": id,
                "actions": actions,
                "customDismissAction": true,
            }))
            .ok()
        })
        .collect();
    if types.len() != categories::ALL.len() {
        tracing::error!(
            built = types.len(),
            expected = categories::ALL.len(),
            "notification category descriptors failed to parse — buttons will be missing"
        );
    }
    match nf.register_action_types(types) {
        Ok(()) => tracing::info!(
            categories = categories::ALL.len(),
            "notification categories registered"
        ),
        Err(e) => tracing::error!(error = %e, "notification category registration failed"),
    }
}

/// Restore the tray-only UX when `win` closes: revert the app to the Accessory
/// activation policy (no dock icon), undoing the Regular policy a window opener
/// sets so the dashboard gets a dock entry while it is open.
///
/// **macOS-only behaviour, deliberately a no-op elsewhere.** Activation policy
/// is an AppKit concept (`NSApplication.activationPolicy`) with no Windows
/// analogue — there is no dock to leave an orphaned icon in — so on Windows the
/// window event is simply never registered.
///
/// # Who calls this
/// Both dashboard openers, which previously each carried their own copy of this
/// closure: [`crate::tray`]'s tray-menu opener and
/// [`crate::commands::system::open_dashboard`].
///
/// # Related
/// - [`crate::commands::system::dismiss_popover_on_focus`] — the other
///   window-event hook the dashboard openers install.
pub(crate) fn revert_to_accessory_on_close(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let app_handle = app.clone();
        win.on_window_event(move |event| {
            if let tauri::WindowEvent::Destroyed = event {
                let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
        });
    }
    // One mechanism for both params, so the signature stays attribute-free.
    #[cfg(not(target_os = "macos"))]
    let _ = (app, win);
}

#[cfg(test)]
mod tests {
    /// The non-macOS half of [`super::is_bundled`], factored out so the
    /// dev-vs-installed decision is testable without spawning a process from a
    /// contrived path.
    ///
    /// Kept in sync with the real implementation by construction: this asserts
    /// the rule (a `target/{debug,release}` PAIR means dev), and the rule is
    /// what the caller encodes.
    fn looks_like_dev_build(exe: &std::path::Path) -> bool {
        exe.ancestors().any(|a| {
            a.file_name()
                .is_some_and(|n| n == "debug" || n == "release")
                && a.parent()
                    .and_then(|p| p.file_name())
                    .is_some_and(|n| n == "target")
        })
    }

    /// Build a path from components so the test means the same thing on every
    /// platform. A literal `r"C:\a\b"` is a single opaque component on unix —
    /// backslash is not a separator there — so hardcoding one spelling would
    /// make these assertions vacuous on whichever OS is not being spelled.
    fn path_of(parts: &[&str]) -> std::path::PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn cargo_layouts_are_recognised_as_dev_builds() {
        assert!(looks_like_dev_build(&path_of(&[
            "src",
            "meridian",
            "target",
            "debug",
            "meridian-tray"
        ])));
        assert!(looks_like_dev_build(&path_of(&[
            "src",
            "meridian",
            "target",
            "release",
            "meridian-tray"
        ])));
    }

    #[test]
    fn installed_layouts_are_not_dev_builds() {
        assert!(!looks_like_dev_build(&path_of(&[
            "Users",
            "me",
            "AppData",
            "Local",
            "Meridian",
            "meridian-tray"
        ])));
        assert!(!looks_like_dev_build(&path_of(&[
            "Program Files",
            "Meridian",
            "meridian-tray"
        ])));
    }

    /// The pair check exists so an install path that merely contains a
    /// directory called "release" is not mistaken for a cargo build dir —
    /// which would silently disable toasts and auto-update for that user.
    #[test]
    fn a_bare_release_directory_is_not_a_dev_build() {
        assert!(!looks_like_dev_build(&path_of(&[
            "Apps",
            "release",
            "Meridian",
            "meridian-tray"
        ])));
    }

    /// [`super::windows_permission_state`] is the sole place that decides
    /// which WinRT settings still let a toast through — get this wrong and
    /// either `system.notif_permission` never fires (false Granted) or fires
    /// permanently (false Denied) for every Windows install.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_enabled_setting_maps_to_granted() {
        use tauri_plugin_notifications::PermissionState;
        use windows::UI::Notifications::NotificationSetting;
        assert_eq!(
            super::windows_permission_state(NotificationSetting::Enabled),
            PermissionState::Granted
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_every_disabled_variant_maps_to_denied() {
        use tauri_plugin_notifications::PermissionState;
        use windows::UI::Notifications::NotificationSetting;
        for setting in [
            NotificationSetting::DisabledForApplication,
            NotificationSetting::DisabledForUser,
            NotificationSetting::DisabledByGroupPolicy,
            NotificationSetting::DisabledByManifest,
        ] {
            assert_eq!(
                super::windows_permission_state(setting),
                PermissionState::Denied,
                "{setting:?} must not be reported as Granted"
            );
        }
    }
}
