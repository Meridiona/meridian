//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Parse the Workstream Builder's answer into this hour's PLACEMENTS — the model's
//! raw output, before any id resolution or merge onto the prior state (that is
//! [`crate::worklog_pipeline::workstream_sanitize`]).
//!
//! Tolerant by design: the answer may be a bare JSON object, a fenced ```json block, or
//! JSON wrapped in prose (schema-less providers). [`crate::llm::parse_json_object`] finds
//! the object; a genuinely unusable answer yields `None`, which the orchestrator
//! ([`crate::worklog_pipeline::workstream`]) turns into "carry the prior state forward".
//!
//! # Who calls this
//! [`crate::worklog_pipeline::workstream::run`].

use serde_json::Value;

use super::segment::{self, Segment};

/// One placement exactly as the model returned it — the target task `id` (empty = a
/// new task), an optional matured/new `title` (empty = keep the existing title),
/// the task's whole rewritten `summary` (a 3–6 bullet story, empty = keep the
/// existing story), and this hour's time `segments`. No ids are resolved or clamped
/// here (the 6-bullet cap is applied in [`super::workstream_sanitize`]).
#[derive(Debug, Clone)]
pub struct ParsedTask {
    pub id: String,
    pub title: String,
    pub summary: Vec<String>,
    pub segments: Vec<Segment>,
}

/// Pull the `{"placements":[…]}` object out of the model's answer, tolerating
/// fences / prose. `None` if there is no usable array. A placement with no segment
/// puts no time on the timeline; unless it renames an existing task (non-empty
/// `id` + `title`), it carries no information and is dropped.
pub fn parse_placements(text: &str) -> Option<Vec<ParsedTask>> {
    let v = crate::llm::parse_json_object(text)?;
    // Accept the current `placements` key; fall back to the legacy `tasks` key so an
    // older provider or cached answer still parses.
    let arr = v.get("placements").or_else(|| v.get("tasks"))?.as_array()?;
    let mut out = Vec::new();
    for it in arr {
        let id = str_field(it.get("id"));
        let title = str_field(it.get("title"));
        let segments = segment::parse_segments(it.get("segments"));
        // A placement with no segment adds no time; keep it only if it is a pure
        // rename of an existing task (id + title), otherwise it is noise.
        if segments.is_empty() && !(!id.is_empty() && !title.is_empty()) {
            continue;
        }
        out.push(ParsedTask {
            id,
            title,
            summary: str_array(it.get("summary")),
            segments,
        });
    }
    Some(out)
}

/// Read a JSON value as a trimmed string (empty if absent / not a string).
fn str_field(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Read a JSON value as a `Vec<String>` of trimmed non-empty strings.
pub fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_placements_with_segments_from_fenced_json() {
        let text = "```json\n{\"placements\":[{\"id\":\"T1\",\"title\":\"\",\
            \"summary\":[\"Reworked SEO\"],\
            \"segments\":[{\"start\":\"08:15\",\"end\":\"08:45\"}]}]}\n```";
        let got = parse_placements(text).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "T1");
        assert_eq!(got[0].title, "");
        assert_eq!(got[0].summary, vec!["Reworked SEO"]);
        assert_eq!(
            got[0].segments,
            vec![Segment {
                start_min: 495,
                end_min: 525
            }]
        );
    }

    #[test]
    fn accepts_legacy_tasks_key() {
        let text = "{\"tasks\":[{\"id\":\"\",\"title\":\"New\",\
            \"summary\":[],\"segments\":[{\"start\":\"09:00\",\"end\":\"09:30\"}]}]}";
        let got = parse_placements(text).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "");
        assert_eq!(got[0].title, "New");
    }

    #[test]
    fn none_on_garbage() {
        assert!(parse_placements("I can't do that").is_none());
    }

    #[test]
    fn drops_placement_with_no_segments_and_no_rename() {
        let text =
            "{\"placements\":[{\"id\":\"\",\"title\":\"\",\"summary\":[\"x\"],\"segments\":[]}]}";
        assert_eq!(parse_placements(text).unwrap().len(), 0);
    }

    #[test]
    fn keeps_pure_rename_without_segments() {
        // An existing task renamed, no new time this hour — id + title, no segments.
        let text =
            "{\"placements\":[{\"id\":\"T1\",\"title\":\"Better name\",\"summary\":[],\"segments\":[]}]}";
        let got = parse_placements(text).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "T1");
        assert_eq!(got[0].title, "Better name");
        assert!(got[0].segments.is_empty());
    }
}
