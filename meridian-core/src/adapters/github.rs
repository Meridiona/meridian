//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! GitHub adapter — reference [`ProviderAdapter`] for GitHub Projects v2 issues.
//!
//! Maps a Projects v2 item node (the shape the daemon's `PROJECT_ITEMS_QUERY`
//! fetches: `{type, fieldValues, content, project}` with the Issue under
//! `content`) — or a bare Issue object — onto [`CanonicalTask`], handling the
//! GitHub-specific traps from the six-provider audit:
//!
//! - **Stable id = the Issue's global node `id`** (`I_kwDO…`). The human
//!   `owner/repo#number` key is kept in `custom_fields` (`repo` + `number`).
//! - **No native status category while open.** The board's "Status" column is a
//!   user-defined single-select with no semantics — it feeds `status_raw`
//!   verbatim and `status_category` stays `None` for OPEN issues. CLOSED issues
//!   DO carry a reliable signal: `stateReason` `COMPLETED` → Done,
//!   `NOT_PLANNED`/`DUPLICATE` → Cancelled (plain CLOSED → Done).
//! - **Issue types are best-effort.** Org-level issue types surface as
//!   `issueType.name` (user-customizable); absent on classic issues → plain
//!   [`TaskKind::Task`] with empty `kind_raw` (the Linear convention).
//! - **Up to 10 assignees** → the multi-assignee `Vec<PersonRef>`.
//! - **Sub-issues**: `parent.id` → a 1-element `ancestor_path` (immediate
//!   parent only — walking the full 8-deep chain needs extra API calls the
//!   adapter, being pure, cannot make).
//! - **No native start/due dates** (Projects v2 date columns are custom
//!   fields) → both `None`.
//!
//! # Who calls this
//! `cdm_columns()` in the daemon's GitHub connector
//! (`src/intelligence/providers/github.rs`) at upsert time.
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

/// Maps GitHub Projects v2 issue items to [`CanonicalTask`].
///
/// Config-less: the item node carries everything needed (global ids, urls,
/// the board Status column) so there's nothing per-installation to thread in.
#[derive(Debug, Default, Clone, Copy)]
pub struct GithubAdapter;

impl ProviderAdapter for GithubAdapter {
    fn provider(&self) -> Provider {
        Provider::Github
    }

    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask> {
        // A Projects v2 item nests the Issue under `content`; a bare Issue
        // object is also accepted (`content` absent → the raw IS the issue).
        let issue = raw.get("content").filter(|c| !c.is_null()).unwrap_or(raw);

        let provider_id = issue
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .context("github: issue missing global node `id`")?
            .to_string();

        let mut custom_fields: BTreeMap<String, Value> = BTreeMap::new();
        // Human-facing key parts kept retrievable; the node id stays the key.
        if let Some(repo) = issue
            .pointer("/repository/nameWithOwner")
            .and_then(Value::as_str)
        {
            custom_fields.insert("repo".to_string(), json!(repo));
        }
        if let Some(number) = issue.get("number").and_then(Value::as_u64) {
            custom_fields.insert("number".to_string(), json!(number));
        }
        if let Some(milestone) = issue
            .pointer("/milestone/title")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            custom_fields.insert("milestone".to_string(), json!(milestone));
        }

        let state = issue
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // Board Status column beats the bare OPEN/CLOSED as the user-facing
        // status; the native state still drives the category below.
        let board_status = board_status(raw);
        let status_raw = board_status.clone().unwrap_or_else(|| state.to_string());

        let (kind, kind_raw) = match issue
            .pointer("/issueType/name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(name) => (map_kind(name), name.to_string()),
            // Classic issues have no type concept — the Linear convention.
            None => (TaskKind::Task, String::new()),
        };

        let assignees = issue
            .pointer("/assignees/nodes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|n| person(Some(n))).collect())
            .unwrap_or_default();

        let ancestor_path = issue
            .pointer("/parent/id")
            .and_then(Value::as_str)
            .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Github, pid)])
            .unwrap_or_default();

        // The board the item sits on, when the raw is a full item node.
        let project_ids = raw
            .pointer("/project/id")
            .and_then(Value::as_str)
            .map(|pid| vec![CanonicalTask::canonical_id_for(Provider::Github, pid)])
            .unwrap_or_default();

        let labels = issue
            .pointer("/labels/nodes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.get("name").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::Github, &provider_id),
            provider: Provider::Github,
            provider_id,
            url: str_field(issue, "url").unwrap_or_default(),
            title: str_field(issue, "title").unwrap_or_default(),
            description: str_field(issue, "body").unwrap_or_default(),
            kind,
            kind_raw,
            status_raw,
            status_category: map_status(state, issue.get("stateReason").and_then(Value::as_str)),
            // GitHub has no native issue priority (board Priority columns are
            // custom fields with no fixed semantics).
            priority: Priority::None,
            assignees,
            reporter: person(issue.get("author")),
            ancestor_path,
            project_ids,
            created_at: str_field(issue, "createdAt"),
            updated_at: str_field(issue, "updatedAt"),
            completed_at: str_field(issue, "closedAt"),
            // No native start/due dates — Projects v2 date columns are custom.
            start_date: None,
            due_date: None,
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

/// The Projects v2 "Status" single-select value from the item's `fieldValues`,
/// if the raw is a full item node and the column is set.
fn board_status(raw: &Value) -> Option<String> {
    let nodes = raw.pointer("/fieldValues/nodes")?.as_array()?;
    for fv in nodes {
        let field_name = fv.pointer("/field/name").and_then(Value::as_str);
        let value_name = fv.get("name").and_then(Value::as_str);
        if let (Some(f), Some(v)) = (field_name, value_name) {
            if f.eq_ignore_ascii_case("status") && !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// A GitHub user node → [`PersonRef`]. `None` for null/absent (e.g. a deleted
/// author) or an empty node.
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
    // Prefer the profile name; fall back to the login handle.
    let display_name = v
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| v.get("login").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    if provider_user_id.is_empty() && display_name.is_empty() {
        return None;
    }
    Some(PersonRef {
        // Public profile email — usually absent without the user:email scope.
        email: v
            .get("email")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        display_name,
        provider_user_id,
    })
}

/// Map an org-level issue-type name → best-effort [`TaskKind`]. Verbatim name
/// is kept in `kind_raw`.
fn map_kind(name: &str) -> TaskKind {
    match name.to_ascii_lowercase().as_str() {
        "epic" => TaskKind::Epic,
        "feature" => TaskKind::Feature,
        "story" | "user story" => TaskKind::Story,
        "task" => TaskKind::Task,
        "bug" => TaskKind::Bug,
        "sub-task" | "subtask" | "sub-issue" => TaskKind::Subtask,
        _ => TaskKind::Other,
    }
}

/// Category from the native state + stateReason. OPEN issues have NO derivable
/// category (board columns are user-defined) → `None`; CLOSED ones do:
/// `COMPLETED` → Done, `NOT_PLANNED`/`DUPLICATE` → Cancelled, reason-less
/// CLOSED → Done.
fn map_status(state: &str, state_reason: Option<&str>) -> Option<StatusCategory> {
    match state {
        "CLOSED" => match state_reason {
            Some("NOT_PLANNED") | Some("DUPLICATE") => Some(StatusCategory::Cancelled),
            _ => Some(StatusCategory::Done),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full Projects v2 item node, as the daemon's query returns it.
    fn sample_item() -> Value {
        json!({
            "type": "ISSUE",
            "project": {"id": "PVT_kwHOproj"},
            "fieldValues": {"nodes": [
                {"name": "In Review", "field": {"name": "Status"}},
                {"name": "High", "field": {"name": "Priority"}}
            ]},
            "content": {
                "id": "I_kwDOabc123",
                "number": 42,
                "title": "Adopt the canonical model",
                "body": "Map GitHub issues into CanonicalTask.",
                "state": "OPEN",
                "stateReason": null,
                "url": "https://github.com/acme/repo/issues/42",
                "createdAt": "2026-06-01T09:00:00Z",
                "updatedAt": "2026-06-28T12:00:00Z",
                "closedAt": null,
                "repository": {"nameWithOwner": "acme/repo"},
                "issueType": {"name": "Bug"},
                "author": {"id": "U_lead", "login": "lead", "name": "Lead"},
                "parent": {"id": "I_kwDOparent"},
                "milestone": {"title": "v2.0"},
                "assignees": {"nodes": [
                    {"id": "U_dev", "login": "dev", "name": "Developer"},
                    {"id": "U_pair", "login": "pair"}
                ]},
                "labels": {"nodes": [{"name": "backend"}, {"name": "cdm"}]}
            }
        })
    }

    #[test]
    fn maps_a_full_item() {
        let task = GithubAdapter.to_canonical(&sample_item()).unwrap();

        // Global node id is the stable key; human parts live in custom_fields.
        assert_eq!(task.provider_id, "I_kwDOabc123");
        assert_eq!(task.canonical_id, "github:I_kwDOabc123");
        assert_eq!(task.custom_fields.get("repo"), Some(&json!("acme/repo")));
        assert_eq!(task.custom_fields.get("number"), Some(&json!(42)));
        assert_eq!(task.custom_fields.get("milestone"), Some(&json!("v2.0")));

        // Board Status column is the user-facing status; OPEN → no category.
        assert_eq!(task.status_raw, "In Review");
        assert_eq!(task.status_category, None);

        // Org issue type mapped, verbatim kept.
        assert_eq!(task.kind, TaskKind::Bug);
        assert_eq!(task.kind_raw, "Bug");

        // No native priority even though the board has a Priority column.
        assert_eq!(task.priority, Priority::None);

        // Multi-assignee; name falls back to login for the pair.
        assert_eq!(task.assignees.len(), 2);
        assert_eq!(task.assignees[0].display_name, "Developer");
        assert_eq!(task.assignees[1].display_name, "pair");

        assert_eq!(task.reporter.as_ref().unwrap().display_name, "Lead");
        assert_eq!(task.ancestor_path, vec!["github:I_kwDOparent"]);
        assert_eq!(task.project_ids, vec!["github:PVT_kwHOproj"]);
        assert_eq!(task.labels, vec!["backend", "cdm"]);
        assert_eq!(task.completed_at, None);
        assert!(!task.is_terminal());
    }

    #[test]
    fn bare_issue_without_item_wrapper_also_maps() {
        // The `content` object alone (no fieldValues/project) must still map —
        // status falls back to the native state, project_ids stays empty.
        let raw = sample_item()["content"].clone();
        let task = GithubAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.provider_id, "I_kwDOabc123");
        assert_eq!(task.status_raw, "OPEN");
        assert!(task.project_ids.is_empty());
    }

    #[test]
    fn closed_state_reason_drives_the_category() {
        let closed = |reason: Value| {
            let mut raw = sample_item();
            raw["content"]["state"] = json!("CLOSED");
            raw["content"]["stateReason"] = reason;
            raw["content"]["closedAt"] = json!("2026-06-30T10:00:00Z");
            GithubAdapter.to_canonical(&raw).unwrap()
        };
        let done = closed(json!("COMPLETED"));
        assert_eq!(done.status_category, Some(StatusCategory::Done));
        assert!(done.is_terminal());
        assert_eq!(done.completed_at.as_deref(), Some("2026-06-30T10:00:00Z"));

        let cancelled = closed(json!("NOT_PLANNED"));
        assert_eq!(cancelled.status_category, Some(StatusCategory::Cancelled));

        // Reason-less CLOSED (pre-stateReason data) → Done.
        let legacy = closed(Value::Null);
        assert_eq!(legacy.status_category, Some(StatusCategory::Done));
    }

    #[test]
    fn classic_issue_without_type_is_a_plain_task() {
        let mut raw = sample_item();
        raw["content"]["issueType"] = Value::Null;
        let task = GithubAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.kind, TaskKind::Task);
        assert_eq!(task.kind_raw, "");
    }

    #[test]
    fn status_column_lookup_is_case_insensitive_and_skips_other_fields() {
        let mut raw = sample_item();
        raw["fieldValues"]["nodes"] = json!([
            {"name": "High", "field": {"name": "Priority"}},
            {"name": "Shipped", "field": {"name": "STATUS"}}
        ]);
        let task = GithubAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.status_raw, "Shipped");
    }

    #[test]
    fn missing_id_is_an_error() {
        assert!(GithubAdapter
            .to_canonical(&json!({"content": {"title": "no id"}}))
            .is_err());
        assert!(GithubAdapter.to_canonical(&json!({})).is_err());
    }
}
