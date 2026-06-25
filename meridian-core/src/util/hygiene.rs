//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Board-hygiene mapping — a faithful port of `ui/lib/hygiene.ts` (`parseIssues`
//! + reason hint/fix/severity). Maps the triage reason codes the daemon stores
//! in `pm_task_curation.reasons_json` into the human hint + the in-app fix the
//! Tasks view renders. Mirrors the Rust engine's `TriageReason::{hint,fix}` —
//! kept in sync by hand (same as the TS did).

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize)]
pub struct HygieneFix {
    pub control: String,
    pub field: String,
    pub label: String,
    pub ai: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HygieneIssue {
    pub code: String,
    pub hint: String,
    pub fix: Option<HygieneFix>,
    /// "must_fix" | "optional"
    pub severity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hygiene {
    pub bucket: String,
    pub issues: Vec<HygieneIssue>,
    pub decision: Option<String>,
}

/// Fields Meridian needs to track a ticket accurately (must-fix); the rest is
/// optional good hygiene.
const MUST_FIX: [&str; 5] = [
    "missing_description",
    "thin_description",
    "vague_title",
    "missing_due_date",
    "overdue",
];

fn reason_severity(code: &str) -> &'static str {
    if MUST_FIX.contains(&code) {
        "must_fix"
    } else {
        "optional"
    }
}

/// A detail number formatted as JS would interpolate it (`${d?.key}`): the
/// integer/float value, or the literal "undefined" when the key is absent.
fn jsnum(detail: Option<&Map<String, Value>>, key: &str) -> String {
    match detail.and_then(|d| d.get(key)) {
        Some(v) if v.is_i64() => v.as_i64().unwrap().to_string(),
        Some(v) if v.is_u64() => v.as_u64().unwrap().to_string(),
        Some(v) if v.is_f64() => v.as_f64().unwrap().to_string(),
        _ => "undefined".to_string(),
    }
}

fn reason_hint(code: &str, detail: Option<&Map<String, Value>>) -> String {
    match code {
        "in_progress" => "In progress on the board.".to_string(),
        "due_soon" => {
            let in_days = detail
                .and_then(|d| d.get("in_days"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            if in_days <= 0 {
                "Due today.".to_string()
            } else {
                format!("Due in {} day(s).", jsnum(detail, "in_days"))
            }
        }
        "in_sprint" => "In the active sprint.".to_string(),
        "start_date_reached" => "Its start date has passed.".to_string(),
        "missing_description" => "No description — nothing to match your work against.".to_string(),
        "thin_description" => format!("Description is only {} characters.", jsnum(detail, "chars")),
        "vague_title" => "Title is generic — make it specific.".to_string(),
        "no_context_anchor" => "Not linked to an epic or parent.".to_string(),
        "missing_due_date" => "No due date — add one so Meridian knows when it's live.".to_string(),
        "overdue" => {
            let by = detail
                .and_then(|d| d.get("by_days"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            format!("Overdue by {by} day(s) — reschedule or close it.")
        }
        "missing_assignee" => "No assignee — who owns this?".to_string(),
        "missing_labels" => "No labels — add one to categorise it.".to_string(),
        "missing_priority" => "No priority set.".to_string(),
        "missing_estimate" => "No estimate — add story points.".to_string(),
        "missing_acceptance_criteria" => {
            "No acceptance criteria — define what 'done' means.".to_string()
        }
        "no_activity_since" => format!("No board activity in {} days.", jsnum(detail, "days")),
        "not_started" => "Still in a not-started column.".to_string(),
        "no_due_date" => "No due date set.".to_string(),
        "overdue_long" => format!(
            "Overdue by {} days with no movement.",
            jsnum(detail, "by_days")
        ),
        "far_future_due" => format!(
            "Not due for {} days — planned, not current work.",
            jsnum(detail, "in_days")
        ),
        "not_in_sprint" => "Not in any sprint.".to_string(),
        "already_done" => "Already marked done.".to_string(),
        "no_activity_signal" => "Open, but nothing yet says it's active.".to_string(),
        "unreadable_updated_at" => "Couldn't read its last-updated time.".to_string(),
        other => other.to_string(),
    }
}

fn fix(control: &str, field: &str, label: &str, ai: bool) -> HygieneFix {
    HygieneFix {
        control: control.to_string(),
        field: field.to_string(),
        label: label.to_string(),
        ai,
    }
}

fn reason_fix(code: &str) -> Option<HygieneFix> {
    match code {
        "missing_description" => Some(fix("edit_text", "description", "Add a description", true)),
        "thin_description" => Some(fix(
            "edit_text",
            "description",
            "Expand the description",
            true,
        )),
        "vague_title" => Some(fix("edit_text", "summary", "Make the title specific", true)),
        "no_context_anchor" => Some(fix(
            "pick_parent",
            "parent",
            "Link to an epic or parent",
            false,
        )),
        "missing_due_date" => Some(fix("date_picker", "duedate", "Add a due date", false)),
        "overdue" => Some(fix("date_picker", "duedate", "Reschedule due date", false)),
        "missing_assignee" => Some(fix("assign_self", "assignee", "Assign to me", false)),
        "missing_labels" => Some(fix("edit_labels", "labels", "Add a label", false)),
        "missing_priority" => Some(fix("pick_priority", "priority", "Set priority", false)),
        "missing_estimate" => Some(fix(
            "number_input",
            "story_points",
            "Add an estimate",
            false,
        )),
        "missing_acceptance_criteria" => Some(fix(
            "edit_checklist",
            "acceptance_criteria",
            "Add acceptance criteria",
            true,
        )),
        _ => None,
    }
}

/// Parse a reasons_json blob into the fixable hygiene issues (drops active/
/// descriptive reasons with no fix, and any the dev chose to ignore).
pub fn parse_issues(reasons_json: Option<&str>, ignored_json: Option<&str>) -> Vec<HygieneIssue> {
    let Some(reasons_json) = reasons_json else {
        return Vec::new();
    };
    let Ok(raw) = serde_json::from_str::<Vec<Value>>(reasons_json) else {
        return Vec::new();
    };
    let ignored: Vec<String> = ignored_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();

    raw.into_iter()
        .filter_map(|r| {
            let code = r.get("code")?.as_str()?.to_string();
            let detail = r.get("detail").and_then(|d| d.as_object());
            let fix = reason_fix(&code);
            // Mirror the TS filter: keep only fixable, non-ignored reasons.
            if fix.is_none() || ignored.contains(&code) {
                return None;
            }
            Some(HygieneIssue {
                hint: reason_hint(&code, detail),
                severity: reason_severity(&code).to_string(),
                fix,
                code,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_fixable_reasons() {
        // in_progress / due_soon have no fix → dropped; missing_due_date stays.
        let reasons = r#"[
            {"code":"in_progress"},
            {"code":"due_soon","detail":{"in_days":2}},
            {"code":"missing_due_date"}
        ]"#;
        let issues = parse_issues(Some(reasons), None);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "missing_due_date");
        assert!(issues[0].fix.is_some());
    }

    #[test]
    fn ignored_codes_are_filtered_out() {
        let reasons = r#"[{"code":"missing_labels"},{"code":"missing_priority"}]"#;
        let issues = parse_issues(Some(reasons), Some(r#"["missing_labels"]"#));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "missing_priority");
    }

    #[test]
    fn severity_splits_must_fix_from_optional() {
        let reasons = r#"[{"code":"missing_due_date"},{"code":"missing_labels"}]"#;
        let issues = parse_issues(Some(reasons), None);
        let by = |c: &str| issues.iter().find(|i| i.code == c).unwrap();
        assert_eq!(by("missing_due_date").severity, "must_fix");
        assert_eq!(by("missing_labels").severity, "optional");
    }

    #[test]
    fn thin_description_hint_carries_the_char_count() {
        let issues = parse_issues(
            Some(r#"[{"code":"thin_description","detail":{"chars":12}}]"#),
            None,
        );
        assert_eq!(issues[0].hint, "Description is only 12 characters.");
    }

    #[test]
    fn fix_shape_matches_the_control_mapping() {
        let issues = parse_issues(Some(r#"[{"code":"missing_assignee"}]"#), None);
        let fix = issues[0].fix.as_ref().unwrap();
        assert_eq!(fix.control, "assign_self");
        assert_eq!(fix.field, "assignee");
        assert!(!fix.ai);
    }

    #[test]
    fn empty_or_malformed_yields_no_issues() {
        assert!(parse_issues(None, None).is_empty());
        assert!(parse_issues(Some(""), None).is_empty());
        assert!(parse_issues(Some("not json"), None).is_empty());
        assert!(parse_issues(Some("[]"), None).is_empty());
    }
}
