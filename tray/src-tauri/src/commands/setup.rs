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

/// Name of the marker holding when the user OPENED the wizard, the counterpart to
/// `onboarded` (which records when they finished).
const STARTED_MARKER: &str = "setup_started";

/// Stamp `~/.meridian/setup_started` with the current time — called once when the
/// wizard's first screen mounts.
///
/// Completion has always been timestamped (`onboarded`), but nothing recorded when
/// onboarding *began*, so the elapsed time was not derivable. That duration is
/// user-facing twice over: the "Setup complete in Ns" line the walkthrough opens
/// with, and the time span on the "Setting up Meridian" day-task card the
/// walkthrough writes at the end (see [`setup_elapsed_secs`]).
///
/// Overwrites rather than preserving a first-ever value: re-running setup (Settings
/// → Account → Re-run Setup) is a fresh run and should report its own duration, not
/// the months since the original install.
#[tauri::command]
#[tracing::instrument]
pub async fn mark_setup_started() -> Result<(), String> {
    let dir = meridian_core::paths::meridian_dir()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create ~/.meridian: {e}"))?;
    tokio::fs::write(dir.join(STARTED_MARKER), chrono::Local::now().to_rfc3339())
        .await
        .map_err(|e| format!("write {STARTED_MARKER}: {e}"))?;
    tracing::info!("setup: start marker written");
    Ok(())
}

/// Seconds elapsed since [`mark_setup_started`] stamped its marker, or `None` when
/// the marker is missing or unparseable (an install that onboarded before this
/// existed, or a hand-edited file).
///
/// `None` is a normal outcome the caller must render around — the walkthrough drops
/// its "in Ns" clause rather than showing a wrong or zero duration. A negative delta
/// (clock moved backwards between the two reads) is also reported as `None` for the
/// same reason.
#[tauri::command]
#[tracing::instrument]
pub async fn setup_elapsed_secs() -> Option<u64> {
    let dir = meridian_core::paths::meridian_dir()?;
    let raw = tokio::fs::read_to_string(dir.join(STARTED_MARKER))
        .await
        .ok()?;
    let started = chrono::DateTime::parse_from_rfc3339(raw.trim()).ok()?;
    let secs = (chrono::Local::now() - started.with_timezone(&chrono::Local)).num_seconds();
    u64::try_from(secs).ok()
}

/// Name of the marker that ARMS the dashboard walkthrough — written when the wizard
/// completes, read by [`walkthrough_is_armed`].
const WALKTHROUGH_MARKER: &str = "walkthrough_armed";

/// Write `~/.meridian/onboarded` (RFC-3339 timestamp) to mark wizard completion,
/// and arm the dashboard walkthrough that follows it.
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
    // The walkthrough is the second half of onboarding, so finishing the wizard is
    // what earns it. See [`walkthrough_is_armed`] for why it needs its own marker
    // rather than reading `onboarded`.
    tokio::fs::write(
        dir.join(WALKTHROUGH_MARKER),
        chrono::Local::now().to_rfc3339(),
    )
    .await
    .map_err(|e| format!("write {WALKTHROUGH_MARKER}: {e}"))?;
    tracing::info!("setup: onboarded flag written, walkthrough armed");
    Ok(())
}

/// Whether this install finished the wizard on a build that has the walkthrough —
/// the one thing that entitles the dashboard to take the screen over on load.
///
/// # Why this is not `!is_first_run()`
///
/// The walkthrough's own "have you seen it" marker is a localStorage key, which no
/// install predating it can possibly hold — so on its own it reads every existing
/// user as brand new and hands a months-old install a full-screen tour of a product
/// they already use. `onboarded` cannot fix that either: it is set for the existing
/// user AND for the new one who just finished the wizard seconds ago, so it cannot
/// tell them apart. This marker can, because only [`mark_setup_complete`] writes it
/// and no upgrade back-fills it: absent means "onboarded before the walkthrough
/// existed", which is exactly the population that must not see it.
///
/// Deliberately NOT consumed on read. Clearing it here would mean quitting mid-tour
/// loses the walkthrough for good; the once-ever guarantee is the localStorage
/// marker's job, written when the user finishes or skips.
#[tauri::command]
#[tracing::instrument]
pub async fn walkthrough_is_armed() -> bool {
    match meridian_core::paths::meridian_dir() {
        Some(dir) => dir.join(WALKTHROUGH_MARKER).exists(),
        None => false,
    }
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
    let mut found = meridian::llm::detect::detect_all_with_cache().await;

    // `detect_all` enumerates BUILT-INS only, on purpose: a custom endpoint is a registry row
    // with no CLI to probe, so it has no install state. But the test cache is keyed by wire id
    // and DOES hold a row for `custom` - written by `test_llm_provider` and by the dev
    // Disconnect button - and dropping it here meant nothing on the chooser could ever see it.
    // The Groq tile therefore rendered IN USE off `value === 'custom'` alone: a green,
    // confident "connected" for a provider the app had just been told is not.
    //
    // So the cached verdict is carried through as its own status row. `installed: true` is the
    // honest answer for a cloud endpoint (there is nothing to install), and the row appears
    // only when there is a cached test to report, so a never-tested endpoint stays absent
    // rather than arriving as a bare green tick.
    if let Some(last) =
        meridian::llm::detect::cached_test_result(meridian_core::LlmProvider::Custom.as_str())
    {
        found.push(meridian::llm::detect::ProviderStatus {
            id: meridian_core::LlmProvider::Custom.as_str().to_string(),
            installed: true,
            path: None,
            authenticated: None,
            last_test: Some(last),
            // Nothing to install, which is the same reason `installed: true` above is the
            // honest answer - a cloud endpoint is reached over HTTP, not spawned. The UI
            // falls back to its static hint when this is None, and never shows an install
            // step for this row anyway.
            install_command: None,
        });
    }

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
    app: tauri::AppHandle,
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
    // A test is the one moment provider health provably changed - don't make the banner
    // wait out the poll loop's 60 s cadence to agree with the card the user just pressed.
    crate::commands::health::push_health_update(&app).await;
    Ok(result)
}

/// Forget `id`'s recorded connectivity test, so the app treats it as signed out.
/// **Dev builds only.**
///
/// This exists because "connected" is app state with no UI that can un-set it: the
/// picker only ever records successes, so once a provider tests OK there is no way
/// back to the not-connected state short of hand-editing
/// `~/.meridian/provider_test_cache.json`. That state is the entire first-run
/// experience — the "Meridian needs an AI engine" prompt, the locked provider
/// screen, the tour beat that leads into it — and it was being re-created by hand
/// every time any of it needed a look.
///
/// Writes a `Failed` outcome rather than deleting the entry, because that is the
/// real shape of the case worth rehearsing: a CLI that is installed but signed
/// out. A missing entry means "never tested", which
/// [`classify_provider_health`](meridian::llm::detect) deliberately treats as fine.
///
/// Gated exactly like the LLM Lab (`commands/llm_lab.rs`): `cfg!(debug_assertions)`,
/// refused in shipped builds even against a hand-crafted `invoke`, so no release
/// can be talked into disconnecting a user's provider.
#[tauri::command]
#[tracing::instrument]
pub async fn disconnect_llm_provider(app: tauri::AppHandle, id: String) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("disconnecting a provider is a dev-only surface".to_string());
    }
    let provider = meridian_core::LlmProvider::from_wire(&id)
        .ok_or_else(|| format!("unknown provider {id:?}"))?;
    meridian::llm::detect::persist_test_result(&meridian::llm::detect::ProviderTestResult {
        id: provider.as_str().to_string(),
        outcome: meridian::llm::detect::ProviderTestOutcome::Failed {
            message: "Disconnected from the dev panel".to_string(),
        },
        elapsed_ms: 0,
        tested_at: chrono::Utc::now().to_rfc3339(),
        // ASSERTED, not measured — so nothing automatic may undo it. Both of the
        // health layer's self-corrections would otherwise erase this within
        // minutes: a successful background call clears a recorded failure, and a
        // day-old failure ages out. Neither is wrong in general; both are wrong
        // for a state the developer is deliberately holding in order to work on
        // it. Only an explicit Test Connection or Rescan replaces this row.
        sticky: true,
    });
    tracing::info!(provider = %id, "llm: provider disconnected (dev)");
    // The whole point of this button is to reach the not-connected state - which IS the
    // banner. Push it now rather than at the next 60 s health tick.
    crate::commands::health::push_health_update(&app).await;
    Ok(())
}

/// Log a sign-in [`InstallOutcome`](meridian::llm::detect::InstallOutcome) at the right level
/// and field name for OpenObserve to actually see a failure — shared by [`cursor_sign_in`],
/// [`codex_sign_in`], and [`claude_sign_in`], whose bodies are otherwise identical modulo which
/// vendor CLI they call.
///
/// Two easy-to-miss couplings this exists to satisfy (see `CLAUDE.md`'s Observability
/// section): a packaged install's ship leg only egresses WARN+ severity, so logging a failure
/// at INFO makes it invisible in OpenObserve even though it was captured locally; and the
/// redaction allowlist (`telemetry_spool::redact::SAFE_STRING_KEYS`) is keyed on exact field
/// names, so a failure message attached under a non-allowlisted key (`detail`, which these
/// three call sites used before this fix) is stripped before shipping even at WARN. `error` is
/// on that allowlist and gets the full free-text scrub, so it is the one to use.
/// `vendor` is a structured `provider` field rather than message interpolation, matching
/// [`log_install_outcome`]. Two reasons beyond the house style: a backend can only group or
/// filter "which CLI is failing sign-in" if the value is a field, and the redaction allowlist is
/// keyed on field NAMES — a vendor baked into the message string reaches the free-text scrubber
/// instead, which is not where it belongs. `provider` is on `SAFE_STRING_KEYS`, so it egresses
/// intact.
///
/// Also marks the enclosing command span ERROR on failure. These commands return
/// `Ok(outcome)` even when the CLI failed — the failure is data, not a transport error — so
/// without this the span of a failed sign-in is indistinguishable from a successful one, and
/// only the WARN log gives it away.
fn log_signin_outcome(vendor: &str, outcome: &meridian::llm::detect::InstallOutcome) {
    if outcome.ok {
        tracing::info!(provider = %vendor, "llm: sign-in complete");
    } else {
        tracing::warn!(provider = %vendor, error = %outcome.message, "llm: sign-in failed");
        mark_span_failed();
    }
}

/// Set the current span's status to ERROR.
///
/// The four commands below hand back `Ok(InstallOutcome { ok: false, .. })` when the vendor CLI
/// fails, because a failed install or sign-in is a result the UI renders, not an error to
/// propagate. Correct for the frontend, invisible in traces: `#[tracing::instrument]` marks a
/// span errored from the RETURN value, so every one of these looks successful.
///
/// `otel.status_code` is the field `tracing-opentelemetry` reads to set span status, and it is
/// on `SAFE_STRING_KEYS`, so it survives the ship leg. Each caller declares it via
/// `fields(otel.status_code = tracing::field::Empty)` — `record` on a field the span never
/// declared is silently a no-op, which is exactly the kind of quiet nothing this fix exists to
/// remove, so the declaration is not optional.
fn mark_span_failed() {
    tracing::Span::current().record("otel.status_code", "ERROR");
}

/// Install one provider's CLI by running its official installer on the user's behalf - the
/// provider detail view's "Install" button. Runs through the user's login shell so `npm`/PATH
/// resolve (see [`meridian::llm::detect::install_provider`]), then confirms the binary is now
/// present. Only ever on an explicit click - the daemon never installs anything automatically.
#[tauri::command]
#[tracing::instrument(fields(otel.status_code = tracing::field::Empty))]
pub async fn install_llm_provider(
    id: String,
) -> Result<meridian::llm::detect::InstallOutcome, String> {
    let provider = meridian_core::LlmProvider::from_wire(&id)
        .ok_or_else(|| format!("unknown provider {id:?}"))?;
    let outcome = meridian::llm::detect::install_provider(provider).await;
    log_install_outcome(&id, &outcome);
    Ok(outcome)
}

/// Log an install [`InstallOutcome`](meridian::llm::detect::InstallOutcome) at the right level
/// for OpenObserve to actually see a failure — see [`log_signin_outcome`]'s doc for why WARN +
/// the `error` key specifically. Split from that function only because this call site also
/// carries a `provider` field the sign-in commands don't have.
fn log_install_outcome(id: &str, outcome: &meridian::llm::detect::InstallOutcome) {
    if outcome.ok {
        tracing::info!(provider = %id, "llm: provider install complete");
    } else {
        tracing::warn!(provider = %id, error = %outcome.message, "llm: provider install failed");
        mark_span_failed();
    }
}

/// Run the interactive `cursor-agent login` - the Cursor detail view's "Sign in to Cursor"
/// button. Opens the user's browser to sign into their own Cursor account, so the summariser
/// runs on their Cursor SUBSCRIPTION (no API key, nothing metered). Only ever on an explicit
/// click; the daemon's own unattended path never opens a browser.
#[tauri::command]
#[tracing::instrument(fields(otel.status_code = tracing::field::Empty))]
pub async fn cursor_sign_in() -> Result<meridian::llm::detect::InstallOutcome, String> {
    let outcome = meridian::llm::detect::cursor_sign_in().await;
    log_signin_outcome("cursor", &outcome);
    Ok(outcome)
}

/// Run the interactive `codex login` - the Codex detail view's "Sign in to Codex" button.
/// Opens the user's browser to sign into their ChatGPT account, so the summariser runs on their
/// ChatGPT SUBSCRIPTION (no API key, nothing metered). Only ever on an explicit click; the
/// daemon's own unattended path never opens a browser. Mirrors [`cursor_sign_in`].
#[tauri::command]
#[tracing::instrument(fields(otel.status_code = tracing::field::Empty))]
pub async fn codex_sign_in() -> Result<meridian::llm::detect::InstallOutcome, String> {
    let outcome = meridian::llm::detect::codex_sign_in().await;
    log_signin_outcome("codex", &outcome);
    Ok(outcome)
}

/// Run the interactive `claude auth login` - the Claude detail view's "Sign in to Claude"
/// button. Opens the user's browser to sign into their Anthropic account, so the summariser runs
/// on their Claude SUBSCRIPTION (no API key, nothing metered). Only ever on an explicit click;
/// the daemon's own unattended path never opens a browser. Mirrors [`cursor_sign_in`].
#[tauri::command]
#[tracing::instrument(fields(otel.status_code = tracing::field::Empty))]
pub async fn claude_sign_in() -> Result<meridian::llm::detect::InstallOutcome, String> {
    let outcome = meridian::llm::detect::claude_sign_in().await;
    log_signin_outcome("claude", &outcome);
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
    use super::{log_install_outcome, log_signin_outcome, notification_state_label};
    use meridian::llm::detect::InstallOutcome;
    use std::sync::{Arc, Mutex};
    use tauri_plugin_notifications::PermissionState;
    use tracing_subscriber::{layer::SubscriberExt, registry, Layer};

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

    /// One captured tracing event: level + `(field name, debug-formatted value)` pairs, so
    /// tests below can assert both the severity (does it clear the packaged-install WARN+
    /// ship threshold) and the exact field name a value landed under (does it clear the
    /// redaction allowlist, which is keyed on exact names — see `log_signin_outcome`'s doc).
    #[derive(Debug)]
    struct CapturedEvent {
        level: tracing::Level,
        fields: Vec<(String, String)>,
    }

    struct Recorder(Arc<Mutex<Vec<CapturedEvent>>>);

    impl<S: tracing::Subscriber> Layer<S> for Recorder {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct FieldVisitor(Vec<(String, String)>);
            impl tracing::field::Visit for FieldVisitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0
                        .push((field.name().to_string(), format!("{value:?}")));
                }
            }
            let mut visitor = FieldVisitor(Vec::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().push(CapturedEvent {
                level: *event.metadata().level(),
                fields: visitor.0,
            });
        }
    }

    /// Run `f` under a bare recording subscriber (no `EnvFilter` — every event is captured
    /// regardless of level) and return what it emitted.
    ///
    /// Reads the captured events back out through the shared `Mutex` rather than demanding
    /// unique ownership of `seen` (`Arc::try_unwrap`) — `with_default`'s guard only resets
    /// the *thread-local* default once it drops, and gives no guarantee about exactly when
    /// the `Dispatch` holding our `Recorder`'s own `Arc` clone is deallocated. That
    /// `try_unwrap` flaked intermittently on Windows CI (`Err` because a second strong ref
    /// was still alive at that instant) despite passing reliably elsewhere. Locking and
    /// draining works regardless of how many clones of `seen` are still outstanding.
    fn capture(f: impl FnOnce()) -> Vec<CapturedEvent> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let subscriber = registry().with(Recorder(Arc::clone(&seen)));
        tracing::subscriber::with_default(subscriber, f);
        // Take the events THROUGH the Arc rather than unwrapping it. `with_default` restores
        // the previous dispatcher on return but tracing does not promise the old one is
        // dropped by then - it keeps a thread-local cache - so a second strong reference to
        // the recorder can still be alive here. `Arc::try_unwrap` then fails, which showed up
        // as this test failing roughly one run in three with the events it was asserting on
        // printed in the panic message.
        let events = std::mem::take(&mut *seen.lock().unwrap());
        events
    }

    fn field<'a>(event: &'a CapturedEvent, name: &str) -> Option<&'a str> {
        event
            .fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn ok_outcome() -> InstallOutcome {
        InstallOutcome {
            ok: true,
            message: "codex 0.1.0".to_string(),
            path: Some("/usr/local/bin/codex".to_string()),
            command: "npm i -g @openai/codex".to_string(),
        }
    }

    fn failed_outcome() -> InstallOutcome {
        InstallOutcome {
            ok: false,
            message: "zsh:1: command not found: npm".to_string(),
            path: None,
            command: "npm i -g @openai/codex".to_string(),
        }
    }

    // Regression: this used to log at INFO even on failure, which never egresses on a
    // packaged install (only WARN+ ships to OpenObserve) — see the `npm: command not found`
    // report this fixes.
    #[test]
    fn install_outcome_failure_logs_at_warn() {
        let events = capture(|| log_install_outcome("codex", &failed_outcome()));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, tracing::Level::WARN);
    }

    #[test]
    fn install_outcome_success_logs_at_info() {
        let events = capture(|| log_install_outcome("codex", &ok_outcome()));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, tracing::Level::INFO);
    }

    // Regression: the failure message must land under `error` — the key
    // `telemetry_spool::redact::SAFE_STRING_KEYS` allowlists — not `detail`, which is
    // silently stripped before shipping even though the event clears the WARN severity gate.
    #[test]
    fn install_outcome_failure_uses_allowlisted_error_field() {
        let events = capture(|| log_install_outcome("codex", &failed_outcome()));
        let msg = field(&events[0], "error").expect("expected an `error` field");
        assert!(msg.contains("command not found"));
        assert!(
            field(&events[0], "detail").is_none(),
            "must not use the non-allowlisted `detail` key"
        );
        assert_eq!(field(&events[0], "provider"), Some("codex"));
    }

    #[test]
    fn signin_outcome_failure_logs_at_warn_with_error_field() {
        let events = capture(|| log_signin_outcome("cursor", &failed_outcome()));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, tracing::Level::WARN);
        let msg = field(&events[0], "error").expect("expected an `error` field");
        assert!(msg.contains("command not found"));
        assert!(
            field(&events[0], "detail").is_none(),
            "must not use the non-allowlisted `detail` key"
        );
    }

    #[test]
    fn signin_outcome_success_logs_at_info() {
        let events = capture(|| log_signin_outcome("cursor", &ok_outcome()));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, tracing::Level::INFO);
    }

    /// The vendor has to be a FIELD, not interpolated into the message, for the same
    /// allowlist reason the `error` key exists: redaction is keyed on field names, and a
    /// backend can only group "which CLI is failing sign-in" over a field. `provider` is
    /// the name `log_install_outcome` already uses and is on `SAFE_STRING_KEYS` — a
    /// different spelling here would ship as nothing.
    #[test]
    fn signin_outcome_records_the_vendor_as_a_provider_field() {
        for (outcome, label) in [(failed_outcome(), "failure"), (ok_outcome(), "success")] {
            let events = capture(|| log_signin_outcome("cursor", &outcome));
            assert_eq!(
                field(&events[0], "provider"),
                Some("cursor"),
                "the {label} branch must carry the vendor as a `provider` field"
            );
        }
    }

    /// Sign-in and install commands return `Ok(InstallOutcome { ok: false, .. })` when the
    /// vendor CLI fails — right for the UI, but it means `#[tracing::instrument]` (which
    /// derives span status from the return value) marks a FAILED sign-in as a successful
    /// span. `mark_span_failed` is what corrects that.
    ///
    /// Asserted through a real span rather than by inspecting the helper, because the part
    /// that silently breaks is the coupling: `record` on a field the span never declared in
    /// `fields(...)` is a no-op, so dropping the declaration from a command's
    /// `#[tracing::instrument]` would lose the status with nothing failing.
    #[test]
    fn a_failed_outcome_marks_the_enclosing_span_errored() {
        let recorded: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

        struct SpanRecorder(Arc<Mutex<Vec<(String, String)>>>);
        impl<S> Layer<S> for SpanRecorder
        where
            S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
        {
            fn on_record(
                &self,
                _id: &tracing::span::Id,
                values: &tracing::span::Record<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                struct V(Arc<Mutex<Vec<(String, String)>>>);
                impl tracing::field::Visit for V {
                    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                        self.0
                            .lock()
                            .unwrap()
                            .push((f.name().to_string(), format!("{v:?}")));
                    }
                }
                values.record(&mut V(Arc::clone(&self.0)));
            }
        }

        let subscriber = registry().with(SpanRecorder(Arc::clone(&recorded)));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("llm_sign_in", otel.status_code = tracing::field::Empty);
            span.in_scope(|| log_signin_outcome("cursor", &failed_outcome()));
        });

        let got = recorded.lock().unwrap().clone();
        assert!(
            got.iter()
                .any(|(k, v)| k == "otel.status_code" && v.contains("ERROR")),
            "a failed sign-in must mark its span ERROR, got {got:?}"
        );

        // And the success path must NOT, or every trace would look broken.
        let clean: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = registry().with(SpanRecorder(Arc::clone(&clean)));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("llm_sign_in", otel.status_code = tracing::field::Empty);
            span.in_scope(|| log_signin_outcome("cursor", &ok_outcome()));
        });
        assert!(
            !clean
                .lock()
                .unwrap()
                .iter()
                .any(|(k, _)| k == "otel.status_code"),
            "a successful sign-in must leave span status unset"
        );
    }

    /// Every command that can hand back `ok: false` must DECLARE `otel.status_code` in its
    /// `#[tracing::instrument]`, or `mark_span_failed`'s `record` is a silent no-op. Source
    /// text is the only place this is checkable — the attribute leaves nothing to inspect at
    /// runtime from here.
    #[test]
    fn the_failable_commands_declare_the_span_status_field() {
        let src = include_str!("setup.rs");
        for func in [
            "pub async fn install_llm_provider(",
            "pub async fn cursor_sign_in()",
            "pub async fn codex_sign_in()",
            "pub async fn claude_sign_in()",
        ] {
            let at = src.find(func).unwrap_or_else(|| panic!("{func} not found"));
            let preceding = &src[at.saturating_sub(200)..at];
            assert!(
                preceding.contains("otel.status_code = tracing::field::Empty"),
                "{func} must declare otel.status_code, or mark_span_failed silently does nothing"
            );
        }
    }
}
