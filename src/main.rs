// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use std::time::Duration;

use anyhow::Result;
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::db::screenpipe::open_screenpipe;
use meridian::etl::run_etl;
use meridian::intelligence::{
    check_classification_ready, run_categorization, run_jira_update, run_pm_sync, run_task_linking,
};
use meridian::observability;
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Load env files first so MERIDIAN_OO_AUTH / MERIDIAN_OTLP_ENDPOINT are
    //    visible to observability::init().  Later file wins on conflicts.
    //    ~/.meridian/.env  — system / user-level defaults
    //    .env in cwd       — project root, easiest to edit during development
    if let Ok(home) = std::env::var("HOME") {
        let env_path = std::path::Path::new(&home).join(".meridian").join(".env");
        let _ = dotenvy::from_path(env_path);
    }
    let _ = dotenvy::dotenv(); // loads .env from current working directory

    // 2. Tracing — layered subscriber (stdout + JSONL file + OTLP to OpenObserve).
    //    Guard must outlive the program; we shut it down explicitly at the end
    //    so OTel's blocking flush doesn't run inside tokio's drop path.
    let obs_guard = observability::init("meridian-rust")?;

    // 3. Load initial config — DB paths and startup parameters come from here.
    //    DB pool paths and observability are fixed at startup and do not change.
    let initial_cfg = Config::from_env();
    tracing::info!(stage = "config_loaded", "configuration ready");

    // 4. Log startup parameters
    tracing::info!(
        screenpipe_db = %initial_cfg.screenpipe_db,
        meridian_db   = %initial_cfg.meridian_db,
        poll_interval_secs = initial_cfg.poll_interval_secs,
        "meridian daemon starting"
    );

    // 4b. Preflight: verify classification stack is ready before starting the daemon.
    //     Fails fast with a clear message rather than silently erroring every tick.
    if let Err(e) = check_classification_ready(&cfg) {
        tracing::error!("{}", e);
        eprintln!("\nERROR: {}\n", e);
        std::process::exit(1);
    }

    // 4. Open screenpipe pool (read-only)
    let screenpipe = open_screenpipe(&initial_cfg.screenpipe_db_uri()).await?;

    // 5. Open / create meridian pool and run migrations
    let meridian = setup_db(&initial_cfg.meridian_db_uri()).await?;

    // 6. Graceful shutdown: listen for SIGINT and SIGTERM
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    // Combines both signals into a single future that resolves on whichever fires first.
    async fn wait_for_shutdown(
        sigint: &mut tokio::signal::unix::Signal,
        sigterm: &mut tokio::signal::unix::Signal,
    ) {
        tokio::select! {
            _ = sigint.recv()  => {},
            _ = sigterm.recv() => {},
        }
    }

    // 7a. Clean up any runs left in 'running' state from a previous crash.
    match cleanup_incomplete_runs(&meridian).await {
        Ok(0) => {}
        Ok(n) => tracing::warn!(
            deleted_partial_sessions = n,
            "cleaned up incomplete ETL run"
        ),
        Err(e) => tracing::error!("cleanup_incomplete_runs failed: {}", e),
    }

    // 7b. Run ETL once immediately before entering the loop.
    //     Re-read config so that any settings.json present at startup takes effect.
    {
        let cfg = Config::from_env();
        tracing::info!("running initial ETL pass");
        if let Err(e) = run_etl(&screenpipe, &meridian).await {
            tracing::error!("ETL run failed: {}", e);
        }
        if let Err(e) = run_pm_sync(&meridian, &cfg).await {
            tracing::error!("intelligence run failed: {}", e);
        }
        if let Err(e) = run_categorization(&meridian, &cfg).await {
            tracing::error!("categorization run failed: {}", e);
        }
        if let Err(e) = run_task_linking(&meridian, &cfg).await {
            tracing::error!("classification run failed: {}", e);
        }
        if let Err(e) = run_jira_update(&meridian, &cfg).await {
            tracing::error!("jira update run failed: {}", e);
        }
    }

    // 8. Poll loop — config is re-read on every tick so that edits to
    //    ~/.meridian/settings.json take effect without a daemon restart.
    loop {
        // Determine the sleep duration from the current settings.json before sleeping.
        let poll_interval = {
            let cfg = Config::from_env();
            Duration::from_secs(cfg.runtime.poll_interval_secs)
        };

        tokio::select! {
            _ = wait_for_shutdown(&mut sigint, &mut sigterm) => {
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Re-read config to pick up any settings.json changes made while sleeping.
                let cfg = Config::from_env();
                tracing::debug!(
                    poll_interval_secs = cfg.runtime.poll_interval_secs,
                    "starting ETL tick"
                );
                if let Err(e) = run_etl(&screenpipe, &meridian).await {
                    tracing::error!("ETL run failed: {}", e);
                }
                if let Err(e) = run_pm_sync(&meridian, &cfg).await {
                    tracing::error!("intelligence run failed: {}", e);
                }
                if let Err(e) = run_categorization(&meridian, &cfg).await {
                    tracing::error!("categorization run failed: {}", e);
                }
                if let Err(e) = run_task_linking(&meridian, &cfg).await {
                    tracing::error!("classification run failed: {}", e);
                }
                if let Err(e) = run_jira_update(&meridian, &cfg).await {
                    tracing::error!("jira update run failed: {}", e);
                }
            }
        }
    }

    // 9. Shutdown
    tracing::info!("shutting down");
    screenpipe.close().await;
    meridian.close().await;

    // Flush OTel exporters while the tokio runtime is still alive.
    obs_guard.shutdown().await;

    Ok(())
}
