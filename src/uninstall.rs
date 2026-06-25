//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `meridian uninstall` — the inverse of the install harness (Gap-2 Bucket 1).
//!
//! Dragging `Meridian.app` to the Trash leaves the launchd agents the tray
//! registered (`com.meridiona.daemon`, `com.meridiona.a11y-helper` — see
//! `tray/src-tauri/src/backend_install.rs`) running forever, because no app code
//! runs once the bundle is gone. This subcommand is the clean teardown: it lives
//! in the daemon binary, which is staged at `~/.meridian/bin/meridian` and so
//! **survives the app being trashed** — the user (or `meridian doctor`, later)
//! can run it to stop + remove every Meridian launchd agent and the staged
//! binaries.
//!
//! By default it removes only the **installed artifacts** (launchd agents, staged
//! binaries, the install marker) and **keeps user data** (`~/.meridian/.env`,
//! `meridian.db`, `oauth/`, `settings.json`, the downloaded MLX `runtime/`). The
//! `--purge` flag removes `~/.meridian` entirely.
//!
//! # Who calls this
//! `main.rs` subcommand dispatch: `meridian uninstall [--purge] [--dry-run] [--yes]`.
//!
//! # Related
//! - `tray/src-tauri/src/backend_install.rs` — the install side this undoes
//!   (same agent labels + staged paths; small intentional duplication across the
//!   crate boundary — the daemon crate can't depend on the tray).
//! - `scripts/uninstall-*.sh` — the per-service shell uninstallers (npm/dev path).

use std::path::{Path, PathBuf};

/// Run `meridian uninstall`. User-facing CLI (prints a plan, confirms on a TTY).
/// Flags: `--purge` (also delete user data), `--dry-run` (show, change nothing),
/// `--yes`/`-y` (skip the confirmation prompt).
pub fn run(args: &[String]) {
    let purge = args.iter().any(|a| a == "--purge");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            eprintln!("✗ HOME not set — cannot locate the install");
            std::process::exit(1);
        }
    };

    let agents = meridiona_agent_plists(&home.join("Library/LaunchAgents"));
    let mut files: Vec<PathBuf> = [
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
    // Under --purge the whole ~/.meridian goes, so listing its members is noise.
    let purge_root = home.join(".meridian");
    if purge {
        files.clear();
    }

    // Print the plan.
    println!("meridian uninstall — plan:");
    for (label, _) in &agents {
        println!("  • stop + remove launchd agent  {label}");
    }
    for f in &files {
        println!("  • remove  {}", f.display());
    }
    if purge {
        println!(
            "  • PURGE all Meridian data:  rm -rf {}",
            purge_root.display()
        );
        println!("    (database, credentials, settings, logs, downloaded MLX runtime + model)");
    } else {
        println!(
            "  keeping your data: {0}/.env, meridian.db, oauth/, settings.json, runtime/",
            purge_root.display()
        );
        println!("    (pass --purge to remove these too)");
    }

    if agents.is_empty() && files.is_empty() && !purge {
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
    for f in &files {
        match std::fs::remove_file(f) {
            Ok(()) => println!("✓ removed  {}", f.display()),
            Err(e) => eprintln!("⚠ could not remove {}: {e}", f.display()),
        }
    }
    if purge {
        match std::fs::remove_dir_all(&purge_root) {
            Ok(()) => println!("✓ purged  {}", purge_root.display()),
            Err(e) => eprintln!("⚠ could not purge {}: {e}", purge_root.display()),
        }
    }

    println!(
        "\nDone. If the Meridian menubar app is still running, quit it (or drag \
         Meridian.app to the Trash) — the MLX server is its child and exits with it."
    );
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
}
