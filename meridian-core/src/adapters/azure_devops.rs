//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Azure DevOps adapter — reference [`ProviderAdapter`] for ADO (DRAFT).
//!
//! Maps an ADO Work Item (REST `/wit/workitems/{id}` shape, dotted field keys
//! under `fields`) onto [`CanonicalTask`], handling the ADO-specific traps from
//! the six-provider audit:
//!
//! - **Org-namespaced id.** Work-item ids are unique only within an
//!   organization, so the stable key is `{org}:{id}` → `canonical_id`
//!   `azure_devops:{org}:{id}`. The adapter carries the org (falling back to
//!   parsing it from the work-item `url`).
//! - **Priority is `Int` 1–4** (`1` = highest) → explicit lookup.
//! - **Tags are semicolon-delimited** (`System.Tags`) → split into `labels`.
//! - **Status is process-template-dependent.** `System.State` is mapped by name
//!   (Removed → Cancelled, Resolved → InReview, Closed/Done/Completed → Done,
//!   …). NOTE: the same state name can mean different categories across
//!   work-item types/processes (the CMMI "Resolved" trap); full fidelity needs
//!   the process's state-category metadata (the WIT states API). `status_raw`
//!   keeps the literal state regardless.
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

/// Maps ADO work items to [`CanonicalTask`].
///
/// Carries the organization name because work-item ids are not globally unique
/// (so the canonical key must be `{org}:{id}`). If `org` is empty, the adapter
/// falls back to parsing it from the work item's `url`.
#[derive(Debug, Default, Clone)]
pub struct AzureDevopsAdapter {
    pub org: String,
}

impl AzureDevopsAdapter {
    /// Build an adapter pinned to an organization.
    pub fn new(org: impl Into<String>) -> Self {
        Self { org: org.into() }
    }

    /// Resolve the org: the configured one, else parsed from a work-item `url`
    /// like `https://dev.azure.com/{org}/_apis/wit/workItems/{id}`.
    fn resolve_org(&self, raw: &Value) -> String {
        if !self.org.is_empty() {
            return self.org.clone();
        }
        raw.get("url")
            .and_then(Value::as_str)
            .and_then(org_from_url)
            .unwrap_or_default()
    }
}

impl ProviderAdapter for AzureDevopsAdapter {
    fn provider(&self) -> Provider {
        Provider::AzureDevops
    }

    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask> {
        let id = id_str(raw.get("id")).context("azure_devops: work item missing `id`")?;
        let org = self.resolve_org(raw);
        // An empty org would yield a degenerate ":{id}" key (canonical
        // "azure_devops::{id}") — exactly the cross-org collision the
        // namespacing exists to prevent. Refuse rather than emit it.
        if org.is_empty() {
            anyhow::bail!(
                "azure_devops: cannot resolve org for work item {id} \
                 (set AzureDevopsAdapter.org or include a dev.azure.com url); \
                 refusing to emit an unnamespaced canonical id"
            );
        }
        // Org-namespaced stable key (ids aren't globally unique).
        let provider_id = format!("{org}:{id}");

        let wit = field_str(raw, "System.WorkItemType").unwrap_or_default();
        let state = field_str(raw, "System.State").unwrap_or_default();

        let labels = field(raw, "System.Tags")
            .and_then(Value::as_str)
            .map(|s| {
                s.split(';')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let ancestor_path = field(raw, "System.Parent")
            .and_then(|v| id_str(Some(v)))
            .map(|pid| {
                vec![CanonicalTask::canonical_id_for(
                    Provider::AzureDevops,
                    &format!("{org}:{pid}"),
                )]
            })
            .unwrap_or_default();

        let mut custom_fields: BTreeMap<String, Value> = BTreeMap::new();
        if let Some(project) = field_str(raw, "System.TeamProject") {
            custom_fields.insert("team_project".to_string(), json!(project));
        }

        Ok(CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::AzureDevops, &provider_id),
            provider: Provider::AzureDevops,
            provider_id,
            url: str_at(raw, "url").unwrap_or_default(),
            title: field_str(raw, "System.Title").unwrap_or_default(),
            description: field_str(raw, "System.Description").unwrap_or_default(),
            kind: map_kind(&wit),
            kind_raw: wit,
            status_category: map_status(&state),
            status_raw: state,
            priority: map_priority(field(raw, "Microsoft.VSTS.Common.Priority")),
            assignees: person(field(raw, "System.AssignedTo"))
                .into_iter()
                .collect(),
            reporter: person(field(raw, "System.CreatedBy")),
            ancestor_path,
            // System.TeamProject is the project *name*, not an id, so it's kept
            // in custom_fields rather than project_ids (which expects ids).
            project_ids: Vec::new(),
            created_at: field_str(raw, "System.CreatedDate"),
            updated_at: field_str(raw, "System.ChangedDate"),
            completed_at: field_str(raw, "Microsoft.VSTS.Common.ClosedDate"),
            start_date: field_str(raw, "Microsoft.VSTS.Scheduling.StartDate"),
            due_date: field_str(raw, "Microsoft.VSTS.Scheduling.DueDate"),
            labels,
            custom_fields,
            raw_payload: raw.clone(),
        })
    }
}

/// Parse the org segment from a `dev.azure.com/{org}/...` work-item URL.
fn org_from_url(url: &str) -> Option<String> {
    let rest = url.split("dev.azure.com/").nth(1)?;
    let org = rest.split('/').next()?;
    (!org.is_empty()).then(|| org.to_string())
}

/// A work-item id (a JSON number or string) as a string.
fn id_str(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// A top-level non-empty string field.
fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// A value under the dotted `fields` object (`System.Title`, etc.).
fn field<'a>(raw: &'a Value, name: &str) -> Option<&'a Value> {
    raw.get("fields").and_then(|f| f.get(name))
}

/// A non-empty string from the dotted `fields` object.
fn field_str(raw: &Value, name: &str) -> Option<String> {
    field(raw, name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// An ADO identity object → [`PersonRef`]. `uniqueName` is usually the email.
fn person(v: Option<&Value>) -> Option<PersonRef> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let provider_user_id = v
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("descriptor").and_then(Value::as_str))
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
        // uniqueName can be a domain account; only treat it as email if it is one.
        email: v
            .get("uniqueName")
            .and_then(Value::as_str)
            .filter(|s| s.contains('@'))
            .map(String::from),
        display_name,
        provider_user_id,
    })
}

/// Map `System.WorkItemType` → best-effort [`TaskKind`]. Verbatim type is kept
/// in `kind_raw`.
fn map_kind(wit: &str) -> TaskKind {
    match wit.to_ascii_lowercase().as_str() {
        "epic" => TaskKind::Epic,
        "feature" => TaskKind::Feature,
        "user story" | "story" | "product backlog item" => TaskKind::Story,
        "task" => TaskKind::Task,
        "bug" => TaskKind::Bug,
        _ => TaskKind::Other,
    }
}

/// Map `System.State` → canonical category by name. Covers the Agile / Scrum /
/// CMMI defaults. See the module note: the same name can differ by process, so
/// this is best-effort and `status_raw` is authoritative.
fn map_status(state: &str) -> Option<StatusCategory> {
    match state.to_ascii_lowercase().as_str() {
        "new" | "proposed" | "approved" | "to do" | "open" => Some(StatusCategory::Todo),
        "active" | "committed" | "in progress" | "doing" => Some(StatusCategory::InProgress),
        "resolved" => Some(StatusCategory::InReview),
        "removed" => Some(StatusCategory::Cancelled),
        "closed" | "done" | "completed" => Some(StatusCategory::Done),
        _ => None,
    }
}

/// Map ADO `Priority` (`Int` 1–4, `1` highest) → canonical [`Priority`].
fn map_priority(p: Option<&Value>) -> Priority {
    match p.and_then(Value::as_i64) {
        Some(1) => Priority::Urgent,
        Some(2) => Priority::High,
        Some(3) => Priority::Medium,
        Some(4) => Priority::Low,
        _ => Priority::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item() -> Value {
        json!({
            "id": 1234,
            "url": "https://dev.azure.com/acme/_apis/wit/workItems/1234",
            "fields": {
                "System.Title": "Adopt the canonical model",
                "System.Description": "<div>Map ADO work items into CanonicalTask.</div>",
                "System.WorkItemType": "User Story",
                "System.State": "Active",
                "System.Tags": "backend; cdm ; ",
                "System.TeamProject": "Platform",
                "System.Parent": 1000,
                "Microsoft.VSTS.Common.Priority": 2,
                "System.AssignedTo": {
                    "id": "ado-1",
                    "displayName": "Developer",
                    "uniqueName": "developer@example.com"
                },
                "System.CreatedBy": {"id": "ado-2", "displayName": "Lead", "uniqueName": "DOMAIN\\lead"},
                "System.CreatedDate": "2026-06-01T09:00:00Z",
                "System.ChangedDate": "2026-06-28T12:00:00Z",
                "Microsoft.VSTS.Scheduling.StartDate": "2026-06-20T00:00:00Z",
                "Microsoft.VSTS.Scheduling.DueDate": "2026-07-01T00:00:00Z"
            }
        })
    }

    #[test]
    fn maps_a_full_item() {
        let task = AzureDevopsAdapter::new("acme")
            .to_canonical(&sample_item())
            .unwrap();

        // Org-namespaced stable key.
        assert_eq!(task.provider_id, "acme:1234");
        assert_eq!(task.canonical_id, "azure_devops:acme:1234");

        assert_eq!(task.title, "Adopt the canonical model");
        assert_eq!(task.kind, TaskKind::Story);
        assert_eq!(task.kind_raw, "User Story");
        assert_eq!(task.status_raw, "Active");
        assert_eq!(task.status_category, Some(StatusCategory::InProgress));
        assert_eq!(task.priority, Priority::High);

        // Semicolon-delimited tags, trimmed, empties dropped.
        assert_eq!(task.labels, vec!["backend", "cdm"]);

        // Parent is org-namespaced too.
        assert_eq!(task.ancestor_path, vec!["azure_devops:acme:1000"]);
        assert_eq!(
            task.custom_fields.get("team_project"),
            Some(&json!("Platform"))
        );

        assert_eq!(task.assignees.len(), 1);
        assert_eq!(
            task.assignees[0].email.as_deref(),
            Some("developer@example.com")
        );
        // Reporter uniqueName is a domain account, not an email → email None.
        let reporter = task.reporter.as_ref().expect("reporter");
        assert_eq!(reporter.email, None);

        assert_eq!(task.start_date.as_deref(), Some("2026-06-20T00:00:00Z"));
        assert_eq!(task.due_date.as_deref(), Some("2026-07-01T00:00:00Z"));
        assert_eq!(task.completed_at, None); // no ClosedDate
        assert!(!task.is_terminal());
    }

    #[test]
    fn org_falls_back_to_url_when_unset() {
        // Default adapter has empty org → parse from the url.
        let task = AzureDevopsAdapter::default()
            .to_canonical(&sample_item())
            .unwrap();
        assert_eq!(task.provider_id, "acme:1234");
    }

    #[test]
    fn unresolvable_org_is_an_error_not_a_degenerate_key() {
        // No configured org AND no parseable dev.azure.com url → refuse rather
        // than emit "azure_devops::1234".
        let raw = json!({"id": 1234, "fields": {"System.Title": "no org"}});
        assert!(AzureDevopsAdapter::default().to_canonical(&raw).is_err());
    }

    #[test]
    fn status_mapping_covers_the_documented_traps() {
        assert_eq!(map_status("New"), Some(StatusCategory::Todo));
        assert_eq!(map_status("Active"), Some(StatusCategory::InProgress));
        assert_eq!(map_status("Resolved"), Some(StatusCategory::InReview));
        assert_eq!(map_status("Removed"), Some(StatusCategory::Cancelled));
        assert_eq!(map_status("Closed"), Some(StatusCategory::Done));
        assert_eq!(map_status("Completed"), Some(StatusCategory::Done));
        assert_eq!(map_status("Whatever"), None);
    }

    #[test]
    fn priority_is_one_through_four() {
        assert_eq!(map_priority(Some(&json!(1))), Priority::Urgent);
        assert_eq!(map_priority(Some(&json!(2))), Priority::High);
        assert_eq!(map_priority(Some(&json!(3))), Priority::Medium);
        assert_eq!(map_priority(Some(&json!(4))), Priority::Low);
        assert_eq!(map_priority(None), Priority::None);
    }

    #[test]
    fn removed_item_is_terminal() {
        let mut raw = sample_item();
        raw["fields"]["System.State"] = json!("Removed");
        raw["fields"]["Microsoft.VSTS.Common.ClosedDate"] = json!("2026-06-30T10:00:00Z");
        let task = AzureDevopsAdapter::new("acme").to_canonical(&raw).unwrap();
        assert_eq!(task.status_category, Some(StatusCategory::Cancelled));
        assert!(task.is_terminal());
        assert_eq!(task.completed_at.as_deref(), Some("2026-06-30T10:00:00Z"));
    }

    #[test]
    fn missing_id_is_an_error() {
        let raw = json!({"fields": {"System.Title": "no id"}});
        assert!(AzureDevopsAdapter::new("acme").to_canonical(&raw).is_err());
    }
}
