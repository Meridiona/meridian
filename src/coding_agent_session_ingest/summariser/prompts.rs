//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared summary contract — the rules, the schema, and limit detection. Lives in
// one place so every agent CLI stays consistent: Claude loads the
// `session-summary` skill (same rules, in SKILL.md); Codex/Copilot/cursor-agent
// get SUMMARY_INSTRUCTION as their prompt. All target SUMMARY_SCHEMA.

use serde_json::{json, Value};

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

/// Structured-output contract, as a value - `codex --output-schema` needs the raw
/// `Value` so `crate::llm::schema::strictify` can be applied to it before writing.
/// `summary` is the prose we store.
pub fn summary_schema_value() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string", "minLength": 1},
        },
        "required": ["summary"],
    })
}

/// Structured-output contract, serialized for `claude --json-schema`. Claude's schema
/// validation doesn't need OpenAI's strict dialect (unlike codex - see
/// `crate::llm::claude::ClaudeBackend`, which also passes its schema through raw), so
/// this stays the plain, un-strictified form.
pub fn summary_schema_json() -> String {
    summary_schema_value().to_string()
}

/// Substrings that mark a subscription usage/rate limit in CLI stderr/output —
/// generic across every engine (Claude `claude -p`, Codex `codex exec`,
/// Copilot, and `cursor-agent`).
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

/// `cursor-agent`-ONLY vocabulary — deliberately NOT in [`RATE_LIMIT_MARKERS`].
///
/// `resource_exhausted` is not a generic phrase like the rest of that list: it is
/// gRPC's own `RESOURCE_EXHAUSTED` status, which Cursor's backend uses to report a
/// free-plan quota wall (`RetriableError: [resource_exhausted] Error`, wrapped in
/// cursor-agent's own connection-retry banner that reads exactly like a network
/// blip) — but the same gRPC code also covers non-quota resource failures (out of
/// memory, a message-size limit) that have nothing to do with a rate limit. Mixed
/// into the shared list it would let Claude/Codex/Copilot's genuinely transient
/// resource errors get misclassified as `RateLimited`, which skips
/// `resolver::complete_inner`'s and `cursor_cli::run_hardened`'s transient retry —
/// exactly backwards for an error that WOULD have recovered on its own. Scoped to
/// cursor-agent alone via [`looks_rate_limited_cursor`], whose only caller is
/// `summariser/cursor_agent.rs`. Without this marker a resource-exhausted Cursor
/// account never returns `LlmError::RateLimited` and instead retries against an
/// already-exhausted quota, taking 20-30s+ per attempt to fail again the same way.
/// Observed live on a free-plan account 2026-08-19.
const CURSOR_RATE_LIMIT_MARKERS: &[&str] = &["resource_exhausted", "resource exhausted"];

fn matches_any_marker(text: &str, markers: &[&str]) -> bool {
    let low = text.to_lowercase();
    markers.iter().any(|m| low.contains(m))
}

/// Generic rate-limit check, safe for every engine. Does NOT match Cursor's
/// gRPC `resource_exhausted` vocabulary — see [`looks_rate_limited_cursor`].
pub fn looks_rate_limited(text: &str) -> bool {
    matches_any_marker(text, RATE_LIMIT_MARKERS)
}

/// `cursor-agent`-only: the generic check, plus Cursor's own gRPC vocabulary. See
/// [`CURSOR_RATE_LIMIT_MARKERS`] for why this must not be the default.
pub fn looks_rate_limited_cursor(text: &str) -> bool {
    looks_rate_limited(text) || matches_any_marker(text, CURSOR_RATE_LIMIT_MARKERS)
}

/// The first line that actually matched a rate-limit marker, capped to 200
/// chars. Engines' stderr often opens with informational banners ("Reading
/// additional input from stdin..."), so quoting `first_line` puts the wrong
/// text in the fallback WARN log — quote the matching line instead.
pub fn rate_limited_line(text: &str) -> Option<String> {
    rate_limited_line_matching(text, looks_rate_limited)
}

/// [`rate_limited_line`], but scoped to [`looks_rate_limited_cursor`] — must be
/// paired with it (see `summariser/cursor_agent.rs`), or a line that only matched
/// via [`CURSOR_RATE_LIMIT_MARKERS`] silently falls through to `first_line`
/// instead of being quoted.
pub fn rate_limited_line_cursor(text: &str) -> Option<String> {
    rate_limited_line_matching(text, looks_rate_limited_cursor)
}

fn rate_limited_line_matching(text: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && matches(line))
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
        assert!(looks_rate_limited_cursor(blob));
        let line = rate_limited_line_cursor(blob).unwrap();
        assert!(line.contains("resource_exhausted"));
    }

    /// The whole point of scoping `resource_exhausted` to Cursor: the SAME gRPC
    /// status also covers non-quota resource failures for other engines (out of
    /// memory, a message-size limit), and misreading one as a rate limit would
    /// skip the transient retry that would have recovered it. Generic
    /// `looks_rate_limited`/`rate_limited_line` (what Claude/Codex/Copilot call)
    /// must never match Cursor's vocabulary.
    #[test]
    fn a_non_cursor_resource_exhausted_message_is_not_a_rate_limit() {
        let blob = "request failed: resource exhausted while allocating worker memory";
        assert!(!looks_rate_limited(blob));
        assert!(rate_limited_line(blob).is_none());
        // But it IS still Cursor's business - the cursor-scoped check must still
        // catch a resource-exhausted message even without the retry-banner dressing.
        assert!(looks_rate_limited_cursor(blob));
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
