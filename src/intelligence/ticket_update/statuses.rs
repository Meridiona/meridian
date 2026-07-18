//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Status list + set write-back. The dashboard's status control needs two things
// per ticket: (1) the set of statuses the ticket can move to, each normalised to
// Meridian's canonical lifecycle taxonomy (`backlog|todo|in_progress|in_review|
// done|cancelled|unknown`) so the UI can colour/group them uniformly, plus the
// ticket's CURRENT status; and (2) a way to move the ticket to a chosen status.
//
// Like `apply` (hygiene write-back) the set outcome is either `Applied` (the move
// landed) or `Redirected` (this board can't reach that status from where it is —
// open the tracker instead). `--status` / the chosen id may be a status id OR a
// status name (case-insensitive): the UI's Undo passes the previous status NAME,
// which is the only stable handle on Jira, where transition ids are
// position-dependent and differ from the target status id.
//
// The per-provider `list_statuses` / `set_status` live in each provider module
// (`jira.rs`, `linear.rs`, …); this file owns the shared response types, the
// canonical-category mapping, the id-or-name resolver, and the top-level
// dispatch that mirrors `apply`'s provider-resolution block.

use anyhow::Result;
use serde_json::{json, Value};

use super::ApplyStatus;
use crate::config::{Config, PmProviderConfig};

/// One status a ticket can be in / move to, normalised for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusOption {
    /// Provider status id (Jira status id, Linear state UUID, Trello list id,
    /// GitHub pseudo-id `open`/`closed`/`not_planned`, Azure state name == id).
    pub id: String,
    /// Human status/column name for display.
    pub name: String,
    /// Canonical lifecycle phase: one of
    /// `backlog|todo|in_progress|in_review|done|cancelled|unknown`.
    pub category: String,
}

impl StatusOption {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "category": self.category })
    }
}

/// The `ticket-statuses` result: the reachable statuses + the ticket's current
/// status (id/name may be `None` when the tracker doesn't expose it here).
#[derive(Debug, Clone)]
pub struct StatusList {
    pub statuses: Vec<StatusOption>,
    pub current_id: Option<String>,
    pub current_name: Option<String>,
}

impl StatusList {
    /// `{"statuses":[…],"current_id":..|null,"current_name":..|null}`.
    pub fn to_json(&self) -> Value {
        json!({
            "statuses": self.statuses.iter().map(StatusOption::to_json).collect::<Vec<_>>(),
            "current_id": self.current_id,
            "current_name": self.current_name,
        })
    }
}

/// The `ticket-set-status` result: the Applied/Redirected outcome + the status
/// the ticket now holds (`None` on a redirect — nothing changed).
#[derive(Debug, Clone)]
pub struct SetStatusResult {
    pub status: ApplyStatus,
    pub new_status: Option<StatusOption>,
}

impl SetStatusResult {
    pub fn applied(new_status: StatusOption) -> Self {
        Self {
            status: ApplyStatus::Applied,
            new_status: Some(new_status),
        }
    }

    pub fn redirected(browse_url: String, reason: impl Into<String>) -> Self {
        Self {
            status: ApplyStatus::Redirected {
                browse_url,
                reason: reason.into(),
            },
            new_status: None,
        }
    }

    /// `{"result":{"status":..,"browse_url":..|null,"reason":..|null},
    ///   "new_status":{…}|null}`.
    pub fn to_json(&self) -> Value {
        let result = match &self.status {
            ApplyStatus::Applied => json!({
                "status": "applied",
                "browse_url": Value::Null,
                "reason": Value::Null,
            }),
            ApplyStatus::Redirected { browse_url, reason } => json!({
                "status": "redirected",
                "browse_url": browse_url,
                "reason": reason,
            }),
        };
        json!({
            "result": result,
            "new_status": self.new_status.as_ref().map(StatusOption::to_json),
        })
    }
}

/// Snake_case wire form of a canonical [`meridian_core::StatusCategory`] — the
/// single source of truth for the taxonomy the whole app groups tickets by.
/// Providers with no native category (Trello lists) emit the literal `"unknown"`.
pub(super) fn category_str(c: meridian_core::StatusCategory) -> &'static str {
    use meridian_core::StatusCategory::*;
    match c {
        Backlog => "backlog",
        Todo => "todo",
        InProgress => "in_progress",
        InReview => "in_review",
        Done => "done",
        Cancelled => "cancelled",
    }
}

/// Resolve a user's status choice against a list of options: an exact id match
/// wins, else a case-insensitive name match. The name fallback is what lets the
/// UI's Undo pass the previous status NAME (stable) rather than an id (which, on
/// Jira, is a position-dependent transition id it can't reconstruct).
pub(super) fn resolve_choice<'a>(
    options: &'a [StatusOption],
    choice: &str,
) -> Option<&'a StatusOption> {
    options
        .iter()
        .find(|o| o.id == choice)
        .or_else(|| options.iter().find(|o| o.name.eq_ignore_ascii_case(choice)))
}

/// List the statuses `key` can move to (+ its current status) on `provider`.
/// Mirrors [`super::apply`]'s provider-resolution block via
/// [`super::resolve_provider`], then dispatches to the provider module.
#[tracing::instrument(skip(config), fields(provider, key))]
pub async fn list_statuses(config: &Config, provider: &str, key: &str) -> Result<StatusList> {
    let pcfg = super::resolve_provider(config, provider)?;
    let result = match pcfg {
        PmProviderConfig::Jira(cfg) => super::jira::list_statuses(cfg, key).await,
        PmProviderConfig::Linear(cfg) => super::linear::list_statuses(cfg, key).await,
        PmProviderConfig::GitHub(cfg) => super::github::list_statuses(cfg, key).await,
        PmProviderConfig::Trello(cfg) => super::trello::list_statuses(cfg, key).await,
        PmProviderConfig::AzureDevOps(cfg) => super::azure_devops::list_statuses(cfg, key).await,
    };
    match &result {
        Ok(list) => tracing::info!(
            provider,
            key,
            statuses = list.statuses.len(),
            current = list.current_name.as_deref().unwrap_or(""),
            "listed ticket statuses"
        ),
        Err(e) => tracing::warn!(provider, key, error = %e, "list_statuses failed"),
    }
    result
}

/// Move `key` to `status` (a status id OR case-insensitive name) on `provider`.
/// Returns `Applied` with the new status, or `Redirected` when the board can't
/// reach it from the ticket's current status. Same dispatch as [`list_statuses`].
#[tracing::instrument(skip(config), fields(provider, key, status))]
pub async fn set_status(
    config: &Config,
    provider: &str,
    key: &str,
    status: &str,
) -> Result<SetStatusResult> {
    let pcfg = super::resolve_provider(config, provider)?;
    let result = match pcfg {
        PmProviderConfig::Jira(cfg) => super::jira::set_status(cfg, key, status).await,
        PmProviderConfig::Linear(cfg) => super::linear::set_status(cfg, key, status).await,
        PmProviderConfig::GitHub(cfg) => super::github::set_status(cfg, key, status).await,
        PmProviderConfig::Trello(cfg) => super::trello::set_status(cfg, key, status).await,
        PmProviderConfig::AzureDevOps(cfg) => {
            super::azure_devops::set_status(cfg, key, status).await
        }
    };
    match &result {
        Ok(r) => tracing::info!(
            provider,
            key,
            outcome = match r.status {
                ApplyStatus::Applied => "applied",
                ApplyStatus::Redirected { .. } => "redirected",
            },
            "set ticket status"
        ),
        Err(e) => tracing::warn!(provider, key, error = %e, "set_status failed"),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(id: &str, name: &str, cat: &str) -> StatusOption {
        StatusOption {
            id: id.into(),
            name: name.into(),
            category: cat.into(),
        }
    }

    #[test]
    fn category_str_covers_the_taxonomy() {
        use meridian_core::StatusCategory::*;
        assert_eq!(category_str(Backlog), "backlog");
        assert_eq!(category_str(Todo), "todo");
        assert_eq!(category_str(InProgress), "in_progress");
        assert_eq!(category_str(InReview), "in_review");
        assert_eq!(category_str(Done), "done");
        assert_eq!(category_str(Cancelled), "cancelled");
    }

    #[test]
    fn resolve_choice_prefers_id_then_name() {
        let opts = vec![
            opt("10", "To Do", "todo"),
            opt("21", "In Progress", "in_progress"),
        ];
        // Exact id.
        assert_eq!(resolve_choice(&opts, "21").unwrap().name, "In Progress");
        // Case-insensitive name (the Undo path).
        assert_eq!(resolve_choice(&opts, "to do").unwrap().id, "10");
        assert_eq!(resolve_choice(&opts, "IN PROGRESS").unwrap().id, "21");
        // No match.
        assert!(resolve_choice(&opts, "Done").is_none());
    }

    #[test]
    fn resolve_choice_id_wins_over_a_name_collision() {
        // A name that happens to equal another option's id must not shadow the
        // exact-id hit.
        let opts = vec![opt("Done", "Done", "done"), opt("x", "done", "done")];
        assert_eq!(resolve_choice(&opts, "Done").unwrap().id, "Done");
    }

    #[test]
    fn status_list_json_shape() {
        let list = StatusList {
            statuses: vec![opt("10", "To Do", "todo")],
            current_id: Some("10".into()),
            current_name: Some("To Do".into()),
        };
        let j = list.to_json();
        assert_eq!(j["statuses"][0]["id"], "10");
        assert_eq!(j["statuses"][0]["category"], "todo");
        assert_eq!(j["current_id"], "10");
        assert_eq!(j["current_name"], "To Do");
    }

    #[test]
    fn set_result_applied_json_shape() {
        let r = SetStatusResult::applied(opt("31", "Done", "done"));
        let j = r.to_json();
        assert_eq!(j["result"]["status"], "applied");
        assert!(j["result"]["browse_url"].is_null());
        assert!(j["result"]["reason"].is_null());
        assert_eq!(j["new_status"]["id"], "31");
        assert_eq!(j["new_status"]["name"], "Done");
    }

    #[test]
    fn set_result_redirected_json_shape() {
        let r = SetStatusResult::redirected("https://x/browse/KAN-1".into(), "no path");
        let j = r.to_json();
        assert_eq!(j["result"]["status"], "redirected");
        assert_eq!(j["result"]["browse_url"], "https://x/browse/KAN-1");
        assert_eq!(j["result"]["reason"], "no path");
        assert!(j["new_status"].is_null());
    }
}
