//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Ticket write-back. The board-hygiene UI surfaces a defect (no due date, no
// assignee, vague title, …) and an in-app fix; applying that fix has to land on
// the user's REAL tracker, not just Meridian's mirror. This module turns a
// (provider, key, field, value) request into the right provider API call.
//
// The contract the UI relies on: every apply returns either `Applied` (the write
// landed on the tracker) or `Redirected` (this provider has no API for that field
// — open the ticket in the tracker instead). The dialog renders an in-app control
// for Applied fields and an "Open in tracker ↗" link for Redirected ones, which is
// exactly the user's rule: do as much as possible in our UI, redirect only what
// genuinely cannot be done here.
//
// Auth + the API base are reused from each provider's existing fetch/worklog
// plumbing (`oauth::jira::resolve`, `LinearConfig.api_key`, …) — this module adds
// no new credential handling.

pub mod azure_devops;
pub mod github;
pub mod jira;
pub mod linear;
pub mod parents;
pub mod trello;

use anyhow::{bail, Result};

use crate::config::{Config, PmProviderConfig};

/// A single field write the dev asked for in the hygiene UI. Parsed from the
/// (field, value) pair the UI control produces; every provider maps these to its
/// own API shape (or declines via `Redirected`).
#[derive(Debug, Clone, PartialEq)]
pub enum WriteField {
    /// `duedate` control — ISO `YYYY-MM-DD`.
    DueDate(String),
    /// `assignee` control — always "assign to me"; the provider resolves the
    /// current user's id from its own `/myself`/`viewer` endpoint.
    AssignMe,
    /// `labels` control — ADD one label (never clobbers existing labels).
    AddLabel(String),
    /// `priority` control — the human priority name (`High`, `Medium`, …).
    Priority(String),
    /// `story_points` control — numeric estimate.
    StoryPoints(f64),
    /// `parent` control — parent/epic key to link under.
    Parent(String),
    /// `summary` control — new title text.
    Summary(String),
    /// `description` control — new description body (plain text / Markdown).
    Description(String),
    /// ticket-level "close" action — transition to the provider's done state.
    Close,
    /// ticket-level "cancel" action — transition to the provider's cancelled/won't-do state.
    Cancel,
}

impl WriteField {
    /// Parse the UI's `(field, value)` into a `WriteField`. `acceptance_criteria`
    /// has no standard field on any tracker, so it is reported as unparseable here
    /// and the caller redirects it.
    pub fn parse(field: &str, value: &str) -> Option<Self> {
        let v = value.trim();
        match field {
            "duedate" => Some(Self::DueDate(v.to_string())),
            "assignee" => Some(Self::AssignMe),
            "labels" => (!v.is_empty()).then(|| Self::AddLabel(v.to_string())),
            "priority" => (!v.is_empty()).then(|| Self::Priority(v.to_string())),
            "story_points" => v.parse::<f64>().ok().map(Self::StoryPoints),
            "parent" => (!v.is_empty()).then(|| Self::Parent(v.to_string())),
            "summary" => (!v.is_empty()).then(|| Self::Summary(v.to_string())),
            "description" => (!v.is_empty()).then(|| Self::Description(v.to_string())),
            "close" => Some(Self::Close),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }

    /// Short label for logs / the UI result line.
    pub fn label(&self) -> &'static str {
        match self {
            Self::DueDate(_) => "due date",
            Self::AssignMe => "assignee",
            Self::AddLabel(_) => "label",
            Self::Priority(_) => "priority",
            Self::StoryPoints(_) => "estimate",
            Self::Parent(_) => "parent",
            Self::Summary(_) => "title",
            Self::Description(_) => "description",
            Self::Close => "status",
            Self::Cancel => "cancel",
        }
    }
}

/// Outcome of an apply. `Redirected` carries the human URL to open instead.
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyStatus {
    /// The write landed on the tracker.
    Applied,
    /// This provider has no API for this field — open the ticket in the tracker.
    Redirected { browse_url: String, reason: String },
}

/// Structured result the CLI serialises to JSON for the UI.
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub provider: String,
    pub key: String,
    pub field: String,
    pub status: ApplyStatus,
}

impl ApplyResult {
    pub fn applied(provider: &str, key: &str, field: &str) -> Self {
        Self {
            provider: provider.to_string(),
            key: key.to_string(),
            field: field.to_string(),
            status: ApplyStatus::Applied,
        }
    }

    pub fn redirected(provider: &str, key: &str, field: &str, url: String, reason: &str) -> Self {
        Self {
            provider: provider.to_string(),
            key: key.to_string(),
            field: field.to_string(),
            status: ApplyStatus::Redirected {
                browse_url: url,
                reason: reason.to_string(),
            },
        }
    }

    /// Serialise for the CLI → UI hop. Kept hand-rolled (no serde derive) so the
    /// shape is obvious at the call site.
    pub fn to_json(&self) -> serde_json::Value {
        match &self.status {
            ApplyStatus::Applied => serde_json::json!({
                "provider": self.provider,
                "key": self.key,
                "field": self.field,
                "status": "applied",
            }),
            ApplyStatus::Redirected { browse_url, reason } => serde_json::json!({
                "provider": self.provider,
                "key": self.key,
                "field": self.field,
                "status": "redirected",
                "browse_url": browse_url,
                "reason": reason,
            }),
        }
    }
}

/// Apply one field write to the named provider's copy of `key`. Looks up the
/// provider's credentials from `config`, parses the field, and dispatches.
pub async fn apply(
    config: &Config,
    provider: &str,
    key: &str,
    field: &str,
    value: &str,
) -> Result<ApplyResult> {
    let pcfg = config
        .pm_providers
        .iter()
        .find(|p| p.provider_name() == provider)
        .or_else(|| {
            // Jira may be OAuth-only (no env config row) yet still configured.
            (provider == "jira")
                .then(|| {
                    config
                        .pm_providers
                        .iter()
                        .find(|p| matches!(p, PmProviderConfig::Jira(_)))
                })
                .flatten()
        });

    let pcfg = match pcfg {
        Some(p) => p,
        None => bail!("provider {provider:?} is not configured"),
    };

    let write = match WriteField::parse(field, value) {
        Some(w) => w,
        // No standard field for this defect (e.g. acceptance_criteria) — redirect.
        None => {
            let url = browse_url(pcfg, key);
            return Ok(ApplyResult::redirected(
                provider,
                key,
                field,
                url,
                "no standard tracker field — edit it directly in the tracker",
            ));
        }
    };

    match pcfg {
        PmProviderConfig::Jira(cfg) => jira::apply(cfg, key, &write).await,
        PmProviderConfig::Linear(cfg) => linear::apply(cfg, key, &write).await,
        PmProviderConfig::GitHub(cfg) => github::apply(cfg, key, &write).await,
        PmProviderConfig::Trello(cfg) => trello::apply(cfg, key, &write).await,
        PmProviderConfig::AzureDevOps(cfg) => azure_devops::apply(cfg, key, &write).await,
    }
}

/// Best-effort human browse URL for the redirect fallback, without an auth round
/// trip. Per-provider modules build a better one when they already hold a ctx.
fn browse_url(pcfg: &PmProviderConfig, key: &str) -> String {
    match pcfg {
        PmProviderConfig::Jira(c) => {
            if c.base_url.is_empty() {
                String::new() // OAuth-only — UI already has the row's url
            } else {
                format!("{}/browse/{}", c.base_url.trim_end_matches('/'), key)
            }
        }
        PmProviderConfig::Linear(_) => format!("https://linear.app/issue/{key}"),
        PmProviderConfig::GitHub(_) => String::new(),
        PmProviderConfig::Trello(_) => format!("https://trello.com/c/{key}"),
        PmProviderConfig::AzureDevOps(c) => {
            if c.api_base.is_empty() {
                String::new()
            } else {
                format!(
                    "{}/{}/_workitems/edit/{}",
                    c.api_base.trim_end_matches('/'),
                    c.project,
                    key
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_control() {
        assert_eq!(
            WriteField::parse("duedate", "2026-06-30"),
            Some(WriteField::DueDate("2026-06-30".into()))
        );
        assert_eq!(
            WriteField::parse("assignee", "@me"),
            Some(WriteField::AssignMe)
        );
        assert_eq!(
            WriteField::parse("labels", "backend"),
            Some(WriteField::AddLabel("backend".into()))
        );
        assert_eq!(
            WriteField::parse("priority", "High"),
            Some(WriteField::Priority("High".into()))
        );
        assert_eq!(
            WriteField::parse("story_points", "3"),
            Some(WriteField::StoryPoints(3.0))
        );
        assert_eq!(
            WriteField::parse("parent", "KAN-5"),
            Some(WriteField::Parent("KAN-5".into()))
        );
        assert_eq!(WriteField::parse("close", ""), Some(WriteField::Close));
        assert_eq!(WriteField::parse("cancel", ""), Some(WriteField::Cancel));
    }

    #[test]
    fn rejects_unwritable_fields() {
        assert_eq!(WriteField::parse("acceptance_criteria", "x"), None);
        assert_eq!(WriteField::parse("labels", "  "), None);
        assert_eq!(WriteField::parse("story_points", "abc"), None);
    }

    #[test]
    fn parses_description_and_summary() {
        // These are the most common hygiene fixes — a regression here silently
        // redirects every description/title write to the tracker instead of applying.
        assert_eq!(
            WriteField::parse("description", "Add a description"),
            Some(WriteField::Description("Add a description".into()))
        );
        assert_eq!(
            WriteField::parse("summary", "New title"),
            Some(WriteField::Summary("New title".into()))
        );
        // Empty description / summary must not produce a field (avoids overwriting
        // with blank text).
        assert_eq!(WriteField::parse("description", "  "), None);
        assert_eq!(WriteField::parse("summary", ""), None);
    }

    #[test]
    fn writefield_trims_whitespace() {
        // The UI may send values with surrounding spaces.
        assert_eq!(
            WriteField::parse("duedate", "  2026-06-30  "),
            Some(WriteField::DueDate("2026-06-30".into()))
        );
        assert_eq!(
            WriteField::parse("priority", "  High  "),
            Some(WriteField::Priority("High".into()))
        );
    }

    #[test]
    fn browse_url_linear() {
        let pcfg = PmProviderConfig::Linear(crate::config::LinearConfig {
            api_key: "k".into(),
            team_ids: vec![],
        });
        assert_eq!(
            browse_url(&pcfg, "ENG-12"),
            "https://linear.app/issue/ENG-12"
        );
    }

    #[test]
    fn browse_url_azure_devops() {
        let pcfg = PmProviderConfig::AzureDevOps(crate::config::AzureDevOpsConfig {
            api_base: "https://dev.azure.com/myorg".into(),
            project: "MyProject".into(),
            pat: "x".into(),
        });
        assert_eq!(
            browse_url(&pcfg, "MyProject#99"),
            "https://dev.azure.com/myorg/MyProject/_workitems/edit/MyProject#99"
        );
    }

    #[test]
    fn browse_url_azure_devops_empty_base() {
        let pcfg = PmProviderConfig::AzureDevOps(crate::config::AzureDevOpsConfig {
            api_base: "".into(),
            project: "".into(),
            pat: "x".into(),
        });
        assert_eq!(browse_url(&pcfg, "P#1"), "");
    }

    #[test]
    fn browse_url_trello() {
        let pcfg = PmProviderConfig::Trello(crate::config::TrelloConfig {
            app_key: "k".into(),
            board_ids: vec![],
        });
        assert_eq!(browse_url(&pcfg, "abc123"), "https://trello.com/c/abc123");
    }

    #[test]
    fn apply_result_applied_json() {
        let r = ApplyResult::applied("linear", "ENG-5", "duedate");
        let j = r.to_json();
        assert_eq!(j["provider"], "linear");
        assert_eq!(j["key"], "ENG-5");
        assert_eq!(j["field"], "duedate");
        assert_eq!(j["status"], "applied");
        assert!(j.get("browse_url").is_none());
    }

    #[test]
    fn apply_result_redirected_json() {
        let r = ApplyResult::redirected(
            "github",
            "owner/repo#42",
            "priority",
            "https://github.com/owner/repo/issues/42".into(),
            "set on board",
        );
        let j = r.to_json();
        assert_eq!(j["status"], "redirected");
        assert_eq!(j["browse_url"], "https://github.com/owner/repo/issues/42");
        assert_eq!(j["reason"], "set on board");
    }
}
