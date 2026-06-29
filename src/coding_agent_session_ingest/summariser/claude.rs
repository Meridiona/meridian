//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Run `claude -p` with the session-summary skill + structured output. Returns
// the validated {summary}, or RateLimited (→ MLX fallback) /
// Failed (→ retry). Port of the former Python summariser/claude_runner.py.
//
// Auth: the user's Claude subscription. We drop ANTHROPIC_API_KEY from the child
// env so a stray key can't silently switch to metered API billing, and set
// MERIDIAN_SUMMARISER=1 so the indexer hook ignores the throwaway session this
// spawns. `--no-session-persistence` means no JSONL is written for it either.
// NOTE: the inherited env must carry HOME/PATH/USER/LOGNAME for the login
// keychain to unlock (see the auth spike) — the daemon's launchd plist owns that.

use serde_json::Value;

use super::config::SummariserConfig;
use super::prompts;
use super::{run_capture, EngineOutput, SummariserError};

pub async fn run_claude(
    stdin_text: &str,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    let prompt = format!(
        "{} Summarise the coding-session transcript provided on stdin.",
        prompts::SUMMARY_RULES
    );
    let args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--output-format".into(),
        "json".into(),
        "--json-schema".into(),
        prompts::summary_schema_json(),
        "--model".into(),
        cfg.claude_model.clone(),
        "--no-session-persistence".into(),
        "--strict-mcp-config".into(), // drop MCP overhead; keeps skills working
    ];

    let cap = run_capture(
        "claude",
        &args,
        stdin_text,
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
