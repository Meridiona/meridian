//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Asana adapter — reference [`ProviderAdapter`] for Asana tasks.
//!
//! Maps an Asana task (REST `/tasks/{gid}` shape) onto [`CanonicalTask`],
//! handling the Asana-specific traps from the six-provider audit:
//!
//! - **Stable id = the task `gid`** (globally unique, immutable).
//! - **`completed` is the only status signal.** Asana has no workflow
//!   categories — `completed: true` → Done, else `None`. The nearest
//!   board-column analogue is the section a task sits in
//!   (`memberships[].section.name`), which feeds `status_raw` verbatim.
//! - **Multi-project is native**: `projects` (and `memberships`) → all of
//!   `project_ids` — the audit driver for `project_ids` being a `Vec`.
//! - **Subtasks are tasks with a `parent`** → [`TaskKind::Subtask`] +
//!   1-element `ancestor_path`; otherwise `resource_subtype`
//!   (`default_task`/`milestone`/`approval`) is kept verbatim in `kind_raw`
//!   with best-effort `kind` = Task.
//! - **No priority concept** → [`Priority::None`] (an Asana "Priority" custom
//!   field, when present, surfaces through `custom_fields` by name).
//! - **Dates are split**: `due_at` (datetime, when a time is set) beats
//!   `due_on` (date-only); same for `start_at`/`start_on`.
//!
//! # Who calls this
//! Nothing yet — Asana has no ingestion connector
//! (`src/intelligence/providers/` has no asana module). The adapter lands with
//! the CDM set (PR #361 Step 4) so a future connector starts at parity with
//! the wired providers instead of re-deriving the mapping.
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

/// Maps Asana tasks to [`CanonicalTask`].
///
/// Config-less: a task carries everything needed (gids, permalink, projects)
/// so there's nothing per-workspace to thread in.
#[derive(Debug, Default, Clone, Copy)]
pub struct AsanaAdapter;

impl ProviderAdapter for AsanaAdapter {
    fn provider(&self) -> Provider {
        Provider::Asana
    }

    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask> {
        let provider_id = raw
            .get("gid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .context("asana: task missing `gid`")?
            .to_string();

        let completed = raw
            .get("completed")
            .and_then(Value::as_bool)
            .unwrap_or_default();

        let parent_gid = raw
            .pointer("/parent/gid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());

        let subtype = raw
            .get("resource_subtype")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // A parented task IS a subtask regardless of subtype; the subtype
        // string stays verbatim in kind_raw either way.
        let kind = if parent_gid.is_some() {
            TaskKind::Subtask
        } else {
            TaskKind::Task
        };

        // Section = the nearest board-column analogue; first membership wins
        // (a task shows one section per project board).
        let status_raw = raw
            .pointer("/memberships/0/section/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // projects + membership projects, deduped, order-preserving.
        let mut project_ids: Vec<String> = Vec::new();
        let mut push_project = |gid: &str| {
            let cid = CanonicalTask::canonical_id_for(Provider::Asana, gid);
            if !project_ids.contains(&cid) {
                project_ids.push(cid);
            }
        };
        if let Some(projects) = raw.get("projects").and_then(Value::as_array) {
            for p in projects {
                if let Some(gid) = p.get("gid").and_then(Value::as_str) {
                    push_project(gid);
                }
            }
        }
        if let Some(memberships) = raw.get("memberships").and_then(Value::as_array) {
            for m in memberships {
                if let Some(gid) = m.pointer("/project/gid").and_then(Value::as_str) {
                    push_project(gid);
                }
            }
        }

        let labels = raw
            .get("tags")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        // Asana custom fields keyed by name → display_value (the raw objects
        // survive in raw_payload).
        let mut custom_fields: BTreeMap<String, Value> = BTreeMap::new();
        if let Some(cfs) = raw.get("custom_fields").and_then(Value::as_array) {
            for cf in cfs {
                if let (Some(name), Some(dv)) = (
                    cf.get("name").and_then(Value::as_str),
                    cf.get("display_value"),
                ) {
                    if !name.is_empty() && !dv.is_null() {
                        custom_fields.insert(name.to_string(), dv.clone());
                    }
                }
            }
        }
        if !subtype.is_empty() {
            custom_fields.insert("resource_subtype".to_string(), json!(subtype));
        }

        Ok(CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::Asana, &provider_id),
            provider: Provider::Asana,
            provider_id,
            url: str_field(raw, "permalink_url").unwrap_or_default(),
            title: str_field(raw, "name").unwrap_or_default(),
            description: str_field(raw, "notes").unwrap_or_default(),
            kind,
            kind_raw: subtype,
            status_raw,
            // completed is the ONLY reliable signal; an incomplete task's
            // section name has no fixed semantics.
            status_category: completed.then_some(StatusCategory::Done),
            priority: Priority::None,
            // Asana is strictly single-assignee.
            assignees: person(raw.get("assignee")).into_iter().collect(),
            reporter: person(raw.get("created_by")),
            ancestor_path: parent_gid
                .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Asana, pid)])
                .unwrap_or_default(),
            project_ids,
            created_at: str_field(raw, "created_at"),
            updated_at: str_field(raw, "modified_at"),
            completed_at: str_field(raw, "completed_at"),
            // Datetime beats date-only when a time is set.
            start_date: str_field(raw, "start_at").or_else(|| str_field(raw, "start_on")),
            due_date: str_field(raw, "due_at").or_else(|| str_field(raw, "due_on")),
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

/// An Asana user object → [`PersonRef`]. `email` needs the `email` opt-field;
/// absent otherwise.
fn person(v: Option<&Value>) -> Option<PersonRef> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let provider_user_id = v
        .get("gid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let display_name = v
        .get("name")
        .and_then(Value::as_str)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task() -> Value {
        json!({
            "gid": "1204512345678901",
            "resource_type": "task",
            "resource_subtype": "default_task",
            "name": "Adopt the canonical model",
            "notes": "Map Asana tasks into CanonicalTask.",
            "completed": false,
            "completed_at": null,
            "created_at": "2026-06-01T09:00:00.000Z",
            "modified_at": "2026-06-28T12:00:00.000Z",
            "due_on": "2026-07-01",
            "due_at": null,
            "start_on": "2026-06-20",
            "start_at": null,
            "permalink_url": "https://app.asana.com/0/p1/1204512345678901",
            "assignee": {"gid": "u1", "name": "Developer"},
            "created_by": {"gid": "u2", "name": "Lead"},
            "parent": null,
            "projects": [{"gid": "p1", "name": "Platform"}, {"gid": "p2", "name": "Q3"}],
            "memberships": [
                {"project": {"gid": "p1"}, "section": {"gid": "s1", "name": "In progress"}},
                {"project": {"gid": "p3"}, "section": {"gid": "s2", "name": "Backlog"}}
            ],
            "tags": [{"gid": "t1", "name": "backend"}, {"gid": "t2", "name": "cdm"}],
            "custom_fields": [
                {"gid": "cf1", "name": "Priority", "display_value": "High"},
                {"gid": "cf2", "name": "Points", "display_value": null}
            ]
        })
    }

    #[test]
    fn maps_a_full_task() {
        let task = AsanaAdapter.to_canonical(&sample_task()).unwrap();

        assert_eq!(task.provider_id, "1204512345678901");
        assert_eq!(task.canonical_id, "asana:1204512345678901");

        // Incomplete → no category; section name is the raw status.
        assert_eq!(task.status_category, None);
        assert_eq!(task.status_raw, "In progress");

        // Top-level default_task → Task, subtype verbatim.
        assert_eq!(task.kind, TaskKind::Task);
        assert_eq!(task.kind_raw, "default_task");

        // Multi-project union of projects + memberships, deduped (p1 once).
        assert_eq!(task.project_ids, vec!["asana:p1", "asana:p2", "asana:p3"]);

        // Single assignee; creator is the reporter.
        assert_eq!(task.assignees.len(), 1);
        assert_eq!(task.assignees[0].display_name, "Developer");
        assert_eq!(task.reporter.as_ref().unwrap().display_name, "Lead");

        // Date-only fallbacks (no *_at datetimes set).
        assert_eq!(task.start_date.as_deref(), Some("2026-06-20"));
        assert_eq!(task.due_date.as_deref(), Some("2026-07-01"));

        // Custom fields by name; null display_value dropped.
        assert_eq!(task.custom_fields.get("Priority"), Some(&json!("High")));
        assert!(!task.custom_fields.contains_key("Points"));

        assert_eq!(task.labels, vec!["backend", "cdm"]);
        assert!(!task.is_terminal());
    }

    #[test]
    fn completed_task_is_done() {
        let mut raw = sample_task();
        raw["completed"] = json!(true);
        raw["completed_at"] = json!("2026-06-30T10:00:00.000Z");
        let task = AsanaAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.status_category, Some(StatusCategory::Done));
        assert!(task.is_terminal());
        assert_eq!(
            task.completed_at.as_deref(),
            Some("2026-06-30T10:00:00.000Z")
        );
    }

    #[test]
    fn parented_task_is_a_subtask() {
        let mut raw = sample_task();
        raw["parent"] = json!({"gid": "1204500000000000", "name": "Parent"});
        let task = AsanaAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.kind, TaskKind::Subtask);
        assert_eq!(task.ancestor_path, vec!["asana:1204500000000000"]);
    }

    #[test]
    fn datetime_beats_date_only_when_set() {
        let mut raw = sample_task();
        raw["due_at"] = json!("2026-07-01T15:30:00.000Z");
        raw["start_at"] = json!("2026-06-20T09:00:00.000Z");
        let task = AsanaAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.due_date.as_deref(), Some("2026-07-01T15:30:00.000Z"));
        assert_eq!(task.start_date.as_deref(), Some("2026-06-20T09:00:00.000Z"));
    }

    #[test]
    fn sparse_task_maps_with_gaps() {
        let raw = json!({"gid": "1204512345678901", "name": "T"});
        let task = AsanaAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.canonical_id, "asana:1204512345678901");
        assert_eq!(task.status_category, None);
        assert!(task.assignees.is_empty());
        assert!(task.reporter.is_none());
        assert!(task.project_ids.is_empty());
    }

    #[test]
    fn missing_gid_is_an_error() {
        assert!(AsanaAdapter
            .to_canonical(&json!({"name": "no gid"}))
            .is_err());
    }
}
