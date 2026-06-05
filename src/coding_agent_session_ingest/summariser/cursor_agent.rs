// meridian — normalises screenpipe activity into structured app sessions
//
// Run `cursor-agent` to summarise a Cursor session (symmetry with claude.rs /
// codex.rs / copilot.rs — each agent's transcripts go to its own CLI, MLX as
// the shared fallback). Flags follow Cursor's public CLI docs (`-p` print
// mode + `--output-format text`); the binary was NOT present on the dev
// machine to probe, so this engine is deliberately conservative: the
// transcript rides in the prompt argument (the one channel guaranteed to
// reach the model), and a missing binary or rejected flag degrades cleanly —
// Failed → primary_attempts → MLX. If `cursor-agent` is installed later the
// engine starts working without a config change.

use super::config::SummariserConfig;
use super::prompts;
use super::{cap_transcript, run_capture, EngineOutput, SummariserError};

/// Argv-embedding budget (chars) — same rationale as copilot.rs: stay clear
/// of the per-arg exec limit (~256 KiB on macOS).
const ARG_TRANSCRIPT_CAP: usize = 180_000;

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
                "cursor-agent init failed (falling back to MLX): {}",
                e
            ))
        })?;
    let prompt = format!(
        "{} Summarise the coding-session transcript below.\n\n{}",
        prompts::summary_instruction(),
        cap_transcript(stdin_text, ARG_TRANSCRIPT_CAP),
    );
    let mut args: Vec<String> = vec!["-p".into(), prompt, "--output-format".into(), "text".into()];
    if !cfg.cursor_model.is_empty() {
        args.push("--model".into());
        args.push(cfg.cursor_model.clone());
    }

    let cap = run_capture(
        "cursor-agent",
        &args,
        "", // transcript is embedded in the prompt (stdin support unprobed)
        &cfg.meridian_home,
        cfg.cursor_timeout_s,
        &[("MERIDIAN_SUMMARISER", "1")],
        &[],
    )
    .await?;

    if !cap.success {
        let blob = format!("{}\n{}", cap.stderr, cap.stdout);
        if prompts::looks_rate_limited(&blob) {
            let msg = prompts::first_line(&cap.stderr);
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

    let (summary, blockers) = prompts::extract_summary(text);
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "cursor-agent output had no usable summary".into(),
        ));
    }
    Ok(EngineOutput { summary, blockers })
}
