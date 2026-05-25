// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::db::screenpipe::open_screenpipe;
use meridian::etl::run_etl;
use meridian::intelligence::{
    check_classification_ready, mark_session_subprocess_error, run_fm_categorization, run_pm_sync,
    run_task_linking, TaskLinkOutcome,
};
use meridian::observability;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;

/// After this many consecutive subprocess failures for the same session,
/// write a `subprocess_error` sentinel and advance the cursor past it.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

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
    // Override so cwd .env beats any empty values injected by launchd plist.
    let _ = dotenvy::dotenv_override();

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
    if let Err(e) = check_classification_ready(&initial_cfg) {
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
        Ok(0) => {
            tracing::info!("no incomplete runs found");
        }
        Ok(n) => tracing::warn!(
            deleted_partial_sessions = n,
            "cleaned up incomplete ETL run"
        ),
        Err(e) => tracing::error!("cleanup_incomplete_runs failed: {}", e),
    }

    // 7b. Notify used to wake the task linker immediately after ETL writes new sessions.
    //     ETL calls notify_one(); the linker waits on notified() instead of polling.
    let etl_notify: Arc<Notify> = Arc::new(Notify::new());

    // 7c. Run ETL once immediately before entering the loop.
    //     Re-read config so that any settings.json present at startup takes effect.
    {
        let cfg = Config::from_env();
        let startup_tick = tracing::info_span!("startup_tick");
        let _guard = startup_tick.enter();
        tracing::info!("running initial ETL pass");
        if let Err(e) = run_etl(&screenpipe, &meridian).await {
            tracing::error!("ETL run failed: {}", e);
        }
        etl_notify.notify_one();
        if let Err(e) = run_pm_sync(&meridian, &cfg).await {
            tracing::error!("intelligence run failed: {}", e);
        }
        if let Err(e) = run_fm_categorization(&meridian, &cfg).await {
            tracing::error!("FM categorization run failed: {}", e);
        }
    }

    // 8a. Task linker loop — wakes immediately when ETL signals new sessions.
    //     Drains oldest-first (preserving the 5-session context window) until caught up,
    //     then waits for the next ETL notification. A 5-min fallback ensures recovery
    //     if a notify was missed (e.g. daemon restart with existing backlog).
    //
    //     Failure handling mirrors category_settler:
    //       - Transient failure  → cursor stays, retry on next notify
    //       - Permanent failure  → sentinel written after MAX_CONSECUTIVE_FAILURES,
    //                              cursor advances, drain continues
    let meridian_linker = meridian.clone();
    let notify_linker = etl_notify.clone();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        // Tracks consecutive subprocess failures per session_id.
        // Reset to zero whenever any session is successfully classified.
        // Persists across drain cycles within this daemon run (lost on restart,
        // which is fine — transient failures before restart won't be double-counted).
        let mut failure_counts: HashMap<i64, u32> = HashMap::new();

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                _ = notify_linker.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(300)) => {}
            }

            // Drain: classify oldest-first until nothing is left or a failure stops us.
            loop {
                let cfg = Config::from_env();
                match run_task_linking(&meridian_linker, &cfg).await {
                    Ok(TaskLinkOutcome::Classified) => {
                        failure_counts.clear();
                        // Loop immediately — more sessions may be waiting.
                    }
                    Ok(TaskLinkOutcome::NoPendingWork) => {
                        break; // Caught up — go back to waiting for next notify.
                    }
                    Ok(TaskLinkOutcome::SubprocessFailed {
                        session_id,
                        pending,
                    }) => {
                        let count = failure_counts.entry(session_id).or_insert(0);
                        *count += 1;

                        if *count >= MAX_CONSECUTIVE_FAILURES {
                            tracing::warn!(
                                session_id,
                                failures = *count,
                                pending,
                                "max consecutive failures — writing subprocess_error sentinel \
                                 and advancing cursor"
                            );
                            if let Err(e) =
                                mark_session_subprocess_error(&meridian_linker, session_id).await
                            {
                                tracing::error!(
                                    session_id,
                                    error = %e,
                                    "failed to write error sentinel — will retry next tick"
                                );
                                break;
                            }
                            failure_counts.remove(&session_id);
                            // Loop again — cursor advanced, try the next session.
                        } else {
                            tracing::warn!(
                                session_id,
                                failures = *count,
                                max = MAX_CONSECUTIVE_FAILURES,
                                pending,
                                "subprocess failed — cursor held, will retry on next ETL tick"
                            );
                            break; // Stop drain, wait for next notify / 5-min fallback.
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "classification run error");
                        break;
                    }
                }
            }
        }
        tracing::info!("task linker loop stopped");
    });

    // 8b. Poll loop — ETL, PM sync, and FM categorization on the configured interval.
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
                let poll_tick = tracing::info_span!(
                    "poll_tick",
                    poll_interval_secs = cfg.runtime.poll_interval_secs
                );
                let _guard = poll_tick.enter();
                tracing::debug!("starting ETL tick");
                if let Err(e) = run_etl(&screenpipe, &meridian).await {
                    tracing::error!("ETL run failed: {}", e);
                }
                etl_notify.notify_one();
                if let Err(e) = run_pm_sync(&meridian, &cfg).await {
                    tracing::error!("intelligence run failed: {}", e);
                }
                if let Err(e) = run_fm_categorization(&meridian, &cfg).await {
                    tracing::error!("FM categorization run failed: {}", e);
                }
            }
        }
    }

    // Signal the task linker loop to stop.
    let _ = shutdown_tx.send(true);

    // 9. Shutdown
    tracing::info!("shutting down");
    screenpipe.close().await;
    meridian.close().await;

    // Flush OTel exporters while the tokio runtime is still alive.
    obs_guard.shutdown().await;

    Ok(())
}
