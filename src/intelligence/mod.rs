//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

pub mod oauth;
pub mod providers;
pub mod session_categorizer;
pub mod task_triage;
pub mod ticket_update;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::config::{Config, PmProviderConfig};

/// True once at least one PM task is cached. Rows only land in `pm_tasks` after a
/// provider authenticated and fetched successfully, so a non-zero count is proof
/// a tracker actually WORKS (not merely that keys are present — bad creds 401 and
/// leave the table empty). A DB error is treated as "not present" (fail closed).
///
/// Personal tasks (`provider = 'local'`, see [`meridian_core::task_create`]) are
/// excluded: the user writes those themselves, so they prove nothing about a tracker.
/// Counting them would make one self-authored task convince the daemon a tracker is
/// connected and working — which gates onboarding, sync scheduling and notifications.
pub async fn pm_tasks_present(pool: &SqlitePool) -> bool {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pm_tasks WHERE provider != ?")
        .bind(meridian_core::task_create::LOCAL_PROVIDER)
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
            PmProviderConfig::AzureDevOps(cfg) => {
                providers::azure_devops::force_refresh(meridian, cfg)
                    .await
                    .map(Some)
            }
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
    triage_after_sync(meridian).await;
    Ok(())
}

/// Refreshes PM task caches from all configured providers.
#[tracing::instrument(skip_all)]
pub async fn run_pm_sync(meridian: &SqlitePool, config: &Config) -> Result<()> {
    if config.pm_providers.is_empty() {
        tracing::warn!("no PM providers configured — pm_tasks will stay empty (set JIRA_BASE_URL/GITHUB_TOKEN/LINEAR_API_KEY/AZURE_DEVOPS_PAT)");
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
            PmProviderConfig::AzureDevOps(cfg) => {
                providers::azure_devops::refresh_if_stale(meridian, cfg).await
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
                // Clear any lingering notice — provider just refreshed successfully
                let _ = providers::clear_sync_error(meridian, name).await;
            }
            Err(e) => {
                tracing::warn!(provider = name, error = %e, "provider refresh failed");
                let _ = providers::stamp_sync_error(meridian, name, &e.to_string()).await;
            }
        }
    }
    triage_after_sync(meridian).await;
    Ok(())
}

/// Re-triage the cached board into `pm_task_curation` after a sync. Best-effort:
/// a triage failure must never fail the sync (the board is still usable), so we
/// log and move on.
async fn triage_after_sync(meridian: &SqlitePool) {
    match task_triage::run_triage(meridian, chrono::Utc::now()).await {
        Ok(s) => {
            tracing::debug!(
                ready = s.ready,
                needs_detail = s.needs_detail,
                looks_stale = s.looks_stale,
                not_sure = s.not_sure,
                pruned = s.pruned,
                "board triaged"
            );
            maybe_notify_board_hygiene(meridian, &s).await;
        }
        Err(e) => tracing::warn!(error = %e, "board triage after sync failed"),
    }
}

/// Daily digest: if the board has tickets needing attention (not `Ready`),
/// enqueue one batched notification rather than nudging on every sync tick —
/// same once-per-day dedup shape as [`crate::daily_plan::maybe_nudge`]. Safe
/// to call unconditionally on every `triage_after_sync` pass (potentially
/// several times a day): the outbox's UNIQUE constraint on `dedup_key` makes
/// a repeat call within the same day a no-op, so no extra gate is needed.
async fn maybe_notify_board_hygiene(pool: &SqlitePool, s: &task_triage::TriageSummary) {
    let attention = s.needs_detail + s.looks_stale + s.not_sure;
    if attention == 0 {
        return;
    }
    let today = meridian_core::date::today_string();
    let dedup = format!("board.hygiene:{today}");
    let body = format!(
        "{attention} ticket{} on your board need{} a closer look.",
        if attention == 1 { "" } else { "s" },
        if attention == 1 { "s" } else { "" }
    );
    let n = crate::notifications::NewNotification::event(
        &dedup,
        "board.hygiene",
        "Board hygiene",
        &body,
    )
    .link("/tasks?integrations=1");
    if let Err(e) = crate::notifications::enqueue(pool, n).await {
        tracing::warn!(error = %e, "board hygiene digest enqueue failed");
    }
}
