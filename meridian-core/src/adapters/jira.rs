//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Jira adapter — the reference [`ProviderAdapter`] implementation (DRAFT).
//!
//! Maps a Jira REST issue (`/rest/api/3/issue/{id}` shape) onto
//! [`CanonicalTask`], handling the Jira-specific traps from the six-provider
//! audit:
//!
//! - **Stable id = numeric `id`, not the `KAN-42` key.** Jira keys are mutable
//!   (re-keyed when an issue moves project), so the canonical key uses the
//!   immutable numeric id; the human key is stashed in `custom_fields.jira_key`.
//! - **Only 3 native status categories** (`new`/`indeterminate`/`done`). The
//!   canonical model has six, so Backlog / In-Review / Cancelled are resolved
//!   from the literal status *name* before falling back to the category key.
//! - **Emails are often hidden (GDPR).** `emailAddress` may be absent → `email`
//!   stays `None`; `accountId` → `provider_user_id` is the reliable join key.
//!
//! # Who calls this
//! Nothing yet (draft). See [`crate::adapters`].
//!
//! # Related
//! - [`crate::adapters::ProviderAdapter`] — the trait this implements.
//! - [`crate::canonical_task`] — the output shape.

use super::ProviderAdapter;
use crate::canonical_task::{
    CanonicalTask, PersonRef, Priority, Provider, StatusCategory, TaskKind,
};
use anyhow::Context;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Maps Jira issues to [`CanonicalTask`].
///
/// Config-less for the sketch. A production adapter will likely carry the site
/// base URL (to build `…/browse/{key}` links) and a custom-field id map (Jira
/// stores start date / story points / sprint under per-instance
/// `customfield_*` ids), threaded in here without changing the trait.
#[derive(Debug, Default, Clone, Copy)]
pub struct JiraAdapter;

impl ProviderAdapter for JiraAdapter {
    fn provider(&self) -> Provider {
        Provider::Jira
    }

    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask> {
        // Stable key is the numeric id, NOT the (mutable) issue key.
        let provider_id = raw
            .get("id")
            .and_then(Value::as_str)
            .context("jira: issue missing string `id`")?
            .to_string();
        let fields = raw.get("fields").context("jira: issue missing `fields`")?;

        let issuetype = fields.get("issuetype").unwrap_or(&Value::Null);
        let status = fields.get("status").unwrap_or(&Value::Null);

        // Human-facing key kept retrievable but never used as the stable key.
        let mut custom_fields: BTreeMap<String, Value> = BTreeMap::new();
        if let Some(key) = raw.get("key").and_then(Value::as_str) {
            custom_fields.insert("jira_key".to_string(), json!(key));
        }

        let assignees = person(fields.get("assignee")).into_iter().collect();

        let ancestor_path = fields
            .get("parent")
            .and_then(|p| p.get("id"))
            .and_then(Value::as_str)
            .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Jira, pid)])
            .unwrap_or_default();

        let project_ids = fields
            .get("project")
            .and_then(|p| p.get("id"))
            .and_then(Value::as_str)
            .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Jira, pid)])
            .unwrap_or_default();

        let labels = fields
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(CanonicalTask {
            // INVARIANT: derive, never assign — keeps canonical_id consistent.
            canonical_id: CanonicalTask::canonical_id_for(Provider::Jira, &provider_id),
            provider: Provider::Jira,
            provider_id,
            // `self` is the REST URL; a configured adapter would build the
            // `…/browse/{key}` link from a site base instead.
            url: str_field(raw, "self").unwrap_or_default(),
            title: str_field(fields, "summary").unwrap_or_default(),
            description: description(fields),
            kind: map_kind(issuetype),
            kind_raw: str_field(issuetype, "name").unwrap_or_default(),
            status_raw: str_field(status, "name").unwrap_or_default(),
            status_category: map_status(status),
            priority: fields
                .get("priority")
                .map(map_priority)
                .unwrap_or(Priority::None),
            assignees,
            reporter: person(fields.get("reporter")),
            ancestor_path,
            project_ids,
            created_at: str_field(fields, "created"),
            updated_at: str_field(fields, "updated"),
            completed_at: str_field(fields, "resolutiondate"),
            // No standard Jira start-date field — it's a per-instance custom
            // field, resolved once the adapter carries a custom-field id map.
            start_date: None,
            due_date: str_field(fields, "duedate"),
            labels,
            custom_fields,
            raw_payload: raw.clone(),
        })
    }
}

/// A non-empty string field, treating JSON null / absent / "" as `None`.
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Jira v2 returns `description` as a string; v3 returns an ADF object. Take the
/// string when present; otherwise leave it empty (the full ADF stays in
/// `raw_payload`, to be flattened by a later slice).
fn description(fields: &Value) -> String {
    match fields.get("description") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A Jira user object → [`PersonRef`]. `None` for null/absent or an empty user.
fn person(v: Option<&Value>) -> Option<PersonRef> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let provider_user_id = v
        .get("accountId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let display_name = v
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if provider_user_id.is_empty() && display_name.is_empty() {
        return None;
    }
    Some(PersonRef {
        // emailAddress is frequently withheld for privacy → optional.
        email: v
            .get("emailAddress")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        display_name,
        provider_user_id,
    })
}

/// Map issuetype → best-effort [`TaskKind`]. The `subtask` flag wins; otherwise
/// match on the (user-customizable) type name. Verbatim name lives in `kind_raw`.
fn map_kind(issuetype: &Value) -> TaskKind {
    if issuetype
        .get("subtask")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return TaskKind::Subtask;
    }
    match issuetype
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "epic" => TaskKind::Epic,
        "feature" => TaskKind::Feature,
        "story" => TaskKind::Story,
        "task" => TaskKind::Task,
        "bug" | "defect" => TaskKind::Bug,
        "sub-task" | "subtask" => TaskKind::Subtask,
        _ => TaskKind::Other,
    }
}

/// Resolve Jira's status into the canonical category. Jira exposes only three
/// native categories (`new`/`indeterminate`/`done`), so Backlog / InReview /
/// Cancelled are derived from the literal status *name* first, then the
/// category key is the fallback. `None` if neither is recognisable.
fn map_status(status: &Value) -> Option<StatusCategory> {
    let name = status
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    // Name-based refinements the 3 native categories can't express.
    if name.contains("backlog") {
        return Some(StatusCategory::Backlog);
    }
    if name.contains("review") {
        return Some(StatusCategory::InReview);
    }
    if name.contains("cancel")
        || name.contains("won't")
        || name.contains("wont")
        || name.contains("reject")
        || name.contains("abandon")
    {
        return Some(StatusCategory::Cancelled);
    }

    match status
        .get("statusCategory")
        .and_then(|c| c.get("key"))
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "new" => Some(StatusCategory::Todo),
        "indeterminate" => Some(StatusCategory::InProgress),
        "done" => Some(StatusCategory::Done),
        _ => None,
    }
}

/// Map Jira priority name → canonical [`Priority`]. Jira priority schemes are
/// configurable; this covers the common defaults. Absent/unknown → `None`.
fn map_priority(p: &Value) -> Priority {
    match p
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "highest" | "blocker" => Priority::Urgent,
        "high" | "critical" | "major" => Priority::High,
        "medium" | "normal" => Priority::Medium,
        "low" | "minor" => Priority::Low,
        "lowest" | "trivial" => Priority::Low,
        _ => Priority::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_issue() -> Value {
        json!({
            "id": "10042",
            "key": "KAN-42",
            "self": "https://acme.atlassian.net/rest/api/3/issue/10042",
            "fields": {
                "summary": "Wire up the canonical adapter",
                "description": "Map Jira issues into CanonicalTask.",
                "issuetype": {"name": "Story", "subtask": false},
                "status": {
                    "name": "In Review",
                    "statusCategory": {"key": "indeterminate", "name": "In Progress"}
                },
                "priority": {"name": "High"},
                "assignee": {
                    "accountId": "acc-1",
                    "displayName": "Developer",
                    "emailAddress": "developer@example.com"
                },
                "reporter": {"accountId": "acc-2", "displayName": "Lead"},
                "parent": {"id": "10001", "key": "KAN-1"},
                "project": {"id": "10000", "key": "KAN"},
                "labels": ["backend", "cdm"],
                "created": "2026-06-01T09:00:00.000+0000",
                "updated": "2026-06-28T12:00:00.000+0000",
                "resolutiondate": null,
                "duedate": "2026-07-01"
            }
        })
    }

    #[test]
    fn maps_a_full_issue() {
        let task = JiraAdapter.to_canonical(&sample_issue()).unwrap();

        // Stable key is the numeric id; the human key is retained separately.
        assert_eq!(task.provider_id, "10042");
        assert_eq!(task.canonical_id, "jira:10042");
        assert_eq!(task.custom_fields.get("jira_key"), Some(&json!("KAN-42")));

        assert_eq!(task.title, "Wire up the canonical adapter");
        assert_eq!(task.kind, TaskKind::Story);
        assert_eq!(task.kind_raw, "Story");
        assert_eq!(task.priority, Priority::High);

        // Name-based refinement: "In Review" over the `indeterminate` category.
        assert_eq!(task.status_raw, "In Review");
        assert_eq!(task.status_category, Some(StatusCategory::InReview));

        assert_eq!(task.assignees.len(), 1);
        assert_eq!(
            task.assignees[0].email.as_deref(),
            Some("developer@example.com")
        );

        // Reporter has no emailAddress (GDPR-hidden) → email None, id intact.
        let reporter = task.reporter.as_ref().expect("reporter present");
        assert_eq!(reporter.provider_user_id, "acc-2");
        assert_eq!(reporter.email, None);

        assert_eq!(task.ancestor_path, vec!["jira:10001"]);
        assert_eq!(task.project_ids, vec!["jira:10000"]);
        assert_eq!(task.completed_at, None); // resolutiondate was null
        assert_eq!(task.due_date.as_deref(), Some("2026-07-01"));
        assert_eq!(task.labels, vec!["backend", "cdm"]);
        assert!(!task.is_terminal());
    }

    #[test]
    fn status_resolution_covers_all_six_categories() {
        let cases = [
            ("Backlog", "new", Some(StatusCategory::Backlog)),
            ("To Do", "new", Some(StatusCategory::Todo)),
            (
                "In Progress",
                "indeterminate",
                Some(StatusCategory::InProgress),
            ),
            (
                "Code Review",
                "indeterminate",
                Some(StatusCategory::InReview),
            ),
            ("Done", "done", Some(StatusCategory::Done)),
            // Terminal-but-cancelled is name-derived over the `done` category.
            ("Cancelled", "done", Some(StatusCategory::Cancelled)),
            ("Won't Do", "done", Some(StatusCategory::Cancelled)),
        ];
        for (name, key, expected) in cases {
            let status = json!({"name": name, "statusCategory": {"key": key}});
            assert_eq!(map_status(&status), expected, "status name {name:?}");
        }
        // Unrecognisable → None.
        assert_eq!(map_status(&json!({})), None);
    }

    #[test]
    fn priority_lookup_is_name_based() {
        assert_eq!(map_priority(&json!({"name": "Highest"})), Priority::Urgent);
        assert_eq!(map_priority(&json!({"name": "High"})), Priority::High);
        assert_eq!(map_priority(&json!({"name": "Medium"})), Priority::Medium);
        assert_eq!(map_priority(&json!({"name": "Lowest"})), Priority::Low);
        assert_eq!(map_priority(&json!({"name": "Whatever"})), Priority::None);
    }

    #[test]
    fn unassigned_issue_has_empty_assignees() {
        let mut raw = sample_issue();
        raw["fields"]["assignee"] = Value::Null;
        let task = JiraAdapter.to_canonical(&raw).unwrap();
        assert!(task.assignees.is_empty());
    }

    #[test]
    fn subtask_flag_wins_over_type_name() {
        let issuetype = json!({"name": "Story", "subtask": true});
        assert_eq!(map_kind(&issuetype), TaskKind::Subtask);
    }

    #[test]
    fn cancelled_issue_is_terminal() {
        let mut raw = sample_issue();
        raw["fields"]["status"] = json!({"name": "Cancelled", "statusCategory": {"key": "done"}});
        raw["fields"]["resolutiondate"] = json!("2026-06-30T10:00:00.000+0000");
        let task = JiraAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.status_category, Some(StatusCategory::Cancelled));
        assert!(task.is_terminal());
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn missing_id_is_an_error() {
        let raw = json!({"fields": {"summary": "no id"}});
        assert!(JiraAdapter.to_canonical(&raw).is_err());
    }

    #[test]
    fn adapter_is_object_safe() {
        let adapter: Box<dyn ProviderAdapter> = Box::new(JiraAdapter);
        assert_eq!(adapter.provider(), Provider::Jira);
        let out = adapter.to_canonical_many(&[sample_issue()]);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_ok());
    }
}
