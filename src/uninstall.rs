//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `meridian uninstall` — the inverse of the install harness (Gap-2 Bucket 1).
//!
//! Dragging `Meridian.app` to the Trash leaves the launchd agents the tray
//! registered (`com.meridiona.daemon`, `com.meridiona.a11y-helper` — see
//! `tray/src-tauri/src/backend_install.rs`) running forever, because no app code
//! runs once the bundle is gone. This subcommand is the clean teardown: it lives
//! in the daemon binary, which is staged at `~/.meridian/bin/meridian` and so
//! **survives the app being trashed** — the user (or the tray's in-app uninstall
//! wizard, which shells out to this binary) can run it to stop + remove every
//! Meridian launchd agent and the staged binaries.
//!
//! By default it removes only the **installed artifacts** (launchd agents, staged
//! binaries, the install marker) and **keeps user data**. Three independent,
//! additive flags widen the scope:
//! - `--remove-data` — `~/.meridian`'s user data (db, credentials, settings,
//!   logs, telemetry spool, icon cache, onboarded/autostart markers), plus
//!   everything Meridian leaves OUTSIDE that directory and outside the app
//!   bundle, which would otherwise survive every uninstall path forever:
//!   - the **OS keychain entry holding the SQLCipher key** for `meridian.db`
//!     ([`remove_db_key_from_keychain`]) — the only user data that lives on no
//!     filesystem path at all;
//!   - the tray bundle id's OS-managed app data — on **macOS** the
//!     WebKit/AppKit caches (`Application Support`/`Caches`/`WebKit`/`Saved
//!     Application State`/`HTTPStorages`), on **Windows** `%LOCALAPPDATA%`/
//!     `%APPDATA%\com.meridiona.tray`, which is where WebView2 keeps the
//!     cookies and localStorage the signed-in account session lives in;
//!   - (macOS) a best-effort `tccutil reset` of the Accessibility/Screen
//!     Recording/Input Monitoring grants.
//! - `--remove-runtime` — the downloaded Python + MLX runtime and any venvs.
//! - `--remove-models` — Meridian's own MLX models from the shared HuggingFace
//!   cache, allowlisted by exact directory name (never touches a model the user
//!   downloaded separately for another tool).
//! - `--purge` is shorthand for all three, and additionally removes `~/.meridian`
//!   as a single `rm -rf` (rather than the itemized list) so nothing new added to
//!   that directory since is left behind. The full `rm -rf` fires on `--purge`
//!   **only** — not on `--remove-data --remove-runtime`, which stay scoped to the
//!   itemized lists the plan shows.
//!
//! `--json` emits a single machine-readable JSON line instead of the human plan/
//! result text — the tray's uninstall-wizard commands parse this to drive its
//! checkboxes and show a real result, rather than re-implementing this scan.
//!
//! # Who calls this
//! `main.rs` subcommand dispatch: `meridian uninstall [--purge] [--remove-data]
//! [--remove-runtime] [--remove-models] [--dry-run] [--yes] [--json]`. Also
//! invoked by `tray/src-tauri/src/commands/uninstall.rs` (the in-app wizard).
//!
//! # Related
//! - [`json`] — the `--json` reporting path (`JsonReport` + `run_json`), split
//!   out of this file to keep it under the 500-line guideline.
//! - `tray/src-tauri/src/backend_install.rs` — the install side this undoes
//!   (same agent labels + staged paths; small intentional duplication across the
//!   crate boundary — the daemon crate can't depend on the tray).
//! - `tray/src-tauri/src/commands/uninstall.rs` — the GUI wizard's plan/execute
//!   commands, both of which shell out to this binary.
//! - `scripts/uninstall-*.sh` / `scripts/meridian-cli.sh` — the per-service shell
//!   uninstallers (npm/dev path); `MODEL_CATALOG` mirrors the bash allowlist.

mod json;
#[cfg(test)]
mod tests;

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Meridian's own MLX model catalog inside the shared HuggingFace cache
/// (`~/.cache/huggingface/hub`). Allowlisted by exact directory name — mirrors
/// `scripts/meridian-cli.sh`'s `cmd_uninstall` — so `--remove-models` never
/// touches a model the user downloaded separately for another tool (Ollama,
/// LM Studio, etc).
///
/// The embedder entry mirrors `crate::embedder::provision`'s model repo — keep it
/// in sync if the embedding model id changes.
///
/// The Qwen3-Reranker entry is DELIBERATELY still here even though the reranker was
/// removed from the pipeline. Machines that ran an older build downloaded its ~700 MB
/// of weights, and this catalog is the only thing that ever cleans them up — dropping
/// the name would silently strand them on disk forever. Same reasoning as the older
/// generative models above, none of which we ship either.
const MODEL_CATALOG: &[&str] = &[
    "models--mlx-community--Llama-3.3-70B-Instruct-4bit",
    "models--mlx-community--DeepSeek-R1-Distill-Llama-70B-4bit",
    "models--mlx-community--Qwen3.6-35B-A3B-4bit",
    "models--mlx-community--DeepSeek-R1-Distill-Qwen-32B-4bit",
    "models--mlx-community--phi-4-4bit",
    "models--mlx-community--DeepSeek-R1-Distill-Qwen-14B-4bit",
    "models--mlx-community--gemma-3-12b-it-qat-4bit",
    "models--mlx-community--Qwen3.5-2B-OptiQ-4bit",
    "models--mlx-community--Qwen3.5-4B-MLX-4bit",
    "models--mlx-community--Llama-3.2-3B-Instruct-4bit",
    "models--kerncore--Qwen3-Reranker-0.6B-MLX-4bit",
    "models--mlx-community--Qwen3-Embedding-0.6B-8bit",
];

/// One item in a plan/result list — a human label plus the path it names.
#[derive(Debug, Clone, Serialize)]
struct Item {
    label: String,
    path: String,
}

impl Item {
    fn new(label: impl Into<String>, path: &Path) -> Self {
        Self {
            label: label.into(),
            path: path.display().to_string(),
        }
    }

    /// An item labeled with `path`'s own file/dir name — the label the wizard
    /// shows when there's no more descriptive name to attach (staged binaries,
    /// data files, runtime dirs, model dirs all fall back to this).
    fn from_path(path: &Path) -> Self {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        Self::new(label, path)
    }
}

/// The scope + mode flags parsed from `argv`. Grouped into one struct so the
/// plan builder and both output paths take a single value rather than a long
/// argument list (Clippy's seven-argument limit).
#[derive(Debug, Clone, Copy)]
struct Flags {
    /// `--purge`: remove everything, then `rm -rf ~/.meridian` wholesale.
    purge: bool,
    remove_data: bool,
    remove_runtime: bool,
    remove_models: bool,
    dry_run: bool,
    yes: bool,
    json: bool,
}

impl Flags {
    fn from_args(args: &[String]) -> Self {
        let purge = args.iter().any(|a| a == "--purge");
        Self {
            purge,
            remove_data: purge || args.iter().any(|a| a == "--remove-data"),
            remove_runtime: purge || args.iter().any(|a| a == "--remove-runtime"),
            remove_models: purge || args.iter().any(|a| a == "--remove-models"),
            dry_run: args.iter().any(|a| a == "--dry-run"),
            yes: args.iter().any(|a| a == "--yes" || a == "-y"),
            json: args.iter().any(|a| a == "--json"),
        }
    }
}

/// A fully-computed uninstall plan: the resolved paths for each scope plus the
/// flags that produced them. Built once in [`run`], then consumed by either the
/// human-text path or [`json::run_json`], so both branches see identical data.
struct Plan {
    meridian_dir: PathBuf,
    /// `(label, plist path)` for each Meridian launchd agent found.
    agents: Vec<(String, PathBuf)>,
    staged_binaries: Vec<PathBuf>,
    data: Vec<PathBuf>,
    runtime: Vec<PathBuf>,
    models: Vec<PathBuf>,
    flags: Flags,
}

impl Plan {
    /// Scan the filesystem and build the plan for the given `home` + `flags`.
    fn build(home: PathBuf, flags: Flags) -> Self {
        let meridian_dir = home.join(".meridian");

        let agents = meridiona_agent_plists(&home.join("Library/LaunchAgents"));
        let staged_binaries: Vec<PathBuf> = [
            // Staged native binaries (DMG path).
            ".meridian/bin/meridian",
            // Same binary as staged by the Windows installer, which needs the
            // .exe suffix to be runnable. Listed unconditionally rather than
            // cfg'd: the filter below drops paths that do not exist, so the
            // wrong-platform entry costs nothing and one list is easier to keep
            // correct than two.
            ".meridian/bin/meridian.exe",
            ".meridian/bin/meridian-a11y-helper",
            ".meridian/backend-version",
            // CLI on PATH — the DMG symlink and the npm node-wrapper both land here;
            // "remove the CLI" (SETUP.md) means clearing whichever is present.
            ".local/bin/meridian",
            ".local/bin/meridian-daemon",
        ]
        .iter()
        .map(|r| home.join(r))
        // symlink_metadata so a dangling symlink (target already gone) still counts.
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();

        let data = if flags.remove_data {
            data_items(&home)
        } else {
            Vec::new()
        };
        let runtime = if flags.remove_runtime {
            runtime_items(&meridian_dir)
        } else {
            Vec::new()
        };
        let models = if flags.remove_models {
            model_items(&home)
        } else {
            Vec::new()
        };

        Self {
            meridian_dir,
            agents,
            staged_binaries,
            data,
            runtime,
            models,
            flags,
        }
    }

    /// True when the scan found no installed artifacts and no in-scope data.
    fn nothing_to_do(&self) -> bool {
        self.agents.is_empty()
            && self.staged_binaries.is_empty()
            && self.data.is_empty()
            && self.runtime.is_empty()
            && self.models.is_empty()
    }
}

/// Run `meridian uninstall`. User-facing CLI (prints a plan, confirms on a TTY,
/// unless `--json` is passed — see the module doc for the full flag list).
pub fn run(args: &[String]) {
    let flags = Flags::from_args(args);

    let home = match meridian_core::paths::home_dir() {
        Some(h) => h,
        None => {
            if flags.json {
                println!(r#"{{"error":"HOME not set - cannot locate the install"}}"#);
            } else {
                eprintln!("✗ HOME not set - cannot locate the install");
            }
            std::process::exit(1);
        }
    };

    let plan = Plan::build(home, flags);

    if flags.json {
        json::run_json(&plan);
        return;
    }

    run_human(&plan);
}

/// The human-text path: print the plan, confirm on a TTY, then execute.
fn run_human(plan: &Plan) {
    let flags = plan.flags;

    println!("meridian uninstall - plan:");
    for (label, _) in &plan.agents {
        println!("  • stop + remove launchd agent  {label}");
    }
    for f in &plan.staged_binaries {
        println!("  • remove  {}", f.display());
    }
    if flags.remove_data {
        println!("  • remove Meridian data:");
        for f in &plan.data {
            println!("      {}", f.display());
        }
    } else {
        println!(
            "  keeping your data: {0}/.env, meridian.db, oauth/, settings.json, logs/ (pass --remove-data or --purge to remove)",
            plan.meridian_dir.display()
        );
    }
    if flags.remove_runtime {
        println!("  • remove the Python/MLX runtime:");
        for f in &plan.runtime {
            println!("      {}", f.display());
        }
    } else {
        println!(
            "  keeping the downloaded MLX runtime: {0}/runtime/ (pass --remove-runtime or --purge to remove)",
            plan.meridian_dir.display()
        );
    }
    if flags.remove_models {
        println!("  • remove downloaded MLX models from the HuggingFace cache:");
        for f in &plan.models {
            println!("      {}", f.display());
        }
    } else {
        println!(
            "  keeping downloaded models in ~/.cache/huggingface/hub/ (pass --remove-models or --purge to remove)"
        );
    }

    // `--purge` must still wipe ~/.meridian even when none of the itemized
    // scopes matched — e.g. the directory holds only files the catalog doesn't
    // name — so don't short-circuit "nothing to do" in that case.
    let purge_pending = flags.purge && plan.meridian_dir.is_dir();
    if plan.nothing_to_do() && !purge_pending {
        println!("\nNothing to remove - Meridian is not installed here.");
        return;
    }
    if flags.dry_run {
        println!("\n(dry run - nothing changed)");
        return;
    }
    if !flags.yes && !confirm("\nProceed?") {
        println!("Aborted - nothing changed.");
        return;
    }

    // Execute.
    //
    // Windows registers the daemon as a per-user scheduled task rather than a
    // launchd agent (see the tray's backend_install::register_service), so the
    // plist loop below finds nothing there. Without this, uninstalling on
    // Windows would leave the task behind and the daemon would keep starting
    // at every login — with its binary deleted.
    //
    // Best-effort: /Delete exits non-zero when the task is already absent,
    // which is the normal case on a second uninstall.
    #[cfg(target_os = "windows")]
    {
        use meridian_core::proc_ext::NoWindow;
        let out = std::process::Command::new("schtasks")
            .args(["/Delete", "/F", "/TN", "Meridian Daemon"])
            .no_window()
            .output();
        match out {
            Ok(o) if o.status.success() => println!("✓ removed login task  Meridian Daemon"),
            _ => println!("✓ login task  Meridian Daemon (not registered)"),
        }

        // The TRAY's launch-at-login is a separate mechanism from the daemon's
        // scheduled task above: `tauri-plugin-autostart` registers it as an
        // HKCU Run value, not a task, so the `schtasks /Delete` never touched
        // it. On macOS the equivalent is a `com.meridiona.*` LaunchAgent plist,
        // which the plist loop below already sweeps up - Windows had no such
        // coverage, so the tray kept trying to start at every login with its
        // executable deleted.
        //
        // The value name is the app's productName, which is what the plugin
        // registers under. Best-effort: `reg delete` exits non-zero when the
        // value is absent, the normal case on a second uninstall.
        let out = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/V",
                "Meridian",
                "/F",
            ])
            .no_window()
            .output();
        match out {
            Ok(o) if o.status.success() => println!("✓ removed login item  Meridian"),
            _ => println!("✓ login item  Meridian (not registered)"),
        }
    }

    let uid = uid_str();
    for (label, plist) in &plan.agents {
        // `bootout` can legitimately return non-zero (the agent isn't currently
        // loaded); it's best-effort, so its exit status isn't an error here.
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{label}")])
            .status();
        // But don't claim "✓ removed" if the plist deletion actually failed.
        match std::fs::remove_file(plist) {
            Ok(()) => println!("✓ removed agent  {label}"),
            // Already gone (a prior partial uninstall) — still booted out above.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("✓ removed agent  {label}")
            }
            Err(e) => eprintln!(
                "⚠ could not remove agent {label} plist {}: {e}",
                plist.display()
            ),
        }
    }
    for f in &plan.staged_binaries {
        remove_path_reporting(f);
    }
    for f in &plan.data {
        remove_path_reporting(f);
    }
    if flags.remove_data {
        for f in reset_tcc_grants() {
            eprintln!("⚠ could not reset TCC grant {f}");
        }
        match remove_db_key_from_keychain() {
            None => println!("✓ removed database key from the OS keychain"),
            Some(reason) => eprintln!("⚠ {reason}"),
        }
    }
    for f in &plan.runtime {
        remove_path_reporting(f);
    }
    for f in &plan.models {
        remove_path_reporting(f);
    }
    // `--purge` also nukes anything else left under ~/.meridian that the
    // itemized lists above didn't name, so a future addition to that directory
    // is never orphaned. This wholesale `rm -rf` fires on `--purge` ONLY — the
    // itemized `--remove-data`/`--remove-runtime` scopes must not silently take
    // out the rest of ~/.meridian (e.g. an unrelated sibling file the plan
    // never listed).
    if flags.purge && plan.meridian_dir.is_dir() {
        match std::fs::remove_dir_all(&plan.meridian_dir) {
            Ok(()) => println!("✓ purged  {}", plan.meridian_dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("⚠ could not purge {}: {e}", plan.meridian_dir.display()),
        }
    }

    println!(
        "\nDone. If the Meridian menubar app is still running, quit it (or drag \
         Meridian.app to the Trash) - the MLX server is its child and exits with it."
    );
    // These Accessibility / Screen Recording / Input Monitoring grants are a
    // macOS TCC concept: there is nothing analogous to reset or explain on other
    // platforms, and `reset_tcc_grants` is a no-op there, so the messaging is
    // gated to macOS rather than falsely claiming a reset was attempted.
    #[cfg(target_os = "macos")]
    if flags.remove_data {
        println!(
            "\nAlso attempted to reset the Accessibility / Screen Recording / Input \
             Monitoring grants for Meridian in System Settings - macOS best-effort \
             only, so double-check they're gone if it matters to you."
        );
    } else {
        println!(
            "\nNote: deleting or uninstalling does NOT revoke the Accessibility / \
             Screen Recording / Input Monitoring grants in System Settings - macOS \
             keeps that entry until you remove it there yourself (pass --remove-data \
             or --purge to have this attempt it automatically)."
        );
    }
}

/// Best-effort `tccutil reset` for every TCC service Meridian's tray
/// (`com.meridiona.tray`) can be granted, so a full uninstall doesn't leave a
/// grayed-out permission entry behind once the app is gone. Mirrors
/// `tray/src-tauri/src/backend_install.rs::cleanup_legacy_a11y_helper`'s use of
/// the same tool for the retired a11y-helper. Non-fatal: `tccutil` may be
/// missing, sandboxed, or refuse without Full Disk Access, and none of that
/// should abort the rest of the uninstall.
///
/// Returns a `service: reason` line for each reset that did NOT clearly succeed
/// (tool failed to launch, or exited non-zero) so both uninstall paths can
/// surface it — previously a denied or missing `tccutil` was indistinguishable
/// from a clean reset. Every outcome is also logged with structured `service`,
/// `status`/`error` fields.
#[cfg(target_os = "macos")]
fn reset_tcc_grants() -> Vec<String> {
    let mut failures = Vec::new();
    for service in ["Accessibility", "ScreenCapture", "ListenEvent"] {
        match std::process::Command::new("tccutil")
            .args(["reset", service, "com.meridiona.tray"])
            .output()
        {
            Ok(out) if out.status.success() => {
                tracing::info!(service, "uninstall: tcc grant reset");
            }
            Ok(out) => {
                let status = out
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "terminated by signal".to_string());
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stderr = stderr.trim();
                tracing::warn!(
                    service,
                    status = %status,
                    stderr = %stderr,
                    "uninstall: tccutil reset returned non-zero"
                );
                let reason = if stderr.is_empty() {
                    format!("{service}: tccutil exited {status}")
                } else {
                    format!("{service}: tccutil exited {status} ({stderr})")
                };
                failures.push(reason);
            }
            Err(e) => {
                tracing::warn!(service, error = %e, "uninstall: tccutil reset failed to launch");
                failures.push(format!("{service}: {e}"));
            }
        }
    }
    failures
}

#[cfg(not(target_os = "macos"))]
fn reset_tcc_grants() -> Vec<String> {
    Vec::new()
}

/// `~/.meridian` user-data items removed by `--remove-data`/`--purge`, plus (on
/// macOS) the OS-managed app caches WebKit/AppKit create for the tray's bundle
/// id (`com.meridiona.tray`) — `Application Support`, `Caches`, `WebKit`,
/// `Saved Application State`, `HTTPStorages`. Dragging the app to the Trash
/// never removes these (they live outside the bundle and outside
/// `~/.meridian`), so without this they'd survive every uninstall path forever.
/// Deliberately excludes the runtime/venv directories (tracked separately by
/// [`runtime_items`]) so the wizard's checkboxes stay independent of each other.
fn data_items(home: &Path) -> Vec<PathBuf> {
    let meridian = home.join(".meridian");
    let mut items: Vec<PathBuf> = [
        ".env",
        "meridian.db",
        "meridian.db-shm",
        "meridian.db-wal",
        "oauth",
        "settings.json",
        "logs",
        "telemetry",
        "onboarded",
        // The wizard's two companion markers: when it was opened, and whether the
        // dashboard walkthrough that follows it has been armed (commands/setup.rs).
        "setup_started",
        "walkthrough_armed",
        "icon-cache",
        "daemon.sock",
        // Written once by the tray after it successfully registers the
        // launch-at-login item (tray/src-tauri/src/autostart.rs). Left behind,
        // a reinstall would believe autostart was already configured and never
        // re-register it, so the app would silently stop starting at login.
        "autostart_configured",
    ]
    .iter()
    .map(|r| meridian.join(r))
    .filter(|p| p.symlink_metadata().is_ok())
    .collect();
    items.extend(app_cache_items(home));
    items
}

/// The tray's OS-managed WebKit/AppKit caches, keyed off its bundle id
/// (`com.meridiona.tray`) — macOS creates these the first time the app runs,
/// independent of anything Meridian's own code writes. No-op on non-macOS.
fn app_cache_items(home: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        const BUNDLE_ID: &str = "com.meridiona.tray";
        [
            "Library/Application Support",
            "Library/Caches",
            "Library/WebKit",
            "Library/Saved Application State",
            "Library/HTTPStorages",
        ]
        .iter()
        .map(|dir| home.join(dir).join(format!("{BUNDLE_ID}{}", suffix(dir))))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect()
    }
    // Windows' equivalent: everything WebView2 and Tauri keep OUTSIDE the
    // install directory, keyed off the same bundle id. `%LOCALAPPDATA%\
    // com.meridiona.tray` is where WebView2 puts its `EBWebView` profile —
    // cookies, localStorage, IndexedDB, cache — which is where the signed-in
    // account session actually lives, so leaving it behind means an uninstall
    // + reinstall silently comes back still logged in. `%APPDATA%\...` is the
    // roaming half. Neither is touched by the NSIS uninstaller, which only
    // removes what it installed.
    //
    // Read from the environment rather than composed from `home`: the two are
    // independently redirectable (roaming profiles, redirected folders), so
    // deriving them from the home directory would miss the real location on
    // exactly the machines where it matters.
    #[cfg(target_os = "windows")]
    {
        let _ = home;
        bundle_dirs_under(
            ["LOCALAPPDATA", "APPDATA"]
                .iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = home;
        Vec::new()
    }
}

/// Joins the tray's bundle id onto each base directory and keeps only the ones
/// that exist — the existence filter every other item list here uses, so a
/// wizard checkbox never advertises removing nothing.
///
/// Split out of [`app_cache_items`]'s Windows branch purely so it is testable
/// WITHOUT mutating `%LOCALAPPDATA%`/`%APPDATA%`: those are process-global, and
/// Rust runs tests in parallel threads, so a test that set them would race every
/// other test in the binary. Compiled on non-Windows only under `cfg(test)`, so
/// the coverage runs on every platform's CI while release builds carry nothing
/// dead.
#[cfg(any(target_os = "windows", test))]
fn bundle_dirs_under(bases: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    const BUNDLE_ID: &str = "com.meridiona.tray";
    bases
        .into_iter()
        .map(|base| base.join(BUNDLE_ID))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect()
}

/// Best-effort removal of the SQLCipher key for `meridian.db` from the OS
/// keychain (macOS Keychain / Windows Credential Manager), written by
/// `tray/src-tauri/src/db_key.rs`.
///
/// This is the one piece of Meridian's user data that lives nowhere on disk, so
/// no amount of deleting directories reaches it. Left behind it is both a
/// privacy leak (the key to a database the user asked us to destroy) and a
/// correctness hazard: a reinstall finds a key in the keychain and assumes the
/// database it decrypts is its own.
///
/// The service/account pair and the `keyring` crate are deliberately the SAME
/// ones the tray writes with, rather than a hand-rolled `security
/// delete-generic-password` / `cmdkey /delete` shell-out. Windows' credential
/// target is an internal `keyring` convention (`{user}.{service}`), so spelling
/// it out here would silently stop matching the day that crate changes it - and
/// the failure mode is invisible, which is exactly what this function exists to
/// prevent.
///
/// Non-fatal, and returns a reason when it did NOT clearly succeed: the item
/// may be ACL-restricted to the tray's code signature, so macOS can refuse or
/// prompt. A missing entry is a SUCCESS - that is the normal second-uninstall
/// case, and the desired end state either way.
fn remove_db_key_from_keychain() -> Option<String> {
    // Mirrors tray/src-tauri/src/db_key.rs's SERVICE/ACCOUNT.
    const SERVICE: &str = "Meridian";
    const ACCOUNT: &str = "db-encryption-key";

    let entry = match keyring::Entry::new(SERVICE, ACCOUNT) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "uninstall: could not open the OS keychain");
            return Some(format!("could not open the OS keychain: {e}"));
        }
    };
    match entry.delete_credential() {
        Ok(()) => {
            tracing::info!("uninstall: removed the database key from the OS keychain");
            None
        }
        Err(keyring::Error::NoEntry) => {
            tracing::info!("uninstall: no database key in the OS keychain");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "uninstall: could not remove the database key");
            Some(format!("could not remove the database key: {e}"))
        }
    }
}

/// `Saved Application State` suffixes the bundle id with `.savedState`; every
/// other cache dir here is named exactly the bundle id.
#[cfg(target_os = "macos")]
fn suffix(dir: &str) -> &'static str {
    if dir == "Library/Saved Application State" {
        ".savedState"
    } else {
        ""
    }
}

/// The downloaded Python + MLX runtime and venvs, removed by
/// `--remove-runtime`/`--purge`.
fn runtime_items(meridian: &Path) -> Vec<PathBuf> {
    [
        "runtime",
        "runtime.incoming",
        "runtime.old",
        "mlx-server-venv",
        "node-runtime",
        "mlx-server.pid",
    ]
    .iter()
    .map(|r| meridian.join(r))
    .filter(|p| p.symlink_metadata().is_ok())
    .collect()
}

/// Meridian's own downloaded models present in `~/.cache/huggingface/hub`,
/// filtered to [`MODEL_CATALOG`] so a model the user fetched separately for
/// another tool is never touched.
fn model_items(home: &Path) -> Vec<PathBuf> {
    let hub = home.join(".cache/huggingface/hub");
    MODEL_CATALOG
        .iter()
        .map(|d| hub.join(d))
        .filter(|p| p.is_dir())
        .collect()
}

/// Remove a file or directory (whichever `path` is), reporting the outcome to
/// stdout/stderr the way the human-text CLI path expects.
fn remove_path_reporting(path: &Path) {
    match remove_path(path) {
        Ok(()) => println!("✓ removed  {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("⚠ could not remove {}: {e}", path.display()),
    }
}

/// Remove `path` whether it's a file/symlink or a directory.
fn remove_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) => Err(e),
    }
}

/// All `~/Library/LaunchAgents/com.meridiona.*.plist` paths, paired with their
/// label (filename without `.plist`). Catches every Meridian agent regardless of
/// which installer wrote it, so an uninstall leaves nothing orphaned.
fn meridiona_agent_plists(launch_agents: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(launch_agents) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(label) = meridiona_label(&path) {
            out.push((label, path));
        }
    }
    out.sort();
    out
}

/// `com.meridiona.<x>` label for a `com.meridiona.<x>.plist` path, else `None`.
fn meridiona_label(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let label = name.strip_suffix(".plist")?;
    label
        .starts_with("com.meridiona.")
        .then(|| label.to_string())
}

/// y/N confirm on a TTY. Returns `false` when not a terminal (never delete
/// non-interactively without `--yes`).
fn confirm(prompt: &str) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        eprintln!("{prompt} refusing without a TTY - pass --yes to confirm.");
        return false;
    }
    print!("{prompt} [y/N]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Current uid as a string for `launchctl gui/<uid>/…`; `"501"` fallback.
fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}
