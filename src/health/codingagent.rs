// meridian — normalises screenpipe activity into structured app sessions
//
// Coding-agent ingest health: whether the Claude Code / Codex CLIs and their
// JSONL session dirs are present (the inputs that gate the indexer + summariser
// into a rich transcript path), and a note about the Cursor blind spot.

use crate::config::Config;
use crate::health::platform::which;
use crate::health::Check;
use std::path::PathBuf;

pub fn checks(_cfg: &Config) -> Vec<Check> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut out = vec![
        cli("claude", "Claude Code"),
        cli("codex", "Codex"),
        dir(&home.join(".claude/projects"), "claude sessions dir"),
        dir(&home.join(".codex/sessions"), "codex sessions dir"),
    ];

    // The Cursor gap: Cursor stores agent sessions outside the ingested paths, so
    // its work is only captured via OCR/a11y, never the rich transcript path.
    let cursor_present =
        which("cursor").is_some() || home.join("Library/Application Support/Cursor").is_dir();
    if cursor_present {
        out.push(Check::info(
            "cursor",
            "L1",
            "detected — captured via OCR/a11y only (no transcript ingest)",
        ));
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
