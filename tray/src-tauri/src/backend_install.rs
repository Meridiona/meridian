//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Bundled-backend first-run install orchestration (Gap-2 Bucket 1, slice 1b).
//!
//! The daemon ships inside `Meridian.app/Contents/Resources/backend/` (wired by
//! `tauri.conf.json`'s `bundle.resources`). This module is the tray side of that:
//! on startup it stages the binary to the **stable** `~/.meridian/bin/` path and
//! registers its launchd agent — so the self-contained `.app` DMG needs no shell
//! installer. (The DMG is the only packaged install path; the old npm bundle was
//! retired.)
//!
//! It is a **faithful port of the launchctl flow, not SMAppService**:
//! SMAppService's managed-Login-Items payoff only matters once the app is
//! Developer-ID-signed (Gap-2 Bucket 3), so it's deferred to then.
//!
//! The DMG daemon's WorkingDirectory is `~/.meridian`, so `dotenvy` self-loads
//! the **canonical** `~/.meridian/.env` the tray already writes to — unifying tray
//! and daemon on one credential file. A migrant from the retired npm bundle (which
//! used WorkingDirectory `~/.meridian/app` and read `~/.meridian/app/.env`) has that
//! file copied across once ([`migrate_legacy_bundle_env`]), and stale bundle launchd
//! agents (screenpipe / a11y-helper / MLX / UI server) are booted out during
//! [`install`].
//!
//! **The `meridian-a11y-helper` launchd agent is retired** (was
//! `com.meridiona.a11y-helper`): it existed to poke `AXManualAccessibility`
//! on Electron/Chromium apps for the old *external* screenpipe process. The
//! in-process capture engine's `screenpipe-a11y` tree walker
//! ([`crate::capture::screenpipe`]) now does that poke itself, under the
//! tray's own Accessibility grant — so the helper's separate launchd agent
//! and its own "meridian-a11y-helper" entry in System Settings → Privacy &
//! Security → Accessibility were pure redundancy. [`cleanup_legacy_a11y_helper`]
//! retires any leftover install from an update.
//!
//! # Who calls this
//! [`crate::run`]'s Tauri `setup` hook spawns [`ensure_backend_installed`] once,
//! off the main thread (the launchd bootout-wait can take seconds).
//!
//! # Related
//! - [`crate::install`] — resolves where data lives; [`crate::install::meridian_bin`]
//!   prefers the `~/.meridian/bin/meridian` this module stages.
//! - `scripts/install-daemon.sh` — the source-install shell flow this parallels.

use std::path::{Path, PathBuf};
use std::time::Duration;

// Used only by the Windows service-registration spawns below (schtasks / taskkill
// / tasklist / the daemon launch) to suppress their console-window flash; the
// trait method is a no-op on other platforms and would be an unused import there.
#[cfg(target_os = "windows")]
use meridian_core::proc_ext::NoWindow;
use tauri::Manager;

/// SHA-256 of a file as a lowercase hex string — the bundled-daemon update marker.
fn sha256_hex_of(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// launchd agents this stages, paired with their bundled plist template.
#[cfg(target_os = "macos")]
const AGENTS: &[(&str, &str)] = &[("com.meridiona.daemon", "com.meridiona.daemon.plist")];

/// The daemon executable's file name inside `Resources/backend/` and at its
/// staged destination. Windows needs the `.exe` suffix to be runnable at all;
/// `tauri.windows.conf.json`'s resource map bundles it under that name.
#[cfg(target_os = "windows")]
const DAEMON_FILE: &str = "meridian.exe";
#[cfg(not(target_os = "windows"))]
const DAEMON_FILE: &str = "meridian";

/// Stage the bundled backend and register its launchd agent — idempotent and
/// non-fatal.
///
/// No-op unless **all** hold: running from a packaged `.app` whose
/// `Resources/backend/` exists (absent under `tauri dev` and source checkouts —
/// those keep using the shell scripts), and the bundled daemon binary's SHA-256
/// differs from the last successful install (first run, or a post-update where
/// the shipped binary changed). Any staging/launchctl failure is logged and
/// swallowed so a backend hiccup never crashes the tray; the marker is persisted
/// **only after** the agent bootstraps, so a partial failure retries next launch.
#[tracing::instrument(skip(app))]
pub async fn ensure_backend_installed(app: &tauri::AppHandle) {
    let backend = match bundled_backend_dir(app) {
        Some(d) => d,
        None => {
            tracing::debug!("backend_install: no bundled backend (dev/source run) — skipping");
            return;
        }
    };
    let home = match meridian_core::paths::home_dir() {
        Some(h) => h,
        None => {
            tracing::warn!(
                "backend_install: home directory could not be resolved — cannot stage backend"
            );
            return;
        }
    };

    let daemon_src = backend.join(DAEMON_FILE);
    let bundled_hash = match sha256_hex_of(&daemon_src) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, src = %daemon_src.display(), "backend_install: cannot hash bundled daemon");
            return;
        }
    };
    let marker = home.join(".meridian/backend-version");
    if tokio::fs::read_to_string(&marker).await.ok().as_deref() == Some(bundled_hash.as_str()) {
        tracing::debug!(hash = %bundled_hash, "backend_install: backend up to date — skipping staging");
        // Staging is current, but the daemon may not be running: the
        // encrypt-in-place migration stops it earlier this launch (see lib.rs's
        // setup hook) to unlock meridian.db, and it can also simply crash. Bring
        // it back so a skipped *staging* never leaves a stopped *daemon*.
        ensure_daemon_running(&home).await;
        return;
    }

    // `ensure_backend_installed` runs off the setup hook, after `app.manage(db_pool)`
    // (see `lib.rs`), so the pool is already there to raise/clear against —
    // `None` only when the DB itself couldn't be opened, in which case there's
    // nowhere to write a notice anyway.
    let pool = app
        .try_state::<Option<meridian_core::SqlitePool>>()
        .and_then(|s| s.inner().clone());

    tracing::info!(hash = %bundled_hash, "backend_install: installing bundled backend");
    if let Err(e) = install(&backend, &home).await {
        tracing::error!(error = %e, "backend_install: install failed — will retry next launch");
        // Previously silent beyond the log line: staging/registration can fail
        // for reasons a user can actually act on (antivirus quarantining the
        // staged exe, a locked-down profile denying the write, a full disk),
        // and until the health-check's 2-strike "went quiet" eventually fires
        // there was no in-app signal at all — and even then, no reason. Surface
        // it immediately and specifically instead of waiting on that generic
        // banner.
        if let Some(p) = pool.as_ref() {
            if let Err(notice_err) = meridian::notices::raise_typed(
                p,
                meridian::notices::Notice {
                    id: "tray.backend_install_failed",
                    severity: "error",
                    title: "Meridian couldn't finish installing.",
                    detail: &e,
                    remedy: None,
                    event_key: "system.health",
                    deep_link: Some("/logs"),
                },
            )
            .await
            {
                tracing::warn!(error = %notice_err, "backend-install-failure notice raise failed");
            }
        }
        return;
    }
    if let Some(p) = pool.as_ref() {
        if let Err(e) =
            meridian::notices::clear_typed(p, "tray.backend_install_failed", "system.health").await
        {
            tracing::warn!(error = %e, "backend-install-failure notice clear failed");
        }
    }

    // Persist the marker only on full success so a partial install retries.
    if let Err(e) = tokio::fs::write(&marker, &bundled_hash).await {
        tracing::error!(error = %e, "backend_install: could not write version marker");
    }
    tracing::info!("backend_install: backend installed");
}

/// Stop the running daemon so the tray's in-place `meridian.db` encryption
/// migration can rename the file. **Windows-only concern**: a rename of a file
/// another process holds open fails there with os error 32, which is why
/// `encrypt_in_place` rolled back every launch while the daemon (autostarted at
/// login) held the DB open. A no-op on other platforms, where the migration
/// tolerates the open handle.
///
/// Called from the tray's setup hook BEFORE `encrypt_in_place`, and only when a
/// migration will actually attempt (a key is set and the DB is still plaintext).
/// The daemon is brought back up afterward by [`ensure_backend_installed`] —
/// either its normal staging path (on an update) or [`ensure_daemon_running`]
/// (on the up-to-date path).
pub(crate) async fn stop_daemon_for_migration() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let home = meridian_core::paths::home_dir()
            .ok_or_else(|| "home directory could not be resolved".to_string())?;
        let daemon_bin = home.join(".meridian").join("bin").join(DAEMON_FILE);
        stop_running_daemon_before_stage(&daemon_bin).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(())
    }
}

/// Ensure the daemon is running, starting it if it isn't. Called on the
/// backend-up-to-date path of [`ensure_backend_installed`] (where staging and
/// [`register_service`] are skipped) so a skipped *staging* never leaves a
/// *stopped* daemon — in particular after [`stop_daemon_for_migration`] stops it
/// for the encryption migration, but also if it simply crashed.
///
/// **Windows-only work**: prefer the scheduled task, else spawn the binary
/// directly (the same fallback [`register_service`] uses on policy-locked
/// machines). On macOS launchd's `KeepAlive` restarts the daemon itself, so this
/// is a no-op there.
async fn ensure_daemon_running(home: &Path) {
    #[cfg(target_os = "windows")]
    {
        let daemon_bin = home.join(".meridian").join("bin").join(DAEMON_FILE);
        if !matching_daemon_pids(&daemon_bin).await.is_empty() {
            return; // already up — nothing to do
        }
        let queued_via_task = tokio::process::Command::new("schtasks")
            .args(["/Run", "/TN", WINDOWS_TASK_NAME])
            .no_window()
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if queued_via_task {
            // `/Run` success only confirms the task was QUEUED — it's
            // registered `/SC ONLOGON`, so a successful exit here doesn't
            // mean the daemon actually launched. Give it a moment, then
            // verify before trusting it; otherwise fall through to the
            // direct spawn below rather than leaving the daemon down.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if !matching_daemon_pids(&daemon_bin).await.is_empty() {
                tracing::info!(
                    task = WINDOWS_TASK_NAME,
                    "backend_install: restarted daemon via scheduled task"
                );
                return;
            }
            tracing::warn!(
                task = WINDOWS_TASK_NAME,
                "backend_install: scheduled task ran but the daemon isn't up yet, falling back to a direct spawn"
            );
        }
        match tokio::process::Command::new(&daemon_bin)
            .no_window()
            .spawn()
        {
            Ok(_) => {
                tracing::info!(bin = %daemon_bin.display(), "backend_install: restarted daemon directly")
            }
            Err(e) => {
                tracing::warn!(error = %e, bin = %daemon_bin.display(), "backend_install: could not restart the daemon")
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // launchd `KeepAlive` restarts the daemon on its own — nothing to do.
        let _ = home;
    }
}

/// `Meridian.app/Contents/Resources/backend/` when it exists, else `None`.
fn bundled_backend_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?.join("backend");
    dir.is_dir().then_some(dir)
}

/// Stage the daemon binary, render + lint its plist, register its agent.
/// Returns `Err` if any step fails so the caller skips the success marker.
async fn install(backend: &Path, home: &Path) -> Result<(), String> {
    for dir in [".meridian/bin", ".meridian/logs"] {
        let p = home.join(dir);
        tokio::fs::create_dir_all(&p)
            .await
            .map_err(|e| format!("mkdir {}: {e}", p.display()))?;
    }

    // A staging attempt killed between `stage_binary`'s copy and its rename
    // (tray crash, force-quit, OOM) leaves an orphaned
    // `<DAEMON_FILE>.staging-<pid>` temp file behind — nothing else ever
    // scans for these, so sweep them here before staging runs again.
    cleanup_stale_staging_files(&home.join(".meridian/bin")).await;

    // Purge leftovers from a pre-cutover **bundle** install before staging the
    // in-process backend. macOS-only by construction: Windows has no install
    // history to migrate from, so there is nothing to boot out and no legacy
    // .env to recover. Skipped there deliberately, not by omission.
    #[cfg(target_os = "macos")]
    {
        // An install upgraded from an older topology may carry launchd agents
        // the new one replaced (the in-process capturer supersedes screenpipe
        // and does its own AX poke; the on-device MLX server is gone); left
        // running they race the tray, contend for :7823, or surface a redundant
        // "meridian-a11y-helper" Accessibility entry — so boot them out. All
        // best-effort + non-fatal.
        cleanup_legacy_screenpipe(home).await;
        cleanup_legacy_mlx_server(home).await;
        cleanup_legacy_a11y_helper(home).await;
        // Recover tracker credentials the bundle wrote to ~/.meridian/app/.env
        // so the DMG daemon (which reads the canonical ~/.meridian/.env)
        // doesn't lose them.
        migrate_legacy_bundle_env(home).await;
    }

    let daemon_bin = home.join(".meridian").join("bin").join(DAEMON_FILE);

    // Windows keeps a running exe's pages mapped, so overwriting it fails with
    // "the process cannot access the file" (os error 32) — and this isn't just
    // a first-install concern: `ensure_backend_installed` re-stages on every
    // version bump while the *previous* build's daemon is still alive from an
    // earlier login. Stop it first so the copy below can succeed; if it can't
    // be stopped, propagate the failure rather than staging over a locked file.
    #[cfg(target_os = "windows")]
    stop_running_daemon_before_stage(&daemon_bin).await?;

    stage_binary(&backend.join(DAEMON_FILE), &daemon_bin).await?;

    register_service(backend, home, &daemon_bin).await
}

/// Register the staged daemon to run at login and keep running.
///
/// macOS: a launchd **LaunchAgent** — per-user, no admin, `KeepAlive`.
#[cfg(target_os = "macos")]
async fn register_service(backend: &Path, home: &Path, daemon_bin: &Path) -> Result<(), String> {
    let launch_agents = home.join("Library/LaunchAgents");
    tokio::fs::create_dir_all(&launch_agents)
        .await
        .map_err(|e| format!("mkdir {}: {e}", launch_agents.display()))?;

    // Render the plist. The bundled template carries {{…}} placeholders the
    // npm installer substitutes too; here REPO_ROOT (the daemon's WorkingDirectory)
    // is ~/.meridian so dotenvy self-loads ~/.meridian/.env, and OTLP is left
    // empty for the daemon to self-load (a baked value would go stale).
    let home_str = home.to_string_lossy();
    render_plist(
        &backend.join("com.meridiona.daemon.plist"),
        &launch_agents.join("com.meridiona.daemon.plist"),
        &[
            ("{{HOME}}", home_str.as_ref()),
            ("{{REPO_ROOT}}", &home.join(".meridian").to_string_lossy()),
            ("{{DAEMON_BIN}}", &daemon_bin.to_string_lossy()),
            ("{{MERIDIAN_OTLP_ENDPOINT}}", ""),
        ],
    )
    .await?;

    for (label, plist) in AGENTS {
        register_agent(label, &launch_agents.join(plist)).await?;
    }
    Ok(())
}

/// Register the staged daemon to run at login and keep running.
///
/// Windows: a **per-user scheduled task**, via `schtasks.exe`, falling back to
/// a Startup-folder launcher when that's blocked.
///
/// # Why not a Windows Service
///
/// A Service is the instinctive analogue of launchd, and it is the wrong one.
/// Creating one requires administrator rights, but Meridian installs per-user
/// with no elevation (`installMode: "currentUser"` in
/// `tauri.windows.conf.json`) — deliberately, so installing never raises a UAC
/// prompt. A Service would force elevation on every user at install time.
///
/// What is being ported here is a launchd **LaunchAgent**, not a LaunchDaemon:
/// per-user, in the logged-in session, started at login. A per-user scheduled
/// task is the exact counterpart — same scope, same trigger, no admin — and
/// unlike a `Run` registry key it can also restart the daemon if it exits,
/// which is what `KeepAlive` buys on macOS.
///
/// `schtasks.exe` is used rather than the `windows-service` crate because this
/// needs the Task Scheduler, not the Service Control Manager, and shelling out
/// avoids taking a dependency for one idempotent call.
///
/// # Fallback: some machines refuse `schtasks /Create`
///
/// Managed/corporate machines can have Task Scheduler creation locked down by
/// policy for standard (non-elevated) users — `schtasks` then fails with
/// "Access is denied" even though the service itself is running fine. Rather
/// than leave the daemon permanently unstarted on those machines, a failed
/// `schtasks /Create` falls back to [`install_startup_folder_launcher`]: a
/// hidden VBScript dropped in the user's Startup folder, which Windows runs
/// at every login with no Task Scheduler involvement. It loses `KeepAlive`
/// (no restart if the daemon crashes — the same trade-off the retired `Run`
/// registry key would have had), but that is strictly better than "never
/// starts at all".
#[cfg(target_os = "windows")]
async fn register_service(_backend: &Path, home: &Path, daemon_bin: &Path) -> Result<(), String> {
    match register_scheduled_task(daemon_bin).await {
        Ok(()) => {
            // A prior run on this same machine may have left a Startup-folder
            // fallback behind (e.g. a policy that has since been relaxed) —
            // clear it so the daemon isn't launched twice at next login.
            let _ = remove_startup_folder_launcher().await;
            tracing::info!(
                task = WINDOWS_TASK_NAME,
                home = %home.display(),
                "backend_install: scheduled task registered"
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "backend_install: schtasks registration failed — falling back to a Startup-folder launcher"
            );
            install_startup_folder_launcher(daemon_bin).await?;
            // Nothing else will start the daemon until next login — start it now.
            if let Err(e) = tokio::process::Command::new(daemon_bin).no_window().spawn() {
                tracing::warn!(
                    error = %e,
                    bin = %daemon_bin.display(),
                    "backend_install: immediate daemon start failed — will still run at next login via Startup folder"
                );
            }
            tracing::info!(
                home = %home.display(),
                "backend_install: Startup-folder launcher installed"
            );
            Ok(())
        }
    }
}

/// How many times [`stop_running_daemon_before_stage`] re-checks that the old
/// daemon has exited, and the gap between checks — ~10s total. Longer than the
/// original 3s on purpose: a kill doesn't release the file instantly, so a
/// tight window gives up while the hold would still have cleared, needlessly
/// surfacing an "couldn't finish installing" notice for a blip.
#[cfg(target_os = "windows")]
const STOP_POLL_ATTEMPTS: u32 = 40;
#[cfg(target_os = "windows")]
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Poll `probe` until it reports the daemon gone (an empty pid list) or the
/// attempt budget is exhausted, returning the final list — empty means it
/// exited in time, non-empty means it's still holding the binary and the caller
/// must fail loudly rather than overwrite a locked file.
///
/// Split out from [`stop_running_daemon_before_stage`] and kept platform-neutral
/// (generic over the probe, no Windows syscalls of its own) so the poll logic is
/// unit-tested on macOS CI — the one platform CI runs — even though its only
/// production caller is Windows-only. `attempts`/`interval` are parameters so a
/// test can drive it with a fake probe and a zero interval, deterministically
/// and without real waiting.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
async fn wait_until_gone<F, Fut>(mut probe: F, attempts: u32, interval: Duration) -> Vec<u32>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<u32>>,
{
    let mut remaining = probe().await;
    let mut tries = 0;
    while !remaining.is_empty() && tries < attempts {
        tokio::time::sleep(interval).await;
        remaining = probe().await;
        tries += 1;
    }
    remaining
}

/// Stop the running instance(s) of **the staged daemon** so [`stage_binary`]
/// can overwrite `~/.meridian/bin/meridian.exe`. Targets only processes whose
/// executable is exactly `daemon_bin` — never an unrelated `meridian.exe` (an
/// interactive CLI run, say, or a different tool sharing that image name).
///
/// The daemon can be running via either path [`register_service`] may have
/// taken, so two stop mechanisms are used:
/// - `schtasks /End` stops it if the scheduled task is what launched it; it
///   targets our task by name, so it never touches anything else.
/// - `taskkill /F /PID` (per matched PID) also catches the Startup-folder
///   launcher fallback, where the process was spawned directly by `wscript`
///   and has no association with the scheduled task for `/End` to find. Killing
///   by PID — not by `/IM` image name — is what keeps unrelated `meridian.exe`
///   processes alive.
///
/// Killing a process doesn't release its file handle instantaneously, so this
/// polls (mirroring the launchd bootout wait in [`register_agent`]) rather than
/// proceeding immediately. If any matching process is *still* alive once the
/// polling window closes, this returns `Err`: staging over a file the daemon
/// still holds open would fail with os error 32, and the caller must surface
/// that as an install failure rather than silently continuing.
#[cfg(target_os = "windows")]
async fn stop_running_daemon_before_stage(daemon_bin: &Path) -> Result<(), String> {
    // Targets our task by name — safe regardless of what else is named meridian.exe.
    let _ = tokio::process::Command::new("schtasks")
        .args(["/End", "/TN", WINDOWS_TASK_NAME])
        .no_window()
        .output()
        .await;

    for pid in matching_daemon_pids(daemon_bin).await {
        let out = tokio::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .no_window()
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => {
                tracing::info!(pid, path = %daemon_bin.display(), "backend_install: stopped staged daemon process");
            }
            Ok(o) => {
                tracing::warn!(
                    pid,
                    path = %daemon_bin.display(),
                    status = %o.status,
                    stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                    "backend_install: taskkill did not stop staged daemon process"
                );
            }
            Err(e) => {
                tracing::warn!(pid, path = %daemon_bin.display(), error = %e, "backend_install: taskkill spawn failed");
            }
        }
    }

    // Poll until the staged daemon's own processes are gone (the file handle
    // lingers briefly after the kill), then require an empty set — a still-alive
    // holder means the overwrite below would fail, so fail loudly here instead.
    // The window is deliberately generous (~10s): a kill doesn't release the
    // file instantly, and on a loaded machine the daemon can take several
    // seconds to unwind. A tighter window gives up while the hold would still
    // have cleared on its own — which is exactly what surfaced the spurious
    // "couldn't finish installing" notice.
    //
    // `wait_until_gone` is deliberately silent (platform-neutral + testable), so
    // wrap the probe to emit a throttled breadcrumb — roughly every ~2s rather
    // than the original per-probe spam — keeping a live `meridian logs` tail
    // informed across the up-to-10s wait without flooding it.
    let mut probes_since_log: u32 = 0;
    let remaining = wait_until_gone(
        || {
            probes_since_log += 1;
            let emit = probes_since_log >= 8;
            if emit {
                probes_since_log = 0;
            }
            async move {
                let pids = matching_daemon_pids(daemon_bin).await;
                if emit && !pids.is_empty() {
                    tracing::debug!(
                        path = %daemon_bin.display(),
                        remaining = pids.len(),
                        "backend_install: still waiting for the previous daemon to exit"
                    );
                }
                pids
            }
        },
        STOP_POLL_ATTEMPTS,
        STOP_POLL_INTERVAL,
    )
    .await;
    if remaining.is_empty() {
        return Ok(());
    }
    tracing::error!(
        path = %daemon_bin.display(),
        pids = ?remaining,
        "backend_install: staged daemon still running after stop attempts"
    );
    Err(format!(
        "staged daemon {} still running after stop attempts (pids: {remaining:?}) - cannot overwrite a locked binary",
        daemon_bin.display()
    ))
}

/// PIDs of running processes whose executable is exactly the staged daemon
/// binary `daemon_bin` — never a process that merely shares the `meridian.exe`
/// image name. Uses a `Get-CimInstance Win32_Process` query for PID + full
/// `ExecutablePath`, which `tasklist` alone can't supply. Enumeration failure
/// is logged and yields an empty set (best-effort): the caller's post-poll
/// check still guards the actual overwrite.
#[cfg(target_os = "windows")]
async fn matching_daemon_pids(daemon_bin: &Path) -> Vec<u32> {
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='{DAEMON_FILE}'\" | ForEach-Object {{ \"$($_.ProcessId)`t$($_.ExecutablePath)\" }}"
    );
    let out = tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .no_window()
        .output()
        .await;
    let stdout = match out {
        Ok(o) => o.stdout,
        Err(e) => {
            tracing::warn!(error = %e, path = %daemon_bin.display(), "backend_install: could not enumerate daemon processes");
            return Vec::new();
        }
    };
    String::from_utf8_lossy(&stdout)
        .lines()
        .filter_map(|line| {
            let (pid, path) = line.split_once('\t')?;
            let pid: u32 = pid.trim().parse().ok()?;
            let path = path.trim();
            if path.is_empty() {
                return None;
            }
            same_windows_path(Path::new(path), daemon_bin).then_some(pid)
        })
        .collect()
}

/// Whether two paths resolve to the same file on Windows, tolerant of case,
/// separator, and verbatim-prefix differences. Prefers `canonicalize` (both
/// paths point at one physical file), falling back to a case-insensitive string
/// compare when the file can no longer be canonicalized.
#[cfg(target_os = "windows")]
fn same_windows_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => {
            let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
            norm(a) == norm(b)
        }
    }
}

/// `schtasks /Create` (+ a best-effort immediate `/Run`) for the daemon's
/// per-user login task. Split out from [`register_service`] so a failure here
/// is a recoverable `Err` the caller can fall back from, not a hard stop.
#[cfg(target_os = "windows")]
async fn register_scheduled_task(daemon_bin: &Path) -> Result<(), String> {
    // `/F` replaces an existing task, so re-registering on every update is
    // idempotent — the role `launchctl bootout` + `bootstrap` plays on macOS.
    //
    // `/RL LIMITED` keeps the task at the user's own privilege level. It must
    // not run elevated: the files it writes under ~/.meridian would then be
    // owned such that the un-elevated tray could not touch them.
    let out = tokio::process::Command::new("schtasks")
        .args([
            "/Create",
            "/F",
            "/TN",
            WINDOWS_TASK_NAME,
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/TR",
        ])
        .arg(format!("\"{}\"", daemon_bin.display()))
        .no_window()
        .output()
        .await
        .map_err(|e| format!("run schtasks: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "schtasks /Create {WINDOWS_TASK_NAME} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // Start it now so the first run does not wait for the next logon. Failure
    // is not fatal — the task is registered either way, so the daemon comes up
    // at login regardless.
    let _ = tokio::process::Command::new("schtasks")
        .args(["/Run", "/TN", WINDOWS_TASK_NAME])
        .no_window()
        .output()
        .await;
    Ok(())
}

/// File name of the Startup-folder fallback launcher. `.vbs` (not `.bat`/`.cmd`)
/// specifically so it runs via `wscript.exe` with no console window flash —
/// `WScript.Shell.Run`'s third argument (`0`) is the hidden window style, the
/// same effect `KeepAlive`-less silence needs.
#[cfg(target_os = "windows")]
const STARTUP_LAUNCHER_NAME: &str = "MeridianDaemon.vbs";

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup` — every file
/// directly inside runs once per login, no registration step required. This
/// is the one directory Windows treats as "run these at logon" without going
/// through Task Scheduler or the registry, which is exactly what a
/// policy-restricted machine still allows a standard user to write to.
#[cfg(target_os = "windows")]
fn startup_folder() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "%APPDATA% is not set".to_string())?;
    Ok(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
}

/// Write the hidden-launch VBScript to the Startup folder. Idempotent:
/// overwrites unconditionally, same as `schtasks /Create /F`.
#[cfg(target_os = "windows")]
async fn install_startup_folder_launcher(daemon_bin: &Path) -> Result<(), String> {
    let dir = startup_folder()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let script = dir.join(STARTUP_LAUNCHER_NAME);
    // VBScript has no backslash-escaping to worry about — only `"` needs
    // doubling to embed a literal quote, wrapping the path so a space
    // anywhere in it (a differently-named Windows profile, say) can't split
    // `Run`'s argument.
    let contents = format!(
        "CreateObject(\"WScript.Shell\").Run \"\"\"{}\"\"\", 0, False\r\n",
        daemon_bin.display()
    );
    tokio::fs::write(&script, contents)
        .await
        .map_err(|e| format!("write {}: {e}", script.display()))
}

/// Remove a previously-installed Startup-folder launcher. Best-effort: a
/// missing file is not an error, and any other failure is swallowed by the
/// caller (this only ever runs opportunistically after `schtasks` succeeds).
#[cfg(target_os = "windows")]
async fn remove_startup_folder_launcher() -> Result<(), String> {
    let path = startup_folder()?.join(STARTUP_LAUNCHER_NAME);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

/// Task Scheduler name for the daemon's per-user login task.
#[cfg(target_os = "windows")]
const WINDOWS_TASK_NAME: &str = "Meridian Daemon";

/// Purge a leftover **pre-cutover screenpipe install**. Before the in-process
/// cutover, capture ran as a separate `screenpipe` binary under a
/// `com.meridiona.screenpipe` launchd agent (staged by the old installer).
/// The in-process build doesn't use screenpipe, but an *update* over such an
/// install leaves that agent running — it respawns `screenpipe record`, which
/// requests Screen Recording (a duplicate prompt) and races the tray's in-process
/// capture. Boot it out, remove its plist + binary, and kill any live process.
/// Entirely best-effort and non-fatal — a launchctl hiccup must not abort install.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn cleanup_legacy_screenpipe(home: &Path) {
    let label = "com.meridiona.screenpipe";
    let target = format!("gui/{}/{label}", crate::sys::uid_str());
    if launchctl(&["print", &target]).await.is_ok_and(|s| s) {
        let _ = launchctl(&["bootout", &target]).await;
        tracing::info!(label, "backend_install: removed leftover screenpipe agent");
    }
    let _ =
        tokio::fs::remove_file(home.join("Library/LaunchAgents/com.meridiona.screenpipe.plist"))
            .await;
    let _ = tokio::fs::remove_file(home.join(".meridian/bin/screenpipe")).await;
    // Kill any still-running screenpipe the agent had spawned (best-effort).
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "screenpipe record"])
        .output()
        .await;
}

/// Purge a leftover **MLX launchd agent** from an older install. Earlier builds
/// registered a local MLX inference server as `com.meridiona.mlx-server` (via
/// `install-mlx-server-daemon.sh`) on port 7823. That whole subsystem has been
/// removed (generation runs through the user's CLI provider now), so on update we
/// boot out any surviving agent and remove its plist. Best-effort.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn cleanup_legacy_mlx_server(home: &Path) {
    let label = "com.meridiona.mlx-server";
    let target = format!("gui/{}/{label}", crate::sys::uid_str());
    if launchctl(&["print", &target]).await.is_ok_and(|s| s) {
        let _ = launchctl(&["bootout", &target]).await;
        tracing::info!(label, "backend_install: removed leftover MLX launchd agent");
    }
    let _ =
        tokio::fs::remove_file(home.join("Library/LaunchAgents/com.meridiona.mlx-server.plist"))
            .await;
}

/// Purge a leftover **a11y-helper** launchd agent + its stale Accessibility
/// grant. The helper existed to poke `AXManualAccessibility` on Electron apps
/// for the old *external* screenpipe process; the in-process capture engine's
/// `screenpipe-a11y` tree walker does that poke itself now, under the tray's
/// own Accessibility grant, so the helper is pure redundancy — and its
/// separate ad-hoc-signed binary shows up as a second, confusingly-named
/// "meridian-a11y-helper" entry in System Settings → Privacy & Security →
/// Accessibility. Boot out the agent, remove its plist + staged binary, kill
/// any live process, and best-effort clear its TCC grant (`tccutil reset`) so
/// the entry doesn't linger grayed-out after the binary is gone. Entirely
/// best-effort + non-fatal — a launchctl/tccutil hiccup must not abort install.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn cleanup_legacy_a11y_helper(home: &Path) {
    let label = "com.meridiona.a11y-helper";
    let target = format!("gui/{}/{label}", crate::sys::uid_str());
    if launchctl(&["print", &target]).await.is_ok_and(|s| s) {
        let _ = launchctl(&["bootout", &target]).await;
        tracing::info!(label, "backend_install: removed leftover a11y-helper agent");
    }
    let _ =
        tokio::fs::remove_file(home.join("Library/LaunchAgents/com.meridiona.a11y-helper.plist"))
            .await;
    let _ = tokio::fs::remove_file(home.join(".meridian/bin/meridian-a11y-helper")).await;
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "meridian-a11y-helper"])
        .output()
        .await;
    let _ = tokio::process::Command::new("tccutil")
        .args(["reset", "Accessibility", "com.meridiona.a11y-helper"])
        .output()
        .await;
}

/// Recover tracker credentials when migrating from a **bundle** install. The
/// npm/curl bundle writes its `.env` to `~/.meridian/app/.env` (its daemon's
/// WorkingDirectory); the DMG daemon's WorkingDirectory is `~/.meridian`, so it
/// reads the **canonical** `~/.meridian/.env`. Without a copy, a bundle→DMG
/// migrant's Jira/GitHub/Linear tokens would silently vanish and need re-entering
/// via the setup wizard. Copy the bundle file across **only when the canonical
/// one doesn't already exist** — never clobber creds the tray already wrote.
/// Best-effort + non-fatal.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn migrate_legacy_bundle_env(home: &Path) {
    let canonical = home.join(".meridian/.env");
    let bundle = home.join(".meridian/app/.env");
    if tokio::fs::metadata(&canonical).await.is_ok() {
        return; // canonical creds already present — leave them untouched
    }
    if tokio::fs::metadata(&bundle).await.is_err() {
        return; // no bundle .env to migrate (fresh install / source run)
    }
    match tokio::fs::copy(&bundle, &canonical).await {
        Ok(_) => tracing::info!(
            src = %bundle.display(),
            dest = %canonical.display(),
            "backend_install: migrated bundle .env to the canonical path"
        ),
        Err(e) => tracing::warn!(error = %e, "backend_install: could not migrate bundle .env"),
    }
}

/// How many times [`rename_with_retry`] attempts the swap, and the base gap
/// between tries (grown linearly). Short by design — the rename either succeeds
/// at once or is briefly blocked by a scanner/indexer that lets go within a
/// second or two; a genuinely locked file should surface quickly, not stall the
/// whole install.
const RENAME_ATTEMPTS: u32 = 5;
const RENAME_BASE_DELAY: Duration = Duration::from_millis(100);

/// `tokio::fs::rename` with the bounded transient-failure retry from the shared
/// [`meridian_core::retry::retry_transient`]. [`stage_binary`]'s rename swaps the
/// freshly staged binary over `~/.meridian/bin/meridian(.exe)` — the live
/// daemon's image path, exactly where a momentary Windows file hold lands — so
/// retrying turns a self-clearing blip into a silent success instead of an
/// install-failed notice. A persistent failure still returns the real error so a
/// genuinely locked file is not swept under the rug.
async fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    meridian_core::retry::retry_transient(RENAME_ATTEMPTS, RENAME_BASE_DELAY, || {
        tokio::fs::rename(from, to)
    })
    .await
}

/// Copy `src` → `dest` only when the bytes differ, then `chmod 0755`.
/// Skipping an identical copy keeps the code hash (and any TCC grant) stable.
///
/// Stages via a temp file in `dest`'s own directory + atomic rename, **not**
/// an in-place overwrite — `dest` (`~/.meridian/bin/meridian`) is often still
/// the executable image of a previous version of the daemon, still running
/// (`ensure_backend_installed` re-stages on every app update, before the
/// caller's `register_service` stops/relaunches it via launchd). An in-place
/// `tokio::fs::copy` truncates and rewrites that same inode; on macOS the
/// hardened-runtime code-signing monitor validates executable pages against
/// the on-disk signature as they're demand-paged in, so the live process gets
/// SIGKILL'd (`EXC_BAD_ACCESS`, `CODESIGNING`/"Invalid Page") the next time it
/// faults in a page that no longer matches — which can happen anywhere from
/// immediately to hours later, well after this function returns. `rename(2)`
/// instead swaps the directory entry without touching the old inode's
/// content, so an already-running process keeps executing its original,
/// still-valid pages undisturbed; only a fresh exec of `dest` picks up the
/// new binary. The temp file must be on the same filesystem as `dest` for the
/// rename to be atomic — using `dest`'s own directory guarantees that.
async fn stage_binary(src: &Path, dest: &Path) -> Result<(), String> {
    let same = match (tokio::fs::read(src).await, tokio::fs::read(dest).await) {
        (Ok(a), Ok(b)) => a == b,
        (Err(e), _) => return Err(format!("read {}: {e}", src.display())),
        _ => false,
    };
    if same {
        tracing::debug!(dest = %dest.display(), "backend_install: binary unchanged");
        return Ok(());
    }

    let tmp = dest.with_file_name(format!("{DAEMON_FILE}.staging-{}", std::process::id()));
    if let Err(e) = tokio::fs::copy(src, &tmp).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("copy {} → {}: {e}", src.display(), tmp.display()));
    }
    if let Err(e) = set_executable(&tmp).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    if let Err(e) = rename_with_retry(&tmp, dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!(
            "rename {} → {}: {e}",
            tmp.display(),
            dest.display()
        ));
    }
    tracing::info!(dest = %dest.display(), "backend_install: staged binary");
    Ok(())
}

/// Remove any `<DAEMON_FILE>.staging-<pid>` leftovers in `bin_dir` from a
/// staging attempt that [`stage_binary`] never finished (killed between its
/// copy and its rename) — see that function's doc comment for why the temp
/// file exists. Best-effort and non-fatal: a listing or removal failure is
/// only logged, since a lingering temp file is harmless clutter, not a
/// correctness problem, and must never block `install()`.
async fn cleanup_stale_staging_files(bin_dir: &Path) {
    let prefix = format!("{DAEMON_FILE}.staging-");
    let mut entries = match tokio::fs::read_dir(bin_dir).await {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!(
                dir = %bin_dir.display(), error = %e,
                "backend_install: could not list bin dir for stale staging sweep"
            );
            return;
        }
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(error = %e, "backend_install: stale staging sweep readdir error");
                return;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                tracing::info!(path = %path.display(), "backend_install: removed stale staging temp file")
            }
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "backend_install: could not remove stale staging temp file")
            }
        }
    }
}

/// `chmod u+rwx,go+rx` (0755) on a freshly staged binary.
#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let perm = std::fs::Permissions::from_mode(0o755);
    tokio::fs::set_permissions(path, perm)
        .await
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Replace each `{{KEY}}` in `template` with its value. Pure — the testable core
/// of [`render_plist`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn apply_subs(template: &str, subs: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (key, val) in subs {
        text = text.replace(key, val);
    }
    text
}

/// Read a bundled plist template, replace each `{{KEY}}`, write it to
/// `~/Library/LaunchAgents/`, and `plutil -lint` the result.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn render_plist(template: &Path, dest: &Path, subs: &[(&str, &str)]) -> Result<(), String> {
    let raw = tokio::fs::read_to_string(template)
        .await
        .map_err(|e| format!("read {}: {e}", template.display()))?;
    let text = apply_subs(&raw, subs);
    tokio::fs::write(dest, text)
        .await
        .map_err(|e| format!("write {}: {e}", dest.display()))?;

    let out = tokio::process::Command::new("plutil")
        .arg("-lint")
        .arg(dest)
        .output()
        .await
        .map_err(|e| format!("run plutil: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "plutil -lint {} failed: {}",
            dest.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    tracing::debug!(plist = %dest.display(), "backend_install: rendered plist");
    Ok(())
}

/// Bootout (and wait for the domain entry to clear), then bootstrap + enable +
/// kickstart the agent under `gui/<uid>` — the same dance the shell installers
/// run, ported. `bootout` is async, so we poll `launchctl print` until the label
/// clears (≤15 s) before bootstrapping, else `bootstrap` can fail with EIO.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn register_agent(label: &str, plist: &Path) -> Result<(), String> {
    let gui = format!("gui/{}", crate::sys::uid_str());
    let target = format!("{gui}/{label}");

    let _ = launchctl(&["bootout", &target]).await; // ok if not loaded
    for _ in 0..15 {
        if !launchctl(&["print", &target]).await.is_ok_and(|s| s) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let _ = launchctl(&["enable", &target]).await;
    let plist_s = plist.to_string_lossy();
    if !launchctl(&["bootstrap", &gui, &plist_s])
        .await
        .unwrap_or(false)
    {
        return Err(format!("launchctl bootstrap {label} failed"));
    }
    let _ = launchctl(&["enable", &target]).await;
    let _ = launchctl(&["kickstart", "-k", &target]).await;
    tracing::info!(label, "backend_install: launchd agent registered");
    Ok(())
}

/// Run `launchctl <args>`, returning `Ok(true)` on exit 0. Errors only on spawn
/// failure; a non-zero exit is `Ok(false)` so callers decide what's fatal.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
async fn launchctl(args: &[&str]) -> Result<bool, String> {
    tokio::process::Command::new("launchctl")
        .args(args)
        .output()
        .await
        .map(|o| o.status.success())
        .map_err(|e| format!("run launchctl: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On non-Windows, `stop_daemon_for_migration` is an inert no-op that returns
    /// `Ok` — the encryption migration tolerates an open handle there, so there
    /// is nothing to stop and the tray's setup hook proceeds straight to
    /// `encrypt_in_place`. Gated off Windows so the test never kills a real
    /// daemon; CI runs on macOS, so it executes there.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn stop_daemon_for_migration_is_a_noop_off_windows() {
        assert!(
            stop_daemon_for_migration().await.is_ok(),
            "off Windows this must be an inert Ok, not attempt to stop anything"
        );
    }

    /// `migrate_legacy_bundle_env` must: copy the bundle `.env` to the canonical
    /// path when only the bundle exists; **never clobber** an existing canonical
    /// file; and no-op when there's nothing to migrate. The clobber guard is the
    /// load-bearing case — overwriting would wipe creds the tray already wrote.
    #[tokio::test]
    async fn migrate_bundle_env_copies_but_never_clobbers() {
        let base = std::env::temp_dir().join(format!("meridian-bundle-env-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&base).await; // clean slate if a prior run died
        let mk = |name: &str| base.join(name);

        // Case 1: bundle present, canonical absent → copies content across.
        let h1 = mk("copy");
        tokio::fs::create_dir_all(h1.join(".meridian/app"))
            .await
            .unwrap();
        tokio::fs::write(h1.join(".meridian/app/.env"), "JIRA_API_TOKEN=abc")
            .await
            .unwrap();
        migrate_legacy_bundle_env(&h1).await;
        assert_eq!(
            tokio::fs::read_to_string(h1.join(".meridian/.env"))
                .await
                .unwrap(),
            "JIRA_API_TOKEN=abc"
        );

        // Case 2: both present → canonical is left untouched (no clobber).
        let h2 = mk("noclobber");
        tokio::fs::create_dir_all(h2.join(".meridian/app"))
            .await
            .unwrap();
        tokio::fs::write(h2.join(".meridian/app/.env"), "FROM=bundle")
            .await
            .unwrap();
        tokio::fs::write(h2.join(".meridian/.env"), "FROM=canonical")
            .await
            .unwrap();
        migrate_legacy_bundle_env(&h2).await;
        assert_eq!(
            tokio::fs::read_to_string(h2.join(".meridian/.env"))
                .await
                .unwrap(),
            "FROM=canonical"
        );

        // Case 3: nothing to migrate → canonical stays absent, no error.
        let h3 = mk("noop");
        tokio::fs::create_dir_all(h3.join(".meridian"))
            .await
            .unwrap();
        migrate_legacy_bundle_env(&h3).await;
        assert!(tokio::fs::metadata(h3.join(".meridian/.env"))
            .await
            .is_err());

        let _ = tokio::fs::remove_dir_all(&base).await;
    }

    /// `cleanup_legacy_a11y_helper` must remove the stale plist + staged binary
    /// it leaves behind from a pre-cutover install — the concrete, automatable
    /// half of the PR #431 review's "confirm the update path boots out the old
    /// agent" ask (the `launchctl`/`tccutil` system calls are real syscalls with
    /// no fake-able state to assert on in a unit test, but the file-removal side
    /// — the part that actually stops the stale entry from lingering once no
    /// process owns it — is fully verifiable here).
    #[tokio::test]
    async fn cleanup_legacy_a11y_helper_removes_stale_files() {
        let home = std::env::temp_dir().join(format!(
            "meridian-a11y-helper-cleanup-{}-{}",
            std::process::id(),
            "test"
        ));
        let _ = tokio::fs::remove_dir_all(&home).await;
        tokio::fs::create_dir_all(home.join("Library/LaunchAgents"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(home.join(".meridian/bin"))
            .await
            .unwrap();

        let plist = home.join("Library/LaunchAgents/com.meridiona.a11y-helper.plist");
        let bin = home.join(".meridian/bin/meridian-a11y-helper");
        tokio::fs::write(&plist, "<plist/>").await.unwrap();
        tokio::fs::write(&bin, "fake binary").await.unwrap();

        cleanup_legacy_a11y_helper(&home).await;

        assert!(
            tokio::fs::metadata(&plist).await.is_err(),
            "leftover a11y-helper plist should have been removed"
        );
        assert!(
            tokio::fs::metadata(&bin).await.is_err(),
            "leftover a11y-helper binary should have been removed"
        );

        let _ = tokio::fs::remove_dir_all(&home).await;
    }

    /// `stage_binary` must never truncate `dest` in place — a reader that
    /// opened `dest` before staging began (standing in for a still-running
    /// daemon with `dest` mapped as its executable image) must still see the
    /// complete, unmodified OLD content afterward, not a mix of old/new bytes
    /// or a truncated file. This is the property the codesigning SIGKILL loop
    /// (see the doc comment on `stage_binary`) depends on: a live process is
    /// only ever affected by future execs of the (now-renamed) path, never by
    /// its own already-open file/mapping.
    #[tokio::test]
    async fn stage_binary_does_not_disturb_a_reader_of_the_old_file() {
        let dir = std::env::temp_dir().join(format!(
            "meridian-stage-binary-reader-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let src = dir.join("src-daemon");
        let dest = dir.join(DAEMON_FILE);
        tokio::fs::write(&src, b"new binary bytes, much longer than the old one")
            .await
            .unwrap();
        tokio::fs::write(&dest, b"old binary bytes").await.unwrap();

        // Hold `dest`'s original inode open across the stage, mirroring a
        // running process's already-mapped executable.
        let mut reader = std::fs::File::open(&dest).unwrap();

        stage_binary(&src, &dest).await.unwrap();

        use std::io::Read;
        let mut held = Vec::new();
        reader.read_to_end(&mut held).unwrap();
        assert_eq!(
            held, b"old binary bytes",
            "a handle opened before staging must still see the intact old content"
        );

        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            b"new binary bytes, much longer than the old one",
            "a fresh read of the path after staging must see the new content"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// `cleanup_stale_staging_files` must remove only leftover
    /// `<DAEMON_FILE>.staging-<pid>` temp files — the orphan a killed
    /// `stage_binary` can leave behind — and must never touch the real staged
    /// binary or anything else sharing the directory.
    #[tokio::test]
    async fn cleanup_stale_staging_files_removes_only_staging_leftovers() {
        let dir = std::env::temp_dir().join(format!(
            "meridian-stage-binary-sweep-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let stale = dir.join(format!("{DAEMON_FILE}.staging-12345"));
        let daemon = dir.join(DAEMON_FILE);
        let unrelated = dir.join("some-other-file");
        tokio::fs::write(&stale, b"orphaned temp copy")
            .await
            .unwrap();
        tokio::fs::write(&daemon, b"the real staged binary")
            .await
            .unwrap();
        tokio::fs::write(&unrelated, b"not ours").await.unwrap();

        cleanup_stale_staging_files(&dir).await;

        assert!(
            tokio::fs::metadata(&stale).await.is_err(),
            "stale staging temp file should have been removed"
        );
        assert_eq!(
            tokio::fs::read(&daemon).await.unwrap(),
            b"the real staged binary",
            "the real staged binary must be untouched"
        );
        assert_eq!(
            tokio::fs::read(&unrelated).await.unwrap(),
            b"not ours",
            "unrelated files must be untouched"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn apply_subs_replaces_every_occurrence() {
        let out = apply_subs(
            "a={{HOME}} b={{HOME}} c={{X}}",
            &[("{{HOME}}", "/Users/me"), ("{{X}}", "v")],
        );
        assert_eq!(out, "a=/Users/me b=/Users/me c=v");
    }

    /// Remove `<!-- … -->` blocks so the placeholder check sees only the live
    /// plist body — the templates document their placeholder names (incl. the
    /// deprecated `{{MERIDIAN_OO_AUTH}}`) inside an XML comment, which is not a
    /// value the daemon ever reads.
    fn strip_xml_comments(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(start) = rest.find("<!--") {
            out.push_str(&rest[..start]);
            match rest[start..].find("-->") {
                Some(end) => rest = &rest[start + end + 3..],
                None => return out, // unterminated comment — drop the tail
            }
        }
        out.push_str(rest);
        out
    }

    /// The bundled daemon plist template must have NO `{{…}}` left **in the
    /// live body** after the exact sub set `install()` applies — a new body
    /// placeholder added upstream without a matching sub would otherwise ship
    /// a broken plist. Reads the real committed template so the two can't
    /// drift apart silently.
    #[test]
    fn bundled_templates_fully_substituted() {
        let scripts = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts");

        let daemon = std::fs::read_to_string(format!("{scripts}/com.meridiona.daemon.plist"))
            .expect("read daemon plist template");
        let rendered = strip_xml_comments(&apply_subs(
            &daemon,
            &[
                ("{{HOME}}", "/Users/me"),
                ("{{REPO_ROOT}}", "/Users/me/.meridian"),
                ("{{DAEMON_BIN}}", "/Users/me/.meridian/bin/meridian"),
                ("{{MERIDIAN_OTLP_ENDPOINT}}", ""),
            ],
        ));
        assert!(
            !rendered.contains("{{"),
            "daemon plist body still has an unsubstituted placeholder: {rendered}"
        );
    }

    // ---------------------------------------------------------------------
    // Windows install-lock hardening: `rename_with_retry` (the tray-side thin
    // wrapper over the shared `meridian_core::retry::retry_transient`) and the
    // daemon-exit poll `wait_until_gone`. The retry primitive itself is
    // unit-tested in `meridian-core::retry`; these cover the wrapper's wiring
    // and the poll loop.
    //
    // The failures they guard against — os error 32 when overwriting a
    // still-open `meridian.exe`, a daemon that needs a beat to release its
    // binary — only occur on Windows, but the *logic* is deliberately
    // platform-neutral so it compiles and these tests actually run on the
    // macOS-only CI. They use zero-delay backoff and call-counting fakes so
    // they are deterministic, fast, and free of any real timing dependence.
    // ---------------------------------------------------------------------

    /// `rename_with_retry` moves the file and clears the source on the happy
    /// path — the same contract as a bare `rename`, so wiring it into
    /// `stage_binary` changes nothing for the common case.
    #[tokio::test]
    async fn rename_with_retry_moves_the_file_on_success() {
        let dir = std::env::temp_dir().join(format!("meridian-rename-ok-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let from = dir.join("from");
        let to = dir.join("to");
        tokio::fs::write(&from, b"payload").await.unwrap();

        rename_with_retry(&from, &to).await.unwrap();

        assert!(
            tokio::fs::metadata(&from).await.is_err(),
            "source is gone after a successful rename"
        );
        assert_eq!(tokio::fs::read(&to).await.unwrap(), b"payload");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// A rename whose source never exists fails on every attempt: it must
    /// surface the real `NotFound` error (proving the loop terminates and does
    /// not mask the cause) and must not conjure a destination file.
    #[tokio::test]
    async fn rename_with_retry_surfaces_a_persistent_failure() {
        let dir = std::env::temp_dir().join(format!("meridian-rename-fail-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let missing = dir.join("does-not-exist");
        let to = dir.join("dest");

        let err = rename_with_retry(&missing, &to).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(
            tokio::fs::metadata(&to).await.is_err(),
            "a failed rename must not have created the destination"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// `wait_until_gone` reports the daemon gone on the very first probe when
    /// it has already exited — no waiting, no wasted install latency.
    #[tokio::test]
    async fn wait_until_gone_returns_on_first_probe_when_already_gone() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let probes = AtomicU32::new(0);
        let remaining = wait_until_gone(
            || {
                probes.fetch_add(1, Ordering::SeqCst);
                async { Vec::<u32>::new() }
            },
            40,
            Duration::ZERO,
        )
        .await;
        assert!(remaining.is_empty());
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "an already-exited daemon is detected on the first probe, with no waiting"
        );
    }

    /// `wait_until_gone` keeps polling across live probes and succeeds the
    /// instant the daemon disappears mid-window — the case the widened 10s
    /// budget exists to catch (a daemon that takes a few seconds to unwind).
    #[tokio::test]
    async fn wait_until_gone_keeps_polling_until_the_daemon_exits() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let probes = AtomicU32::new(0);
        let remaining = wait_until_gone(
            || {
                let n = probes.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 3 {
                        vec![4321]
                    } else {
                        Vec::new()
                    }
                }
            },
            40,
            Duration::ZERO,
        )
        .await;
        assert!(
            remaining.is_empty(),
            "polling continues across live probes until the daemon is gone"
        );
        assert_eq!(probes.load(Ordering::SeqCst), 4);
    }

    /// A daemon that never exits within the budget must be *returned*, not
    /// hidden — that non-empty list is what makes `stop_running_daemon_before_stage`
    /// fail the install loudly rather than overwrite a locked binary.
    #[tokio::test]
    async fn wait_until_gone_reports_survivors_when_the_window_expires() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let probes = AtomicU32::new(0);
        let remaining = wait_until_gone(
            || {
                probes.fetch_add(1, Ordering::SeqCst);
                async { vec![12516u32] }
            },
            3,
            Duration::ZERO,
        )
        .await;
        assert_eq!(
            remaining,
            vec![12516],
            "a daemon that never exits is surfaced so the caller fails loudly"
        );
        // one initial probe + `attempts` (3) re-probes
        assert_eq!(probes.load(Ordering::SeqCst), 4);
    }

    /// Degenerate floor: `attempts == 0` still does the one initial probe and
    /// returns its result — it must never skip probing entirely (which would
    /// let a locked binary through) nor loop.
    #[tokio::test]
    async fn wait_until_gone_probes_once_when_attempts_is_zero() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let probes = AtomicU32::new(0);
        let remaining = wait_until_gone(
            || {
                probes.fetch_add(1, Ordering::SeqCst);
                async { vec![7u32] }
            },
            0,
            Duration::ZERO,
        )
        .await;
        assert_eq!(
            remaining,
            vec![7],
            "the initial probe's result is returned as-is"
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "exactly one probe, never zero"
        );
    }
}
