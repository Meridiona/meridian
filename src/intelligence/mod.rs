// meridian — normalises screenpipe activity into structured app sessions

pub mod categorizer;
pub mod classifier;
pub mod providers;
pub mod settler;

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::warn;

use crate::config::{Config, PmProviderConfig};

/// Runs one intelligence cycle after ETL completes.
/// Iterates all configured PM providers; silently skips if none are configured.
pub async fn run_intelligence(meridian: &SqlitePool, config: &Config) -> Result<()> {
    // Refresh PM task caches first
    if !config.pm_providers.is_empty() {
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
    }

    let backend = classifier::backends::build_backend(&config.llm_backend);

    if let Err(e) = settler::settle_sessions(meridian, &backend).await {
        warn!(error = %e, "task settler failed");
    }

    if let Err(e) = settler::settle_chrome_categories(meridian, &backend).await {
        warn!(error = %e, "chrome category settler failed");
    }

    Ok(())
}
