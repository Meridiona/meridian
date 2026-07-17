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
/// day-task card action. Takes a day-level workstream + the day's planned tasks and
/// returns, in one pass: `matches` XOR `propose` (advance the existing tickets this
/// work moved — one or several — or draft a new one), plus a high-level `update`
/// (summary / free-form `sections` / status) posted as a comment on each matched
/// ticket. Same one-prompt-all-providers rule as the others.
pub const WORKLOG_GENERATE: &str = include_str!("../../services/prompts/worklog-generate.md");

/// The JSON shape the "Generate worklog" call must answer in. `matches` is an array
/// (a day's strand of work can advance several planned tasks) and `propose` is a
/// nullable object; the model takes exactly one branch — a non-empty `matches` XOR
/// a `propose` — and code enforces that after parsing. `update` is required with a
/// `summary`, free-form `sections`, and a `status` line; `reasoning` is required.
///
/// `propose` is `["object", "null"]` so a schema-enforcing backend can emit `null`
/// for the branch it didn't take; neither branch is in `required`, so a backend that
/// omits the unused one entirely is also valid (an omitted `matches` reads as no
/// matches). Parsing is tolerant either way (see `parse_json_object`).
pub fn worklog_generate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "matches": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "task_key":   {"type": "string"},
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1}
                    },
                    "required": ["task_key", "confidence"],
                    "additionalProperties": false
                }
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
                    "summary": {"type": "string"},
                    "sections": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "heading": {"type": "string"},
                                "points":  {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["heading", "points"],
                            "additionalProperties": false
                        }
                    },
                    "status": {"type": "string"}
                },
                "required": ["summary", "sections", "status"],
                "additionalProperties": false
            },
            "reasoning": {"type": "string"}
        },
        "required": ["update", "reasoning"],
        "additionalProperties": false
    })
}

/// The "draft a task from my note" prompt — behind the daily plan's task composer.
/// Takes one rough note the dev typed while planning and shapes it into
/// `{title, description, issue_type}` for them to review and edit. Deliberately a
/// FORMATTER, not an expander: the prompt's load-bearing rule is `INVENT NOTHING`,
/// because the failure mode of a four-word note is a fabricated three-paragraph
/// ticket. Same one-prompt-all-providers rule as the others.
pub const PLAN_TASK_DRAFT: &str = include_str!("../../services/prompts/plan-task-draft.md");

/// The JSON shape the task-draft call must answer in. All three fields required.
///
/// `title` is bounded at 120 even though the prompt asks for <=80: on a
/// schema-enforcing backend (the `local` outlines FSM) the bound is a hard token-level
/// cut, so setting it to 80 would truncate a slightly-long title mid-word rather than
/// reject it. Prose holds the 80; the schema only catches a runaway. `issue_type` is
/// an enum — it is the one field with a closed set, and a backend that can enforce it
/// should.
pub fn plan_task_draft_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title":       {"type": "string", "maxLength": 120},
            "description": {"type": "string"},
            "issue_type":  {"type": "string", "enum": ["Task", "Bug"]}
        },
        "required": ["title", "description", "issue_type"],
        "additionalProperties": false
    })
}

/// The daily-summary prompt — the AI-composed end-of-day review. Takes a day's
/// evidence (workstreams, their segments, time by app/category, the hourly shape,
/// and the hour reports as prose) and returns a narrative, a few insight lines, and
/// 2-4 Vega-Lite specs it chose itself. Same one-prompt-all-providers rule.
///
/// The daily plan is deliberately NOT an input: this is what the day WAS, not what
/// was promised, and mixing them turns a review into a scorecard.
pub const DAILY_SUMMARY: &str = include_str!("../../services/prompts/daily-summary.md");

/// The JSON shape the daily-summary call must answer in.
///
/// `spec` is deliberately an **unconstrained object**: it is a whole raw Vega-Lite
/// spec, and Vega-Lite's own schema is 1.8 MB with 458 definitions — inlining a
/// subset of it here would be a second, drifting copy of someone else's grammar,
/// and would also cap what the model is allowed to express, which is the one thing
/// this feature is for. Validity is established after the fact instead, against the
/// real schema, in `crate::day_summary::validate`.
///
/// That has a real consequence worth stating: on a schema-enforcing backend the
/// bounds here are hard token-level cuts, so the FSM can guarantee the WRAPPER is
/// well-formed but nothing about the spec inside it. Hence `panels` is bounded
/// (2-4, the screen holds no more) while its contents are not, and hence the
/// validator — not the schema — is what keeps a broken chart off the screen.
///
/// `why` is required on purpose: asking the model to justify the form is what makes
/// it consider fit at all, rather than reaching for a bar chart every time.
pub fn daily_summary_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "narrative": {"type": "string"},
            "insights": {
                "type": "array",
                "items": {"type": "string"},
                "minItems": 2,
                "maxItems": 4
            },
            "panels": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "why":   {"type": "string"},
                        // A whole Vega-Lite spec. See the doc comment above.
                        "spec":  {"type": "object"}
                    },
                    "required": ["title", "why", "spec"],
                    "additionalProperties": false
                },
                // The floor is load-bearing, not decoration. The prompt asks for
                // 2-4; with `minItems: 1` sonnet returned a ONE-panel screen on a
                // real day, which is a poor answer to "show me my day" and not what
                // the prose asked for. Claude validates --json-schema server-side,
                // so the floor is genuinely enforced there. The ceiling is what the
                // screen physically holds (code caps it too — see MAX_PANELS).
                "minItems": 2,
                "maxItems": 4
            }
        },
        "required": ["narrative", "insights", "panels"],
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
        // The Generate-worklog prompt's whole design rests on: matches XOR propose,
        // no-match being valid, a high-level status update (not a time worklog),
        // and never naming the person.
        assert!(WORKLOG_GENERATE.contains("MUTUALLY EXCLUSIVE"));
        assert!(WORKLOG_GENERATE.contains("AN EMPTY `matches` IS A VALID"));
        // Multi-match's two halves: list every ticket the work advanced, but make
        // each one earn its place alone. Without the second the model hedges, and a
        // hedge is a comment on someone's board.
        assert!(WORKLOG_GENERATE.contains("MATCH EVERY TICKET THIS WORK GENUINELY ADVANCED"));
        assert!(WORKLOG_GENERATE.contains("EARN its place independently"));
        assert!(WORKLOG_GENERATE.to_lowercase().contains("status update"));
        assert!(WORKLOG_GENERATE.contains("NEVER NAME THE PERSON"));
        // The task-draft prompt's whole design rests on it being a FORMATTER, not an
        // expander: the note is the only source of fact, a short note deserves a
        // short task, and not every note is engineering work.
        assert!(PLAN_TASK_DRAFT.contains("THE CONTEXT IS THEIRS, NOT YOURS"));
        assert!(PLAN_TASK_DRAFT.contains("SHORT, HONEST TASK IS THE CORRECT ANSWER"));
        assert!(PLAN_TASK_DRAFT.contains("NOT ALL WORK IS ENGINEERING"));
        assert!(PLAN_TASK_DRAFT.contains("NEVER NAME THE PERSON"));
        assert!(PLAN_TASK_DRAFT.contains("Return a JSON object with these fields:"));
        // The daily summary's whole design rests on four clauses. The data-binding
        // rule is the load-bearing one: it is what makes a hallucinated number
        // impossible, since the model never gets to write a value down. The rest
        // are what stop it becoming the thing it must not be — a timesheet that
        // replays a day back at the person who just lived it.
        assert!(DAILY_SUMMARY.contains("NEVER write `\"data\": {\"values\": [...]}`"));
        assert!(DAILY_SUMMARY.contains("bind data by name, never inline"));
        assert!(DAILY_SUMMARY.contains("THIS IS NOT A REPORT AND NOT A TIMESHEET"));
        assert!(DAILY_SUMMARY.contains("SHOW THEM SOMETHING THEY DID NOT ALREADY KNOW"));
        assert!(DAILY_SUMMARY.contains("CHOOSE THE FORM FROM THE DATA"));
        assert!(DAILY_SUMMARY.contains("INVENT NOTHING"));
        assert!(DAILY_SUMMARY.contains("NEVER NAME THE PERSON"));
    }

    #[test]
    fn daily_summary_schema_bounds_the_wrapper_but_not_the_spec() {
        let s = daily_summary_schema();
        assert_eq!(s["required"], json!(["narrative", "insights", "panels"]));
        let panels = &s["properties"]["panels"];
        // The screen is one page — code enforces this too, but a schema-enforcing
        // backend should never even offer a fifth panel.
        assert_eq!(panels["maxItems"], json!(4));
        // And a one-panel "summary" is not a summary. Observed for real on sonnet
        // before this floor existed, hence the assert rather than trusting prose.
        assert_eq!(panels["minItems"], json!(2));
        let panel = &panels["items"];
        assert_eq!(panel["required"], json!(["title", "why", "spec"]));
        // `spec` is deliberately unconstrained: it is a whole raw Vega-Lite spec,
        // and constraining it here would both duplicate a 1.8 MB grammar and cap
        // what the model may express. `day_summary::validate` is what checks it.
        assert_eq!(panel["properties"]["spec"], json!({"type": "object"}));
    }

    /// The prompt's worked examples name real datasets. If a dataset is renamed in
    /// `meridian_core::day_evidence::datasets` the examples must move with it, or the model is
    /// taught a contract the validator then rejects.
    #[test]
    fn daily_summary_examples_only_name_real_datasets() {
        for name in ["categories", "segments"] {
            assert!(
                DAILY_SUMMARY.contains(&format!("\"name\": \"{name}\"")),
                "prompt example references dataset {name}"
            );
            assert!(
                meridian_core::day_evidence::datasets::by_name(name).is_some(),
                "prompt example names dataset '{name}', which no longer exists"
            );
        }
        // Every field the examples encode must be real, for the same reason.
        for (ds, field) in [
            ("categories", "seconds"),
            ("categories", "category"),
            ("segments", "start_min"),
            ("segments", "end_min"),
            ("segments", "title"),
            ("segments", "task_id"),
            ("segments", "minutes"),
        ] {
            assert!(
                meridian_core::day_evidence::datasets::has_field(ds, field),
                "prompt example encodes {ds}.{field}, which no longer exists"
            );
        }
    }

    #[test]
    fn plan_task_draft_schema_pins_the_task_shape() {
        let s = plan_task_draft_schema();
        let props = &s["properties"];
        assert_eq!(s["required"], json!(["title", "description", "issue_type"]));
        assert_eq!(s["additionalProperties"], json!(false));
        // issue_type is the one closed set — a schema-enforcing backend should hold it.
        assert_eq!(props["issue_type"]["enum"], json!(["Task", "Bug"]));
        // The title bound is deliberately looser than the prompt's <=80: on the FSM
        // backend the bound is a hard token-level cut, so 80 here would truncate a
        // slightly-long title mid-word instead of rejecting it.
        assert_eq!(props["title"]["maxLength"], json!(120));
        assert_eq!(props["description"]["type"], json!("string"));
    }

    #[test]
    fn worklog_generate_schema_pins_matches_xor_propose_and_update() {
        let s = worklog_generate_schema();
        let props = &s["properties"];
        // matches is an ARRAY: one strand of a day's work can advance several
        // planned tasks, and each listed ticket gets the update posted to it.
        // Neither branch is in `required` (the XOR is enforced in code).
        assert_eq!(props["matches"]["type"], json!("array"));
        assert_eq!(props["propose"]["type"], json!(["object", "null"]));
        assert_eq!(s["required"], json!(["update", "reasoning"]));
        // Each match carries a task_key + a 0-1 confidence.
        let m = &props["matches"]["items"]["properties"];
        assert_eq!(m["task_key"]["type"], json!("string"));
        assert_eq!(m["confidence"]["maximum"], json!(1));
        // The propose branch carries issue_type / title / description.
        let p = &props["propose"]["properties"];
        assert_eq!(p["issue_type"]["type"], json!("string"));
        assert_eq!(p["title"]["type"], json!("string"));
        assert_eq!(p["description"]["type"], json!("string"));
        // The update requires summary/sections/status; sections is an array of
        // {heading, points[]} groups the model names to fit the work.
        let u = &props["update"];
        assert_eq!(u["required"], json!(["summary", "sections", "status"]));
        assert_eq!(u["properties"]["sections"]["type"], json!("array"));
        let sec = &u["properties"]["sections"]["items"]["properties"];
        assert_eq!(sec["heading"]["type"], json!("string"));
        assert_eq!(sec["points"]["type"], json!("array"));
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
