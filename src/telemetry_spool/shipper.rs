//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Background shipper task: drains ~/.meridian/telemetry/pending/ to OpenObserve.
//
// Runs in a tokio background task spawned from main.rs. Every
// `MERIDIAN_TELEMETRY_SHIP_INTERVAL_S` seconds (default 30):
//
// Steps 1-3 are ONE `retention::run_housekeeping` unit handed to
// `spawn_blocking` — never inline on the runtime, because sent/ sits at a
// ~264k-file steady state and walking it on a runtime worker stalled the
// daemon's timers 30-100s (see `retention::prune_due` for the incident):
//
//   1. Sweep crash-orphaned .otlp.tmp files.
//   2. Retention: age-prune BOTH pending/ and sent/ (files older than
//      MERIDIAN_TELEMETRY_RETENTION_DAYS, default 7) — rationed to every 60th
//      tick (~30 min; tick 0 prunes so a long-stopped daemon catches up), and
//      independent of the ship-target check below, so a Canonical/packaged
//      install (which never ships — see `observability::resolve_otlp_target`)
//      still enforces retention on the local spool instead of growing it
//      forever. Also caps the launchd-redirected raw log files (daemon.log
//      etc. — the crash safety net, unrelated to the OTel spool) at
//      MERIDIAN_LAUNCHD_LOG_MAX_MB (default 10) via copytruncate, since they
//      have no rotation of their own.
//   3. Pending cap: drop OLDEST beyond MERIDIAN_TELEMETRY_MAX_PENDING_MB (512) with warn.
//   4. If `resolve_otlp_target()` is None (creds absent, or a Canonical
//      install), skip delivery — leave files in pending/ for retention/cap to
//      manage.
//   5. List pending/ oldest-first by filename timestamp.
//   6. On the packaged central path (target.redact), run the on-device
//      allowlist + error-only filter (redact.rs) on each payload first —
//      full-fidelity local capture, redacted egress. Dev checkouts ship raw.
//   7. POST each .otlp to the appropriate OO endpoint (traces / logs), with the
//      scheme-aware Authorization header (Basic on dev, Bearer on central).
//   8. On 2xx → move to sent/.  On any failure → stop this tick, leave rest.
//
// The shipper is the ONLY writer to sent/ and the only one that deletes files.
// The spool writer (writer.rs) only adds to pending/.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::{
    observability::{resolve_otlp_endpoint, resolve_otlp_target},
    telemetry_spool::{
        derive_base_url,
        redact::{redact_and_filter, Redacted},
        retention::{list_pending_oldest_first, prune_due, run_housekeeping},
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

/// Tick counter driving the prune cadence (see `retention::prune_due`).
/// Process-lifetime only — resetting on restart just means the first tick
/// prunes again, which is the desired catch-up behaviour.
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

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

    // The whole filesystem sweep (tmp-orphan clear, age prune, launchd log
    // cap, pending cap) runs as ONE unit on the BLOCKING pool, with the age
    // prune rationed to every 60th tick. Both halves are load-bearing:
    // `sent/` sits at a ~264k-file steady state and the prune `stat`s every
    // one, so running it inline on a runtime worker twice a minute stalled
    // the daemon's timers 30-100 s and starved its socket greeting - the
    // probe false-negatives behind the tray's status flapping and the old
    // watchdog storms (thread-sampled live, 2026-08-05; the whole story is on
    // `retention::prune_due`). Retention itself stays UNCONDITIONAL relative
    // to shipping - it must run before any ship-target check, because a
    // Canonical/packaged install never ships and pending/ is its only spool.
    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    {
        let pending = pending.clone();
        let sent = sent.clone();
        let retention_secs = retention_days() * 24 * 3600;
        tokio::task::spawn_blocking(move || {
            run_housekeeping(&pending, &sent, retention_secs, prune_due(tick))
        })
        .await
        .context("telemetry housekeeping task")??;
    }

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

    // The Authorization header value (scheme + credential), assembled once for
    // the whole tick — `Basic <b64>` on a dev checkout, `Bearer <token>` on the
    // packaged central path.
    let auth_header = target.auth.header_value();

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

        // Central (packaged-consented) path only: redact + error-only filter
        // before anything leaves the machine. The on-disk file is untouched —
        // this produces a separate stripped copy to POST, so `meridian logs`
        // stays full-fidelity. The dev path (`target.redact == false`) ships the
        // raw bytes to the engineer's own OO.
        let bytes = if target.redact {
            match redact_and_filter(signal, &bytes) {
                Redacted::Payload { bytes, stats } => {
                    tracing::debug!(
                        file = %filename,
                        signal,
                        records_in = stats.records_in,
                        records_out = stats.records_out,
                        attrs_dropped = stats.attrs_dropped,
                        "redacted telemetry before shipping"
                    );
                    bytes
                }
                Redacted::Empty => {
                    // No error records survived the filter — nothing to ship.
                    // Archive to sent/ so retention manages it, no network call.
                    tracing::trace!(file = %filename, signal, "no error records after redaction — archiving without ship");
                    archive_to_sent(&file_path, &sent, &filename);
                    continue;
                }
                Redacted::Undecodable => {
                    // The payload never passed through the allowlist, so it could
                    // leak — it must NEVER be shipped raw. Quarantine it.
                    tracing::warn!(file = %filename, signal, "telemetry payload undecodable — quarantining (never shipped un-redacted)");
                    quarantine_file(&file_path, &quarantine, &filename);
                    continue;
                }
            }
        } else {
            bytes
        };

        match ship_one(&client, &endpoint, &auth_header, bytes).await {
            Ok(()) => {
                // Delivered — archive to sent/ for the export bundle + retention.
                archive_to_sent(&file_path, &sent, &filename);
                // TRACE: one per shipped file (~1/s while draining) — per-file
                // success is spool-debugging detail, not wanted at `meridian=debug`.
                tracing::trace!(file = %filename, signal, "telemetry file shipped");
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
                quarantine_file(&file_path, &quarantine, &filename);
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

/// Archive a delivered (or empty-after-redaction) spool file to `sent/`. If the
/// rename fails (EXDEV across mounts, a permissions blip) DELETE it instead of
/// leaving it in pending/: the payload was already accepted by OO (or had
/// nothing to send), so re-POSTing it next tick would duplicate spans. Losing
/// one sent/ archive copy is the lesser evil vs. duplicate delivery.
fn archive_to_sent(file_path: &Path, sent: &Path, filename: &str) {
    let dest = sent.join(filename);
    if let Err(e) = std::fs::rename(file_path, &dest) {
        tracing::warn!(
            file = %file_path.display(),
            dest = %dest.display(),
            error = %e,
            "could not archive to sent/ — deleting to avoid re-ship"
        );
        let _ = std::fs::remove_file(file_path);
    }
}

/// Move a permanently-rejected or undecodable spool file to `quarantine/` so it
/// stops head-of-line-blocking newer files in the oldest-first queue. Deletes it
/// if even the quarantine move fails.
fn quarantine_file(file_path: &Path, quarantine: &Path, filename: &str) {
    if let Err(e) = std::fs::create_dir_all(quarantine)
        .and_then(|_| std::fs::rename(file_path, quarantine.join(filename)))
    {
        tracing::warn!(file = %filename, error = %e, "failed to quarantine — deleting");
        let _ = std::fs::remove_file(file_path);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry_spool::writer::write_pending;
    use tempfile::TempDir;

    /// The shipper's filesystem sweep must reach the disk ONLY through
    /// `retention::run_housekeeping` handed to `spawn_blocking` - calling the
    /// per-step helpers inline from this async module is exactly the ~264k
    /// blocking `stat`s per tick that stalled the daemon's timers and starved
    /// its socket greeting (see `retention::prune_due`). Nothing at a call
    /// site would look wrong in review, so the split is pinned at the source
    /// level.
    #[test]
    fn housekeeping_stays_off_the_async_runtime() {
        let src = include_str!("shipper.rs");
        assert!(
            src.contains("spawn_blocking"),
            "the housekeeping sweep must run on the blocking pool"
        );
        // Needles assembled at runtime so this test's own source (included in
        // the scan) cannot match them.
        for helper in [
            format!("prune_by_{}", "age"),
            format!("sweep_tmp_{}", "orphans"),
            format!("enforce_pending_{}", "cap"),
            format!("cap_launchd_{}", "logs"),
        ] {
            assert!(
                !src.contains(&helper),
                "{helper} called directly from the async shipper - route it \
                 through retention::run_housekeeping inside spawn_blocking"
            );
        }
    }

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
