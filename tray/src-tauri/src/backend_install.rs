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
//! **Windows has no equivalent of that plist key**, and getting it wrong is
//! silent: `schtasks /Create` has no "Start in" field and the Startup-folder
//! VBScript defaults to system32, so a daemon launched by either sees no `.env`
//! — and therefore no `MERIDIAN_DB_KEY`, which is fatal once meridian.db has
//! been encrypted in place. Every launch path here consequently sets the
//! working directory to `~/.meridian` explicitly (the VBScript's
//! `CurrentDirectory`, and `current_dir` on both direct spawns). The daemon
//! *also* loads that file by absolute path (`src/main.rs`), so this is defence
//! in depth and the fix for anyone whose task was registered by an older build.
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

/// The daemon's launchd label and plist file name. Named separately from
/// [`AGENTS`] because [`stop_daemon_for_migration`] and [`ensure_daemon_running`]
/// address this one agent directly rather than iterating the table.
#[cfg(target_os = "macos")]
pub(crate) const DAEMON_LABEL: &str = "com.meridiona.daemon";
#[cfg(target_os = "macos")]
const DAEMON_PLIST: &str = "com.meridiona.daemon.plist";

/// launchd agents this stages, paired with their bundled plist template.
#[cfg(target_os = "macos")]
const AGENTS: &[(&str, &str)] = &[(DAEMON_LABEL, DAEMON_PLIST)];

/// The daemon executable's file name inside `Resources/backend/` and at its
/// staged destination. Windows needs the `.exe` suffix to be runnable at all;
/// `tauri.windows.conf.json`'s resource map bundles it under that name.
#[cfg(target_os = "windows")]
pub(crate) const DAEMON_FILE: &str = "meridian.exe";
#[cfg(not(target_os = "windows"))]
pub(crate) const DAEMON_FILE: &str = "meridian";

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
    let home = match meridian_core::paths::home_dir() {
        Some(h) => h,
        None => {
            tracing::warn!(
                "backend_install: home directory could not be resolved — cannot stage backend"
            );
            return;
        }
    };
    let backend = match bundled_backend_dir(app) {
        Some(d) => d,
        None => {
            // Nothing to STAGE on a dev/source run - but the daemon may still
            // need STARTING, and this is the only place that does it. Quit now
            // stops the daemon (`crate::daemon_lifecycle`), and a `bootout`
            // cannot be undone by `KeepAlive` or by `RunAtLoad` at the next
            // login: the job is no longer loaded for either to act on. Bailing
            // here without this call is what would turn one dev quit into a
            // permanently dead daemon.
            //
            // Safe on every path: the macOS arm no-ops when no plist is
            // installed (a fresh clone) and when the daemon is already up.
            tracing::debug!(
                "backend_install: no bundled backend (dev/source run) — skipping staging"
            );
            crate::daemon_lifecycle::restore_unless_paused(&home).await;
            return;
        }
    };

    let daemon_src = backend.join(DAEMON_FILE);
    let bundled_hash = match sha256_hex_of(&daemon_src) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, src = %daemon_src.display(), "backend_install: cannot hash bundled daemon");
            crate::daemon_lifecycle::restore_unless_paused(&home).await;
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
        crate::daemon_lifecycle::restore_unless_paused(&home).await;
        return;
    }

    // `ensure_backend_installed` runs off the setup hook, after `app.manage(db_pool)`
    // (see `lib.rs`), so the pool is already there to raise/clear against —
    // `None` only when the DB itself couldn't be opened, in which case there's
    // nowhere to write a notice anyway.
    let pool = app
        .try_state::<crate::db_pool::DbPool>()
        .and_then(|s| s.get());

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
                    deep_link: Some(meridian_core::notifications::deep_links::LOGS),
                },
            )
            .await
            {
                tracing::warn!(error = %notice_err, "backend-install-failure notice raise failed");
            }
        }
        // A failed *staging* must not also mean a stopped *daemon*. The
        // previously staged binary and its plist are still on disk, so the
        // last-known-good daemon can usually still be started - and it has to
        // be, because quit boots the agent out and nothing else will bring it
        // back before the next launch (which may fail here again).
        crate::daemon_lifecycle::restore_unless_paused(&home).await;
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
/// migration can swap the file with no other process writing to it.
///
/// **Required on BOTH platforms, for opposite reasons.** On Windows, renaming a
/// file another process holds open fails with os error 32, so `encrypt_in_place`
/// rolled back every launch while the daemon (autostarted at login) held the DB.
/// On macOS the rename *succeeds* despite the open handle — which is the
/// dangerous case, not the safe one:
///
/// - the daemon keeps writing through its handle on the old (plaintext) inode
///   while new connections open the swapped-in encrypted file, and
/// - SQLite addresses the WAL and shm sidecars **by path**, not by inode, so a
///   plaintext writer and an encrypted writer end up sharing one journal —
///   with `finalize_encryption_swap` having deleted `-wal`/`-shm` out from under
///   the live daemon, breaking the locking protocol on top.
///
/// That is what corrupted `meridian.db` on macOS installs from v1.80.0 onward:
/// `SQLITE_IOERR_SHORT_READ` (522) as the file shortens beneath the open handle,
/// then `file is not a database` (26), then `database disk image is malformed`
/// (11). Central telemetry over the 30 days after the rollout put code 11 at six
/// machines on macOS and **zero** on Windows — Windows' rename failure had been
/// the only thing protecting it. The original "a no-op on other platforms, where
/// the migration tolerates the open handle" had that safety exactly backwards.
///
/// A plain SIGTERM is not enough on macOS: the agent is registered with launchd
/// `KeepAlive`, so it returns within seconds — straight back into the swap
/// window. `bootout` removes it from the domain entirely, so nothing resurrects
/// it until the migration is done.
///
/// Called from the tray's setup hook BEFORE `encrypt_in_place`, and only when a
/// migration will actually attempt (a key is set and the DB is still plaintext).
/// The daemon is brought back up afterward by [`ensure_backend_installed`] —
/// either its normal staging path (on an update) or [`ensure_daemon_running`]
/// (on the up-to-date path), which re-bootstraps the booted-out agent.
pub(crate) async fn stop_daemon_for_migration(db_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = db_path;
        let home = meridian_core::paths::home_dir()
            .ok_or_else(|| "home directory could not be resolved".to_string())?;
        let daemon_bin = home.join(".meridian").join("bin").join(DAEMON_FILE);
        stop_running_daemon_before_stage(&daemon_bin).await
    }
    #[cfg(target_os = "macos")]
    {
        bootout_agent_and_wait(DAEMON_LABEL).await?;
        // A bootout only removes the LAUNCHD-managed daemon. Anything else
        // holding meridian.db - a dev `cargo run`, a binary spawned directly, an
        // orphan from an overlapping install (all three have been seen on one
        // machine here) - survives it untouched, and swapping the file under any
        // of them reproduces exactly the corruption this function exists to
        // prevent. So confirm the file is actually unheld rather than inferring
        // it from the bootout succeeding.
        wait_for_db_unheld(db_path).await
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = db_path;
        Ok(())
    }
}

/// Poll until nothing but this process holds `db_path` open (≤10 s), so the
/// caller can swap the file knowing there is no second writer.
///
/// Deliberately keyed on **the database file**, not on a daemon binary path:
/// the hazard is "some process has this file open", and a check that enumerates
/// known daemon locations misses every writer that isn't at one of them.
///
/// A spawn failure of `lsof` is treated as **still held**, not as clear. Being
/// unable to prove the file is free is not evidence that it is, and the cost of
/// each answer is asymmetric: declining leaves the DB plaintext and retries next
/// launch (annoying, and exactly what Windows has always done), while wrongly
/// proceeding corrupts the user's data irrecoverably.
#[cfg(target_os = "macos")]
async fn wait_for_db_unheld(db_path: &Path) -> Result<(), String> {
    for _ in 0..10 {
        let out = tokio::process::Command::new("lsof")
            .arg("-t")
            .arg(db_path)
            .output()
            .await
            .map_err(|e| format!("run lsof: {e}"))?;
        // Exit status is non-zero when NO process holds the file, which is the
        // success case here - read stdout rather than the status.
        let holders = parse_lsof_holders(&String::from_utf8_lossy(&out.stdout), std::process::id());
        if holders.is_empty() {
            return Ok(());
        }
        tracing::debug!(holders = ?holders, "backend_install: waiting for db writers to exit");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err("meridian.db is still held open 10s after stopping the daemon".to_string())
}

/// The pids in `lsof -t` output, excluding our own — i.e. the processes that
/// would still be writing if the swap went ahead now.
///
/// Split from the spawn in [`wait_for_db_unheld`] so the rule can be tested:
/// that function is `cfg(target_os = "macos")` and shells out, and what counts
/// as "held" decides whether the user's database gets corrupted.
///
/// Unparseable lines are skipped rather than treated as holders — `lsof` writes
/// its diagnostics to stderr, so anything non-numeric on stdout is noise. The
/// asymmetry that matters is handled by the caller, not here: an `lsof` that
/// cannot be RUN is an error, never an empty holder list.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_lsof_holders(stdout: &str, self_pid: u32) -> Vec<u32> {
    stdout
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter(|pid| *pid != self_pid)
        .collect()
}

/// Whether `encrypt_in_place` may swap `meridian.db`, given what
/// [`stop_daemon_for_migration`] returned — or `None` when no stop was attempted
/// because nothing was going to migrate anyway (already encrypted, no key, or a
/// debug build).
///
/// The whole point is `Some(Err(_)) => false`. v1.80.0 logged the failed stop and
/// migrated regardless, and on macOS that is the data-destroying path: the rename
/// succeeds under the live daemon and the two writers then share one WAL. Staying
/// plaintext for another launch is the cheap failure, and is what Windows has
/// always done — its rename fails with os error 32 rather than proceeding.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn may_swap_database(stop_outcome: Option<&Result<(), String>>) -> bool {
    matches!(stop_outcome, None | Some(Ok(())))
}

/// `launchctl bootout gui/<uid>/<label>`, then poll `launchctl print` until the
/// entry actually clears (≤15 s). Extracted from [`register_agent`], which
/// performs the same wait for the same reason — `bootout` is asynchronous, and
/// acting before the label clears is what makes a follow-up `bootstrap` fail
/// with EIO.
///
/// [`stop_daemon_for_migration`] needs the wait for a sharper reason: returning
/// while the daemon is still shutting down would hand the DB swap the very
/// concurrent writer the bootout exists to remove. Returns `Err` if the entry is
/// still present after the timeout, so the caller can decline to migrate rather
/// than swap the file under a process that never went away.
///
/// `cfg_attr(allow(dead_code))` rather than `cfg(target_os = "macos")`, matching
/// [`register_agent`] and [`launchctl`]: those are COMPILED on every platform
/// and merely allowed to be dead, so a `cfg`-gated callee vanishes underneath
/// them and breaks the Windows build even though nothing there can call it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) async fn bootout_agent_and_wait(label: &str) -> Result<(), String> {
    let target = agent_target(label);
    let _ = launchctl(&["bootout", &target]).await; // ok if not loaded
    for _ in 0..15 {
        if !launchctl(&["print", &target]).await.is_ok_and(|s| s) {
            tracing::info!(label, "backend_install: daemon booted out for db migration");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err(format!(
        "launchd agent {label} still loaded 15s after bootout"
    ))
}

/// Ensure the daemon is running, starting it if it isn't. Called on the
/// backend-up-to-date path of [`ensure_backend_installed`] (where staging and
/// [`register_service`] are skipped) so a skipped *staging* never leaves a
/// *stopped* daemon — in particular after [`stop_daemon_for_migration`] stops it
/// for the encryption migration, but also if it simply crashed.
///
/// On **Windows**: prefer the scheduled task, else spawn the binary directly
/// (the same fallback [`register_service`] uses on policy-locked machines).
///
/// On **macOS**: re-register the agent. `KeepAlive` normally makes this
/// unnecessary — it restarts a daemon that merely crashed — but it cannot
/// resurrect one that [`stop_daemon_for_migration`] `bootout`ed, which is the
/// whole point of using `bootout` there. Leaving this a no-op on macOS would
/// therefore trade the corruption bug for a permanently dead daemon on every
/// machine that ran the encryption migration.
///
/// It is **not** idempotent, which an earlier version of this comment claimed:
/// [`register_agent`] finishes with `launchctl kickstart -k`, and `-k` is
/// precisely what kills a running instance. Both platforms therefore return
/// early when the daemon is already up, or an ordinary launch would SIGTERM a
/// healthy daemon mid-write.
pub(crate) async fn ensure_daemon_running(home: &Path) {
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
        // `current_dir`: the daemon resolves its `.env` relative to the working
        // directory, and a bare spawn would inherit the tray's instead.
        match tokio::process::Command::new(&daemon_bin)
            .current_dir(home.join(".meridian"))
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
    #[cfg(target_os = "macos")]
    {
        let plist = home.join("Library/LaunchAgents").join(DAEMON_PLIST);
        if !plist.is_file() {
            // Nothing staged yet (first launch); `install` renders the plist and
            // registers the agent itself, so there is nothing to restore here.
            return;
        }
        // Already up — leave it alone. `register_agent` ends in
        // `launchctl kickstart -k`, which KILLS a running instance, so calling
        // it unconditionally bounced a healthy daemon on every ordinary tray
        // launch. That is a fourth path to the macOS `meridian.db` corruption
        // (see `crate::poll::watchdog` for the measured one): a SIGTERM
        // mid-WAL-write, landing at app start when the daemon is busiest.
        // Windows has always returned early here for exactly this reason; macOS
        // is only now matching it.
        //
        // This deliberately does NOT break the case the function exists for.
        // `stop_daemon_for_migration` `bootout`s the agent, and a booted-out
        // label is not loaded at all: `launchctl print` exits non-zero, so
        // `process_alive()` reports `Some(false)` and registration still runs.
        // Verified across all three states — booted-out (exit 113), loaded but
        // stopped (exit 0, no `pid` line), and running (exit 0, `pid` present);
        // only the last one returns `Some(true)`.
        if crate::commands::daemon_control::process_alive().await == Some(true) {
            tracing::debug!(
                label = DAEMON_LABEL,
                "backend_install: daemon already running — not re-registering"
            );
            return;
        }
        if let Err(e) = register_agent(DAEMON_LABEL, &plist).await {
            tracing::warn!(
                error = %e,
                label = DAEMON_LABEL,
                "backend_install: could not re-register the daemon agent"
            );
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = home;
    }
}

/// `Meridian.app/Contents/Resources/backend/` when it exists, else `None`.
///
/// Gated on `!cfg!(debug_assertions)` as well as the directory check — a
/// `tauri dev` / `cargo run` build's `resource_dir()` resolves under
/// `target/debug/`, and if ANYTHING ever leaves a `backend/` subdirectory
/// there (a stray artifact from an earlier debug-profile `tauri build`, a
/// manual test, whatever), `dir.is_dir()` alone can't tell that apart from a
/// real packaged install. That false positive is not cosmetic:
/// [`ensure_backend_installed`] would then call [`install`], which on macOS
/// runs [`stop_daemon_for_migration`] against `meridian.db` — booting out a
/// launchd agent that was never registered for this run (a no-op) and then
/// polling `lsof` for up to 10s, which always times out because the actual
/// holder is the sibling `cargo run` daemon `bootout` can't touch, not the
/// launchd job it targets. That raises `tray.backend_install_failed`
/// ("meridian.db is still held open 10s after stopping the daemon") on every
/// dev-tray launch, with a timestamp that tracks each restart — measured
/// verbatim on 2026-08-31, traced to exactly one stray directory left over
/// from an Aug 16 debug build. `debug_assertions` is a compile-time constant
/// for the running binary's own profile, so this can never be fooled by
/// what's left lying around on disk the way the directory check alone was.
/// Matches the same convention already used for the encryption-migration
/// gate in `lib.rs`'s setup hook.
fn bundled_backend_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        return None;
    }
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

    // HOLD THE STAGING WINDOW ACROSS stop → copy → register.
    //
    // The tray supervises the daemon as well as installing it, and until this
    // guard existed the two halves raced: the stop below killed the daemon, the
    // watchdog saw a silent endpoint ~10 s later and started it again from the
    // very path being staged, and the stop's own poll then reported that new
    // process as an un-killable holder of the binary. The full account is on
    // [`crate::daemon_lifecycle::begin_staging`].
    //
    // Held past `stage_binary` and through `register_service` deliberately:
    // registering is what legitimately brings the daemon back, and a watchdog
    // start racing it would spawn a second one against the same DB.
    //
    // Not `cfg`-gated even though the lock hazard is Windows-only - the
    // installer restarting the daemon while the watchdog independently decides
    // to start it is a double-spawn on any platform, and a flag that exists on
    // one OS only is the kind of asymmetry that rots.
    let _staging = crate::daemon_lifecycle::begin_staging();

    // Windows keeps a running exe's pages mapped, so overwriting it fails with
    // "the process cannot access the file" (os error 32) — and this isn't just
    // a first-install concern: `ensure_backend_installed` re-stages on every
    // version bump while the *previous* build's daemon is still alive from an
    // earlier login. Stop it first so the copy below can succeed; if it can't
    // be stopped, propagate the failure rather than staging over a locked file.
    #[cfg(target_os = "windows")]
    stop_running_daemon_before_stage(&daemon_bin).await?;

    // macOS needs the SAME stop, for the opposite reason, and did not have it.
    //
    // Windows FAILS the copy while the daemon holds the exe (os error 32), so
    // the omission there is loud. macOS lets the rename SUCCEED - the running
    // daemon simply keeps executing the unlinked inode and keeps writing
    // meridian.db - so the omission is silent, and what follows is the
    // double-writer window every `code: 11` report in this fleet has come
    // through: `register_service` below bootstraps a NEW daemon against the
    // same database while the old one is still live.
    //
    // Measured on a staging install 2026-08-25: `staged binary` at 08:20:37.586
    // and the old daemon's `SIGTERM received` at 08:20:37.595 - the swap landed
    // NINE MILLISECONDS before anything asked it to stop, and the stop that did
    // arrive came from `register_agent`'s bootout, after the fact.
    //
    // Reuses the migration path's stop rather than a second implementation:
    // bootout the launchd job, then prove via `lsof` that NOTHING holds the
    // database - a dev `cargo run`, a directly-spawned binary and an orphan
    // from an overlapping install have all been seen here, and a bootout
    // removes none of them.
    //
    // Failing here declines the STAGING, exactly as Windows does: the update
    // does not apply, the existing daemon keeps running, and the next launch
    // retries. That is the right asymmetry - a deferred update costs a version,
    // proceeding costs the user's database.
    #[cfg(target_os = "macos")]
    stop_daemon_for_migration(std::path::Path::new(&crate::install::meridian_db_path())).await?;

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
            let meridian_home = home.join(".meridian");
            install_startup_folder_launcher(&meridian_home, daemon_bin).await?;
            // Nothing else will start the daemon until next login — start it now.
            // `current_dir` for the same reason the launcher sets it: the daemon
            // resolves its `.env` relative to the working directory, and this
            // spawn inherits the tray's otherwise.
            if let Err(e) = tokio::process::Command::new(daemon_bin)
                .current_dir(&meridian_home)
                .no_window()
                .spawn()
            {
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
pub(crate) async fn stop_running_daemon_before_stage(daemon_bin: &Path) -> Result<(), String> {
    // Targets our task by name — safe regardless of what else is named meridian.exe.
    let _ = tokio::process::Command::new("schtasks")
        .args(["/End", "/TN", WINDOWS_TASK_NAME])
        .no_window()
        .output()
        .await;

    kill_pids(&matching_daemon_pids(daemon_bin).await, daemon_bin).await;

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
                if !pids.is_empty() {
                    if emit {
                        tracing::debug!(
                            path = %daemon_bin.display(),
                            remaining = pids.len(),
                            "backend_install: still waiting for the previous daemon to exit"
                        );
                    }
                    // RE-KILL, don't just observe. The `taskkill` above ran once,
                    // against the pid set as it stood before this poll began, so
                    // anything that appeared afterwards was watched to the end of
                    // the budget and then reported as an un-killable holder of the
                    // binary — which is exactly how a daemon respawned two seconds
                    // into the wait produced "still running after stop attempts".
                    //
                    // The staging guard in `install` is what stops the tray's own
                    // watchdog doing that; this is the backstop for every spawner
                    // it does not own — a second tray instance, the Startup-folder
                    // launcher firing on a fast re-login, someone running the
                    // daemon by hand. Re-killing a pid that is already dying is
                    // harmless (`taskkill` just reports it gone), and the loop
                    // still terminates on the same attempt budget.
                    //
                    // Kills the pids THIS probe just enumerated rather than
                    // re-querying: `matching_daemon_pids` spawns a whole
                    // `powershell -Command Get-CimInstance`, which costs far
                    // more than the `taskkill`s it feeds. Re-querying here
                    // would pay that twice on every one of the up-to-40
                    // attempts, stretching the stop on exactly the slow,
                    // loaded machines where this path is reached at all.
                    kill_pids(&pids, daemon_bin).await;
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

/// `taskkill /F` each of `pids`. `daemon_bin` is only for the log lines.
///
/// Takes an already-enumerated pid list rather than looking one up itself:
/// both callers have just run [`matching_daemon_pids`], and that spawns an
/// entire `powershell -Command Get-CimInstance`, which dwarfs the `taskkill`s
/// it feeds. Re-querying inside here would pay it twice per poll attempt, up
/// to 40 times, on precisely the loaded machines slow enough to reach the
/// later attempts at all.
///
/// Killing by PID — not by `/IM` image name — is what keeps an unrelated
/// `meridian.exe` (an interactive CLI run, a different tool sharing the image
/// name) alive. Best-effort per pid: a failure is logged and the next one is
/// still attempted, because the caller's post-kill poll is what actually
/// decides whether the stop succeeded.
///
/// Called both before the wait and on every probe inside it — see the re-kill
/// note in [`stop_running_daemon_before_stage`].
#[cfg(target_os = "windows")]
async fn kill_pids(pids: &[u32], daemon_bin: &Path) {
    // Imported in-body rather than at module scope: this fn is
    // `cfg(target_os = "windows")` and the trait has no other user here, so a
    // top-level `use` would be an unused import on macOS - which is a hard
    // error under `-D warnings`, from a file the macOS build otherwise
    // compiles fine.
    use tracing::Instrument;
    for &pid in pids {
        // One span PER PID rather than one for the loop: this runs on the
        // update path, where the interesting question is never "did the stop
        // work" but "which of these processes refused to die" - and a failure
        // here is what surfaces later as "cannot overwrite a locked binary",
        // several steps removed from its cause.
        async {
            let out = tokio::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .no_window()
                .output()
                .await;
            let s = tracing::Span::current();
            match out {
                Ok(o) if o.status.success() => {
                    tracing::info!(pid, path = %daemon_bin.display(), "backend_install: stopped staged daemon process");
                }
                Ok(o) => {
                    // Span status, not only the log line: the ship leg is
                    // error-only, so a WARN with no ERROR span is the shape
                    // that reaches central OO stripped of the context needed
                    // to act on it.
                    s.record("otel.status_code", "ERROR");
                    tracing::warn!(
                        pid,
                        path = %daemon_bin.display(),
                        status = %o.status,
                        stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                        "backend_install: taskkill did not stop staged daemon process"
                    );
                }
                Err(e) => {
                    s.record("otel.status_code", "ERROR");
                    tracing::warn!(pid, path = %daemon_bin.display(), error = %e, "backend_install: taskkill spawn failed");
                }
            }
        }
        .instrument(tracing::debug_span!(
            "backend_install.taskkill",
            pid,
            otel.status_code = tracing::field::Empty,
        ))
        .await;
    }
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
///
/// `meridian_home` is the daemon's working directory (`~/.meridian`), the
/// Windows counterpart of the macOS plist's `WorkingDirectory`.
#[cfg(target_os = "windows")]
async fn install_startup_folder_launcher(
    meridian_home: &Path,
    daemon_bin: &Path,
) -> Result<(), String> {
    let dir = startup_folder()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let script = dir.join(STARTUP_LAUNCHER_NAME);
    // `CurrentDirectory` is set for parity with the macOS plist's
    // `WorkingDirectory`: the daemon's `dotenvy` walk starts at the CWD, and a
    // launcher that leaves it at system32 gives the daemon no `.env` — and so
    // no MERIDIAN_DB_KEY — which is fatal once meridian.db is encrypted. The
    // daemon also resolves ~/.meridian/.env by absolute path (`src/main.rs`),
    // so this is defence in depth rather than the sole fix.
    //
    // VBScript has no backslash-escaping to worry about — only `"` needs
    // doubling to embed a literal quote, wrapping the path so a space
    // anywhere in it (a differently-named Windows profile, say) can't split
    // `Run`'s argument.
    let contents = format!(
        "Set sh = CreateObject(\"WScript.Shell\")\r\n\
         sh.CurrentDirectory = \"{}\"\r\n\
         sh.Run \"\"\"{}\"\"\", 0, False\r\n",
        meridian_home.display(),
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
    let target = agent_target(label);

    // Best-effort: if the entry hasn't cleared we still try to bootstrap, which
    // is the long-standing behaviour here (this used to `break` out of the same
    // wait loop and carry on regardless). Only the migration path treats a stuck
    // entry as fatal — see `bootout_agent_and_wait`'s doc comment.
    //
    // WARN, not DEBUG. This used to be `debug!` on the reasoning that WARN+ is
    // what egresses to central telemetry and a slow-clearing entry is common
    // enough on an install/update that warning would add fleet noise. The noise
    // half of that does not hold, and the cost of it was severe.
    //
    // What this branch means: the installer is about to bootstrap the daemon
    // while the previous one may still be running — the double-writer
    // precondition behind the recurring `database disk image is malformed`
    // (`code: 11`), which as of 2026-08-17 had damaged the databases of 8 hosts,
    // every one of them macOS. Nothing else records that moment, so the one
    // condition worth catching was the one condition that could not leave the
    // machine.
    //
    // On volume: this runs only during staging, and staging only runs on a
    // version change. Measured over the same week, the whole fleet logged 38
    // version transitions across 28 hosts — so the ceiling here is ~38 records
    // a week, against the ~800k `code: 11` errors a single damaged host emits
    // in the same window. There is no noise problem to trade against.
    if let Err(e) = bootout_agent_and_wait(label).await {
        tracing::warn!(error = %e, label, "backend_install: proceeding to bootstrap anyway");
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
    // `kickstart` WITHOUT `-k`. The agent was just booted out and bootstrapped,
    // so there is nothing running for `-k` to kill on the happy path — it only
    // ever bit when `bootout` had NOT cleared (the "proceeding to bootstrap
    // anyway" branch above), which is exactly when something may still be
    // running. It also closed a TOCTOU: callers check "is the daemon alive?"
    // first, and the daemon can start between that check and this line, so a
    // `-k` here could kill an instance the caller had just decided not to
    // touch. Dropping the flag makes the worst case a no-op instead of a
    // SIGTERM mid-WAL-write. See [`crate::poll::watchdog`] for what that cost.
    let _ = launchctl(&["kickstart", &target]).await;
    tracing::info!(label, "backend_install: launchd agent registered");
    Ok(())
}

/// `gui/<uid>/<label>` — the launchd domain target for a per-user agent, as
/// every `launchctl` subcommand here addresses it. Split out so the shape can be
/// unit-tested without shelling out to `launchctl` (which would act on a real
/// agent on the developer's or CI machine).
///
/// Compiled on every platform for the same reason as [`bootout_agent_and_wait`]
/// — [`register_agent`] calls it and is itself always compiled.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn agent_target(label: &str) -> String {
    format!("gui/{}/{label}", crate::sys::uid_str())
}

/// Run `launchctl <args>`, returning `Ok(true)` on exit 0. Errors only on spawn
/// failure; a non-zero exit is `Ok(false)` so callers decide what's fatal.
///
/// `pub(crate)` for [`crate::autostart::macos`], which owns the TRAY's own
/// LaunchAgent and needs the same `bootout` when retiring the plugin-era login
/// item. Shared rather than re-rolled so there is one spelling of "shell out to
/// launchctl" in the tray.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) async fn launchctl(args: &[&str]) -> Result<bool, String> {
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

    /// The daemon must be stopped BEFORE its binary is staged, on BOTH
    /// platforms.
    ///
    /// Windows has always done this and fails loudly if it cannot - the copy
    /// returns os error 32 while the daemon holds the exe. macOS had no such
    /// call at all, and there the rename SUCCEEDS: the running daemon keeps
    /// executing the unlinked inode and keeps writing `meridian.db`, after
    /// which `register_service` bootstraps a second daemon against the same
    /// file. That is the double-writer window every `code: 11` report in this
    /// fleet has come through, and it is silent by construction.
    ///
    /// Measured on a staging install 2026-08-25: `staged binary` at
    /// 08:20:37.586, the old daemon's `SIGTERM received` at 08:20:37.595 - the
    /// swap landed NINE MILLISECONDS before anything asked it to stop.
    ///
    /// Source-scanned because the ordering is the whole behaviour and neither
    /// `launchctl` nor a real daemon exists in a unit test - the same idiom as
    /// `every_early_return_still_restores_the_daemon` above and
    /// `single_instance_check_precedes_setup_db_and_bind_follows_it` in
    /// `main.rs`.
    #[test]
    fn the_daemon_is_stopped_before_its_binary_is_staged() {
        const SRC: &str = include_str!("backend_install.rs");
        // Truncate at THIS test module first: the file scans itself, and the
        // needles below appear in this test's own body. Without this the scan
        // matches its own source and can never fail.
        let prod = SRC
            .split_once("\n#[cfg(test)]")
            .map_or(SRC, |(before, _)| before);

        let body = prod
            .split_once("async fn install(backend: &Path, home: &Path)")
            .expect("install() must exist")
            .1;
        let end = ["\npub ", "\npub(crate) ", "\nasync fn", "\nfn "]
            .iter()
            .filter_map(|m| body.find(m))
            .min()
            .unwrap_or(body.len());
        let body = &body[..end];

        let stage = body
            .find("stage_binary(")
            .expect("install() must stage the binary");

        for (label, needle) in [
            ("windows", "stop_running_daemon_before_stage("),
            ("macos", "stop_daemon_for_migration("),
        ] {
            let stop = body.find(needle).unwrap_or_else(|| {
                panic!(
                    "install() has no {label} stop before staging - on macOS that means the \
                     binary is swapped under a live daemon that keeps writing meridian.db, \
                     and register_service then starts a SECOND one against the same file"
                )
            });
            assert!(
                stop < stage,
                "the {label} stop must come BEFORE stage_binary, not after it - staging \
                 first is what makes the window silent. stop at {stop}, stage at {stage}"
            );
        }
    }

    /// The branch that bootstraps a launchd agent whose `bootout` never cleared
    /// must report at **WARN**, because WARN+ is the only severity that leaves
    /// the machine.
    ///
    /// That branch is the one moment the installer knowingly proceeds while the
    /// previous daemon may still be alive - the double-writer precondition
    /// behind the recurring `database disk image is malformed`. It was logged at
    /// `debug!` to keep central telemetry quiet, which meant the condition could
    /// never be observed on any of the affected machines. See the call site for
    /// the measurement that retired that concern.
    ///
    /// Scanned from source rather than executed: reaching the branch needs a
    /// stuck `launchctl` domain entry, which cannot be staged in a unit test.
    /// Same tactic as the sibling scans in this module.
    #[test]
    fn a_stuck_bootout_is_reported_at_warn() {
        const SRC: &str = include_str!("backend_install.rs");
        // Truncate at the test module FIRST. This file scans itself, so the
        // needle below also appears in this very function - without the cut the
        // assertion would match its own source and could never fail.
        let prod = SRC.split_once("\n#[cfg(test)]").map_or(SRC, |(a, _)| a);
        let body = prod
            .split_once("async fn register_agent")
            .expect("register_agent must exist")
            .1;
        const NEEDLE: &str = "proceeding to bootstrap anyway";
        let line = body
            .lines()
            .find(|l| l.contains(NEEDLE) && !l.trim_start().starts_with("//"))
            .unwrap_or_else(|| panic!("the stuck-bootout branch should still log \"{NEEDLE}\""));
        assert!(
            line.contains("tracing::warn!"),
            "the stuck-bootout branch must log at WARN, not `{}`. Only WARN+ \
             egresses to central OpenObserve, so any lower level makes the \
             double-writer precondition invisible on exactly the machines \
             whose databases are being corrupted.",
            line.trim()
        );
    }

    /// Every path out of [`ensure_backend_installed`] that could leave a daemon
    /// down must call [`crate::daemon_lifecycle::restore_unless_paused`] first.
    ///
    /// This is the other half of "quit stops the daemon"
    /// ([`crate::daemon_lifecycle`]) and it is the half with no natural failure
    /// signal. A `bootout` is not reversed by `KeepAlive`, and not by
    /// `RunAtLoad` at the next login either — the job is no longer loaded for
    /// either to act on — so dropping this call does not break a test or raise
    /// a notice. It just means the daemon never comes back, and the symptom
    /// (no sessions, ever) surfaces hours later somewhere else entirely.
    ///
    /// Scanned from source rather than executed: the function needs a live
    /// `AppHandle` and would shell out to `launchctl` against the developer's
    /// own daemon. Same tactic as the `cfg` audit further down this module.
    #[test]
    fn every_early_return_still_restores_the_daemon() {
        const SRC: &str = include_str!("backend_install.rs");
        let body = SRC
            .split_once("pub async fn ensure_backend_installed")
            .expect("ensure_backend_installed must exist")
            .1;
        // Bound the scan to the function: whichever item marker at column 0
        // comes FIRST ends it. Splitting on `"\nasync fn"` alone was wrong -
        // the next item is `pub(crate) async fn stop_daemon_for_migration`,
        // which that pattern does not match, so the scan ran on to
        // `wait_for_db_unheld` and swept a second function into the range.
        // Harmless only by luck: `stop_daemon_for_migration` returns a
        // `Result` and so cannot hold a bare `return;`. The next `pub(crate)
        // fn` added between them would have failed this test with a message
        // naming `ensure_backend_installed`, which is the worst kind of
        // red - a true failure pointing at the wrong function.
        let end = ["\npub ", "\npub(crate) ", "\nasync fn", "\nfn "]
            .iter()
            .filter_map(|m| body.find(m))
            .min()
            .unwrap_or(body.len());
        let body = &body[..end];

        // Pins the bound itself. Without it the two assertions below silently
        // widen as the module grows, and a guard that scans the wrong lines
        // reports on a function nobody edited.
        assert!(
            !body.contains("stop_daemon_for_migration"),
            "the scan must stop at the end of ensure_backend_installed - it \
             has run on into the following item"
        );

        // Every `return;` must be preceded by a restore. The one exception is
        // the path where `$HOME` itself could not be resolved - there is no
        // `home` to hand the restore, and nothing it could do.
        let lines: Vec<&str> = body.lines().collect();
        let mut unguarded: Vec<usize> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.trim() != "return;" {
                continue;
            }
            let window = lines[i.saturating_sub(6)..i].join("\n");
            let guarded =
                window.contains("restore_unless_paused") || window.contains("cannot stage backend");
            if !guarded {
                unguarded.push(i);
            }
        }
        assert!(
            unguarded.is_empty(),
            "every bail-out from ensure_backend_installed must call \
             daemon_lifecycle::restore_unless_paused first, or a quit leaves \
             the daemon permanently down - a `bootout` survives both KeepAlive \
             and the next login. Unguarded `return;` at body lines {unguarded:?}"
        );

        // And the dev/source path specifically, since that is the one that
        // returned without ever reaching the restore before this change.
        let dev_arm = body
            .split_once("no bundled backend")
            .expect("the dev/source bail-out must still be identifiable")
            .1;
        let dev_arm = dev_arm.split_once("};").map(|(a, _)| a).unwrap_or(dev_arm);
        assert!(
            dev_arm.contains("restore_unless_paused"),
            "the dev/source bail-out must restore the daemon before returning"
        );

        // ...and it must go through the pause gate, never call the raw start
        // directly. A restore that skips the gate can start a daemon the user
        // just paused, leaving a running daemon under a Paused label that the
        // watchdog will not correct - because the pause flag is exactly what
        // tells it to stand down.
        assert!(
            !body.contains("ensure_daemon_running(&home)"),
            "restores must route through daemon_lifecycle::restore_unless_paused, \
             not call ensure_daemon_running directly"
        );
    }

    /// The launchd target `stop_daemon_for_migration` boots out must address the
    /// daemon in the CURRENT user's GUI domain. A malformed target makes
    /// `launchctl bootout` a silent no-op (it exits non-zero and the code
    /// deliberately ignores that), which would put the migration straight back
    /// into swapping `meridian.db` under a live daemon — the macOS-only
    /// corruption this whole path exists to prevent.
    ///
    /// The bootout itself is deliberately NOT exercised here: it shells out to
    /// `launchctl` and would stop the real daemon on a developer's machine and
    /// on CI. This pins the part that can be checked without that side effect.
    /// The previous test at this location asserted the opposite invariant — that
    /// the function was an inert no-op off Windows — which is precisely the
    /// behaviour that corrupted six macOS installs.
    #[cfg(target_os = "macos")]
    #[test]
    fn migration_bootout_targets_the_daemon_in_the_user_gui_domain() {
        let target = agent_target(DAEMON_LABEL);
        assert_eq!(
            target,
            format!("gui/{}/com.meridiona.daemon", crate::sys::uid_str()),
            "bootout target must be gui/<uid>/<daemon label>"
        );
        assert!(
            !target.contains("gui//"),
            "empty uid would silently make bootout a no-op: {target}"
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

    /// The swap in `encrypt_in_place` renames `meridian.db` out from under
    /// anything holding it, and on macOS `rename(2)` SUCCEEDS while the daemon
    /// has it open — which is how six installs got `database disk image is
    /// malformed`. `wait_for_db_unheld` is the check that stops that, so what
    /// counts as "held" is a data-integrity decision, not a parsing detail.
    ///
    /// Keyed on the DB file rather than on known daemon paths on purpose: a dev
    /// build, a directly-spawned binary and an orphan from an overlapping
    /// install have all been seen on one machine, and none of them is launchd's.
    #[test]
    fn any_process_holding_the_db_counts_as_a_writer() {
        const SELF: u32 = 4242;

        // Nothing holds it — the only state in which the swap may go ahead.
        assert!(parse_lsof_holders("", SELF).is_empty());
        assert!(parse_lsof_holders("\n  \n", SELF).is_empty());

        // The daemon holds it. One survivor is enough to block the swap.
        assert_eq!(parse_lsof_holders("50184\n", SELF), vec![50184]);
        assert_eq!(
            parse_lsof_holders("50184\n63667\n", SELF),
            vec![50184, 63667],
            "every holder counts, not just the first"
        );

        // Our own pid is not a reason to refuse to migrate.
        assert!(parse_lsof_holders("4242\n", SELF).is_empty());
        assert_eq!(parse_lsof_holders("4242\n50184\n", SELF), vec![50184]);

        // Junk must not silently read as "clear" — that would green-light the
        // swap on exactly the machines where lsof behaved unexpectedly.
        assert!(parse_lsof_holders("lsof: WARNING: can't stat()\n", SELF).is_empty());
        assert_eq!(parse_lsof_holders("garbage\n50184\n", SELF), vec![50184]);
    }

    /// The gate that decides whether `encrypt_in_place` may run at all.
    ///
    /// Warning and swapping anyway is what shipped in v1.80.0, and on macOS that
    /// is precisely the data-destroying path: the rename succeeds under a live
    /// writer. Leaving the database plaintext for one more launch is the cheap
    /// failure — and is exactly what Windows has always done, where the rename
    /// fails with os error 32 instead.
    ///
    /// This is the single assertion standing between a failed daemon stop and
    /// corrupting the user's database, so it is pinned in both directions.
    #[test]
    fn a_failed_daemon_stop_must_block_the_database_swap() {
        assert!(
            !may_swap_database(Some(&Err("still loaded 15s after bootout".to_string()))),
            "the daemon could not be stopped - swapping now corrupts the database"
        );

        // The two states in which proceeding is correct, so the guard cannot be
        // satisfied by simply always refusing.
        assert!(
            may_swap_database(Some(&Ok(()))),
            "daemon stopped cleanly - the migration must still be able to run"
        );
        assert!(
            may_swap_database(None),
            "no stop was needed (already encrypted, no key, or a debug build) - \
             nothing is going to swap anything"
        );
    }

    /// Two annotations in this file look interchangeable and are not:
    ///
    /// - `#[cfg_attr(not(target_os = "macos"), allow(dead_code))]` keeps the item
    ///   **compiled on every platform**, merely permitting it to be unused;
    /// - `#[cfg(target_os = "macos")]` **removes** it everywhere else.
    ///
    /// So a `cfg`-gated item called from a `cfg_attr` one compiles cleanly on
    /// macOS and fails on Windows with `error[E0425]: cannot find function …`.
    /// That shipped once (PR #671) and broke a staging release, because every
    /// local check a macOS developer runs — `cargo clippy`, `cargo test`, the
    /// pre-push hook — is structurally blind to it, and `cargo check --target
    /// x86_64-pc-windows-msvc` can't stand in from macOS (it dies earlier in
    /// `aws-lc-sys`, which needs `windows.h`).
    ///
    /// A unit test cannot catch this either — tests compile for one platform at
    /// a time. So this reads the source instead, in the same spirit as
    /// `ui/__tests__/no-native-dialogs.test.ts`. It runs on macOS, where the
    /// mistake is made.
    #[test]
    fn no_macos_only_item_is_reachable_from_always_compiled_code() {
        const SRC: &str = include_str!("backend_install.rs");
        const MACOS_ONLY: &str = "#[cfg(target_os = \"macos\")]";
        const ALWAYS: &str = "#[cfg_attr(not(target_os = \"macos\"), allow(dead_code))]";

        /// The `fn` / `const` / `static` name declared at or just after `from`.
        fn item_name_after(lines: &[&str], from: usize) -> Option<(String, usize)> {
            for (offset, line) in lines.iter().enumerate().skip(from).take(6) {
                let l = line.trim_start().trim_start_matches("pub(crate) ");
                let l = l.trim_start_matches("pub ").trim_start_matches("async ");
                for kw in ["fn ", "const ", "static "] {
                    if let Some(rest) = l.strip_prefix(kw) {
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            return Some((name, offset));
                        }
                    }
                }
            }
            None
        }

        /// Line index one past the item's closing brace, by brace matching.
        fn item_end(lines: &[&str], start: usize) -> usize {
            let (mut depth, mut opened) = (0i32, false);
            for (i, line) in lines.iter().enumerate().skip(start) {
                depth += line.matches('{').count() as i32;
                depth -= line.matches('}').count() as i32;
                if line.contains('{') {
                    opened = true;
                }
                if opened && depth <= 0 {
                    return i + 1;
                }
            }
            lines.len()
        }

        let lines: Vec<&str> = SRC.lines().collect();
        let mut macos_only: Vec<String> = Vec::new();
        let mut always_compiled: Vec<(String, usize, usize)> = Vec::new();

        // Everything from `mod tests` onward is the test module, whose items are
        // `cfg`-gated on purpose and never called from non-test code.
        //
        // Excluded by POSITION, not by a name prefix. The previous guard was
        // `!name.starts_with("test_")`, and no test in this file carries that
        // prefix — they read as sentences
        // (`migration_bootout_targets_the_daemon_in_the_user_gui_domain`) — so
        // the condition was always true, the exclusion never fired, and test
        // items were counted toward the `macos_only` floor below. That left the
        // floor unable to do its one job: prove the scanner still recognises the
        // PRODUCTION items it exists to guard.
        let tests_start = lines
            .iter()
            .position(|l| l.trim_start().starts_with("mod tests"))
            .unwrap_or(lines.len());

        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            if i >= tests_start {
                break;
            }
            if t == MACOS_ONLY {
                if let Some((name, _at)) = item_name_after(&lines, i + 1) {
                    macos_only.push(name);
                }
            } else if t == ALWAYS {
                if let Some((name, at)) = item_name_after(&lines, i + 1) {
                    always_compiled.push((name, at, item_end(&lines, at)));
                }
            }
        }

        assert!(
            macos_only.len() >= 3 && always_compiled.len() >= 3,
            "the scanner stopped recognising this file's shape \
             ({} macos-only, {} always-compiled) - fix the scanner, \
             do not delete the test",
            macos_only.len(),
            always_compiled.len()
        );

        for (fname, start, end) in &always_compiled {
            // Drop `#[cfg(target_os = "macos")] { … }` blocks inside the body:
            // code in those DOES compile only on macOS, so it may legitimately
            // reference a macOS-only item.
            let mut body = String::new();
            let mut skip_until = 0usize;
            for i in *start..*end {
                if i < skip_until {
                    continue;
                }
                if lines[i].trim() == MACOS_ONLY {
                    skip_until = item_end(&lines, i + 1);
                    continue;
                }
                body.push_str(lines[i]);
                body.push('\n');
            }

            for item in &macos_only {
                // Word-boundary match so `DAEMON_LABEL` doesn't hit
                // `DAEMON_LABEL_X`, and a mention in a doc comment doesn't count.
                let referenced = body
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .any(|l| {
                        l.match_indices(item.as_str()).any(|(idx, _)| {
                            let before = l[..idx].chars().next_back();
                            let after = l[idx + item.len()..].chars().next();
                            let boundary = |c: Option<char>| {
                                !matches!(c, Some(ch) if ch.is_alphanumeric() || ch == '_')
                            };
                            boundary(before) && boundary(after)
                        })
                    });
                assert!(
                    !referenced,
                    "`{fname}` is compiled on every platform \
                     (cfg_attr allow(dead_code)) but references `{item}`, which is \
                     `#[cfg(target_os = \"macos\")]` and does not exist off macOS. \
                     This builds on macOS and fails on Windows with E0425. \
                     Give `{item}` the same cfg_attr form, or move the call into a \
                     `#[cfg(target_os = \"macos\")]` block."
                );
            }
        }
    }
}
