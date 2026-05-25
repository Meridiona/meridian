// meridian — normalises screenpipe activity into structured app sessions

pub mod category_llm;
pub mod category_settler;
pub mod providers;
pub mod session_categorizer;
pub mod task_linker;

pub use task_linker::{
    check_classification_ready, link_range, mark_session_subprocess_error, run_task_linking,
    TaskLinkOutcome,
};

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::warn;

use crate::config::{Config, PmProviderConfig};

/// Re-classifies all sessions that still have a rule-based category using Foundation Models.
#[tracing::instrument(skip_all)]
pub async fn run_fm_categorization(meridian: &SqlitePool, config: &Config) -> Result<()> {
    let backend = category_llm::backends::build_backend(&config.llm_backend);
    if let Err(e) = category_settler::run_fm_categorization(
        meridian,
        &backend,
        config.min_classification_duration_s,
        config.category_backfill,
    )
    .await
    {
        warn!(error = %e, "fm categorization failed");
    }
    Ok(())
}

/// Refreshes PM task caches from all configured providers.
#[tracing::instrument(skip_all)]
pub async fn run_pm_sync(meridian: &SqlitePool, config: &Config) -> Result<()> {
    if config.pm_providers.is_empty() {
        tracing::warn!("no PM providers configured — pm_tasks will stay empty (set JIRA_BASE_URL/GITHUB_TOKEN/LINEAR_API_KEY)");
        return Ok(());
    }
    let provider_count = config.pm_providers.len();
    tracing::debug!(provider_count, "syncing PM providers");

    for provider in &config.pm_providers {
        let name = provider.provider_name();
        let result = match provider {
            PmProviderConfig::Jira(cfg) => providers::jira::refresh_if_stale(meridian, cfg).await,
            PmProviderConfig::GitHub(cfg) => {
                providers::github::refresh_if_stale(meridian, cfg).await
            }
            PmProviderConfig::Linear(cfg) => {
                providers::linear::refresh_if_stale(meridian, cfg).await
            }
        };
        match result {
            Ok(None) => {
                tracing::debug!(provider = name, "provider cache is fresh — skipped");
            }
            Ok(Some(ref keys)) => {
                tracing::debug!(
                    provider = name,
                    refreshed_count = keys.len(),
                    ?keys,
                    "provider cache was stale — refreshed"
                );
            }
            Err(e) => {
                tracing::warn!(provider = name, error = %e, "provider refresh failed");
            }
        }
    }
    Ok(())
}
