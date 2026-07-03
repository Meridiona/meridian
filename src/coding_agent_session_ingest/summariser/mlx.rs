//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Fallback summariser — the local MLX server's schema-constrained /summarise
// endpoint (the ONLY remaining Python hop). Used when claude/codex are
// rate-limited or fail. The local model is a reasoner, so we (1) feed it only
// the TAIL of the transcript (most recent activity / outcome) and (2) keep a
// cheap reasoning-leak filter as defence even though the endpoint's outlines FSM
// already forces the {summary} shape. Port of
// the former Python summariser/mlx_fallback.py.

use std::time::Duration;

use serde_json::{json, Value};

use super::config::SummariserConfig;
use super::prompts;
use super::SummariserError;

pub async fn run_mlx(stdin_text: &str, cfg: &SummariserConfig) -> Result<String, SummariserError> {
    // Single global LLM gate: one model call in flight across all stages.
    let _llm_permit = crate::llm_gate::acquire().await;

    let url = format!("http://{}:{}/summarise", cfg.mlx_host, cfg.mlx_port);
    let body = json!({
        "transcript": tail_cap(stdin_text, cfg),  // MLX-only: tail of the session
        "system": prompts::SUMMARY_RULES,
        "max_tokens": cfg.mlx_max_tokens,
        "temperature": 0.2,
        // Non-thinking fast path: the fallback runs under a tight 30s timeout, so
        // we skip the <think> block entirely (no reasoning budget to burn). The
        // outlines FSM still forces the {summary} shape.
        "enable_thinking": false,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.mlx_timeout_s))
        .build()
        .map_err(|e| SummariserError::Failed(format!("MLX client: {e}")))?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| SummariserError::Failed(format!("MLX fallback unreachable: {e}")))?;
    let payload: Value = resp
        .json()
        .await
        .map_err(|e| SummariserError::Failed(format!("MLX fallback bad JSON: {e}")))?;

    let raw = payload
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let text = clean(raw);
    if text.is_empty() {
        return Err(SummariserError::Failed(
            "MLX fallback returned empty summary".into(),
        ));
    }
    if looks_like_reasoning(&text) {
        // Local model leaked its chain-of-thought instead of a clean summary.
        // Reject rather than store garbage — the row stays NULL for a later
        // claude/codex pass.
        return Err(SummariserError::Failed(
            "MLX output looks like leaked reasoning — rejected".into(),
        ));
    }
    Ok(text)
}

/// Keep only the TAIL (~mlx_input_max_tokens) of the transcript for MLX. The
/// bottom of a session holds the most recent activity / outcome. Char-counted.
fn tail_cap(text: &str, cfg: &SummariserConfig) -> String {
    let max_chars = cfg.mlx_input_max_tokens * cfg.mlx_chars_per_token;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let tail: String = chars[chars.len() - max_chars..].iter().collect();
    format!("…[earlier session truncated — most recent activity below]…\n\n{tail}")
}

const REASONING_MARKERS: &[&str] = &[
    "thinking process",
    "analyze the request",
    "**analyze",
    "constraint:",
    "decision:",
    "re-evaluation",
    "let me think",
    "i must follow",
    "the transcript is",
];

fn looks_like_reasoning(text: &str) -> bool {
    let low = text.to_lowercase();
    let lstripped = low.trim_start();
    if lstripped.starts_with("thinking process")
        || lstripped.starts_with("1.")
        || lstripped.starts_with("1)")
        || lstripped.starts_with("**analyze")
    {
        return true;
    }
    REASONING_MARKERS
        .iter()
        .filter(|m| low.contains(*m))
        .count()
        >= 2
}

/// Defensively strip a reasoner's leakage to leave just the prose: drop
/// `<think>…</think>`, prefer an embedded `{"summary": …}`, then drop a leading
/// reasoning preamble. String-based (no regex dependency).
fn clean(text: &str) -> String {
    let mut t = strip_think_tags(text).trim().to_string();

    // If it emitted JSON anyway, take the summary field.
    if let (Some(start), Some(end)) = (t.find('{'), t.rfind('}')) {
        if start < end {
            if let Ok(obj) = serde_json::from_str::<Value>(&t[start..=end]) {
                if let Some(s) = obj.get("summary").and_then(Value::as_str) {
                    if !s.trim().is_empty() {
                        return s.trim().to_string();
                    }
                }
            }
        }
    }

    // Drop a leading reasoning preamble up to the first blank line.
    let low = t.to_lowercase();
    let lead = low.trim_start();
    const PREAMBLES: &[&str] = &["thinking process", "reasoning", "analysis", "let me think"];
    if PREAMBLES.iter().any(|p| lead.starts_with(p)) {
        if let Some(idx) = t.find("\n\n") {
            t = t[idx + 2..].trim().to_string();
        }
    }
    t.trim().to_string()
}

/// Remove `<think>…</think>` blocks and stray think tags, case-insensitively.
fn strip_think_tags(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let bytes_open = "<think>";
    let bytes_close = "</think>";
    while i < text.len() {
        if lower[i..].starts_with(bytes_open) {
            // Skip to the matching close tag (or end of string).
            match lower[i..].find(bytes_close) {
                Some(rel) => i += rel + bytes_close.len(),
                None => i = text.len(),
            }
        } else if lower[i..].starts_with(bytes_close) {
            i += bytes_close.len(); // stray closing tag
        } else {
            // Copy one char (respecting UTF-8 boundaries).
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}
