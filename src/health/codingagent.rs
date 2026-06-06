// meridian — normalises screenpipe activity into structured app sessions
//
// Coding-agent ingest health: per-agent presence of the CLIs (the summariser
// engines — a missing CLI means that agent's sessions silently fall back to MLX
// summaries) and of the session stores the indexer sweeps (Claude / Codex
// JSONLs, Copilot session-state, Cursor state.vscdb + cursor-agent chats).

use crate::config::Config;
use crate::health::platform::which;
use crate::health::Check;
use std::path::PathBuf;

pub fn checks(_cfg: &Config) -> Vec<Check> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let claude_present = which("claude").is_some();
    let skill_path = home.join(".claude/commands/session-summary.md");

    let mut out = vec![
        cli("claude", "Claude Code"),
        cli("codex", "Codex"),
        cli("copilot", "GitHub Copilot CLI"),
        dir(&home.join(".claude/projects"), "claude sessions dir"),
        dir(&home.join(".codex/sessions"), "codex sessions dir"),
        dir(
            &env_path(
                "COPILOT_SESSION_STATE_DIR",
                home.join(".copilot/session-state"),
            ),
            "copilot sessions dir",
        ),
    ];

    // The session-summary skill must exist for `claude -p /session-summary` to
    // work. Without it the command returns "Unknown command" and every Claude
    // Code session silently falls through to the local MLX model instead.
    if claude_present {
        if skill_path.exists() {
            out.push(Check::ok(
                "session-summary skill",
                "L2",
                "~/.claude/commands/session-summary.md present",
            ));
        } else {
            out.push(
                Check::warn(
                    "session-summary skill",
                    "L2",
                    "~/.claude/commands/session-summary.md missing — claude summariser falls back to MLX for every session",
                )
                .with_remedy("meridian doctor --fix  (or: meridian coding-agent-install-skill)"),
            );
        }
    }

    // Cursor: sidebar/IDE-agent transcripts are ingested from state.vscdb and
    // cursor-agent CLI chats from ~/.cursor/chats. Summaries use the
    // cursor-agent CLI when present + authenticated, else MLX.
    let cursor_present =
        which("cursor").is_some() || home.join("Library/Application Support/Cursor").is_dir();
    if cursor_present {
        let vscdb = env_path(
            "CURSOR_STATE_VSCDB",
            home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
        );
        if vscdb.is_file() {
            out.push(Check::ok(
                "cursor transcripts",
                "L1",
                "state.vscdb present — sidebar/IDE-agent chats ingested",
            ));
        } else {
            out.push(Check::info(
                "cursor transcripts",
                "L1",
                "Cursor detected but state.vscdb not found — sidebar chats not ingested (set CURSOR_STATE_VSCDB for non-standard installs)",
            ));
        }
        out.push(dir(
            &env_path("CURSOR_CLI_CHATS_DIR", home.join(".cursor/chats")),
            "cursor-agent chats dir",
        ));
        if which("cursor-agent").is_some() {
            out.push(
                Check::ok(
                    "cursor-agent CLI",
                    "L2",
                    "cursor-agent on PATH — Cursor summaries use it when authenticated",
                )
                .with_remedy("verify auth: cursor-agent status  (login: cursor-agent login)"),
            );
        } else {
            out.push(
                Check::info(
                    "cursor-agent CLI",
                    "L2",
                    "Cursor detected but cursor-agent not on PATH — Cursor summaries fall back to MLX",
                )
                .with_remedy(
                    "install: curl https://cursor.com/install -fsS | bash; then: cursor-agent login — or set CURSOR_AGENT_AUTO_INSTALL=1 in ~/.meridian/app/.env to let the daemon install it",
                ),
            );
        }
    }
    out
}

fn cli(bin: &str, label: &'static str) -> Check {
    if which(bin).is_some() {
        Check::ok(label, "L2", format!("{bin} on PATH"))
    } else {
        Check::info(
            label,
            "L2",
            format!("{bin} not on PATH — those sessions fall back to MLX summaries"),
        )
    }
}

fn dir(path: &std::path::Path, label: &'static str) -> Check {
    if path.is_dir() {
        Check::ok(label, "L1", "present")
    } else {
        Check::info(label, "L1", "none yet")
    }
}

/// Resolve a store path the same way the source adapters do: env override
/// first (tilde-expanded), else the default.
fn env_path(var: &str, default: PathBuf) -> PathBuf {
    match std::env::var(var) {
        Ok(raw) if !raw.trim().is_empty() => {
            PathBuf::from(shellexpand::tilde(raw.trim()).into_owned())
        }
        _ => default,
    }
}
