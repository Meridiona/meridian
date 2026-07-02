//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Canonical task model — the single normalised shape every tracker maps onto.
//!
//! The shape was revised after a six-provider API audit (Jira, Linear, GitHub
//! Projects, Azure DevOps, Asana, Trello) so adapters and the Step-3 migration
//! lock in a representation that none of those trackers silently lose data
//! against. The guiding rule is unchanged: normalise a SMALL best-effort core,
//! keep everything verbatim alongside it (`*_raw`, `labels`, `custom_fields`,
//! `raw_payload`) so normalisation is never lossy.
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
/// Normalise a SMALL best-effort core; retain everything else raw (`*_raw`,
/// `labels`, `custom_fields`, `raw_payload`) so normalisation is never lossy.
/// Several trackers expose no native concept for a normalised field (e.g.
/// GitHub/Trello/Asana have no status *category*, Asana/GitHub have no native
/// priority); in those cases the normalised field is best-effort while the
/// verbatim `*_raw` companion is always populated.
///
/// Not `Eq` because `serde_json::Value` isn't.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalTask {
    /// Deterministic cross-tracker id: `"{provider}:{provider_id}"`.
    /// Always derive with [`CanonicalTask::canonical_id_for`] so the rule
    /// stays consistent. `provider_id` may itself contain colons for
    /// namespacing (e.g. Azure DevOps `"{org}:{id}"`), so recover the provider
    /// by splitting on the FIRST colon only.
    pub canonical_id: String,
    pub provider: Provider,
    /// Tracker-native id used as the STABLE key — pick the immutable one:
    /// Jira numeric id (not the re-keyable `KAN-42`), Linear UUID, the GitHub
    /// global node id, ADO `"{org}:{id}"`. Human-facing keys live in
    /// `custom_fields`/`raw_payload`.
    pub provider_id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    /// Best-effort normalised type. Most trackers have no type field (Linear/
    /// Asana/Trello/classic GitHub) or a user-customizable one (Jira/ADO/GitHub
    /// issue-types), so this collapses unknowns into [`TaskKind::Other`].
    pub kind: TaskKind,
    /// The tracker's literal type string, verbatim — the lossless companion to
    /// `kind` (mirrors `status_raw`). Empty when the tracker has no type field.
    pub kind_raw: String,
    /// Tracker's literal status string, verbatim — never normalised away.
    pub status_raw: String,
    /// Best-effort normalised lifecycle phase. `None` when the tracker exposes
    /// no native status category (GitHub/Trello/Asana) and none can be derived;
    /// `status_raw` is always populated regardless.
    pub status_category: Option<StatusCategory>,
    /// Normalised priority. [`Priority::None`] doubles as "unset" and "this
    /// tracker has no priority concept" (Asana/GitHub Projects native).
    pub priority: Priority,
    /// Everyone the task is assigned to. A `Vec` because GitHub issues allow up
    /// to 10 assignees and Trello cards carry a multi-member `idMembers`;
    /// single-assignee trackers (Jira/Linear/ADO/Asana) emit a 1-element vec.
    pub assignees: Vec<PersonRef>,
    /// The creator/reporter — first-class in Jira/ADO/Linear, needed for
    /// worklog attribution. `None` when the tracker doesn't expose it.
    pub reporter: Option<PersonRef>,
    /// Canonical ids of the task's ancestors, ordered root-first (index 0 is
    /// the top-most ancestor, the last element is the direct parent). A `Vec`
    /// because hierarchies are deep and variable: ADO is 4-level
    /// (Epic→Feature→Story→Task), GitHub sub-issues nest up to 8. Empty for a
    /// top-level item.
    pub ancestor_path: Vec<String>,
    /// Canonical ids of the project(s)/board(s) the task belongs to. A `Vec`
    /// because Asana/Linear tasks can live in multiple projects. Kept distinct
    /// from `ancestor_path` so "project" is never conflated with "epic".
    pub project_ids: Vec<String>,
    // ISO-8601 UTC strings. DELIBERATE: matches pm_tasks' Option<String> date
    // convention (see readers/tasks.rs) and avoids enabling chrono's non-default
    // `serde` feature, keeping this PR additive with zero Cargo.toml changes.
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// When the task entered a terminal state (done or cancelled). Distinct
    /// from `is_terminal` (which is a yes/no derived from `status_category`):
    /// this carries the timestamp Linear/ADO/GitHub expose as
    /// completedAt/canceledAt/closedAt.
    pub completed_at: Option<String>,
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
    ///
    /// `provider_id` may itself contain colons (e.g. ADO `"{org}:{id}"`), which
    /// is fine — consumers recover the provider by splitting on the FIRST colon.
    pub fn canonical_id_for(provider: Provider, provider_id: &str) -> String {
        format!("{}:{}", provider.as_str(), provider_id)
    }

    /// Whether the task is in a terminal state. `false` when `status_category`
    /// is unknown (`None`).
    pub fn is_terminal(&self) -> bool {
        self.status_category
            .is_some_and(StatusCategory::is_terminal)
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
    Trello,
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
            Provider::Trello => "trello",
        }
    }
}

/// Coarse task classification, provider-agnostic. Best-effort — the verbatim
/// type is always preserved in [`CanonicalTask::kind_raw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Epic,
    /// ADO portfolio level between Epic and Story; also used by trackers that
    /// surface a distinct "Feature" work-item type.
    Feature,
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

    /// A fully-populated task used as the round-trip baseline; tests tweak
    /// individual fields off this.
    fn sample_task() -> CanonicalTask {
        CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::Linear, "uuid-99"),
            provider: Provider::Linear,
            provider_id: "uuid-99".to_string(),
            url: "https://linear.app/team/issue/ENG-99".to_string(),
            title: "Implement canonical task model".to_string(),
            description: "Add CanonicalTask + supporting enums to meridian-core.".to_string(),
            kind: TaskKind::Task,
            kind_raw: "Task".to_string(),
            status_raw: "In Progress".to_string(),
            status_category: Some(StatusCategory::InProgress),
            priority: Priority::High,
            assignees: vec![
                PersonRef {
                    email: Some("developer@example.com".to_string()),
                    display_name: "Developer".to_string(),
                    provider_user_id: "usr_abc123".to_string(),
                },
                PersonRef {
                    email: None,
                    display_name: "Pair".to_string(),
                    provider_user_id: "usr_def456".to_string(),
                },
            ],
            reporter: Some(PersonRef {
                email: Some("lead@example.com".to_string()),
                display_name: "Lead".to_string(),
                provider_user_id: "usr_lead".to_string(),
            }),
            ancestor_path: vec![
                CanonicalTask::canonical_id_for(Provider::Linear, "uuid-epic"),
                CanonicalTask::canonical_id_for(Provider::Linear, "uuid-story"),
            ],
            project_ids: vec![
                CanonicalTask::canonical_id_for(Provider::Linear, "proj-1"),
                CanonicalTask::canonical_id_for(Provider::Linear, "proj-2"),
            ],
            created_at: Some("2026-06-01T09:00:00Z".to_string()),
            updated_at: Some("2026-06-28T12:00:00Z".to_string()),
            completed_at: None,
            start_date: Some("2026-06-20".to_string()),
            due_date: Some("2026-07-01".to_string()),
            labels: vec!["backend".to_string(), "core".to_string()],
            custom_fields: {
                let mut m = BTreeMap::new();
                m.insert("story_points".to_string(), json!(3));
                m
            },
            raw_payload: json!({
                "id": "uuid-99",
                "team": {"key": "ENG"},
                "stateType": "started"
            }),
        }
    }

    #[test]
    fn canonical_id_for_standard_provider() {
        assert_eq!(
            CanonicalTask::canonical_id_for(Provider::Jira, "10042"),
            "jira:10042"
        );
        assert_eq!(
            CanonicalTask::canonical_id_for(Provider::Linear, "uuid-7"),
            "linear:uuid-7"
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
    fn canonical_id_for_namespaced_provider_id_keeps_provider_recoverable() {
        // ADO ids namespace on org → provider_id itself carries a colon.
        let id = CanonicalTask::canonical_id_for(Provider::AzureDevops, "myorg:1234");
        assert_eq!(id, "azure_devops:myorg:1234");
        // Recover the provider by splitting on the FIRST colon.
        assert_eq!(id.split_once(':').map(|(p, _)| p), Some("azure_devops"));
    }

    #[test]
    fn provider_as_str_equals_serde_wire_form() {
        let cases = [
            Provider::Jira,
            Provider::Linear,
            Provider::Github,
            Provider::AzureDevops,
            Provider::Asana,
            Provider::Trello,
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
    fn task_is_terminal_handles_unknown_category() {
        let mut t = sample_task();
        t.status_category = Some(StatusCategory::Done);
        assert!(t.is_terminal());
        t.status_category = Some(StatusCategory::InProgress);
        assert!(!t.is_terminal());
        // Unknown category → not terminal.
        t.status_category = None;
        assert!(!t.is_terminal());
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
            serde_json::to_string(&TaskKind::Feature).unwrap(),
            r#""feature""#
        );
        assert_eq!(
            serde_json::to_string(&Priority::Urgent).unwrap(),
            r#""urgent""#
        );
    }

    #[test]
    fn canonical_task_round_trips_through_json() {
        let task = sample_task();
        let json = serde_json::to_string(&task).expect("serialize");
        let restored: CanonicalTask = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(task, restored);

        // Spot-check a few fields after the round-trip.
        assert_eq!(restored.canonical_id, "linear:uuid-99");
        assert_eq!(restored.assignees.len(), 2);
        assert_eq!(restored.ancestor_path.len(), 2);
        assert_eq!(restored.project_ids.len(), 2);
        assert_eq!(restored.labels, vec!["backend", "core"]);
        assert_eq!(restored.custom_fields.get("story_points"), Some(&json!(3)));
        assert!(!restored.is_terminal());
    }

    #[test]
    fn round_trips_with_best_effort_gaps() {
        // The GitHub/Asana shape: no native status category, no assignee, no
        // reporter, no type string — the lossless raw companions still carry it.
        let mut task = sample_task();
        task.provider = Provider::Github;
        task.status_category = None;
        task.kind = TaskKind::Other;
        task.kind_raw = String::new();
        task.assignees = Vec::new();
        task.reporter = None;
        task.ancestor_path = Vec::new();
        task.priority = Priority::None;

        let json = serde_json::to_string(&task).expect("serialize");
        let restored: CanonicalTask = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(task, restored);
        assert!(restored.status_category.is_none());
        assert!(restored.assignees.is_empty());
        assert!(restored.reporter.is_none());
        assert!(!restored.is_terminal());
    }
}
