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

use serde::Serialize;

/// Returns `true` on the first launch — no `~/.meridian/onboarded` flag exists.
/// The wizard auto-opens when `true` and is skipped on subsequent launches.
#[tauri::command]
#[tracing::instrument]
pub async fn is_first_run() -> bool {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    !std::path::Path::new(&format!("{home}/.meridian/onboarded")).exists()
}

/// Write `~/.meridian/onboarded` (RFC-3339 timestamp) to mark wizard completion.
/// Future tray launches skip the auto-open. Idempotent — safe to call more than once.
#[tauri::command]
#[tracing::instrument]
pub async fn mark_setup_complete() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(format!("{home}/.meridian"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create ~/.meridian: {e}"))?;
    tokio::fs::write(dir.join("onboarded"), chrono::Local::now().to_rfc3339())
        .await
        .map_err(|e| format!("write onboarded: {e}"))?;
    tracing::info!("setup: onboarded flag written");
    Ok(())
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
    #[cfg(target_os = "macos")]
    return ax_is_trusted();
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
fn ax_is_trusted() -> bool {
    // ApplicationServices.framework is a system framework present on all macOS
    // versions; no additional Cargo dep required.
    #[link(name = "ApplicationServices", kind = "framework")]
    #[allow(improper_ctypes)]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    // Safety: AXIsProcessTrusted is a pure status read with no side effects or UB.
    unsafe { AXIsProcessTrusted() }
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

/// Current macOS notification authorization for the wizard's Notifications card.
///
/// Returns `granted` / `denied` / `prompt` (never asked — requesting will show
/// the system dialog) / `unavailable` (unbundled run: the plugin registers only
/// inside a `.app` bundle, so `tauri dev` has no notification backend at all).
///
/// Unlike the TCC probes above this is not a boolean: `denied` and `prompt`
/// need different grant actions (macOS shows the authorization dialog exactly
/// once — after a deny the only path back is System Settings → Notifications),
/// so the wizard must know which side of that line the user is on.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn check_notifications(app: tauri::AppHandle) -> String {
    let Some(nf) = crate::sys::notifier(&app) else {
        return "unavailable".into();
    };
    match nf.permission_state().await {
        Ok(state) => notification_state_label(state).into(),
        Err(e) => {
            tracing::warn!(error = %e, "setup: notification permission probe failed");
            "unavailable".into()
        }
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
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn request_notifications(app: tauri::AppHandle) -> String {
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

/// Detected hardware specs for the wizard's "Local intelligence" step. Drives the
/// real spec panel + the memory gauge (the model's resident footprint is sized
/// against `ram_gb`), so every number here must be live — never fabricated.
#[derive(Debug, Serialize, Default)]
pub struct SystemSpecs {
    /// Marketing chip name, e.g. "Apple M3 Pro" (empty if undetectable).
    pub chip: String,
    /// "macOS <version>", e.g. "macOS 15.5".
    pub macos: String,
    /// Physical CPU core count (`hw.physicalcpu`).
    pub cpu_cores: u32,
    /// GPU core count parsed from `system_profiler` (0 when unknown).
    pub gpu_cores: u32,
    /// Unified memory in whole GB (`hw.memsize`, rounded).
    pub ram_gb: u32,
    /// Free space in whole GB on the data volume (0 when undetectable).
    pub free_disk_gb: u32,
}

/// Probe this Mac's chip, core counts, unified memory, and free disk for the
/// wizard's Local-intelligence step. macOS-only — all reads are non-privileged
/// (`sysctl`, `sw_vers`, `system_profiler`, `df`); on other platforms the wizard
/// gets `SystemSpecs::default()` (all zeros) and degrades to a generic panel.
///
/// Runs the shell-outs on a blocking thread (`system_profiler` can take ~1 s) so
/// the async runtime isn't stalled during the one-time wizard probe.
#[tauri::command]
#[tracing::instrument]
pub async fn detect_system_specs() -> SystemSpecs {
    #[cfg(target_os = "macos")]
    {
        let specs = tokio::task::spawn_blocking(detect_specs_blocking)
            .await
            .unwrap_or_default();
        tracing::info!(
            chip = %specs.chip,
            cpu_cores = specs.cpu_cores,
            gpu_cores = specs.gpu_cores,
            ram_gb = specs.ram_gb,
            free_disk_gb = specs.free_disk_gb,
            "setup: system specs detected"
        );
        specs
    }
    #[cfg(not(target_os = "macos"))]
    SystemSpecs::default()
}

#[cfg(target_os = "macos")]
fn detect_specs_blocking() -> SystemSpecs {
    use std::process::Command;

    // Trim a command's stdout to a clean single-line String ("" on any failure).
    let run = |bin: &str, args: &[&str]| -> String {
        Command::new(bin)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };

    let chip = run("sysctl", &["-n", "machdep.cpu.brand_string"]);
    let macos_ver = run("sw_vers", &["-productVersion"]);
    let macos = if macos_ver.is_empty() {
        String::new()
    } else {
        format!("macOS {macos_ver}")
    };
    let cpu_cores: u32 = run("sysctl", &["-n", "hw.physicalcpu"])
        .parse()
        .unwrap_or(0);
    let mem_bytes: u64 = run("sysctl", &["-n", "hw.memsize"]).parse().unwrap_or(0);
    // hw.memsize is bytes of unified memory; divide by 1024³ (GiB) so a 16 GiB
    // Mac (17179869184 bytes) reads as 16 — dividing by 1e9 would round it to 17.
    let ram_gb = ((mem_bytes as f64) / 1_073_741_824.0).round() as u32;

    // GPU cores live in `system_profiler SPDisplaysDataType` as a
    // "Total Number of Cores: <n>" line on Apple Silicon.
    let gpu_cores = run("system_profiler", &["SPDisplaysDataType"])
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("Total Number of Cores:")
                .and_then(|n| n.trim().parse::<u32>().ok())
        })
        .unwrap_or(0);

    // Free space on the user-data volume. `df -k` prints 1 K-blocks; column 4 is
    // available. Parse the data row (the 2nd line of output).
    let free_disk_gb = run("df", &["-k", "/System/Volumes/Data"])
        .lines()
        .nth(1)
        .and_then(|row| row.split_whitespace().nth(3))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| ((kb as f64) * 1024.0 / 1e9).round() as u32)
        .unwrap_or(0);

    SystemSpecs {
        chip,
        macos,
        cpu_cores,
        gpu_cores,
        ram_gb,
        free_disk_gb,
    }
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
