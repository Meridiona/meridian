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
//   embedded in the prompt argument — re-capped well under macOS's per-arg
//   exec limit (~256 KiB).
// * No `--json-schema`, so the JSON contract rides in the prompt and
//   `extract` tolerates fenced/prose-wrapped objects (codex pattern).

use serde_json::Value;

use super::config::SummariserConfig;
use super::prompts;
use super::{cap_transcript, run_capture, EngineOutput, SummariserError};

/// Argv-embedding budget (chars). The transcript rides in the `-p` argument,
/// so it must stay clear of the per-arg exec limit; prompt scaffolding and the
/// prior-burst section share the same string.
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
            let msg = prompts::first_line(&cap.stderr);
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

    let (summary, blockers) = extract(text);
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "copilot output had no usable summary".into(),
        ));
    }
    Ok(EngineOutput { summary, blockers })
}

/// Pull (summary, blockers) from copilot's response. Without schema
/// enforcement the JSON may arrive bare, fenced, or wrapped in prose; fall
/// back to treating the whole text as the summary (same policy as codex).
fn extract(text: &str) -> (String, Vec<String>) {
    if let Some(obj) = try_json_object(text) {
        if let Some(summary) = obj.get("summary").and_then(Value::as_str) {
            let blockers = obj
                .get("blockers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|b| b.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            return (summary.trim().to_string(), blockers);
        }
    }
    (text.trim().to_string(), Vec::new())
}

fn try_json_object(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if start < end {
        serde_json::from_str::<Value>(&text[start..=end]).ok()
    } else {
        None
    }
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bare_json() {
        let (s, b) = extract(r#"{"summary": "Fixed the login bug.", "blockers": ["CI was red"]}"#);
        assert_eq!(s, "Fixed the login bug.");
        assert_eq!(b, vec!["CI was red"]);
    }

    #[test]
    fn extract_fenced_json() {
        let (s, b) = extract("```json\n{\"summary\": \"Did the thing.\"}\n```");
        assert_eq!(s, "Did the thing.");
        assert!(b.is_empty());
    }

    #[test]
    fn extract_prose_falls_back_to_full_text() {
        let (s, b) = extract("The developer fixed auth.ts and reran the tests.");
        assert_eq!(s, "The developer fixed auth.ts and reran the tests.");
        assert!(b.is_empty());
    }
}
