// meridian — normalises screenpipe activity into structured app sessions
//
// Run `copilot -p` to summarise a GitHub Copilot session (symmetry with
// claude.rs / codex.rs — each agent's transcripts are summarised by its own
// CLI on the user's subscription, MLX as the shared fallback). Non-interactive
// and side-effect-free: `-p` + `-s` (response only), no tool permissions
// granted, custom instructions disabled so a stray AGENTS.md can't leak into
// the summary.
//
// Two divergences from the other engines, both probed against copilot 1.0.49:
// * `-p` does NOT forward piped stdin to the model (the model literally
//   replied "Waiting for transcript input on stdin..."), so the transcript is
//   embedded in the prompt argument — see ARG_TRANSCRIPT_CAP for the limit
//   analysis.
// * No `--json-schema`, so the JSON contract rides in the prompt and
//   `extract` tolerates fenced/prose-wrapped objects (codex pattern).

use super::config::SummariserConfig;
use super::prompts;
use super::{cap_transcript, run_capture, EngineOutput, SummariserError};

/// Argv-embedding budget (chars). The transcript rides in the `-p` argument.
/// macOS has no per-argument string limit — only the ~1 MiB total ARG_MAX
/// (argv + env combined); a 256 000-char single argument execs fine (verified
/// empirically on macOS 15, 2026-06-06). 180 k + prompt scaffolding leaves
/// ample headroom. NOTE: Linux caps a single string at 128 KiB
/// (MAX_ARG_STRLEN) — lower this cap if the daemon is ever ported.
const ARG_TRANSCRIPT_CAP: usize = 180_000;

pub async fn run_copilot(
    stdin_text: &str,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    let prompt = format!(
        "{} Summarise the coding-session transcript below.\n\n{}",
        prompts::summary_instruction(),
        cap_transcript(stdin_text, ARG_TRANSCRIPT_CAP),
    );
    let args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "-s".into(), // response only — no stats banner around the JSON
        "--no-color".into(),
        "--log-level".into(),
        "none".into(),
        "--no-custom-instructions".into(),
    ];

    let cap = run_capture(
        "copilot",
        &args,
        "", // stdin is ignored by `copilot -p` — transcript is in the prompt
        &cfg.meridian_home,
        cfg.copilot_timeout_s,
        &[("MERIDIAN_SUMMARISER", "1")],
        &[],
    )
    .await?;

    if !cap.success {
        let blob = format!("{}\n{}", cap.stderr, cap.stdout);
        if prompts::looks_rate_limited(&blob) {
            let msg = prompts::rate_limited_line(&blob)
                .unwrap_or_else(|| prompts::first_line(&cap.stderr));
            return Err(SummariserError::RateLimited(if msg.is_empty() {
                "copilot usage limit".into()
            } else {
                msg
            }));
        }
        return Err(SummariserError::Failed(format!(
            "copilot exited {:?}: {}",
            cap.code,
            prompts::first_line(&cap.stderr)
        )));
    }

    let text = cap.stdout.trim();
    if text.is_empty() {
        return Err(SummariserError::Failed("copilot produced no output".into()));
    }

    let (summary, blockers) = prompts::extract_summary(text);
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "copilot output had no usable summary".into(),
        ));
    }
    Ok(EngineOutput { summary, blockers })
}
