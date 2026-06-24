//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// `meridian doctor --fix`: attempt to repair the warnings/criticals the sweep
// found. Fixes are tiered by risk —
//   Auto   : safe + idempotent, run silently (restart a dead service)
//   Guided : run, but show the command and ask y/N first (drain a queue)
//   Manual : only a human can (regenerate a token) — print the remedy
// Anything still failing after a pass is escalated: a content-free bundle is
// written for the team / a `claude` hand-off. `--dry-run` plans without acting.

use crate::config::Config;
use crate::health::platform::repo_root;
use crate::health::{Check, Report, Severity};
use std::io::{BufRead, IsTerminal, Write};
use std::process::Command;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Tier {
    Auto,
    Guided,
    Manual,
}

pub struct FixAction {
    pub tier: Tier,
    pub label: String,
    /// argv to execute (empty for Manual). Run in the repo root.
    pub command: Vec<String>,
}

/// Map a failing check to a repair, if one exists.
pub fn fix_for(c: &Check) -> Option<FixAction> {
    let (g, n) = (c.group, c.name.as_str());
    let auto = |label: &str, cmd: &[&str]| {
        Some(FixAction {
            tier: Tier::Auto,
            label: label.into(),
            command: cmd.iter().map(|s| s.to_string()).collect(),
        })
    };
    let guided = |label: &str, cmd: &[&str]| {
        Some(FixAction {
            tier: Tier::Guided,
            label: label.into(),
            command: cmd.iter().map(|s| s.to_string()).collect(),
        })
    };
    let manual = |label: &str| {
        Some(FixAction {
            tier: Tier::Manual,
            label: label.into(),
            command: Vec::new(),
        })
    };

    match (g, n) {
        // A dead launchd service → bring the stack up (safe, idempotent).
        (_, name) if name.contains("running") || name.contains("service") => {
            auto("restart the daemons", &["meridian", "start"])
        }
        // Stale Jira cache → force a refresh.
        ("jira", name) if name.contains("ticket sync") => {
            guided("refresh the Jira ticket cache", &["meridian", "restart"])
        }
        // Missing session-summary skill → write the file (safe, idempotent).
        ("coding-agent", name) if name.contains("session-summary skill") => auto(
            "install the session-summary Claude Code command",
            &["meridian", "coding-agent-install-skill"],
        ),
        // Summariser backlog → drain it (mutates data → guided).
        ("meridian daemon", name) if name.contains("summariser queue") => guided(
            "drain the summariser queue",
            &["meridian", "coding-agent-summarise"],
        ),
        // Sentinelled sessions → re-run classification (guided).
        ("meridian daemon", name) if name.contains("classify errors") => guided(
            "re-classify sentinelled sessions",
            &["meridian", "coding-agent-classify"],
        ),
        // UI not built → build it.
        ("ui", name) if name.contains("built") => guided(
            "build the UI",
            &["bash", "-lc", "cd ui && npm ci && npm run build"],
        ),
        // a11y regression — re-establishing capture often fixes it (guided).
        ("capture", name) if name.contains("a11y_regression") => {
            guided("restart Meridian to re-establish a11y capture", &["meridian", "restart"])
        }
        // a11y permission off — a human must grant it in System Settings.
        ("capture", name) if name.contains("a11y_permission") => manual(
            "System Settings ▸ Privacy & Security ▸ Accessibility ▸ enable Meridian, then restart it",
        ),
        // Token/permission/config — a human must act.
        ("jira", name) if name.contains("auth") => {
            manual("regenerate the Jira API token and update JIRA_API_TOKEN in .env")
        }
        ("config", name) if name.contains("settings") => {
            manual("align <repo>/settings.json with ~/.meridian/settings.json")
        }
        _ => None,
    }
}

/// Run the repair pass over a report. Returns true if anything still needs a
/// human (so the caller can exit non-zero).
pub fn run(_cfg: &Config, report: &Report, dry_run: bool) -> bool {
    let failing: Vec<&Check> = report
        .checks
        .iter()
        .filter(|c| c.severity >= Severity::Warn)
        .collect();

    if failing.is_empty() {
        println!("\n  ✓ nothing to fix — all checks pass\n");
        return false;
    }

    println!(
        "\n  Meridian doctor --fix{}",
        if dry_run {
            "  (dry run — nothing will be executed)"
        } else {
            ""
        }
    );
    println!("  ════════════════════════════════════════════════════════");

    let mut planned: Vec<FixAction> = Vec::new();
    let mut residual = false;

    for c in &failing {
        let Some(action) = fix_for(c) else {
            println!("  ·  {} › {} — no automatic fix", c.group, c.name);
            residual = true;
            continue;
        };
        // De-dupe: many dead services map to one `meridian start`.
        if planned
            .iter()
            .any(|p| p.command == action.command && p.label == action.label)
        {
            continue;
        }
        match action.tier {
            Tier::Manual => {
                println!("  ⚠  manual: {}", action.label);
                residual = true;
            }
            Tier::Auto => {
                if dry_run {
                    println!(
                        "  →  auto: {}  [{}]",
                        action.label,
                        action.command.join(" ")
                    );
                } else {
                    print!("  →  auto: {} … ", action.label);
                    let _ = std::io::stdout().flush();
                    let ok = exec(&action.command);
                    println!("{}", if ok { "done" } else { "FAILED" });
                    residual |= !ok;
                }
            }
            Tier::Guided => {
                if dry_run {
                    println!(
                        "  ?  guided: {}  [{}]",
                        action.label,
                        action.command.join(" ")
                    );
                } else if confirm(&action.label, &action.command) {
                    print!("     running … ");
                    let _ = std::io::stdout().flush();
                    let ok = exec(&action.command);
                    println!("{}", if ok { "done" } else { "FAILED" });
                    residual |= !ok;
                } else {
                    println!("     skipped — run later: {}", action.command.join(" "));
                    residual = true;
                }
            }
        }
        planned.push(action);
    }

    if residual && !dry_run {
        match write_bundle(report) {
            Some(path) => println!(
                "\n  Some items still need attention. Diagnostic bundle saved:\n    {}\n    Share it with the team, or run: claude \"debug this meridian doctor bundle\"\n",
                path
            ),
            None => println!("\n  Some items still need attention — share `meridian doctor` output with the team.\n"),
        }
    } else if !residual && !dry_run {
        println!("\n  ✓ applied fixes — re-run `meridian doctor` to confirm\n");
    } else {
        println!();
    }
    residual
}

fn exec(argv: &[String]) -> bool {
    let Some((bin, args)) = argv.split_first() else {
        return false;
    };
    let mut cmd = Command::new(bin);
    cmd.args(args);
    if let Some(root) = repo_root() {
        cmd.current_dir(root);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// y/N prompt. Non-interactive (piped) input defaults to No so `--fix` never
/// auto-runs a guided action without a human.
fn confirm(label: &str, argv: &[String]) -> bool {
    if !std::io::stdin().is_terminal() {
        println!(
            "  ?  guided: {} [{}] — skipped (non-interactive)",
            label,
            argv.join(" ")
        );
        return false;
    }
    print!("  ?  {} — run `{}`? [y/N] ", label, argv.join(" "));
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes")
}

/// Write a content-free diagnostic bundle (the porcelain report) for handoff.
fn write_bundle(report: &Report) -> Option<String> {
    let dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)?
        .join(".meridian");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("doctor-bundle.txt");
    let mut body = String::from("meridian doctor diagnostic bundle\n\n");
    body.push_str(&report.render(false));
    let dx = crate::health::diagnose::root_causes(report);
    body.push_str(&crate::health::diagnose::render(&dx, false));
    std::fs::write(&path, body).ok()?;
    Some(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_service_maps_to_auto_restart() {
        let c =
            Check::critical("daemon running", "system", "not loaded").in_group("meridian daemon");
        let f = fix_for(&c).unwrap();
        assert_eq!(f.tier, Tier::Auto);
        assert_eq!(f.command, vec!["meridian", "start"]);
    }

    #[test]
    fn summariser_backlog_is_guided() {
        let c = Check::warn("summariser queue", "L2", "293").in_group("meridian daemon");
        assert_eq!(fix_for(&c).unwrap().tier, Tier::Guided);
    }

    #[test]
    fn jira_auth_is_manual() {
        let c = Check::critical("auth", "L2", "401").in_group("jira");
        assert_eq!(fix_for(&c).unwrap().tier, Tier::Manual);
    }

    #[test]
    fn unknown_check_has_no_fix() {
        let c = Check::warn("a11y_per_app", "L1", "x").in_group("capture");
        assert!(fix_for(&c).is_none());
    }
}
