//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Linear adapter — reference [`ProviderAdapter`] for Linear (DRAFT).
//!
//! Maps a Linear GraphQL `Issue` node onto [`CanonicalTask`], handling the
//! Linear-specific traps from the six-provider audit:
//!
//! - **Stable id = the UUID `id`.** The human `identifier` (`ENG-123`) is kept
//!   in `custom_fields.linear_identifier`.
//! - **Priority is an inverted `Int` 0–4** (`1` = Urgent … `4` = Low, `0` = no
//!   priority). Mapped through an explicit table — never an ordinal cast.
//! - **Status comes from `WorkflowState.type`** (the 6 system types incl.
//!   `triage`), not the state name. Teams' custom "In Review" states have
//!   `type = "started"`, so they fold into [`StatusCategory::InProgress`] — the
//!   canonical `InReview` is never emitted for Linear (by design).
//! - **No issue-type concept** — every issue maps to [`TaskKind::Task`] with an
//!   empty `kind_raw`.
//! - `completedAt` / `canceledAt` feed `completed_at`; `parent` → `ancestor_path`,
//!   `project` → `project_ids`; `estimate` / `cycle` are kept in `custom_fields`.
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

/// Maps Linear issues to [`CanonicalTask`].
///
/// Config-less: a Linear `Issue` node carries everything needed (`url`,
/// `identifier`, `state.type`) so there's nothing per-workspace to thread in.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinearAdapter;

impl ProviderAdapter for LinearAdapter {
    fn provider(&self) -> Provider {
        Provider::Linear
    }

    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask> {
        let provider_id = raw
            .get("id")
            .and_then(Value::as_str)
            .context("linear: issue missing string `id`")?
            .to_string();

        let mut custom_fields: BTreeMap<String, Value> = BTreeMap::new();
        // Human-facing identifier kept retrievable; the UUID stays the key.
        if let Some(ident) = raw.get("identifier").and_then(Value::as_str) {
            custom_fields.insert("linear_identifier".to_string(), json!(ident));
        }
        // Linear-specific signals worth preserving but with no canonical slot.
        if let Some(est) = raw.get("estimate").filter(|v| !v.is_null()) {
            custom_fields.insert("estimate".to_string(), est.clone());
        }
        if let Some(cycle) = raw
            .get("cycle")
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
        {
            custom_fields.insert("cycle_id".to_string(), json!(cycle));
        }

        let assignees = person(raw.get("assignee")).into_iter().collect();

        let ancestor_path = raw
            .get("parent")
            .and_then(|p| p.get("id"))
            .and_then(Value::as_str)
            .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Linear, pid)])
            .unwrap_or_default();

        let project_ids = raw
            .get("project")
            .and_then(|p| p.get("id"))
            .and_then(Value::as_str)
            .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Linear, pid)])
            .unwrap_or_default();

        let labels = raw
            .get("labels")
            .and_then(|l| l.get("nodes"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.get("name").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Terminal timestamp: completedAt, else canceledAt.
        let completed_at = str_field(raw, "completedAt").or_else(|| str_field(raw, "canceledAt"));

        Ok(CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::Linear, &provider_id),
            provider: Provider::Linear,
            provider_id,
            url: str_field(raw, "url").unwrap_or_default(),
            title: str_field(raw, "title").unwrap_or_default(),
            description: str_field(raw, "description").unwrap_or_default(),
            // Linear has no issue-type concept — everything is a plain issue.
            kind: TaskKind::Task,
            kind_raw: String::new(),
            status_raw: raw
                .get("state")
                .and_then(|s| s.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status_category: map_status(raw.get("state")),
            priority: map_priority(raw.get("priority")),
            assignees,
            reporter: person(raw.get("creator")),
            ancestor_path,
            project_ids,
            created_at: str_field(raw, "createdAt"),
            updated_at: str_field(raw, "updatedAt"),
            completed_at,
            // Linear has no planned start-date field.
            start_date: None,
            due_date: str_field(raw, "dueDate"),
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

/// A Linear `User` node → [`PersonRef`]. `None` for null/absent or an empty user.
fn person(v: Option<&Value>) -> Option<PersonRef> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let provider_user_id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Linear exposes both `displayName` and `name`; prefer the former.
    let display_name = v
        .get("displayName")
        .and_then(Value::as_str)
        .or_else(|| v.get("name").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    if provider_user_id.is_empty() && display_name.is_empty() {
        return None;
    }
    Some(PersonRef {
        email: v
            .get("email")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        display_name,
        provider_user_id,
    })
}

/// Map `WorkflowState.type` → canonical category. Linear's six system types are
/// the source of truth; custom states (e.g. "In Review") carry one of these
/// types (`started`), so name-based refinement is deliberately NOT done.
fn map_status(state: Option<&Value>) -> Option<StatusCategory> {
    match state?.get("type").and_then(Value::as_str)? {
        "triage" | "backlog" => Some(StatusCategory::Backlog),
        "unstarted" => Some(StatusCategory::Todo),
        // Custom "In Review" states are `type = started` → fold into InProgress.
        "started" => Some(StatusCategory::InProgress),
        "completed" => Some(StatusCategory::Done),
        "canceled" => Some(StatusCategory::Cancelled),
        _ => None,
    }
}

/// Map Linear's inverted `Int` priority (0–4) → canonical [`Priority`].
/// `1` = Urgent … `4` = Low, `0`/absent = no priority. Explicit table — an
/// ordinal cast would invert the meaning.
fn map_priority(p: Option<&Value>) -> Priority {
    match p.and_then(Value::as_i64).unwrap_or(0) {
        1 => Priority::Urgent,
        2 => Priority::High,
        3 => Priority::Medium,
        4 => Priority::Low,
        _ => Priority::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_issue() -> Value {
        json!({
            "id": "11111111-2222-3333-4444-555555555555",
            "identifier": "ENG-123",
            "url": "https://linear.app/acme/issue/ENG-123",
            "title": "Adopt the canonical model",
            "description": "Map Linear issues into CanonicalTask.",
            "priority": 2,
            "state": {"type": "started", "name": "In Review"},
            "assignee": {
                "id": "usr-1",
                "displayName": "Developer",
                "name": "dev",
                "email": "developer@example.com"
            },
            "creator": {"id": "usr-2", "displayName": "Lead"},
            "parent": {"id": "uuid-parent"},
            "project": {"id": "proj-uuid"},
            "labels": {"nodes": [{"name": "backend"}, {"name": "cdm"}]},
            "createdAt": "2026-06-01T09:00:00.000Z",
            "updatedAt": "2026-06-28T12:00:00.000Z",
            "completedAt": null,
            "canceledAt": null,
            "dueDate": "2026-07-01",
            "estimate": 3,
            "cycle": {"id": "cycle-9"}
        })
    }

    #[test]
    fn maps_a_full_issue() {
        let task = LinearAdapter.to_canonical(&sample_issue()).unwrap();

        // UUID is the stable key; the human identifier is retained separately.
        assert_eq!(task.provider_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(
            task.canonical_id,
            "linear:11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(
            task.custom_fields.get("linear_identifier"),
            Some(&json!("ENG-123"))
        );
        assert_eq!(task.custom_fields.get("estimate"), Some(&json!(3)));
        assert_eq!(task.custom_fields.get("cycle_id"), Some(&json!("cycle-9")));

        // No issue-type concept in Linear.
        assert_eq!(task.kind, TaskKind::Task);
        assert_eq!(task.kind_raw, "");

        // "In Review" custom state has type=started → InProgress (verbatim kept).
        assert_eq!(task.status_raw, "In Review");
        assert_eq!(task.status_category, Some(StatusCategory::InProgress));

        // Inverted priority: 2 → High.
        assert_eq!(task.priority, Priority::High);

        assert_eq!(task.assignees.len(), 1);
        assert_eq!(
            task.assignees[0].email.as_deref(),
            Some("developer@example.com")
        );
        assert_eq!(task.ancestor_path, vec!["linear:uuid-parent"]);
        assert_eq!(task.project_ids, vec!["linear:proj-uuid"]);
        assert_eq!(task.completed_at, None);
        assert_eq!(task.labels, vec!["backend", "cdm"]);
        assert!(!task.is_terminal());
    }

    #[test]
    fn priority_table_is_inverted_not_ordinal() {
        let p = |n: i64| map_priority(Some(&json!(n)));
        assert_eq!(p(0), Priority::None);
        assert_eq!(p(1), Priority::Urgent);
        assert_eq!(p(2), Priority::High);
        assert_eq!(p(3), Priority::Medium);
        assert_eq!(p(4), Priority::Low);
        // Absent priority → None.
        assert_eq!(map_priority(None), Priority::None);
    }

    #[test]
    fn status_maps_from_state_type() {
        let by = |t: &str| map_status(Some(&json!({"type": t, "name": "x"})));
        assert_eq!(by("triage"), Some(StatusCategory::Backlog));
        assert_eq!(by("backlog"), Some(StatusCategory::Backlog));
        assert_eq!(by("unstarted"), Some(StatusCategory::Todo));
        assert_eq!(by("started"), Some(StatusCategory::InProgress));
        assert_eq!(by("completed"), Some(StatusCategory::Done));
        assert_eq!(by("canceled"), Some(StatusCategory::Cancelled));
        assert_eq!(map_status(None), None);
    }

    #[test]
    fn completed_at_falls_back_to_canceled_at() {
        let mut raw = sample_issue();
        raw["state"] = json!({"type": "canceled", "name": "Cancelled"});
        raw["canceledAt"] = json!("2026-06-30T10:00:00.000Z");
        let task = LinearAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.status_category, Some(StatusCategory::Cancelled));
        assert!(task.is_terminal());
        assert_eq!(
            task.completed_at.as_deref(),
            Some("2026-06-30T10:00:00.000Z")
        );
    }

    #[test]
    fn missing_id_is_an_error() {
        assert!(LinearAdapter
            .to_canonical(&json!({"title": "no id"}))
            .is_err());
    }
}
