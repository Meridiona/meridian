//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared summary contract — the rules, the schema, and limit detection. Lives in
// one place so every agent CLI stays consistent: Claude loads the
// `session-summary` skill (same rules, in SKILL.md); Codex/Copilot/cursor-agent
// get SUMMARY_INSTRUCTION as their prompt. All target SUMMARY_SCHEMA.

use serde_json::json;

/// Fingerprint of the summariser's own prompt. The source sweep refuses to
/// ingest any conversation whose first user message carries this marker, so a
/// summariser engine that PERSISTS its sessions (cursor-agent's behaviour is
/// unprobed; a future engine may too) can never feed its own runs back into
/// app_sessions — the loop is cut at ingest regardless of engine flags.
/// A unit test pins this to SUMMARY_RULES so the two can't drift apart.
pub const SUMMARY_PROMPT_MARKER: &str =
    "You summarise ONE work-burst of a developer's coding-agent session";

/// The shared rules, without an output-format clause (engines append their own).
/// Single source of truth lives in assets/skills/coding-agent/session-summary/SKILL.md —
/// edit that file, rebuild, and every agent CLI picks up the change.
pub const SUMMARY_RULES: &str =
    include_str!("../../../assets/skills/coding-agent/session-summary/SKILL.md");

/// For schema-enforced engines (Codex `--output-schema`): ask for JSON.
pub fn summary_instruction() -> String {
    format!(
        "{} Return JSON matching the schema: `summary` (the prose).",
        SUMMARY_RULES
    )
}

/// Structured-output contract, serialized for `claude --json-schema` /
/// `codex --output-schema`. `summary` is the prose we store.
pub fn summary_schema_json() -> String {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string", "minLength": 1},
        },
        "required": ["summary"],
    })
    .to_string()
}

/// Substrings that mark a subscription usage/rate limit in CLI stderr/output —
/// for Claude (`claude -p`), Codex (`codex exec`), and `cursor-agent` alike.
///
/// `resource_exhausted` / `retriableerror` are Cursor's own vocabulary, not a
/// generic phrase like the rest of this list — its backend is gRPC-flavoured and
/// reports a free-plan quota wall as `RetriableError: [resource_exhausted] Error`,
/// wrapped in cursor-agent's own connection-retry banner ("Connection lost,
/// reconnecting..." x3) that reads exactly like a network blip. Without this
/// marker a resource-exhausted account never returns `LlmError::RateLimited`
/// (`degradable_message` -> `None`, no further retry) - it falls through as a
/// plain `Failed`, which both `resolver::complete_inner` AND
/// `cursor_cli::run_hardened`'s transient lever then retry, burning more calls
/// against an already-exhausted quota and taking 20-30s+ per attempt to fail
/// again the same way. Observed live on a free-plan account 2026-08-19.
const RATE_LIMIT_MARKERS: &[&str] = &[
    "usage limit",
    "rate limit",
    "rate_limit",
    "429",
    "overloaded",
    "exceeded your",
    "quota",
    "resets at",
    "try again at",
    "upgrade to plus",
    "resource_exhausted",
    "resource exhausted",
];

pub fn looks_rate_limited(text: &str) -> bool {
    let low = text.to_lowercase();
    RATE_LIMIT_MARKERS.iter().any(|m| low.contains(m))
}

/// The first line that actually matched a rate-limit marker, capped to 200
/// chars. Engines' stderr often opens with informational banners ("Reading
/// additional input from stdin..."), so quoting `first_line` puts the wrong
/// text in the fallback WARN log — quote the matching line instead.
pub fn rate_limited_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && looks_rate_limited(line))
        .map(|line| line.chars().take(200).collect())
}

/// First non-empty line, capped to 200 chars (for compact error messages).
pub fn first_line(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            return line.chars().take(200).collect();
        }
    }
    String::new()
}

/// Pull summary from an engine's final message. Schema-less engines (codex
/// prose mode, copilot, cursor-agent) may return the JSON bare, ```fenced```,
/// or wrapped in prose; if no `summary` key is found the whole text is used.
pub fn extract_summary(text: &str) -> String {
    if let Some(obj) = try_json_object(text) {
        if let Some(summary) = obj.get("summary").and_then(serde_json::Value::as_str) {
            return summary.trim().to_string();
        }
    }
    text.trim().to_string()
}

fn try_json_object(text: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        return Some(v);
    }
    // Tolerate a JSON object embedded in fences or surrounding prose.
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if start < end {
        serde_json::from_str::<serde_json::Value>(&text[start..=end]).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bare_json() {
        let s = extract_summary(r#"{"summary": "Fixed the login bug."}"#);
        assert_eq!(s, "Fixed the login bug.");
    }

    #[test]
    fn extract_fenced_json() {
        let s = extract_summary("```json\n{\"summary\": \"Did the thing.\"}\n```");
        assert_eq!(s, "Did the thing.");
    }

    #[test]
    fn extract_prose_falls_back_to_full_text() {
        let s = extract_summary("The developer fixed auth.ts and reran the tests.");
        assert_eq!(s, "The developer fixed auth.ts and reran the tests.");
    }

    /// The ingest-side circular-dependency guard matches on this marker; if
    /// SUMMARY_RULES is reworded the marker (and this pin) must move with it.
    #[test]
    fn summary_prompt_marker_pins_summary_rules() {
        assert!(SUMMARY_RULES.starts_with(SUMMARY_PROMPT_MARKER));
        assert!(summary_instruction().starts_with(SUMMARY_PROMPT_MARKER));
    }

    /// Regression: codex stderr opens with an informational banner; the
    /// fallback WARN must quote the line that matched the rate-limit marker,
    /// not the banner (observed live 2026-06-06, row 32162).
    #[test]
    fn rate_limited_line_skips_informational_banner() {
        let stderr = "Reading additional input from stdin...\n\
                      ERROR: You've hit your usage limit. Upgrade to Plus to continue using Codex, or try again at Jul 5th, 2026 1:16 PM.";
        assert!(looks_rate_limited(stderr));
        let line = rate_limited_line(stderr).unwrap();
        assert!(line.starts_with("ERROR: You've hit your usage limit"));
        assert!(rate_limited_line("all good, no errors here").is_none());
    }

    /// Pinned regression: the exact message a free-plan Cursor account hit live
    /// (2026-08-19) - `cursor-agent`'s own retry banner wrapping a gRPC
    /// `resource_exhausted` quota wall. Without this marker the message reads as
    /// a plain `Failed` and gets retried against an already-exhausted quota
    /// instead of surfacing the soft "rate limited, catching up on its own" state.
    #[test]
    fn cursor_resource_exhausted_is_recognised_as_a_rate_limit() {
        let blob = "Connection lost, reconnecting to https://agentn.global.api5.cursor.sh \
                     (attempt 1)...\nRetry attempt 1...\nConnection lost, reconnecting to \
                     https://agentn.global.api5.cursor.sh (attempt 2)...\nRetry attempt 2...\n\
                     RetriableError: [resource_exhausted] Error";
        assert!(looks_rate_limited(blob));
        let line = rate_limited_line(blob).unwrap();
        assert!(line.contains("resource_exhausted"));
    }

    /// A bare "connection lost" with no exhaustion signal must NOT be read as a
    /// rate limit - that would silently stop `run_hardened`'s transient retry
    /// from ever firing for a genuine one-off network blip.
    #[test]
    fn a_plain_connection_drop_is_not_mistaken_for_a_rate_limit() {
        assert!(!looks_rate_limited(
            "Connection lost, reconnecting to https://agentn.global.api5.cursor.sh (attempt 1)...."
        ));
    }
}
