// meridian — normalises screenpipe activity into structured app sessions
//
// Shared summary contract — the rules, the schema, and limit detection. Lives in
// one place so the three engines stay consistent: Claude loads the
// `session-summary` skill (same rules, in SKILL.md); Codex gets
// SUMMARY_INSTRUCTION as its prompt; MLX gets SUMMARY_RULES as its system
// message. All three target SUMMARY_SCHEMA. Port of
// the former Python summariser/prompts.py.

use serde_json::json;

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
}
