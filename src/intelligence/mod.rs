// meridian — normalises screenpipe activity into structured app sessions

pub mod oauth;
pub mod providers;
pub mod session_categorizer;
pub mod task_linker;

pub use task_linker::{
    check_classification_ready, link_range, mark_session_subprocess_error,
    run_coding_agent_classification, run_task_linking, TaskLinkOutcome,
};

use anyhow::Result;
use sqlx::SqlitePool;
use std::time::Duration;

use crate::config::{Config, PmProviderConfig};

/// True once at least one PM task is cached. Rows only land in `pm_tasks` after a
/// provider authenticated and fetched successfully, so a non-zero count is proof
/// a tracker actually WORKS (not merely that keys are present — bad creds 401 and
/// leave the table empty). A DB error is treated as "not present" (fail closed).
pub async fn pm_tasks_present(pool: &SqlitePool) -> bool {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pm_tasks")
        .fetch_one(pool)
        .await
    {
        Ok(n) => n > 0,
        Err(e) => {
            tracing::warn!(error = %e, "pm_tasks count failed — treating tracker as not ready");
            false
        }
    }
}

/// True when the MLX classifier is actually WORKING: reachable AND, if it reports
/// model-load status, the model is loaded. This is a POSITIVE readiness probe
/// (short timeout) — never inferred from a failed classify call, and never the
/// 120 s startup wait of `check_classification_ready`. Safe to call every cycle.
pub async fn mlx_ready(cfg: &Config) -> bool {
    let base = format!("http://127.0.0.1:{}", cfg.mlx_server_port);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Liveness: the port must answer /health.
    if client.get(format!("{base}/health")).send().await.is_err() {
        return false;
    }
    // Readiness: /info reports `loaded_at`. If this build exposes it, require the
    // model to be loaded; if /info is absent (older build), liveness is enough.
    match client.get(format!("{base}/info")).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(body) => match body.get("loaded_at") {
                Some(v) => !v.is_null(),
                None => true,
            },
            Err(_) => true,
        },
        Err(_) => true,
    }
}

/// The gate for the whole task-linking + worklog pipeline: BOTH halves must be
/// working before any session is classified or any worklog drafted — the LLM
/// classifier loaded AND a PM tracker that has synced tasks. Either side failing
/// pauses the pipeline; it resumes automatically once both recover, because the
/// driving loops re-check each cycle and the classification cursor is held while
/// paused (so the backlog links retroactively rather than being skipped).
pub async fn pipeline_ready(pool: &SqlitePool, cfg: &Config) -> bool {
    mlx_ready(cfg).await && pm_tasks_present(pool).await
}

/// Forces an immediate refresh of all configured PM providers, bypassing the staleness gate.
pub async fn run_pm_force_sync(meridian: &SqlitePool, config: &Config) -> Result<()> {
    if config.pm_providers.is_empty() {
        return Ok(());
    }
    for provider in &config.pm_providers {
        let name = provider.provider_name();
        let result = match provider {
            PmProviderConfig::Jira(cfg) => providers::jira::force_refresh(meridian, cfg).await,
            PmProviderConfig::GitHub(cfg) => providers::github::force_refresh(meridian, cfg).await,
            PmProviderConfig::Linear(cfg) => providers::linear::force_refresh(meridian, cfg).await,
            PmProviderConfig::Trello(cfg) => providers::trello::force_refresh(meridian, cfg).await,
        };
        match result {
            Ok(None) => tracing::info!(provider = name, "force sync: auth unavailable or no tasks"),
            Ok(Some(ref keys)) => {
                tracing::info!(provider = name, count = keys.len(), "force sync: refreshed");
                println!("{name}: synced {} task(s)", keys.len());
            }
            Err(e) => {
                tracing::warn!(provider = name, error = %e, "force sync failed");
                eprintln!("{name}: sync failed: {e}");
            }
        }
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
            PmProviderConfig::Trello(cfg) => {
                providers::trello::refresh_if_stale(meridian, cfg).await
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
