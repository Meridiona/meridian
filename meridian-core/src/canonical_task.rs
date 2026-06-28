//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Canonical task model — the single normalised shape every tracker maps onto.
//!
//! # Who calls this
//! Nothing yet. These types are purely additive (Step 1 of 5 of the CDM +
//! provider-adapters migration). Future ingestion adapters for Jira, Linear,
//! GitHub Projects, Azure DevOps, and Asana will produce [`CanonicalTask`]
//! values and write them into a uniform store.
//!
//! # Related
//! - [`crate::tasks`] — the current `pm_tasks`/[`crate::tasks::TaskSummary`]
//!   shape that this model will eventually feed once the migration lands.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The single normalised task shape every tracker maps onto.
///
/// Normalise a SMALL core; retain everything else raw (`labels` /
/// `custom_fields` / `raw_payload`) so normalisation is never lossy.
/// Not `Eq` because `serde_json::Value` isn't.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalTask {
    /// Deterministic cross-tracker id: `"{provider}:{provider_id}"`.
    /// Always derive with [`CanonicalTask::canonical_id_for`] so the rule
    /// stays consistent.
    pub canonical_id: String,
    pub provider: Provider,
    /// Tracker-native id — the equivalent of today's `pm_tasks.task_key`.
    pub provider_id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub kind: TaskKind,
    /// Tracker's literal status string, verbatim — never normalised away.
    pub status_raw: String,
    pub status_category: StatusCategory,
    pub priority: Priority,
    pub assignee: Option<PersonRef>,
    /// Canonical id of the direct parent task, if any.
    pub parent_id: Option<String>,
    pub epic_id: Option<String>,
    pub epic_title: Option<String>,
    // ISO-8601 UTC strings. DELIBERATE: matches pm_tasks' Option<String> date
    // convention (see readers/tasks.rs) and avoids enabling chrono's non-default
    // `serde` feature, keeping this PR additive with zero Cargo.toml changes.
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub labels: Vec<String>,
    /// Tracker-specific fields that don't map to the canonical core.
    pub custom_fields: BTreeMap<String, Value>,
    /// The full tracker API response, verbatim — normalisation is never lossy.
    pub raw_payload: Value,
}

impl CanonicalTask {
    /// Build the deterministic canonical id for a given provider + tracker-native id.
    ///
    /// Rule: `"{provider_str}:{provider_id}"` where `provider_str` is the
    /// snake_case serde wire form (e.g. `"azure_devops"`). Use this whenever
    /// populating [`CanonicalTask::canonical_id`]; never format it inline.
    pub fn canonical_id_for(provider: Provider, provider_id: &str) -> String {
        format!("{}:{}", provider.as_str(), provider_id)
    }
}

/// The supported task-tracker providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Jira,
    Linear,
    Github,
    AzureDevops,
    Asana,
}

impl Provider {
    /// The snake_case string form — equals the serde wire form.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Jira => "jira",
            Provider::Linear => "linear",
            Provider::Github => "github",
            Provider::AzureDevops => "azure_devops",
            Provider::Asana => "asana",
        }
    }
}

/// Coarse task classification, provider-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Epic,
    Story,
    Task,
    Bug,
    Subtask,
    Other,
}

/// Which lifecycle phase a task is in, regardless of the tracker's own naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
    Cancelled,
}

impl StatusCategory {
    /// True when the task is in a terminal state (no further work expected).
    pub fn is_terminal(self) -> bool {
        matches!(self, StatusCategory::Done | StatusCategory::Cancelled)
    }
}

/// Priority level, normalised across trackers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    None,
    Low,
    Medium,
    High,
    Urgent,
}

/// One human across trackers. `email` is the best cross-tool join key when
/// the tracker exposes it; `provider_user_id` is the fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonRef {
    pub email: Option<String>,
    pub display_name: String,
    pub provider_user_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_id_for_standard_provider() {
        assert_eq!(
            CanonicalTask::canonical_id_for(Provider::Jira, "KAN-42"),
            "jira:KAN-42"
        );
        assert_eq!(
            CanonicalTask::canonical_id_for(Provider::Linear, "ENG-7"),
            "linear:ENG-7"
        );
    }

    #[test]
    fn canonical_id_for_azure_devops_uses_snake_case_prefix() {
        let id = CanonicalTask::canonical_id_for(Provider::AzureDevops, "12345");
        assert!(
            id.starts_with("azure_devops:"),
            "expected azure_devops: prefix, got {id}"
        );
        assert_eq!(id, "azure_devops:12345");
    }

    #[test]
    fn provider_as_str_equals_serde_wire_form() {
        let cases = [
            Provider::Jira,
            Provider::Linear,
            Provider::Github,
            Provider::AzureDevops,
            Provider::Asana,
        ];
        for provider in cases {
            let wire = serde_json::to_string(&provider).expect("serialize provider");
            // serde produces a quoted string; strip quotes.
            let wire = wire.trim_matches('"');
            assert_eq!(
                wire,
                provider.as_str(),
                "as_str() must match serde wire form for {provider:?}"
            );
        }
    }

    #[test]
    fn status_category_is_terminal_only_for_done_and_cancelled() {
        assert!(StatusCategory::Done.is_terminal());
        assert!(StatusCategory::Cancelled.is_terminal());

        assert!(!StatusCategory::Backlog.is_terminal());
        assert!(!StatusCategory::Todo.is_terminal());
        assert!(!StatusCategory::InProgress.is_terminal());
        assert!(!StatusCategory::InReview.is_terminal());
    }

    #[test]
    fn enum_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&StatusCategory::InProgress).unwrap(),
            r#""in_progress""#
        );
        assert_eq!(
            serde_json::to_string(&Provider::AzureDevops).unwrap(),
            r#""azure_devops""#
        );
        assert_eq!(
            serde_json::to_string(&TaskKind::Subtask).unwrap(),
            r#""subtask""#
        );
        assert_eq!(
            serde_json::to_string(&Priority::Urgent).unwrap(),
            r#""urgent""#
        );
    }

    #[test]
    fn canonical_task_round_trips_through_json() {
        let task = CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::Linear, "ENG-99"),
            provider: Provider::Linear,
            provider_id: "ENG-99".to_string(),
            url: "https://linear.app/team/issue/ENG-99".to_string(),
            title: "Implement canonical task model".to_string(),
            description: "Add CanonicalTask + supporting enums to meridian-core.".to_string(),
            kind: TaskKind::Task,
            status_raw: "In Progress".to_string(),
            status_category: StatusCategory::InProgress,
            priority: Priority::High,
            assignee: Some(PersonRef {
                email: Some("akarsh@meridiona.com".to_string()),
                display_name: "Akarsh".to_string(),
                provider_user_id: "usr_abc123".to_string(),
            }),
            parent_id: Some(CanonicalTask::canonical_id_for(Provider::Linear, "ENG-50")),
            epic_id: Some(CanonicalTask::canonical_id_for(Provider::Linear, "ENG-50")),
            epic_title: Some("CDM migration".to_string()),
            created_at: Some("2026-06-01T09:00:00Z".to_string()),
            updated_at: Some("2026-06-28T12:00:00Z".to_string()),
            start_date: Some("2026-06-20".to_string()),
            due_date: Some("2026-07-01".to_string()),
            labels: vec!["backend".to_string(), "core".to_string()],
            custom_fields: {
                let mut m = BTreeMap::new();
                m.insert("story_points".to_string(), json!(3));
                m
            },
            raw_payload: json!({
                "id": "ENG-99",
                "team": {"key": "ENG"},
                "stateType": "started"
            }),
        };

        let json = serde_json::to_string(&task).expect("serialize");
        let restored: CanonicalTask = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(task, restored);

        // Spot-check a few fields after the round-trip.
        assert_eq!(restored.canonical_id, "linear:ENG-99");
        assert_eq!(restored.labels, vec!["backend", "core"]);
        assert_eq!(restored.custom_fields.get("story_points"), Some(&json!(3)));
        assert!(!restored.status_category.is_terminal());
    }
}
