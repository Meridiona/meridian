//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The prose prompts — ONE text file per prompt, shared by every provider.
//!
//! This is the deliberate anti-Dayflow decision. Dayflow forks its prompts per backend
//! (`GeminiDirectProvider+Prompting.swift`, `ChatCLIProvider+Prompts.swift`,
//! `OllamaProvider+Summaries.swift`), so every prompt fix has to be made four times and
//! the backends silently drift apart. Here the prompt is a `.md` file `include_str!`'d
//! once and handed to whichever backend the user chose. Same words, every model.
//!
//! (The coding-agent summariser already set this precedent with `SKILL.md`.)
//!
//! # The two calls
//!
//! The hourly report is deliberately split in two, because one call could not write good
//! prose *and* get the clock times right: on identical input the 2B produced 1, 5 and 30
//! paragraphs across runs, and invented durations in words that contradicted the measured
//! sessions. So:
//!
//! 1. [`ACTIVITY_SUMMARY`] — the activities, as numbered prose, with **no time in them**.
//!    Time is not mentioned, so it cannot be got wrong.
//! 2. [`ACTIVITY_TIME`] — how many minutes each activity took. A short list and the
//!    measured timeline: one small job.
//!
//! Code then clamps and fills the result. The model never writes a number we trust.

use serde_json::{json, Value};

/// The single hourly report prompt: activities (prose, no time) AND minutes per
/// activity, in one structured call. Replaces the former two-call split
/// ([`ACTIVITY_SUMMARY`] + [`ACTIVITY_TIME`]) — the prose stays time-free, the
/// time lives in a separate `minutes` field, so one call yields both without the
/// model writing clock times into the words.
pub const ACTIVITY_REPORT: &str = include_str!("../../services/prompts/activity-report.md");

/// Former call 1: what happened this hour, no time. Kept for reference / tests;
/// the live pipeline now uses the merged [`ACTIVITY_REPORT`].
pub const ACTIVITY_SUMMARY: &str = include_str!("../../services/prompts/activity-summary.md");

/// Former call 2: minutes per activity. Superseded by [`ACTIVITY_REPORT`].
pub const ACTIVITY_TIME: &str = include_str!("../../services/prompts/activity-time.md");

/// The shape the merged hourly report must answer in: activities in time order,
/// each with its prose and an integer minute estimate. `minutes` is capped at 600
/// so a runaway can't claim ten hours in a one-hour window (the caller clamps to
/// the hour's real span anyway); the count is unbounded — the prompt holds it to
/// 3-6 activities and code fills/clamps afterward.
pub fn activity_report_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "activities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "activity": {"type": "string"},
                        "minutes":  {"type": "integer", "minimum": 1, "maximum": 600}
                    },
                    "required": ["activity", "minutes"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["activities"],
        "additionalProperties": false
    })
}

/// The Workstream Builder: fold this hour's activity report into the running set of
/// day-level tasks (workstreams) shown on the timeline. Anchored incremental fold —
/// the current tasks are handed in as data (stable anchors), and the model returns
/// only THIS hour's placements (match an existing task or open a new one), never a
/// rewrite of the whole set. The prompt (not code) owns matching, how work groups
/// into tasks, how to group time into approximate segments, and what counts as work
/// worth showing. Same one-prompt-all-providers rule as the activity prompts.
pub const WORKSTREAM: &str = include_str!("../../services/prompts/workstream.md");

/// The JSON shape call 2 must answer in: one `{activity, minutes}` per activity.
///
/// `activity` is **bounded to the real count**, and that bound is load-bearing, not
/// decoration: with an unbounded schema the 2B echoed the input back, returning 13
/// entries for 4 activities. `minutes` is capped at 600 so a runaway can't claim ten
/// hours inside a one-hour window (the caller clamps to the hour's true span anyway).
pub fn activity_time_schema(n_activities: usize) -> Value {
    let max = n_activities.max(1);
    json!({
        "type": "object",
        "properties": {
            "activities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "activity": {"type": "integer", "minimum": 1, "maximum": max},
                        "minutes":  {"type": "integer", "minimum": 1, "maximum": 600}
                    },
                    "required": ["activity", "minutes"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["activities"],
        "additionalProperties": false
    })
}

/// The schema rendered for a CLI that takes it as a file or a flag.
pub fn activity_time_schema_json(n_activities: usize) -> String {
    activity_time_schema(n_activities).to_string()
}

/// The JSON shape the Workstream Builder must answer in: this hour's PLACEMENTS,
/// not the whole task set. Each placement is one piece of the new hour's work,
/// assigned to an existing task (`id` = its `T<n>`) or to a new task (`id` empty).
/// `title` is the mature/new title — empty on an existing task means "keep the
/// current title". `summary` is the task's WHOLE rewritten story (a tight 3–6
/// bullets, capped at 6 — code replaces the task's summary with it, it is not a
/// per-hour delta); `segments` is only THIS hour's `HH:MM-HH:MM` ranges (code
/// unions them with the task's earlier segments). `id` and `title` are optional so
/// a new task can omit `id` and an unchanged title can be left off; `summary` and
/// `segments` are required (a placement exists to add this hour's time + story).
/// Unbounded on count on purpose — the prompt (not code) decides how work groups.
pub fn workstream_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "placements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id":      {"type": "string"},
                        "title":   {"type": "string"},
                        "summary": {"type": "array", "items": {"type": "string"}, "maxItems": 6},
                        "segments": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "start": {"type": "string"},
                                    "end":   {"type": "string"}
                                },
                                "required": ["start", "end"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["summary", "segments"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["placements"],
        "additionalProperties": false
    })
}

/// The "Generate worklog" prompt — one combined, provider-agnostic call behind the
/// day-task card action. Takes a day-level workstream + the open PM tickets and
/// returns, in one pass: `match` XOR `propose` (advance an existing ticket or draft
/// a new one), plus a high-level `update` (summary / decisions / architecture /
/// status) to post as a comment. Same one-prompt-all-providers rule as the others.
pub const WORKLOG_GENERATE: &str = include_str!("../../services/prompts/worklog-generate.md");

/// The JSON shape the "Generate worklog" call must answer in. `match` and `propose`
/// are nullable objects (the model returns exactly one non-null — code enforces the
/// XOR after parsing); `update` is required with a `summary`, `decisions`/
/// `architecture` string arrays, and a `status` line; `reasoning` is required.
///
/// The nullable branches are `["object", "null"]` so a schema-enforcing backend can
/// emit `null` for the branch it didn't take; `match`/`propose` are deliberately NOT
/// in `required` so a backend that omits the unused branch entirely is also valid.
/// Parsing is tolerant either way (see `parse_json_object`).
pub fn worklog_generate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "match": {
                "type": ["object", "null"],
                "properties": {
                    "task_key":   {"type": "string"},
                    "confidence": {"type": "number", "minimum": 0, "maximum": 1}
                },
                "required": ["task_key", "confidence"],
                "additionalProperties": false
            },
            "propose": {
                "type": ["object", "null"],
                "properties": {
                    "issue_type":  {"type": "string"},
                    "title":       {"type": "string"},
                    "description": {"type": "string"}
                },
                "required": ["issue_type", "title", "description"],
                "additionalProperties": false
            },
            "update": {
                "type": "object",
                "properties": {
                    "summary":      {"type": "string"},
                    "decisions":    {"type": "array", "items": {"type": "string"}},
                    "architecture": {"type": "array", "items": {"type": "string"}},
                    "status":       {"type": "string"}
                },
                "required": ["summary", "decisions", "architecture", "status"],
                "additionalProperties": false
            },
            "reasoning": {"type": "string"}
        },
        "required": ["update", "reasoning"],
        "additionalProperties": false
    })
}

/// Appended to the prompt for backends with NO schema mechanism (copilot, cursor).
///
/// They cannot be constrained at the token level, so the contract has to ride in the
/// prompt and the answer is parsed tolerantly ([`super::parse_json_object`]).
pub fn schema_instruction(schema: &Value) -> String {
    format!(
        "\n\nReturn ONLY a JSON object matching this schema. No prose, no explanation, \
         no code fence:\n{schema}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_present_and_carry_their_load_bearing_clauses() {
        // If someone edits the .md files, these are the clauses that must survive: the
        // whole two-call design rests on call 1 never mentioning time.
        assert!(ACTIVITY_SUMMARY.contains("NO TIME"));
        assert!(ACTIVITY_SUMMARY.contains("NEVER NAME THE PERSON"));
        assert!(ACTIVITY_TIME.contains("how many minutes"));
        assert!(!ACTIVITY_SUMMARY.trim().is_empty());
        // The Workstream Builder's whole design rests on: real workstreams (not
        // per-fix tasks), segments as the time unit, work-only (no leisure), and
        // never naming the person. The count / grouping / negligibility are the
        // model's judgement, guided by prose — no hardcoded number to assert on.
        assert!(WORKSTREAM.contains("workstream"));
        assert!(WORKSTREAM.contains("segment"));
        assert!(WORKSTREAM.to_lowercase().contains("leisure"));
        assert!(WORKSTREAM.contains("NEVER NAME THE PERSON"));
        // The Generate-worklog prompt's whole design rests on: match XOR propose,
        // no-match being valid, a high-level status update (not a time worklog),
        // and never naming the person.
        assert!(WORKLOG_GENERATE.contains("MUTUALLY EXCLUSIVE"));
        assert!(WORKLOG_GENERATE.contains("NO MATCH IS A VALID"));
        assert!(WORKLOG_GENERATE.to_lowercase().contains("status update"));
        assert!(WORKLOG_GENERATE.contains("NEVER NAME THE PERSON"));
    }

    #[test]
    fn worklog_generate_schema_pins_match_xor_propose_and_update() {
        let s = worklog_generate_schema();
        let props = &s["properties"];
        // match / propose are nullable objects, not in `required` (XOR enforced in code).
        assert_eq!(props["match"]["type"], json!(["object", "null"]));
        assert_eq!(props["propose"]["type"], json!(["object", "null"]));
        assert_eq!(s["required"], json!(["update", "reasoning"]));
        // The match branch carries a task_key + a 0-1 confidence.
        let m = &props["match"]["properties"];
        assert_eq!(m["task_key"]["type"], json!("string"));
        assert_eq!(m["confidence"]["maximum"], json!(1));
        // The propose branch carries issue_type / title / description.
        let p = &props["propose"]["properties"];
        assert_eq!(p["issue_type"]["type"], json!("string"));
        assert_eq!(p["title"]["type"], json!("string"));
        assert_eq!(p["description"]["type"], json!("string"));
        // The update requires all four fields; decisions/architecture are arrays.
        let u = &props["update"];
        assert_eq!(
            u["required"],
            json!(["summary", "decisions", "architecture", "status"])
        );
        assert_eq!(u["properties"]["decisions"]["type"], json!("array"));
        assert_eq!(u["properties"]["architecture"]["type"], json!("array"));
    }

    #[test]
    fn workstream_schema_pins_the_task_shape() {
        let s = workstream_schema();
        let props = &s["properties"]["placements"]["items"]["properties"];
        assert_eq!(props["summary"]["type"], json!("array"));
        // The whole-story summary is capped at 6 bullets (see workstream.md).
        assert_eq!(props["summary"]["maxItems"], json!(6));
        assert_eq!(props["segments"]["type"], json!("array"));
        let seg = &props["segments"]["items"]["properties"];
        assert_eq!(seg["start"]["type"], json!("string"));
        assert_eq!(seg["end"]["type"], json!("string"));
        assert_eq!(props["id"]["type"], json!("string"));
    }

    #[test]
    fn schema_bounds_the_activity_index_to_the_real_count() {
        let s = activity_time_schema(4);
        let props = &s["properties"]["activities"]["items"]["properties"];
        assert_eq!(props["activity"]["maximum"], json!(4));
        assert_eq!(props["activity"]["minimum"], json!(1));
    }

    #[test]
    fn schema_never_bounds_to_zero() {
        // An empty activity list would make `maximum: 0` — an unsatisfiable grammar the
        // FSM cannot generate any value for. Floor it at 1.
        assert_eq!(
            activity_time_schema(0)["properties"]["activities"]["items"]["properties"]["activity"]
                ["maximum"],
            json!(1)
        );
    }
}
