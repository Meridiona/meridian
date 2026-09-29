//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Run `claude -p` with the summary rules embedded inline + structured output.
// (It does NOT invoke a `/session-summary` slash-skill — that was replaced by
// the inline SUMMARY_RULES prompt below, and `SUMMARISER_SKILL` is deprecated.
// This comment claiming otherwise is what kept a stale doctor check alive.)
// Returns
// the validated {summary}, or RateLimited / Failed — both leave the row pending
// for a later drain (no cross-engine fallback).
//
// Auth: the user's Claude subscription. We drop ANTHROPIC_API_KEY from the child
// env so a stray key can't silently switch to metered API billing, and set
// MERIDIAN_SUMMARISER=1 so the indexer hook ignores the throwaway session this
// spawns. `--no-session-persistence` means no JSONL is written for it either.
// NOTE: the inherited env must carry HOME/PATH/USER/LOGNAME for the login
// keychain to unlock (see the auth spike) — the daemon's launchd plist owns that.
//
// # The whole prompt goes over stdin, not argv (Windows)
//
// `claude` resolves to `claude.cmd` on Windows for an npm install (`npm i -g
// @anthropic-ai/claude-code`, the exact command `install_command()` runs for this
// provider) - an npm-generated batch file, not a native exe - and Rust's std library
// refuses to spawn a `.bat`/`.cmd` target when an argument contains characters it
// cannot safely escape (the CVE-2024-24576 "BatBadBut" fix), notably embedded
// newlines. SUMMARY_RULES is sourced from a Markdown rules file, so it is always
// multi-line - meaning every real call through this function failed to even spawn
// on Windows with `io::Error { InvalidInput, "batch file arguments are invalid" }`.
// `crate::llm::claude::ClaudeBackend` (the hourly worklog pipeline / connectivity
// test) already moved off argv for exactly this reason - this applies the same fix
// here, which had the identical bug all along. `-p` as a bare flag (no positional
// value) reads the whole prompt from stdin instead.

use serde_json::Value;

use super::config::SummariserConfig;
use super::prompts;
use super::{run_capture, EngineOutput, SummariserError};

/// The instructions + the session transcript, combined into the one blob `claude -p`
/// reads from stdin now that no positional prompt is passed - see the module doc.
fn claude_stdin_payload(stdin_text: &str) -> String {
    let instructions = format!(
        "{} Summarise the coding-session transcript provided on stdin.",
        prompts::SUMMARY_RULES
    );
    format!("{instructions}\n\n{stdin_text}")
}

/// The `claude -p` argv - no prompt in here, see the module doc. Split out so the
/// no-newline invariant this function exists to guarantee is directly testable.
fn claude_args(model: &str) -> Vec<String> {
    vec![
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--json-schema".into(),
        prompts::summary_schema_json(),
        "--model".into(),
        model.to_string(),
        "--no-session-persistence".into(),
        "--strict-mcp-config".into(), // drop MCP overhead; keeps skills working
    ]
}

pub async fn run_claude(
    stdin_text: &str,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    // SUMMARISER_SKILL is deprecated: the prompt is now embedded inline via
    // SUMMARY_RULES (no slash-skill invocation). Warn once so operators know
    // the env var has no effect and can remove it from their config.
    if cfg.skill_name != "session-summary" {
        tracing::warn!(
            skill_name = %cfg.skill_name,
            "SUMMARISER_SKILL is deprecated — prompt is now embedded inline; \
             the env var has no effect and can be removed"
        );
    }
    let args = claude_args(&cfg.claude_model);

    let cap = run_capture(
        "claude",
        &args,
        &claude_stdin_payload(stdin_text),
        &cfg.meridian_home,
        cfg.claude_timeout_s,
        &[("MERIDIAN_SUMMARISER", "1")],
        &["ANTHROPIC_API_KEY"],
    )
    .await?;

    if !cap.success {
        let blob = format!("{}\n{}", cap.stderr, cap.stdout);
        if prompts::looks_rate_limited(&blob) {
            let msg = prompts::rate_limited_line(&blob)
                .unwrap_or_else(|| prompts::first_line(&cap.stderr));
            return Err(SummariserError::RateLimited(if msg.is_empty() {
                "rate/usage limit".into()
            } else {
                msg
            }));
        }
        let detail = {
            let s = prompts::first_line(&cap.stderr);
            if s.is_empty() {
                prompts::first_line(&cap.stdout)
            } else {
                s
            }
        };
        return Err(SummariserError::Failed(format!(
            "claude exited {:?}: {}",
            cap.code, detail
        )));
    }

    let payload: Value = serde_json::from_str(&cap.stdout).map_err(|e| {
        let head: String = cap.stdout.chars().take(200).collect();
        SummariserError::Failed(format!("claude output not JSON ({e}): {head:?}"))
    })?;

    // Even on exit 0 the envelope can report an error (e.g. a mid-run limit).
    let is_error = payload
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let subtype = payload.get("subtype").and_then(Value::as_str);
    if is_error || !matches!(subtype, None | Some("success")) {
        let detail: String = payload
            .get("result")
            .and_then(Value::as_str)
            .or(subtype)
            .unwrap_or("error")
            .chars()
            .take(200)
            .collect();
        if prompts::looks_rate_limited(&detail) {
            return Err(SummariserError::RateLimited(detail));
        }
        return Err(SummariserError::Failed(format!(
            "claude result error: {detail}"
        )));
    }

    let structured = payload.get("structured_output");
    let summary = structured
        .and_then(|s| s.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "claude returned no usable structured summary".into(),
        ));
    }
    Ok(EngineOutput { summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The premise the whole fix rests on: SUMMARY_RULES is sourced from a Markdown
    /// rules file and is always multi-line. If `SKILL.md` ever became single-line, this
    /// fix would no longer be guarding against anything real - this pins that it still is.
    #[test]
    fn summary_rules_is_multi_line() {
        assert!(prompts::SUMMARY_RULES.contains('\n'));
    }

    /// The regression this fix exists to close: `claude -p` argv must never carry the
    /// instructions prompt (or anything else newline-bearing) - that is exactly what
    /// makes Rust's std refuse to spawn `claude.cmd` on Windows. A future edit that
    /// reintroduces the prompt into `args` fails this test.
    #[test]
    fn claude_args_never_contains_a_newline() {
        let args = claude_args("claude-opus-5");
        for arg in &args {
            assert!(!arg.contains('\n'), "argv entry carries a newline: {arg:?}");
        }
    }

    /// `claude -p` must be a bare flag - a positional value right after it would force
    /// the prompt back through argv.
    #[test]
    fn claude_args_has_no_positional_prompt() {
        let args = claude_args("claude-opus-5");
        assert_eq!(args[0], "-p");
        assert_eq!(
            args[1], "--output-format",
            "the second argv entry must be a flag, not a prompt"
        );
    }

    #[test]
    fn claude_args_carries_the_model() {
        let args = claude_args("claude-opus-5");
        let i = args
            .iter()
            .position(|a| a == "--model")
            .expect("--model flag present");
        assert_eq!(args[i + 1], "claude-opus-5");
    }

    #[test]
    fn claude_stdin_payload_carries_both_the_instructions_and_the_transcript() {
        let payload = claude_stdin_payload("the transcript");
        assert!(payload.contains("the transcript"));
        assert!(payload.starts_with(&prompts::SUMMARY_RULES[..40]));
    }
}
