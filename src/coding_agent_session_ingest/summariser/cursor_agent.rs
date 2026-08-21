//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Run `cursor-agent` to summarise a Cursor session (symmetry with claude.rs /
// codex.rs / copilot.rs — each agent's transcripts go to its own CLI, with no
// cross-engine fallback). The transcript rides in the prompt argument, and any
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

/// One `cursor-agent` attempt with the safety argv/environment chosen by the ladder.
async fn run_once(
    prompt: &str,
    safety: Vec<String>,
    env: Vec<(&'static str, String)>,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    let mut args: Vec<String> = vec!["-p".into(), prompt.to_string()];
    args.extend(["--output-format".into(), "text".into()]);
    args.extend(safety);

    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let cap = run_capture(
        "cursor-agent",
        &args,
        "", // transcript is embedded in the prompt (stdin support unprobed)
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
