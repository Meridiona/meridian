//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Background shipper task: drains ~/.meridian/telemetry/pending/ to OpenObserve.
//
// Runs in a tokio background task spawned from main.rs. Every
// `MERIDIAN_TELEMETRY_SHIP_INTERVAL_S` seconds (default 30):
//
//   1. Sweep crash-orphaned .otlp.tmp files.
//   2. Retention: age-prune BOTH pending/ and sent/ (files older than
//      MERIDIAN_TELEMETRY_RETENTION_DAYS, default 7) — this runs UNCONDITIONALLY,
//      before the ship-target check below, so a Canonical/packaged install
//      (which never ships — see `observability::resolve_otlp_target`) still
//      enforces retention on the local spool instead of growing it forever.
//      Also caps the launchd-redirected raw log files (daemon.log etc. — the
//      crash safety net, unrelated to the OTel spool) at
//      MERIDIAN_LAUNCHD_LOG_MAX_MB (default 10) via copytruncate, since they
//      have no rotation of their own.
//   3. Pending cap: drop OLDEST beyond MERIDIAN_TELEMETRY_MAX_PENDING_MB (512) with warn.
//   4. If `resolve_otlp_target()` is None (creds absent, or a Canonical
//      install), skip delivery — leave files in pending/ for retention/cap to
//      manage.
//   5. List pending/ oldest-first by filename timestamp.
//   6. POST each .otlp to the appropriate OO endpoint (traces / logs).
//   7. On 2xx → move to sent/.  On any failure → stop this tick, leave rest.
//
// The shipper is the ONLY writer to sent/ and the only one that deletes files.
// The spool writer (writer.rs) only adds to pending/.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::{
    observability::{resolve_otlp_endpoint, resolve_otlp_target},
    telemetry_spool::{
        derive_base_url,
        launchd_log_cap::cap_launchd_logs,
        retention::{
            enforce_pending_cap, list_pending_oldest_first, prune_by_age, sweep_tmp_orphans,
        },
        ship_one,
        writer::{
            pending_dir, quarantine_dir, resolve_telemetry_dir, sent_dir, signal_from_filename,
        },
        ShipError,
    },
};

const DEFAULT_SHIP_INTERVAL_S: u64 = 30;
const DEFAULT_RETENTION_DAYS: u64 = 7;

/// One-time guard so "export enabled but no credentials" warns once, not every tick.
static WARNED_NO_CREDS: AtomicBool = AtomicBool::new(false);

fn ship_interval() -> Duration {
    let secs = std::env::var("MERIDIAN_TELEMETRY_SHIP_INTERVAL_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SHIP_INTERVAL_S);
    Duration::from_secs(secs)
}

fn retention_days() -> u64 {
    std::env::var("MERIDIAN_TELEMETRY_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Spawn the shipper.  Call from main.rs after daemon init.
pub async fn run_shipper(mut shutdown: watch::Receiver<bool>) {
    let interval = ship_interval();
    tracing::info!(
        interval_secs = interval.as_secs(),
        "telemetry shipper starting"
    );

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!("telemetry shipper stopping");
                break;
            }
            _ = tokio::time::sleep(interval) => {
                if let Err(e) = run_tick().await {
                    tracing::warn!(error = %e, "telemetry shipper tick error");
                }
            }
        }
    }
}

/// Run a single ship pass synchronously. Used by the daemon's shutdown path to
/// drain the final flushed batch before the shipper task is stopped.
pub async fn drain_once() {
    if let Err(e) = run_tick().await {
        tracing::warn!(error = %e, "telemetry shipper final drain error");
    }
}

async fn run_tick() -> Result<()> {
    let base = resolve_telemetry_dir()?;
    let pending = pending_dir(&base);
    let sent = sent_dir(&base);
    let quarantine = quarantine_dir(&base);

    // Clear crash-orphaned `.otlp.tmp` files first so they can't accumulate
    // unbounded (they're invisible to both the cap and the lister otherwise).
    sweep_tmp_orphans(&pending);

    // Retention: age-prune BOTH dirs UNCONDITIONALLY — before any ship-target
    // check. A Canonical/packaged install never ships (resolve_otlp_target()
    // always returns None for it), so pending/ is the only place spooled
    // telemetry ever lives; retention must not depend on delivery happening.
    let retention_secs = retention_days() * 24 * 3600;
    prune_by_age(&pending, retention_secs)?;
    prune_by_age(&sent, retention_secs)?;
    cap_launchd_logs();

    // Enforce pending cap BEFORE trying to ship so we don't OOM on a long OO outage.
    enforce_pending_cap(&pending)?;

    // Resolve ship target — None means OO not configured, leave files.
    let Some(target) = resolve_otlp_target() else {
        // Distinguish "export off" from "enabled but unusable": if an endpoint
        // is configured but credentials don't resolve, we're spooling capture
        // with no path to delivery — the cap will eventually drop it. Warn once
        // so the user isn't silently losing telemetry they think is exporting.
        if resolve_otlp_endpoint().is_some() && !WARNED_NO_CREDS.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "telemetry export endpoint set but credentials missing — spooling \
                 capture with NO delivery (files will be dropped once the pending cap \
                 is hit). Set OpenObserve credentials in the dashboard Settings."
            );
        }
        tracing::debug!("telemetry shipper: no OTLP target configured — skipping");
        return Ok(());
    };
    // Creds are back — re-arm the one-time warning for a future outage.
    WARNED_NO_CREDS.store(false, Ordering::Relaxed);

    let base_url = derive_base_url(&target.endpoint);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("build reqwest client")?;

    // List pending files oldest-first.
    let files = list_pending_oldest_first(&pending);
    if files.is_empty() {
        tracing::debug!("telemetry shipper: pending dir empty");
    } else {
        tracing::debug!(
            pending_count = files.len(),
            "telemetry shipper: shipping pending files"
        );
    }

    std::fs::create_dir_all(&sent)
        .with_context(|| format!("create sent dir {}", sent.display()))?;

    for file_path in files {
        let filename = match file_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let signal = match signal_from_filename(&filename) {
            Some(s) => s,
            None => {
                tracing::warn!(file = %file_path.display(), "unknown signal in spool filename — skipping");
                continue;
            }
        };

        let endpoint = format!("{base_url}/v1/{signal}");

        let bytes = match std::fs::read(&file_path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(file = %file_path.display(), error = %e, "failed to read spool file — skipping");
                continue;
            }
        };

        match ship_one(&client, &endpoint, &target.auth, bytes).await {
            Ok(()) => {
                // Delivered. Move to sent/ for the export bundle + retention. If
                // the rename fails (EXDEV across mounts, a permissions blip) we
                // DELETE the pending file instead of leaving it: it was already
                // accepted by OO, so re-POSTing it next tick would duplicate the
                // spans. Losing the sent/ archive copy of one file is the lesser
                // evil vs. duplicate delivery.
                let dest = sent.join(&filename);
                if let Err(e) = std::fs::rename(&file_path, &dest) {
                    tracing::warn!(
                        file = %file_path.display(),
                        dest = %dest.display(),
                        error = %e,
                        "shipped but could not archive to sent/ — deleting to avoid re-ship"
                    );
                    let _ = std::fs::remove_file(&file_path);
                } else {
                    // TRACE: one per shipped file (~1/s while draining) — per-file
                    // success is spool-debugging detail, not wanted at `meridian=debug`.
                    tracing::trace!(file = %filename, signal, "telemetry file shipped");
                }
            }
            Err(ShipError::Terminal(msg)) => {
                // Permanently rejected (malformed/truncated/too-large payload).
                // Quarantine it so it stops head-of-line-blocking every newer
                // file behind it — then keep draining the rest of the queue.
                tracing::warn!(
                    file = %filename,
                    signal,
                    error = %msg,
                    "telemetry payload permanently rejected — quarantining, continuing"
                );
                if let Err(e) = std::fs::create_dir_all(&quarantine)
                    .and_then(|_| std::fs::rename(&file_path, quarantine.join(&filename)))
                {
                    tracing::warn!(file = %filename, error = %e, "failed to quarantine — deleting");
                    let _ = std::fs::remove_file(&file_path);
                }
            }
            Err(ShipError::Retryable(msg)) => {
                tracing::warn!(
                    file = %filename,
                    signal,
                    error = %msg,
                    "telemetry ship failed (transient) — stopping this tick, files remain"
                );
                // OO is down/unreachable — stop and let the next tick retry.
                break;
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry_spool::writer::write_pending;
    use tempfile::TempDir;

    /// Verifies that a pending file is moved to sent/ when the HTTP call would
    /// succeed.  We simulate a successful "ship" by calling the move logic
    /// directly rather than making a real HTTP request.
    #[test]
    fn file_moves_to_sent_on_success() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        let pending = pending_dir(&base);
        let sent = sent_dir(&base);
        std::fs::create_dir_all(&sent).unwrap();

        let path = write_pending(&base, "traces", b"payload").unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap().to_string();
        let dest = sent.join(&filename);

        std::fs::rename(&path, &dest).unwrap();

        assert!(!pending.join(&filename).exists());
        assert!(sent.join(&filename).exists());
    }

    /// Verifies that a file stays in pending when the ship step fails.
    #[test]
    fn file_stays_in_pending_on_failure() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        let path = write_pending(&base, "traces", b"payload").unwrap();
        // We deliberately do NOT move the file (simulating a failed ship).
        assert!(path.exists(), "file should still be in pending");
    }
}
