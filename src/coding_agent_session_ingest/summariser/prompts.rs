//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared summary contract — the rules, the schema, and limit detection. Lives in
// one place so the three engines stay consistent: Claude loads the
// `session-summary` skill (same rules, in SKILL.md); Codex gets
// SUMMARY_INSTRUCTION as its prompt; MLX gets SUMMARY_RULES as its system
// message. All three target SUMMARY_SCHEMA. Port of
// the former Python summariser/prompts.py.

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
pub const SUMMARY_RULES: &str =
    "You summarise ONE work-burst of a developer's coding-agent session for a \
Jira work-log. The transcript (provided on stdin) is timestamped as \
`[<ISO ts>] [role] <message>`. Write a factual prose summary of 10-40 \
sentences: name the files edited, commands run, errors hit, decisions \
made, tests/validations performed, and any rework or blockers (an approach \
abandoned, a failed build/test, something deleted and rebuilt). State ONLY \
what is in the transcript — never invent files, tickets, commands, or \
outcomes. No preamble, no markdown headings, no bullet lists — just clear \
paragraphs. If an 'EARLIER IN THIS SESSION' section is present, do not \
repeat it; summarise only this burst.";

/// For schema-enforced engines (Codex `--output-schema`): ask for JSON.
pub fn summary_instruction() -> String {
    format!(
        "{} Return JSON matching the schema: `summary` (the prose) and \
         `blockers` (a list of distinct blockers / failures / rework, possibly empty).",
        SUMMARY_RULES
    )
}

/// Structured-output contract, serialized for `claude --json-schema` /
/// `codex --output-schema`. `summary` is the prose we store; `blockers` forces
/// the model to surface rework/failures (the bit weaker models drop).
pub fn summary_schema_json() -> String {
    json!({
        "type": "object",
        "properties": {
            "summary":  {"type": "string", "minLength": 1},
            "blockers": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["summary"],
    })
    .to_string()
}

/// Substrings that mark a subscription usage/rate limit in CLI stderr/output —
/// for Claude (`claude -p`) and Codex (`codex exec`) alike.
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

/// Pull (summary, blockers) from an engine's final message. Schema-less
/// engines (codex prose mode, copilot, cursor-agent) may return the JSON
/// bare, ```fenced```, or wrapped in prose; if no `summary` key is found the
/// whole text is treated as the summary.
pub fn extract_summary(text: &str) -> (String, Vec<String>) {
    if let Some(obj) = try_json_object(text) {
        if let Some(summary) = obj.get("summary").and_then(serde_json::Value::as_str) {
            let blockers = obj
                .get("blockers")
                .and_then(serde_json::Value::as_array)
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
        let (s, b) =
            extract_summary(r#"{"summary": "Fixed the login bug.", "blockers": ["CI was red"]}"#);
        assert_eq!(s, "Fixed the login bug.");
        assert_eq!(b, vec!["CI was red"]);
    }

    #[test]
    fn extract_fenced_json() {
        let (s, b) = extract_summary("```json\n{\"summary\": \"Did the thing.\"}\n```");
        assert_eq!(s, "Did the thing.");
        assert!(b.is_empty());
    }

    #[test]
    fn extract_prose_falls_back_to_full_text() {
        let (s, b) = extract_summary("The developer fixed auth.ts and reran the tests.");
        assert_eq!(s, "The developer fixed auth.ts and reran the tests.");
        assert!(b.is_empty());
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
}
