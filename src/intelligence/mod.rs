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
                // No `.map(Some)`: azure_devops now returns `Option` like the
                // other four, so a failure it already classified arrives as
                // `Ok(None)` instead of an `Err` this loop would re-report.
                providers::azure_devops::force_refresh(meridian, cfg).await
            }
        };
        match result {
            Ok(None) => tracing::info!(provider = name, "force sync: auth unavailable or no tasks"),
            Ok(Some(ref keys)) => {
                tracing::info!(provider = name, count = keys.len(), "force sync: refreshed");
                println!("{name}: synced {} task(s)", keys.len());
            }
            Err(e) => {
                // `{e:#}` - the full cause chain. A bare `{e}` printed only the
                // outermost context, so the operator running a force sync saw
                // "POST /graphql viewer" with the actual cause discarded.
                let detail = providers::http::chain(&e);
                tracing::warn!(provider = name, error = %detail, "force sync failed");
                eprintln!("{name}: sync failed: {detail}");
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
                // Was `stamp_sync_error(name, &e.to_string())`: unconditionally
                // terminal, and `{e}` rather than `{e:#}` — both of the bugs the
                // provider-level fix removes. That made this a silent undo
                // button, since a provider could classify its own failure
                // correctly and then have this generic write land on top and
                // win. Anything still reaching here is a genuine infra error
                // (a DB write, say), which classifies as terminal by default and
                // now carries its whole cause chain.
                providers::record_sync_failure(meridian, name, "refresh", &e).await;
            }
        }
    }
    triage_after_sync(meridian).await;
    Ok(())
}

/// Re-triage the cached board into `pm_task_curation` after a sync. Best-effort:
/// a triage failure must never fail the sync (the board is still usable), so we
/// log and move on.
///
/// This used to also enqueue a once-a-day `board.hygiene` digest toast ("N
/// tickets on your board need a closer look"). That was removed: its whole
/// purpose was to drive the user into the Board Cleanup flow, and that UI is
/// disabled (see the commented-out `CleanupModal` in
/// `ui/components/timeline/MeridianTimelineShell.tsx`), so the toast nudged
/// toward a surface that no longer opens — and its deep link (`/tasks`) is not
/// a route the static-exported dashboard has either. Re-enabling Board Cleanup
/// means restoring a producer here; migration 077 clears the rows this one left
/// behind. The triage data itself is untouched — `pm_task_curation` still feeds
/// the tasks panel and `HygieneDialog`.
async fn triage_after_sync(meridian: &SqlitePool) {
    match task_triage::run_triage(meridian, chrono::Utc::now()).await {
        Ok(s) => tracing::debug!(
            ready = s.ready,
            needs_detail = s.needs_detail,
            looks_stale = s.looks_stale,
            not_sure = s.not_sure,
            pruned = s.pruned,
            "board triaged"
        ),
        Err(e) => tracing::warn!(error = %e, "board triage after sync failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    /// Board Cleanup is disabled, so triage must stay silent: a board full of
    /// tickets needing attention triages into `pm_task_curation` (the tasks
    /// panel still reads it) without enqueueing a `board.hygiene` toast.
    ///
    /// The seed mirrors `task_triage::store`'s own fixtures: an empty
    /// description on an active ticket buckets as `needs_detail`, and an
    /// untouched-since-January backlog ticket as `looks_stale` — between them
    /// the two counts the old digest summed over.
    #[tokio::test]
    async fn triage_after_sync_enqueues_no_board_hygiene_digest() {
        let pool = db().await;
        for (key, status, desc, updated) in [
            ("THIN-1", "In Progress", "", "2026-06-11T00:00:00Z"),
            ("STALE-1", "Backlog", "A", "2026-01-01T00:00:00Z"),
        ] {
            sqlx::query(
                "INSERT INTO pm_tasks (task_key, provider, title, description_text, status_raw, \
                    is_terminal, url, updated_at) \
                 VALUES (?, 'jira', 'A title that is plenty specific', ?, ?, 0, 'http://x', ?)",
            )
            .bind(key)
            .bind(desc)
            .bind(status)
            .bind(updated)
            .execute(&pool)
            .await
            .unwrap();
        }

        triage_after_sync(&pool).await;

        let triaged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pm_task_curation")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(triaged, 2, "triage itself must still run and persist");

        let queued: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications WHERE event_key = 'board.hygiene'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(queued, 0, "the board hygiene digest producer was removed");
    }
}
