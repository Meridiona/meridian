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
//! - [`crate::commands::walkthrough`] — owns the walkthrough's markers; this
//!   module writes one of them and reads it back, nothing else.

// Name of the marker that ARMS the dashboard walkthrough — written here when a
// FIRST RUN completes the wizard, read back by `walkthrough_is_armed`. Owned by
// `commands::walkthrough`, which does the other read and the clear, so the name
// cannot drift between writer and readers.
use crate::commands::walkthrough::ARMED_MARKER as WALKTHROUGH_MARKER;

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

/// Write `~/.meridian/onboarded` (RFC-3339 timestamp) to mark wizard completion.
/// Future tray launches skip the auto-open. Idempotent — safe to call more than once.
///
/// # Setup is not onboarding
///
/// The same wizard serves two different events: a first run, and Settings →
/// Account → Re-run Setup. Only the first is ONBOARDING, and only it writes the
/// two markers that belong to onboarding rather than to configuration — the
/// walkthrough entitlement (`walkthrough_armed`) and the "you have effectively
/// read this release's notes" stamp (`last_seen_version`). A re-run refreshes
/// `onboarded` and nothing else.
///
/// The two are told apart by reading `onboarded` before overwriting it; see
/// [`write_completion_markers`], where that single line is the whole separation.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn mark_setup_complete(app: tauri::AppHandle) -> Result<(), String> {
    let home = meridian_core::paths::home_dir()
        .ok_or_else(|| "could not resolve home directory".to_string())?;
    // Read the version off the AppHandle rather than `env!("CARGO_PKG_VERSION")`
    // so it is byte-identical to what `poll::whats_new_auto_open` compares the
    // marker against — the two are only equal by construction, and a marker
    // written in a format the reader never matches would silently do nothing.
    let version = app.package_info().version.to_string();
    let first_run = write_completion_markers(&home, &version).map_err(
        |e| crate::cmd_err!(level: error, e, "setup: could not write the completion markers"),
    )?;
    // `first_run` is the whole story of this call and is not derivable after the
    // fact — the marker it keys off has just been overwritten — so it goes on the
    // log line. Without it, a re-run and a genuine first run are indistinguishable
    // in the trace, which is exactly the confusion this change exists to end.
    tracing::info!(
        version = %version,
        first_run,
        "setup: onboarded flag written"
    );
    Ok(())
}

/// The filesystem half of [`mark_setup_complete`], split out so it is reachable
/// from a test without an `AppHandle` (see this module's tests).
///
/// `home` is the HOME directory, not `~/.meridian` — matching
/// [`crate::commands::whats_new`]'s readers, which do their own `.meridian`
/// join. Sync `std::fs` rather than `tokio::fs`: three small writes, and it
/// matches the sibling marker helpers in `whats_new`, which are already called
/// straight from the async poll loop.
///
/// Returns whether this was a FIRST RUN — the caller logs it, and it is not
/// recoverable afterwards because the marker it is read from has by then been
/// overwritten.
fn write_completion_markers(home: &std::path::Path, version: &str) -> anyhow::Result<bool> {
    use anyhow::Context;

    let dir = home.join(".meridian");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    // Read BEFORE the write below overwrites it. This is the ONE moment "setup"
    // and "onboarding" are distinguishable, and everything that separates them
    // hangs off this line — see the two `first_run` branches below.
    let first_run = !dir.join("onboarded").exists();
    let now = chrono::Local::now().to_rfc3339();
    // ORDER IS LOAD-BEARING: the first-run markers are written BEFORE `onboarded`,
    // and `onboarded` only once they have all succeeded.
    //
    // `onboarded` is the flag that makes the NEXT call read `first_run == false`.
    // Writing it first meant a partial failure here — a full disk, a permission
    // fault, anything — left the install permanently marked as onboarded while the
    // two markers that first run owed it were never written. The next attempt then
    // looks like a re-run and takes neither branch, so the user never gets the
    // walkthrough and What's New fires the moment the wizard closes: both of the
    // bugs these markers exist to prevent, arriving together and silently.
    //
    // Written last, a failure leaves `onboarded` absent, the run is still a first
    // run next time, and the whole thing retries.
    if first_run {
        // The walkthrough is the second half of ONBOARDING, so a first run is what
        // earns it. See [`walkthrough_is_armed`] for why it needs its own marker
        // rather than reading `onboarded`.
        //
        // Deliberately NOT written on a re-run (Settings → Account → Re-run
        // Setup), which is configuration, not onboarding. Arming there does not
        // even show the tour at the time — the hook that reads this marker runs
        // once per dashboard mount and the dashboard is already mounted — so the
        // entitlement just waits on disk and ambushes the next fresh mount, which
        // in practice is a relaunch after an app update. That is the exact
        // scenario this marker was introduced to prevent, arriving through the
        // one door nobody checked. See `rerunning_setup_does_not_rearm_the_walkthrough`.
        std::fs::write(dir.join(WALKTHROUGH_MARKER), &now)
            .with_context(|| format!("write {WALKTHROUGH_MARKER}"))?;
        // Stamp the running version as already seen, so "What's New" starts
        // announcing the NEXT release rather than firing the instant the wizard
        // closes. `has_unseen_release_notes` reads a missing marker as unseen —
        // right for an update, wrong for a first run, where the notes describe a
        // release the user has never not been on. `poll::whats_new_auto_open`'s
        // `onboarded` gate only defers that to the moment onboarding ends, so the
        // suppression has to be a written marker, not another gate.
        //
        // Also first-run only, and for the mirror-image reason: someone re-running
        // setup on a release whose notes they have NOT read is owed that
        // announcement, and swallowing it here would be silent.
        crate::commands::whats_new::mark_whats_new_seen(home, version)
            .context("stamp the running version as seen")?;
    }
    // Last, and only once every first-run marker above landed. See the comment on
    // `first_run` for why this ordering is the whole point of the function.
    std::fs::write(dir.join("onboarded"), &now).context("write the onboarded marker")?;
    Ok(first_run)
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
/// loses the walkthrough for good.
///
/// It IS cleared elsewhere — by [`crate::commands::walkthrough::disarm_walkthrough`],
/// invoked from the same place the frontend writes its localStorage seen key, so
/// the marker is not write-once and must not be assumed to be. The distinction
/// that matters is *where*: on finish or skip, never on a read and never on an
/// abort, which is what preserves the mid-tour-quit guarantee above.
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
    // with no CLI to probe, so it has no install state. But the test cache DOES hold a row
    // for the ACTIVE custom endpoint - written by `test_llm_provider` and by the dev
    // Disconnect button - and dropping it here meant nothing on the chooser could ever see it.
    // The Groq tile therefore rendered IN USE off `value === 'custom'` alone: a green,
    // confident "connected" for a provider the app had just been told is not.
    //
    // Keyed by `provider_key` for the ACTIVE endpoint specifically - NOT the bare literal
    // `"custom"`, which every configured cloud endpoint shares regardless of which is active.
    // Looking that up directly used to mean an Ollama row inherited whatever a PREVIOUSLY
    // active Groq row had last tested as, the moment the user switched between them - a real
    // cross-contamination bug, only ever invisible while a user had exactly one cloud
    // endpoint configured at all. Only the ACTIVE endpoint's row is injected here (not every
    // configured row) - a non-active endpoint's card falls back to "not yet tested" until the
    // user opens it, which is the existing, honest default for anything untested.
    let settings = meridian_core::settings::load_runtime_settings();
    if let Some(active) = settings.active_custom_provider() {
        let key = meridian::llm::resolver::provider_key(
            meridian_core::LlmProvider::Custom,
            Some(active.id.as_str()),
        );
        if let Some(last) = meridian::llm::detect::cached_test_result(&key) {
            found.push(meridian::llm::detect::ProviderStatus {
                id: key,
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

/// Parse `test_llm_provider`'s `id` argument: either a bare wire name ("claude", "custom",
/// …) or a specific custom endpoint's variant id (`ui/lib/llm-providers.ts::customVariantId`,
/// `"custom:<id>"`) - the picker addresses one tile per configured endpoint, not one shared
/// "custom" tile, so [`meridian_core::LlmProvider::from_wire`] alone (which only knows the
/// bare wire names) rejects the variant form outright with "unknown provider". Pulled out of
/// the command as a pure fn so the parsing is unit-testable without a `tauri::AppHandle`.
fn parse_test_provider_id(
    id: &str,
) -> Result<(meridian_core::LlmProvider, Option<String>), String> {
    match id.split_once(':') {
        Some(("custom", endpoint_id)) if !endpoint_id.is_empty() => Ok((
            meridian_core::LlmProvider::Custom,
            Some(endpoint_id.to_string()),
        )),
        _ => Ok((
            meridian_core::LlmProvider::from_wire(id)
                .ok_or_else(|| format!("unknown provider {id:?}"))?,
            None,
        )),
    }
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
    let (provider, custom_id) = parse_test_provider_id(&id)?;
    let mut settings = meridian_core::settings::load_runtime_settings();
    // `test_provider` reads the endpoint to test from `settings.active_custom_provider()`,
    // which only ever resolves the GLOBALLY selected endpoint - so testing a non-active
    // endpoint's tile means overriding the selection on this throwaway copy before calling
    // it, not touching what's actually persisted or globally active.
    if let Some(custom_id) = custom_id {
        settings.llm_provider = "custom".to_string();
        settings.llm_provider_custom_id = Some(custom_id);
    }
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
    // The SAME key `test_llm_provider`/the pipeline file results under (see `provider_key`) -
    // for `Custom` that is the ACTIVE endpoint's own id, never the bare literal `"custom"`
    // every configured endpoint would otherwise share.
    let settings = meridian_core::settings::load_runtime_settings();
    let key = meridian::llm::resolver::provider_key(
        provider,
        settings.active_custom_provider().map(|c| c.id.as_str()),
    );
    meridian::llm::detect::persist_test_result(&meridian::llm::detect::ProviderTestResult {
        id: key,
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
    use super::{
        log_install_outcome, log_signin_outcome, notification_state_label, parse_test_provider_id,
        write_completion_markers, WALKTHROUGH_MARKER,
    };
    use crate::commands::whats_new::has_unseen_release_notes;
    use meridian::llm::detect::InstallOutcome;
    use std::sync::{Arc, Mutex};
    use tauri_plugin_notifications::PermissionState;
    use tracing_subscriber::{layer::SubscriberExt, registry, Layer};

    /// The bug this fn exists to fix: the picker addresses a specific custom endpoint as
    /// `"custom:<id>"` (`ui/lib/llm-providers.ts::customVariantId`), which bare
    /// `LlmProvider::from_wire` rejects outright.
    #[test]
    fn a_custom_variant_id_parses_into_custom_plus_its_endpoint_id() {
        let (provider, custom_id) = parse_test_provider_id("custom:groq1").unwrap();
        assert_eq!(provider, meridian_core::LlmProvider::Custom);
        assert_eq!(custom_id, Some("groq1".to_string()));
    }

    #[test]
    fn bare_wire_names_still_parse_with_no_endpoint_id() {
        let (provider, custom_id) = parse_test_provider_id("claude").unwrap();
        assert_eq!(provider, meridian_core::LlmProvider::Claude);
        assert_eq!(custom_id, None);

        let (provider, custom_id) = parse_test_provider_id("custom").unwrap();
        assert_eq!(provider, meridian_core::LlmProvider::Custom);
        assert_eq!(custom_id, None);
    }

    #[test]
    fn an_unknown_provider_name_is_rejected() {
        assert!(parse_test_provider_id("groq").is_err());
        assert!(parse_test_provider_id("").is_err());
    }

    /// `"custom:"` with an empty id names no endpoint - must fail loudly rather than
    /// silently falling back to whatever happens to be globally active.
    #[test]
    fn a_custom_variant_with_an_empty_endpoint_id_is_rejected() {
        assert!(parse_test_provider_id("custom:").is_err());
    }

    // A fresh install has no `last_seen_version`, and `has_unseen_release_notes`
    // reads a missing marker as "unseen" (`Err(_) => true`) — correct for an
    // update, wrong for a first run. `poll::whats_new_auto_open`'s only other
    // guard is the `onboarded` flag, so the modal was suppressed for exactly as
    // long as the wizard was open and then fired the moment it closed: a brand
    // new user's first sight of the dashboard was a changelog for a release
    // they had never run.
    //
    // Finishing the wizard is therefore what stamps the running version as
    // seen. This asserts the whole three-way contract, because getting the
    // version string right matters as much as writing the file at all.
    #[test]
    fn finishing_the_wizard_marks_the_running_version_as_already_seen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();

        // Fresh install: nothing seen yet, so the auto-open would fire.
        assert!(
            has_unseen_release_notes(home, "1.85.1"),
            "precondition: a fresh install has no last_seen_version marker"
        );

        write_completion_markers(home, "1.85.1").expect("write markers");

        // 1. The version they onboarded on is NOT new to them.
        assert!(
            !has_unseen_release_notes(home, "1.85.1"),
            "What's New must not auto-open for the version the user just onboarded on"
        );
        // 2. The NEXT release still is — this is the whole point of the modal,
        //    and a fix that suppressed it forever would pass assertion 1 too.
        assert!(
            has_unseen_release_notes(home, "1.86.0"),
            "the next release must still auto-open"
        );
        // 3. The markers this function already owned are untouched.
        assert!(home.join(".meridian").join("onboarded").exists());
        assert!(home.join(".meridian").join(WALKTHROUGH_MARKER).exists());
    }

    /// THE ONE THAT MATTERS. Settings → Account → Re-run Setup opens the same
    /// wizard as the first run, so its completion used to write the same three
    /// markers — including the one that ENTITLES the dashboard to take the
    /// screen over with a first-run tour.
    ///
    /// The tour is not delivered at that moment: the hook that reads the marker
    /// is guarded to run once per dashboard mount, and re-running setup happens
    /// with the dashboard already mounted. So the entitlement sat on disk and
    /// was cashed in on the next FRESH mount — in the reported case a relaunch
    /// after an app update, which is precisely the "months-old install gets
    /// handed a full-screen tour" scenario the entitlement marker exists to
    /// prevent.
    ///
    /// `onboarded` already carries the answer; it just has to be read before it
    /// is overwritten.
    #[test]
    fn rerunning_setup_does_not_rearm_the_walkthrough() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let meridian = home.join(".meridian");
        std::fs::create_dir_all(&meridian).expect("mkdir");

        // An install that onboarded some time ago, has already taken the tour
        // (so no armed marker), and is now re-running setup to fix a permission.
        std::fs::write(meridian.join("onboarded"), "2026-01-01T00:00:00+00:00").expect("seed");

        let first_run = write_completion_markers(home, "1.86.0").expect("write markers");

        assert!(
            !first_run,
            "an install with an onboarded marker is not a first run"
        );
        assert!(
            !meridian.join(WALKTHROUGH_MARKER).exists(),
            "re-running setup must not arm the first-run walkthrough"
        );
        // Setup still did its own job — the onboarded stamp is refreshed.
        assert!(meridian.join("onboarded").exists());
    }

    /// The companion to the test above, and the reason it cannot simply drop the
    /// arming: a genuine first run is the one case that MUST arm it.
    #[test]
    fn a_first_run_still_arms_the_walkthrough() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();

        let first_run = write_completion_markers(home, "1.86.0").expect("write markers");

        assert!(first_run, "nothing on disk yet - this is the first run");
        assert!(
            home.join(".meridian").join(WALKTHROUGH_MARKER).exists(),
            "a first run is exactly who the walkthrough is for"
        );
    }

    /// `last_seen_version` is stamped for the same reason and on the same terms:
    /// a first-run user has never *not* been on the release the notes describe,
    /// so announcing it is noise. Someone re-running setup on a version they
    /// have not yet read the notes for is in the opposite position — suppressing
    /// What's New there silently eats a release announcement, and setup is
    /// configuration, not a changelog.
    #[test]
    fn rerunning_setup_leaves_unread_release_notes_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let meridian = home.join(".meridian");
        std::fs::create_dir_all(&meridian).expect("mkdir");
        std::fs::write(meridian.join("onboarded"), "2026-01-01T00:00:00+00:00").expect("seed");

        write_completion_markers(home, "1.86.0").expect("write markers");

        assert!(
            has_unseen_release_notes(home, "1.86.0"),
            "re-running setup must not mark an unread release as seen"
        );
    }

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
