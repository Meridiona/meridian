// meridian — normalises screenpipe activity into structured app sessions
//
// Shared summary contract — the rules, the schema, and limit detection. Lives in
// one place so the three engines stay consistent: Claude loads the
// `session-summary` skill (same rules, in SKILL.md); Codex gets
// SUMMARY_INSTRUCTION as its prompt; MLX gets SUMMARY_RULES as its system
// message. All three target SUMMARY_SCHEMA. Port of
// services/coding_agent_summariser/prompts.py.

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
