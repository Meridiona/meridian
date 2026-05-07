// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use std::time::Duration;

use anyhow::Result;
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::db::screenpipe::open_screenpipe;
use meridian::etl::run_etl;
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Tracing — respect RUST_LOG; default to "meridian=info"
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("meridian=info")),
        )
        .init();

    // 2. Load config
    let cfg = Config::from_env();

    // 3. Log startup parameters
    tracing::info!(
        screenpipe_db = %cfg.screenpipe_db,
        meridian_db   = %cfg.meridian_db,
        poll_interval_secs = cfg.poll_interval_secs,
        "meridian daemon starting"
    );

    // 4. Open screenpipe pool (read-only)
    let screenpipe = open_screenpipe(&cfg.screenpipe_db_uri()).await?;

    // 5. Open / create meridian pool and run migrations
    let meridian = setup_db(&cfg.meridian_db_uri()).await?;

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

    let poll_interval = Duration::from_secs(cfg.poll_interval_secs);

    // 7a. Clean up any runs left in 'running' state from a previous crash.
    match cleanup_incomplete_runs(&meridian).await {
        Ok(0) => {}
        Ok(n) => tracing::warn!(deleted_partial_sessions = n, "cleaned up incomplete ETL run"),
        Err(e) => tracing::error!("cleanup_incomplete_runs failed: {}", e),
    }

    // 7b. Run ETL once immediately before entering the loop
    tracing::info!("running initial ETL pass");
    if let Err(e) = run_etl(&screenpipe, &meridian).await {
        tracing::error!("ETL run failed: {}", e);
    }

    // 7b. Poll loop
    loop {
        tokio::select! {
            _ = wait_for_shutdown(&mut sigint, &mut sigterm) => {
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                tracing::debug!("starting ETL tick");
                if let Err(e) = run_etl(&screenpipe, &meridian).await {
                    tracing::error!("ETL run failed: {}", e);
                    // Don't exit — retry next tick
                }
            }
        }
    }

    // 8. Shutdown
    tracing::info!("shutting down");
    screenpipe.close().await;
    meridian.close().await;

    Ok(())
}
