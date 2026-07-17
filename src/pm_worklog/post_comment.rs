//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The centralised comment primitive — post a PLAIN status-update comment to any
//! provider's ticket, `body` verbatim.
//!
//! This is deliberately NOT a worklog: no time is logged, no `⏱`/`meridian-worklog`
//! marker, no [`super::comment::format_worklog_comment`] wrapper. It is what the
//! day-task "Generate worklog" flow ([`super::generate::approve`]) posts — a rich
//! status update for a PM/lead. Each provider module owns its own
//! `post_comment(cfg, key, body)`; this dispatches to the right one by provider
//! name, resolving the provider config exactly like [`super::create`].
//!
//! # Related
//! - [`super::create`] — the sibling ticket-CREATE dispatcher (same config finders).
//! - [`super::generate`] — the only caller: creates a ticket if proposed, then
//!   posts the update here.

use anyhow::{bail, Context, Result};

use crate::config::{
    AzureDevOpsConfig, Config, GitHubConfig, JiraConfig, LinearConfig, PmProviderConfig,
    TrelloConfig,
};

/// Post `body` as a plain comment to `task_key` on `provider`, returning the
/// created comment/action id. An unconfigured provider is an explicit error
/// ("… is not configured on this daemon"), never a panic.
pub async fn post_comment(
    config: &Config,
    provider: &str,
    task_key: &str,
    body: &str,
) -> Result<String> {
    match provider {
        "jira" => super::jira::post_comment(jira_cfg(config)?, task_key, body).await,
        "linear" => super::linear::post_comment(linear_cfg(config)?, task_key, body).await,
        "github" => super::github::post_comment(github_cfg(config)?, task_key, body).await,
        "trello" => super::trello::post_comment(trello_cfg(config)?, task_key, body).await,
        "azure_devops" => {
            super::azure_devops::post_comment(azure_cfg(config)?, task_key, body).await
        }
        other => bail!("post_comment: unknown provider '{other}'"),
    }
}

// ── Config finders (mirror `create.rs`) ───────────────────────────────────────

fn jira_cfg(c: &Config) -> Result<&JiraConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::Jira(j) => Some(j),
            _ => None,
        })
        .context("Jira is not configured on this daemon")
}
fn linear_cfg(c: &Config) -> Result<&LinearConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::Linear(l) => Some(l),
            _ => None,
        })
        .context("Linear is not configured on this daemon")
}
fn github_cfg(c: &Config) -> Result<&GitHubConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::GitHub(g) => Some(g),
            _ => None,
        })
        .context("GitHub is not configured on this daemon")
}
fn trello_cfg(c: &Config) -> Result<&TrelloConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::Trello(t) => Some(t),
            _ => None,
        })
        .context("Trello is not configured on this daemon")
}
fn azure_cfg(c: &Config) -> Result<&AzureDevOpsConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::AzureDevOps(a) => Some(a),
            _ => None,
        })
        .context("Azure DevOps is not configured on this daemon")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn empty_config() -> Config {
        let mut cfg = Config::from_env();
        cfg.pm_providers = vec![];
        cfg
    }

    #[tokio::test]
    async fn unknown_provider_is_an_explicit_error_not_a_panic() {
        let cfg = empty_config();
        let err = post_comment(&cfg, "notaprovider", "KAN-1", "hi")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown provider"), "{err}");
    }

    #[tokio::test]
    async fn unconfigured_provider_is_an_explicit_error() {
        // Jira dispatch with no Jira configured → the config finder's clean error.
        let cfg = empty_config();
        let err = post_comment(&cfg, "jira", "KAN-1", "hi")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not configured"), "{err}");
    }
}
