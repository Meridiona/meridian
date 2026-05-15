// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use chrono::{Local, Timelike, Utc};
use sqlx::SqlitePool;
use std::time::Instant;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::intelligence::task_linker::{find_services_dir, resolve_python};

async fn get_last_update_time(pool: &SqlitePool) -> Result<Option<i64>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT MAX(created_at) FROM jira_update_log")
            .fetch_optional(pool)
            .await
            .context("querying jira_update_log")?;

    if let Some((Some(ts),)) = row {
        // created_at is stored as "YYYY-MM-DDTHH:MM:SSZ"
        let ts_clean = ts.replace('Z', "+00:00");
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&ts_clean) {
            return Ok(Some(dt.timestamp()));
        }
    }
    Ok(None)
}

/// Spawns `python3 -m agents.run_jira_updater` when:
///   - `jira_update_enabled` is true
///   - current local hour is within [office_start, office_end)
///   - `jira_update_interval_s` seconds have elapsed since the last update
pub async fn run_jira_update(pool: &SqlitePool, cfg: &Config) -> Result<()> {
    if !cfg.jira_update_enabled {
        return Ok(());
    }

    // Office hours check using local time
    let now_local = Local::now();
    let now_hour = now_local.hour();
    if now_hour < cfg.jira_office_start_hour || now_hour >= cfg.jira_office_end_hour {
        debug!(
            hour = now_hour,
            start = cfg.jira_office_start_hour,
            end = cfg.jira_office_end_hour,
            "outside office hours — skipping jira update"
        );
        return Ok(());
    }

    // Interval check: skip if last update was less than interval_s ago
    let now_ts = Utc::now().timestamp();
    if let Some(last_ts) = get_last_update_time(pool).await? {
        let elapsed = now_ts - last_ts;
        if elapsed < cfg.jira_update_interval_s as i64 {
            debug!(
                elapsed_s = elapsed,
                interval_s = cfg.jira_update_interval_s,
                "jira update interval not elapsed"
            );
            return Ok(());
        }
    }

    let services_dir = match find_services_dir(cfg) {
        Some(d) => d,
        None => {
            warn!("cannot locate services dir — skipping jira update");
            return Ok(());
        }
    };

    let python = resolve_python(&services_dir);

    info!(
        python = %python,
        services_dir = %services_dir.display(),
        "spawning run_jira_updater subprocess"
    );

    let t0 = Instant::now();

    let mut child = match Command::new(&python)
        .arg("-m")
        .arg("agents.run_jira_updater")
        .current_dir(&services_dir)
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "could not spawn run_jira_updater");
            return Ok(());
        }
    };

    // Drain stderr in background to prevent the subprocess blocking on a full pipe buffer.
    let stderr_task = {
        use tokio::io::AsyncReadExt;
        let mut stderr = child.stderr.take().expect("stderr was piped");
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        })
    };

    match tokio::time::timeout(
        std::time::Duration::from_secs(cfg.classification_timeout_s),
        child.wait(),
    )
    .await
    {
        Ok(Ok(status)) => {
            let stderr_bytes = stderr_task.await.unwrap_or_default();
            if !stderr_bytes.is_empty() {
                let stderr_str = String::from_utf8_lossy(&stderr_bytes);
                debug!(stderr = %stderr_str, "run_jira_updater stderr");
            }
            info!(elapsed = ?t0.elapsed(), exit_status = ?status, "jira update complete");
        }
        Ok(Err(e)) => {
            stderr_task.abort();
            warn!(error = %e, "jira updater process error");
        }
        Err(_) => {
            warn!("jira updater timed out — killing subprocess");
            let _ = child.kill().await;
            stderr_task.abort();
        }
    }

    Ok(())
}
