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
//! - `--remove-data` — `~/.meridian`'s user data: db, credentials, settings,
//!   logs, telemetry spool, icon cache, onboarded marker.
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
            data_items(&meridian_dir)
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
        let out = std::process::Command::new("schtasks")
            .args(["/Delete", "/F", "/TN", "Meridian Daemon"])
            .output();
        match out {
            Ok(o) if o.status.success() => println!("✓ removed login task  Meridian Daemon"),
            _ => println!("✓ login task  Meridian Daemon (not registered)"),
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
    println!(
        "\nNote: deleting or uninstalling does NOT revoke the Accessibility / \
         Screen Recording / Input Monitoring grants in System Settings - macOS \
         keeps that entry until you remove it there yourself."
    );
}

/// `~/.meridian` user-data items removed by `--remove-data`/`--purge`.
/// Deliberately excludes the runtime/venv directories (tracked separately by
/// [`runtime_items`]) so the wizard's checkboxes stay independent of each other.
fn data_items(meridian: &Path) -> Vec<PathBuf> {
    [
        ".env",
        "meridian.db",
        "meridian.db-shm",
        "meridian.db-wal",
        "oauth",
        "settings.json",
        "logs",
        "telemetry",
        "onboarded",
        "icon-cache",
        "daemon.sock",
    ]
    .iter()
    .map(|r| meridian.join(r))
    .filter(|p| p.symlink_metadata().is_ok())
    .collect()
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
