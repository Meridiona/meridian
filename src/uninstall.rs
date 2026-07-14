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
//!   that directory since is left behind.
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
//! - `tray/src-tauri/src/backend_install.rs` — the install side this undoes
//!   (same agent labels + staged paths; small intentional duplication across the
//!   crate boundary — the daemon crate can't depend on the tray).
//! - `tray/src-tauri/src/commands/uninstall.rs` — the GUI wizard's plan/execute
//!   commands, both of which shell out to this binary.
//! - `scripts/uninstall-*.sh` / `scripts/meridian-cli.sh` — the per-service shell
//!   uninstallers (npm/dev path); `MODEL_CATALOG` mirrors the bash allowlist.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Meridian's own MLX model catalog inside the shared HuggingFace cache
/// (`~/.cache/huggingface/hub`). Allowlisted by exact directory name — mirrors
/// `scripts/meridian-cli.sh`'s `cmd_uninstall` — so `--remove-models` never
/// touches a model the user downloaded separately for another tool (Ollama,
/// LM Studio, etc).
///
/// The last two entries are the reranker + embedder roles from
/// `services/agents/model_registry.py`'s `RERANKER`/`EMBEDDER` `ModelSpec`s —
/// that module is the authoritative source for the pipeline's active model
/// ids; keep these two in sync with its defaults if they ever change.
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

/// The `--json` output shape — a full plan (dry-run) or a full result
/// (executed), depending on `executed`. The tray's uninstall commands are the
/// only consumers; keep this contract in sync with
/// `tray/src-tauri/src/commands/uninstall.rs`.
#[derive(Debug, Serialize, Default)]
struct JsonReport {
    dry_run: bool,
    executed: bool,
    remove_data: bool,
    remove_runtime: bool,
    remove_models: bool,
    agents: Vec<Item>,
    staged_binaries: Vec<Item>,
    data: Vec<Item>,
    runtime: Vec<Item>,
    models: Vec<Item>,
    /// Paths successfully removed (only populated when `executed`).
    removed: Vec<String>,
    /// Non-fatal removal failures, `"<path>: <error>"` (only when `executed`).
    errors: Vec<String>,
}

/// Run `meridian uninstall`. User-facing CLI (prints a plan, confirms on a TTY,
/// unless `--json` is passed — see the module doc for the full flag list).
pub fn run(args: &[String]) {
    let purge = args.iter().any(|a| a == "--purge");
    let remove_data = purge || args.iter().any(|a| a == "--remove-data");
    let remove_runtime = purge || args.iter().any(|a| a == "--remove-runtime");
    let remove_models = purge || args.iter().any(|a| a == "--remove-models");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");
    let json = args.iter().any(|a| a == "--json");

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            if json {
                println!(r#"{{"error":"HOME not set — cannot locate the install"}}"#);
            } else {
                eprintln!("✗ HOME not set — cannot locate the install");
            }
            std::process::exit(1);
        }
    };
    let meridian_dir = home.join(".meridian");

    let agents = meridiona_agent_plists(&home.join("Library/LaunchAgents"));
    let staged_binaries: Vec<PathBuf> = [
        // Staged native binaries (DMG path).
        ".meridian/bin/meridian",
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

    let data = if remove_data {
        data_items(&meridian_dir)
    } else {
        Vec::new()
    };
    let runtime = if remove_runtime {
        runtime_items(&meridian_dir)
    } else {
        Vec::new()
    };
    let models = if remove_models {
        model_items(&home)
    } else {
        Vec::new()
    };

    let nothing_to_do = agents.is_empty()
        && staged_binaries.is_empty()
        && data.is_empty()
        && runtime.is_empty()
        && models.is_empty();

    if json {
        run_json(
            &home,
            &meridian_dir,
            agents,
            staged_binaries,
            data,
            runtime,
            models,
            purge,
            remove_data,
            remove_runtime,
            remove_models,
            dry_run,
            yes,
        );
        return;
    }

    // Print the plan (human text).
    println!("meridian uninstall — plan:");
    for (label, _) in &agents {
        println!("  • stop + remove launchd agent  {label}");
    }
    for f in &staged_binaries {
        println!("  • remove  {}", f.display());
    }
    if remove_data {
        println!("  • remove Meridian data:");
        for f in &data {
            println!("      {}", f.display());
        }
    } else {
        println!(
            "  keeping your data: {0}/.env, meridian.db, oauth/, settings.json, logs/ (pass --remove-data or --purge to remove)",
            meridian_dir.display()
        );
    }
    if remove_runtime {
        println!("  • remove the Python/MLX runtime:");
        for f in &runtime {
            println!("      {}", f.display());
        }
    } else {
        println!(
            "  keeping the downloaded MLX runtime: {0}/runtime/ (pass --remove-runtime or --purge to remove)",
            meridian_dir.display()
        );
    }
    if remove_models {
        println!("  • remove downloaded MLX models from the HuggingFace cache:");
        for f in &models {
            println!("      {}", f.display());
        }
    } else {
        println!(
            "  keeping downloaded models in ~/.cache/huggingface/hub/ (pass --remove-models or --purge to remove)"
        );
    }

    if nothing_to_do {
        println!("\nNothing to remove — Meridian is not installed here.");
        return;
    }
    if dry_run {
        println!("\n(dry run — nothing changed)");
        return;
    }
    if !yes && !confirm("\nProceed?") {
        println!("Aborted — nothing changed.");
        return;
    }

    // Execute.
    let uid = uid_str();
    for (label, plist) in &agents {
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{label}")])
            .status();
        let _ = std::fs::remove_file(plist);
        println!("✓ removed agent  {label}");
    }
    for f in &staged_binaries {
        remove_path_reporting(f);
    }
    for f in &data {
        remove_path_reporting(f);
    }
    for f in &runtime {
        remove_path_reporting(f);
    }
    for f in &models {
        remove_path_reporting(f);
    }
    // `--purge` also nukes anything else left under ~/.meridian that the
    // itemized lists above didn't name, so a future addition to that directory
    // is never orphaned.
    if remove_data && remove_runtime && meridian_dir.is_dir() {
        match std::fs::remove_dir_all(&meridian_dir) {
            Ok(()) => println!("✓ purged  {}", meridian_dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("⚠ could not purge {}: {e}", meridian_dir.display()),
        }
    }

    println!(
        "\nDone. If the Meridian menubar app is still running, quit it (or drag \
         Meridian.app to the Trash) — the MLX server is its child and exits with it."
    );
    println!(
        "\nNote: deleting or uninstalling does NOT revoke the Accessibility / \
         Screen Recording / Input Monitoring grants in System Settings — macOS \
         keeps that entry until you remove it there yourself."
    );
}

/// The `--json` path: same computation as the human branch, emitted as one
/// JSON line, no prompts (a missing `--yes` under `--json` is treated as a
/// refusal to execute non-interactively — callers must pass both).
#[allow(clippy::too_many_arguments)]
fn run_json(
    home: &Path,
    meridian_dir: &Path,
    agents: Vec<(String, PathBuf)>,
    staged_binaries: Vec<PathBuf>,
    data: Vec<PathBuf>,
    runtime: Vec<PathBuf>,
    models: Vec<PathBuf>,
    _purge: bool,
    remove_data: bool,
    remove_runtime: bool,
    remove_models: bool,
    dry_run: bool,
    yes: bool,
) {
    let mut report = JsonReport {
        dry_run,
        executed: false,
        remove_data,
        remove_runtime,
        remove_models,
        agents: agents
            .iter()
            .map(|(label, path)| Item::new(label.clone(), path))
            .collect(),
        staged_binaries: staged_binaries.iter().map(|p| Item::from_path(p)).collect(),
        data: data.iter().map(|p| Item::from_path(p)).collect(),
        runtime: runtime.iter().map(|p| Item::from_path(p)).collect(),
        models: models.iter().map(|p| Item::from_path(p)).collect(),
        removed: Vec::new(),
        errors: Vec::new(),
    };

    if dry_run || !yes {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return;
    }

    report.executed = true;
    let uid = uid_str();
    for (label, plist) in &agents {
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{label}")])
            .status();
        match std::fs::remove_file(plist) {
            Ok(()) => report.removed.push(plist.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.errors.push(format!("{}: {e}", plist.display())),
        }
    }
    for f in staged_binaries
        .iter()
        .chain(&data)
        .chain(&runtime)
        .chain(&models)
    {
        match remove_path(f) {
            Ok(()) => report.removed.push(f.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.errors.push(format!("{}: {e}", f.display())),
        }
    }
    if remove_data && remove_runtime && meridian_dir.is_dir() {
        match std::fs::remove_dir_all(meridian_dir) {
            Ok(()) => report.removed.push(meridian_dir.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report
                .errors
                .push(format!("{}: {e}", meridian_dir.display())),
        }
    }
    let _ = home; // reserved for future report fields (e.g. echoing HOME)
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
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
        eprintln!("{prompt} refusing without a TTY — pass --yes to confirm.");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_matches_only_meridiona_plists() {
        assert_eq!(
            meridiona_label(Path::new("/x/com.meridiona.daemon.plist")).as_deref(),
            Some("com.meridiona.daemon")
        );
        assert_eq!(
            meridiona_label(Path::new("/x/com.meridiona.a11y-helper.plist")).as_deref(),
            Some("com.meridiona.a11y-helper")
        );
        assert_eq!(meridiona_label(Path::new("/x/com.apple.thing.plist")), None);
        assert_eq!(meridiona_label(Path::new("/x/com.meridiona.daemon")), None); // not a plist
        assert_eq!(meridiona_label(Path::new("/x/notes.txt")), None);
    }

    #[test]
    fn enumerates_only_meridiona_agents() {
        let dir = std::env::temp_dir().join("meridian-uninstall-test-agents");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "com.meridiona.daemon.plist",
            "com.meridiona.a11y-helper.plist",
            "com.apple.something.plist",
            "random.txt",
        ] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let found: Vec<String> = meridiona_agent_plists(&dir)
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(
            found,
            vec![
                "com.meridiona.a11y-helper".to_string(),
                "com.meridiona.daemon".to_string()
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_launch_agents_dir_is_empty() {
        let dir = std::env::temp_dir().join("meridian-uninstall-test-nope-xyz");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(meridiona_agent_plists(&dir).is_empty());
    }

    /// `data_items`/`runtime_items` must only list entries that actually exist
    /// (a wizard checkbox should never advertise removing nothing), and must
    /// stay disjoint from each other so the two checkboxes don't double-count.
    #[test]
    fn data_and_runtime_items_only_list_existing_entries_and_stay_disjoint() {
        let dir = std::env::temp_dir().join(format!(
            "meridian-uninstall-test-items-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env"), "x").unwrap();
        std::fs::write(dir.join("meridian.db"), "x").unwrap();
        std::fs::create_dir_all(dir.join("runtime")).unwrap();
        // "settings.json" and "mlx-server-venv" deliberately absent.

        let data = data_items(&dir);
        let runtime = runtime_items(&dir);

        assert!(data.contains(&dir.join(".env")));
        assert!(data.contains(&dir.join("meridian.db")));
        assert!(!data.iter().any(|p| p.ends_with("settings.json")));
        assert!(runtime.contains(&dir.join("runtime")));
        assert!(!runtime.iter().any(|p| p.ends_with("mlx-server-venv")));

        for p in &data {
            assert!(
                !runtime.contains(p),
                "data and runtime item lists must be disjoint: {p:?}"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `model_items` only returns catalog entries that exist on disk, and never
    /// invents a path for a model that isn't in [`MODEL_CATALOG`].
    #[test]
    fn model_items_filters_to_existing_catalog_dirs() {
        let home = std::env::temp_dir().join(format!(
            "meridian-uninstall-test-models-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        let hub = home.join(".cache/huggingface/hub");
        std::fs::create_dir_all(&hub).unwrap();
        std::fs::create_dir_all(hub.join("models--mlx-community--Qwen3.5-2B-OptiQ-4bit")).unwrap();
        // A non-Meridian model the user downloaded for another tool — must be ignored.
        std::fs::create_dir_all(hub.join("models--some-other-org--unrelated-model")).unwrap();

        let found = model_items(&home);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("models--mlx-community--Qwen3.5-2B-OptiQ-4bit"));

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn remove_path_handles_files_and_directories() {
        let dir = std::env::temp_dir().join(format!(
            "meridian-uninstall-test-remove-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let file = dir.join("a-file");
        std::fs::write(&file, "x").unwrap();
        remove_path(&file).unwrap();
        assert!(!file.exists());

        let subdir = dir.join("a-dir");
        std::fs::create_dir_all(subdir.join("nested")).unwrap();
        remove_path(&subdir).unwrap();
        assert!(!subdir.exists());

        // A path that doesn't exist reports NotFound rather than panicking.
        assert_eq!(
            remove_path(&dir.join("missing")).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
