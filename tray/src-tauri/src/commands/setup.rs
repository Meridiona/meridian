//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Setup wizard Tauri commands — first-run detection, permission probes, provider
//! detection.
//!
//! Every interactive step of `ui/app/setup/page.tsx` calls one or more commands
//! from this module. No stubs — all commands return live state so the wizard can
//! advance only when real requirements are met.
//!
//! # Who calls this
//! Command: registered in `lib.rs`; invoked from `ui/app/setup/page.tsx`.
//!
//! # Related
//! - [`crate::commands::system::open_permission_pane`] — opens System Settings panes

/// Returns `true` on the first launch — no `~/.meridian/onboarded` flag exists.
/// The wizard auto-opens when `true` and is skipped on subsequent launches.
///
/// Uses [`meridian_core::paths::meridian_dir`] rather than a raw `HOME`
/// env var read — `HOME` is unset on Windows, which used to make this fall
/// back to a bogus `.`-relative (cwd) path instead of `%USERPROFILE%\.meridian`.
#[tauri::command]
#[tracing::instrument]
pub async fn is_first_run() -> bool {
    match meridian_core::paths::meridian_dir() {
        Some(dir) => !dir.join("onboarded").exists(),
        None => true,
    }
}

/// Write `~/.meridian/onboarded` (RFC-3339 timestamp) to mark wizard completion.
/// Future tray launches skip the auto-open. Idempotent — safe to call more than once.
#[tauri::command]
#[tracing::instrument]
pub async fn mark_setup_complete() -> Result<(), String> {
    let dir = meridian_core::paths::meridian_dir()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create ~/.meridian: {e}"))?;
    tokio::fs::write(dir.join("onboarded"), chrono::Local::now().to_rfc3339())
        .await
        .map_err(|e| format!("write onboarded: {e}"))?;
    tracing::info!("setup: onboarded flag written");
    Ok(())
}

/// The current OS, for the wizard to adapt its copy/steps — e.g. the
/// Permissions step is macOS-only (Windows capture needs no TCC-style grant;
/// see `screenpipe_a11y::platform::windows`) and is skipped entirely when this
/// returns `"windows"`.
#[tauri::command]
pub fn get_platform() -> &'static str {
    std::env::consts::OS
}

/// Returns `true` when the current process (the tray) has Accessibility trust.
///
/// `AXIsProcessTrusted()` is keyed on the code-signing identity of the calling
/// process. In the B-in-process capture track the tray is the capture binary, so
/// this is the authoritative signal. For the current architecture (a11y-helper is
/// separate), this tells the wizard whether the tray itself is trusted — which is
/// also the correct target since the wizard prompts the user to add Meridian.
#[tauri::command]
#[tracing::instrument]
pub async fn check_accessibility() -> bool {
    crate::sys::accessibility_trusted()
}

/// Returns `true` when the tray itself holds macOS Screen Recording permission.
///
/// Post-cutover (Gap-2 Bucket 2) capture runs in-process, so the **tray** is
/// the process that needs Screen Recording. `CGPreflightScreenCaptureAccess()`
/// reads that grant directly — no prompt, no side effects — replacing the old
/// external-capture proxy checks that misreported on an in-process install.
/// The wizard's *grant* action (which surfaces the system prompt via
/// `CGRequestScreenCaptureAccess`) is separate slice-5 work.
#[tauri::command]
#[tracing::instrument]
pub async fn check_screen_recording() -> bool {
    crate::sys::screen_recording_trusted()
}

/// Surface the macOS Screen Recording prompt **and register the app** so it
/// appears in System Settings → Privacy → Screen Recording, then return the
/// resulting grant state.
///
/// [`check_screen_recording`] uses `CGPreflightScreenCaptureAccess` — a pure
/// status read that never registers the app. On a fresh install this means the
/// list under Privacy → Screen Recording shows "No Items", because macOS only
/// adds an entry the *first* time the app calls `CGRequestScreenCaptureAccess`.
/// This command calls that request variant so clicking the wizard's grant
/// button both registers the app and shows the system dialog in one shot.
#[tauri::command]
#[tracing::instrument]
pub async fn request_screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGRequestScreenCaptureAccess() -> bool;
        }
        // Safety: CGRequestScreenCaptureAccess shows the TCC prompt + registers the
        // app, then returns the resulting grant state — no UB.
        let granted = unsafe { CGRequestScreenCaptureAccess() };
        tracing::info!(granted, "setup: requested Screen Recording access");
        granted
    }
    #[cfg(not(target_os = "macos"))]
    false
}

// Input Monitoring is intentionally NOT a wizard permission — the `check_` /
// `request_input_monitoring` commands were removed. The signals the daemon
// consumes (clipboard + app_switch) ride the Accessibility-only capture path;
// Input Monitoring only added the click/key/text tap (minor Option-C ended_at
// refinement) and is redundant with Accessibility for everything the wizard
// gates. See the note on PERMISSIONS in `ui/app/setup/data.ts` and
// `capture::ui_events::run_ui_event_recorder`.

/// Map the notification plugin's [`tauri_plugin_notifications::PermissionState`]
/// to the wizard's wire string. Pure so the mapping is unit-testable without a
/// live plugin: `granted` / `denied` / `prompt` (not yet asked — a request will
/// show the one-shot macOS dialog).
fn notification_state_label(state: tauri_plugin_notifications::PermissionState) -> &'static str {
    use tauri_plugin_notifications::PermissionState;
    match state {
        PermissionState::Granted => "granted",
        PermissionState::Denied => "denied",
        PermissionState::Prompt | PermissionState::PromptWithRationale => "prompt",
    }
}

/// Current notification authorization for the wizard's Notifications card —
/// a real `UNUserNotificationCenter` read on macOS, a real WinRT
/// `ToastNotifier::Setting()` read on Windows (see
/// [`crate::sys::notification_permission_state`]).
///
/// Returns `granted` / `denied` / `prompt` (never asked — requesting will show
/// the system dialog; Windows never reports this, see [`request_notifications`]) /
/// `unavailable` (unbundled run: the plugin registers only inside a bundled
/// install, so `tauri dev` has no notification backend at all).
///
/// Unlike the TCC probes above this is not a boolean: `denied` and `prompt`
/// need different grant actions (macOS shows the authorization dialog exactly
/// once — after a deny the only path back is System Settings → Notifications),
/// so the wizard must know which side of that line the user is on.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn check_notifications(app: tauri::AppHandle) -> String {
    match crate::sys::notification_permission_state(&app).await {
        Some(state) => notification_state_label(state).into(),
        None => "unavailable".into(),
    }
}

/// Surface the one-shot macOS notification authorization dialog and return the
/// resulting state (same strings as [`check_notifications`]).
///
/// The tray already fires this request on every bundled launch (`lib.rs`), so
/// on a normal first run the dialog appears alongside the wizard; this command
/// exists for the card's explicit button — covering the user who dismissed
/// that dialog unanswered. If permission is already `denied`, macOS will NOT
/// re-prompt: the call returns `denied` unchanged and the frontend falls back
/// to opening the System Settings pane
/// ([`crate::commands::system::open_permission_pane`] `"notifications"`).
///
/// **Windows has no request dialog to surface** — WinRT exposes only a
/// passive `ToastNotifier::Setting()` read (see
/// [`crate::sys::notification_permission_state`]), no equivalent of
/// `UNUserNotificationCenter`'s one-shot authorization prompt. So on Windows
/// this command skips the (stubbed, always-`Granted`) plugin call entirely
/// and just re-reads the real WinRT state — `denied` there drives the same
/// frontend fallback to the Settings pane as a macOS deny.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn request_notifications(app: tauri::AppHandle) -> String {
    #[cfg(target_os = "windows")]
    {
        return match crate::sys::notification_permission_state(&app).await {
            Some(state) => notification_state_label(state).into(),
            None => "unavailable".into(),
        };
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Some(nf) = crate::sys::notifier(&app) else {
            return "unavailable".into();
        };
        match nf.request_permission().await {
            Ok(state) => {
                let label = notification_state_label(state);
                tracing::info!(state = label, "setup: requested notification permission");
                label.into()
            }
            Err(e) => {
                tracing::warn!(error = %e, "setup: notification permission request failed");
                "unavailable".into()
            }
        }
    }
}

/// Which AI provider CLIs are installed on this Mac (`claude`, `codex`, `cursor-agent`,
/// `copilot`). Drives the wizard's Intelligence step, which lets the user pick one - and
/// says plainly when the CLI they picked isn't here yet.
///
/// Reports *installed*, not *signed in*: this is the free, always-fresh half of provider
/// state (`meridian::llm::detect::detect`), never itself run authentication - it just
/// merges in the last real connectivity test on record for each provider (see
/// [`test_llm_provider`]/[`test_all_llm_providers`]), if any. Not cached itself - the user
/// will alt-tab out to `npm i -g …` and come back, and Rescan must tell the truth when
/// they do.
#[tauri::command]
#[tracing::instrument]
pub async fn detect_llm_providers() -> Result<Vec<meridian::llm::detect::ProviderStatus>, String> {
    let found = meridian::llm::detect::detect_all_with_cache().await;
    tracing::info!(
        installed = found.iter().filter(|p| p.installed).count(),
        verified = found
            .iter()
            .filter(|p| matches!(
                p.last_test.as_ref().map(|t| &t.outcome),
                Some(meridian::llm::detect::ProviderTestOutcome::Ok)
            ))
            .count(),
        "llm: provider detection complete"
    );
    Ok(found)
}

/// Run one real, trivial call against `id`'s CLI and report + persist what happened - the
/// Intelligence panel's per-card "Test" button. Spends one real request against the user's
/// subscription, so this only ever runs on explicit user action, never automatically.
#[tauri::command]
#[tracing::instrument]
pub async fn test_llm_provider(
    id: String,
) -> Result<meridian::llm::detect::ProviderTestResult, String> {
    let provider = meridian_core::LlmProvider::from_wire(&id)
        .ok_or_else(|| format!("unknown provider {id:?}"))?;
    let settings = meridian_core::settings::load_runtime_settings();
    let result = meridian::llm::detect::test_provider(provider, &settings).await;
    meridian::llm::detect::persist_test_result(&result);
    tracing::info!(
        provider = %id,
        ok = matches!(result.outcome, meridian::llm::detect::ProviderTestOutcome::Ok),
        elapsed_ms = result.elapsed_ms,
        "llm: provider test complete"
    );
    Ok(result)
}

/// Install one provider's CLI by running its official installer on the user's behalf - the
/// provider detail view's "Install" button. Runs through the user's login shell so `npm`/PATH
/// resolve (see [`meridian::llm::detect::install_provider`]), then confirms the binary is now
/// present. Only ever on an explicit click - the daemon never installs anything automatically.
#[tauri::command]
#[tracing::instrument]
pub async fn install_llm_provider(
    id: String,
) -> Result<meridian::llm::detect::InstallOutcome, String> {
    let provider = meridian_core::LlmProvider::from_wire(&id)
        .ok_or_else(|| format!("unknown provider {id:?}"))?;
    let outcome = meridian::llm::detect::install_provider(provider).await;
    tracing::info!(
        provider = %id,
        ok = outcome.ok,
        "llm: provider install complete"
    );
    Ok(outcome)
}

/// Run the interactive `cursor-agent login` - the Cursor detail view's "Sign in to Cursor"
/// button. Opens the user's browser to sign into their own Cursor account, so the summariser
/// runs on their Cursor SUBSCRIPTION (no API key, nothing metered). Only ever on an explicit
/// click; the daemon's own unattended path never opens a browser.
#[tauri::command]
#[tracing::instrument]
pub async fn cursor_sign_in() -> Result<meridian::llm::detect::InstallOutcome, String> {
    let outcome = meridian::llm::detect::cursor_sign_in().await;
    tracing::info!(ok = outcome.ok, "llm: cursor sign-in complete");
    Ok(outcome)
}

/// Run the interactive `codex login` - the Codex detail view's "Sign in to Codex" button.
/// Opens the user's browser to sign into their ChatGPT account, so the summariser runs on their
/// ChatGPT SUBSCRIPTION (no API key, nothing metered). Only ever on an explicit click; the
/// daemon's own unattended path never opens a browser. Mirrors [`cursor_sign_in`].
#[tauri::command]
#[tracing::instrument]
pub async fn codex_sign_in() -> Result<meridian::llm::detect::InstallOutcome, String> {
    let outcome = meridian::llm::detect::codex_sign_in().await;
    tracing::info!(ok = outcome.ok, "llm: codex sign-in complete");
    Ok(outcome)
}

/// Test every currently-installed provider at once - the Intelligence panel's Rescan
/// action. Each result is persisted as it lands (see
/// [`meridian::llm::detect::test_all_installed`]), so a slow or hanging CLI can't hold the
/// others' verified state hostage.
#[tauri::command]
#[tracing::instrument]
pub async fn test_all_llm_providers(
) -> Result<Vec<meridian::llm::detect::ProviderTestResult>, String> {
    let settings = meridian_core::settings::load_runtime_settings();
    let results = meridian::llm::detect::test_all_installed(&settings).await;
    tracing::info!(
        tested = results.len(),
        ok = results
            .iter()
            .filter(|r| matches!(r.outcome, meridian::llm::detect::ProviderTestOutcome::Ok))
            .count(),
        "llm: provider rescan-test complete"
    );
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::notification_state_label;
    use tauri_plugin_notifications::PermissionState;

    // The wizard's card branches on these exact strings (grant button action:
    // prompt → request dialog, denied → System Settings pane), so the mapping
    // is contract, not cosmetics.
    #[test]
    fn notification_states_map_to_wire_strings() {
        assert_eq!(
            notification_state_label(PermissionState::Granted),
            "granted"
        );
        assert_eq!(notification_state_label(PermissionState::Denied), "denied");
        assert_eq!(notification_state_label(PermissionState::Prompt), "prompt");
        assert_eq!(
            notification_state_label(PermissionState::PromptWithRationale),
            "prompt"
        );
    }
}
