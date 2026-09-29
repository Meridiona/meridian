//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Run `cursor-agent` to summarise a Cursor session (symmetry with claude.rs /
// codex.rs / copilot.rs — each agent's transcripts go to its own CLI, with no
// cross-engine fallback). The transcript goes over stdin (see below), and any
// failure leaves the row pending for a later drain (Failed → retried up to
// primary_attempts, then left pending).
//
// Only `-p --output-format text` is decided here; every safety flag, the
// environment and the skills-free sandbox HOME come from `llm::cursor_cli`, so
// this path and the LLM-provider backend cannot diverge. See that module for
// why each lever exists.
// Persistence probed: `-p` runs write to ~/.cursor/chats/, NOT the vscdb we
// ingest from — so a summariser run cannot re-enter the indexer (the
// SUMMARY_PROMPT_MARKER guard in sources/mod.rs backstops this anyway).
//
// # The whole prompt goes over stdin, not argv (Windows)
//
// It used to be `cursor-agent -p <prompt>` (positional argv). `cursor-agent` resolves to
// `cursor-agent.cmd` on Windows - a batch file, not a native exe - and Rust's std library
// refuses to spawn a `.bat`/`.cmd` target when an argument contains characters it cannot
// safely escape (the CVE-2024-24576 "BatBadBut" fix), notably embedded newlines. The prompt
// (instructions + transcript) is always multi-line, so every real call through this function
// failed to even spawn on Windows with `io::Error { InvalidInput, "batch file arguments are
// invalid" }`. `crate::llm::cursor::CursorBackend` (the hourly worklog pipeline / connectivity
// test) already moved off argv for exactly this reason, "confirmed live" per its own module
// doc - this applies the identical fix here, which had the identical bug all along.
// `cursor-agent -p` as a bare flag (no positional value) reads the whole prompt from stdin.

use super::config::SummariserConfig;
use super::prompts;
use super::{cap_transcript, run_capture, EngineOutput, SummariserError};

/// Argv-embedding budget (chars) — see copilot.rs for the limit analysis
/// (macOS: no per-arg limit, ~1 MiB total ARG_MAX, 256 k single arg verified;
/// Linux would cap a single string at 128 KiB).
const ARG_TRANSCRIPT_CAP: usize = 180_000;

/// Lets `cursor_cli::run_hardened` classify our failures. Mirrors the provider backend's impl
/// for `LlmError`: a `Failed` may name a degradable cause, a `RateLimited` never does - Cursor
/// limits are account-level, so degrading and retrying would spend quota to hit the same wall.
impl crate::llm::cursor_cli::CursorCallError for SummariserError {
    fn degradable_message(&self) -> Option<&str> {
        match self {
            SummariserError::Failed(m) => Some(m),
            SummariserError::RateLimited(_) => None,
        }
    }
}

pub async fn run_cursor_agent(
    stdin_text: &str,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    // Lazy init: on first use of Cursor Agent, check if cursor-agent is available.
    // If not, attempt auto-install and auto-login. Cached after first attempt.
    crate::coding_agent_session_ingest::cursor_agent_init::ensure_ready()
        .await
        .map_err(|e| {
            SummariserError::Failed(format!(
                "cursor-agent init failed (row left pending): {}",
                e
            ))
        })?;
    let prompt = format!(
        "{} Summarise the coding-session transcript below.\n\n{}",
        prompts::summary_instruction(),
        cap_transcript(stdin_text, ARG_TRANSCRIPT_CAP),
    );
    // Safety flags, environment and the skills-free sandbox HOME all come from
    // `llm::cursor_cli` — the single source of truth shared with the LLM-provider backend. This
    // call site used to build its own argv, which meant it summarised UNTRUSTED transcripts with
    // cursor-agent's default full write+shell tool access; routing through the shared helper is
    // what closes that gap and keeps the two sites from drifting again.
    let model = if cfg.cursor_model.is_empty() {
        // Follow the account default here rather than pinning: the summariser predates the
        // provider layer and has its own configured model, so an empty value means "unset".
        "auto"
    } else {
        cfg.cursor_model.as_str()
    };
    // The degradation ladder comes from `cursor_cli` too, not just the flags. Passing the
    // undocumented `--allowed-tools` with no way to retry without it would mean that the day
    // Cursor drops the flag, the provider backend degrades gracefully while EVERY coding-agent
    // summary fails - the same drift this shared module exists to prevent, one level up.
    crate::llm::cursor_cli::run_hardened(model, |safety, env| run_once(&prompt, safety, env, cfg))
        .await
}

/// The `cursor-agent -p` argv - no prompt in here, see the module doc. Split out so the
/// no-newline invariant this function exists to guarantee is directly testable.
fn cursor_agent_args(safety: Vec<String>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into()];
    args.extend(["--output-format".into(), "text".into()]);
    args.extend(safety);
    args
}

/// One `cursor-agent` attempt with the safety argv/environment chosen by the ladder.
async fn run_once(
    prompt: &str,
    safety: Vec<String>,
    env: Vec<(&'static str, String)>,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    let args = cursor_agent_args(safety);

    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let cap = run_capture(
        "cursor-agent",
        &args,
        prompt, // payload goes over stdin - see the module doc
        // See the note in llm/cursor.rs: a cwd outside $HOME so rule discovery cannot regress.
        &crate::llm::cursor_cli::neutral_workspace(),
        cfg.cursor_timeout_s,
        &env_refs,
        crate::llm::cursor_cli::ENV_REMOVE,
    )
    .await?;

    if !cap.success {
        let blob = format!("{}\n{}", cap.stderr, cap.stdout);
        if prompts::looks_rate_limited_cursor(&blob) {
            let msg = prompts::rate_limited_line_cursor(&blob)
                .unwrap_or_else(|| prompts::first_line(&cap.stderr));
            return Err(SummariserError::RateLimited(if msg.is_empty() {
                "cursor-agent usage limit".into()
            } else {
                msg
            }));
        }
        return Err(SummariserError::Failed(format!(
            "cursor-agent exited {:?}: {}",
            cap.code,
            prompts::first_line(&cap.stderr)
        )));
    }

    let text = cap.stdout.trim();
    if text.is_empty() {
        return Err(SummariserError::Failed(
            "cursor-agent produced no output".into(),
        ));
    }

    let summary = prompts::extract_summary(text);
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "cursor-agent output had no usable summary".into(),
        ));
    }
    Ok(EngineOutput { summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The premise the whole fix rests on: the instructions are sourced from a Markdown
    /// rules file and are always multi-line. If `SKILL.md` ever became single-line, this
    /// fix would no longer be guarding against anything real - this pins that it still is.
    #[test]
    fn the_instructions_prompt_is_multi_line() {
        assert!(prompts::summary_instruction().contains('\n'));
    }

    /// The regression this fix exists to close: `cursor-agent -p` argv must never carry the
    /// prompt (or anything else newline-bearing) - that is exactly what makes Rust's std
    /// refuse to spawn `cursor-agent.cmd` on Windows. A future edit that reintroduces the
    /// prompt into `args` fails this test.
    #[test]
    fn cursor_agent_args_never_contains_a_newline() {
        let args = cursor_agent_args(vec!["--allowed-tools".into(), String::new()]);
        for arg in &args {
            assert!(!arg.contains('\n'), "argv entry carries a newline: {arg:?}");
        }
    }

    /// `-p` must be a bare flag - a positional value right after it would force the prompt
    /// back through argv.
    #[test]
    fn cursor_agent_args_has_no_positional_prompt() {
        let args = cursor_agent_args(vec![]);
        assert_eq!(args[0], "-p");
        assert_eq!(
            args[1], "--output-format",
            "the second argv entry must be a flag, not a prompt"
        );
    }

    #[test]
    fn cursor_agent_args_carries_the_safety_flags() {
        let args = cursor_agent_args(vec!["--force".into()]);
        assert!(args.contains(&"--force".to_string()));
    }
}
