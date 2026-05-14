// meridian — normalises screenpipe activity into structured app sessions

pub mod categorizer;
pub mod classifier;
pub mod jira_updater;
pub mod providers;
pub mod settler;
pub mod task_linker;

pub use jira_updater::run_jira_update;
pub use task_linker::run_task_linking;

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::warn;

use crate::config::{Config, PmProviderConfig};

/// Re-classifies all sessions that still have a rule-based category using Foundation Models.
pub async fn run_categorization(meridian: &SqlitePool, config: &Config) -> Result<()> {
    let backend = classifier::backends::build_backend(&config.llm_backend);
    if let Err(e) = settler::settle_all_categories(meridian, &backend, config.min_classification_duration_s).await {
        warn!(error = %e, "category settler failed");
    }
    Ok(())
}

/// Refreshes PM task caches from all configured providers.
/// Session-to-task linking is handled exclusively by run_task_linking (hermes).
pub async fn run_pm_sync(meridian: &SqlitePool, config: &Config) -> Result<()> {
    if config.pm_providers.is_empty() {
        warn!("no PM providers configured — pm_tasks will stay empty (set JIRA_BASE_URL/GITHUB_TOKEN/LINEAR_API_KEY)");
        return Ok(());
    }
    for provider in &config.pm_providers {
        let name = provider.provider_name();
        let result = match provider {
            PmProviderConfig::Jira(cfg) => {
                providers::jira::refresh_if_stale(meridian, cfg).await
            }
            PmProviderConfig::GitHub(cfg) => {
                providers::github::refresh_if_stale(meridian, cfg).await
            }
            PmProviderConfig::Linear(cfg) => {
                providers::linear::refresh_if_stale(meridian, cfg).await
            }
        };
        if let Err(e) = result {
            warn!(provider = name, error = %e, "provider refresh failed");
        }
    }
    Ok(())
}
