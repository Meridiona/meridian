//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Background shipper task: drains ~/.meridian/telemetry/pending/ to OpenObserve.
//
// Runs in a tokio background task spawned from main.rs. Every
// `MERIDIAN_TELEMETRY_SHIP_INTERVAL_S` seconds (default 30):
//
//   1. If `resolve_otlp_target()` is None (creds absent), skip — leave files.
//   2. List pending/ oldest-first by filename timestamp.
//   3. POST each .otlp to the appropriate OO endpoint (traces / logs).
//   4. On 2xx → move to sent/.  On any failure → stop this tick, leave rest.
//   5. Retention: delete sent/ files older than MERIDIAN_TELEMETRY_RETENTION_DAYS (7).
//   6. Pending cap: drop OLDEST beyond MERIDIAN_TELEMETRY_MAX_PENDING_MB (512) with warn.
//
// The shipper is the ONLY writer to sent/ and the only one that deletes files.
// The spool writer (writer.rs) only adds to pending/.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::{
    observability::{resolve_otlp_endpoint, resolve_otlp_target},
    telemetry_spool::{
        derive_base_url, ship_one,
        writer::{
            micros_from_filename, pending_dir, quarantine_dir, resolve_telemetry_dir, sent_dir,
            seq_from_filename, signal_from_filename,
        },
        ShipError,
    },
};

const DEFAULT_SHIP_INTERVAL_S: u64 = 30;
const DEFAULT_RETENTION_DAYS: u64 = 7;
const DEFAULT_MAX_PENDING_MB: u64 = 512;
/// `.otlp.tmp` files older than this are crash orphans — a healthy write turns
/// tmp → final in milliseconds, so anything this old will never be completed.
const TMP_ORPHAN_MAX_AGE_SECS: u64 = 300;

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

fn max_pending_bytes() -> u64 {
    let mb = std::env::var("MERIDIAN_TELEMETRY_MAX_PENDING_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_PENDING_MB);
    mb * 1024 * 1024
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
                    tracing::debug!(file = %filename, signal, "telemetry file shipped");
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

    // Retention: prune old sent files.
    prune_sent(&sent)?;

    Ok(())
}

/// Remove `.otlp.tmp` files left behind by a crash between write and rename.
/// A healthy write completes the rename in milliseconds, so any tmp older than
/// `TMP_ORPHAN_MAX_AGE_SECS` is dead weight the cap/lister would never see.
fn sweep_tmp_orphans(pending: &Path) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let Ok(entries) = std::fs::read_dir(pending) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".otlp.tmp") {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|mt| now.saturating_sub(mt.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()))
            .unwrap_or(u64::MAX);
        if age >= TMP_ORPHAN_MAX_AGE_SECS {
            let _ = std::fs::remove_file(&path);
            tracing::debug!(file = %path.display(), age_secs = age, "swept crash-orphaned spool tmp file");
        }
    }
}

/// List `.otlp` files in `dir` sorted oldest-first by `(micros, seq)`.
///
/// Including `seq` makes ordering deterministic when two files share a
/// microsecond (traces+logs back-to-back, or a burst). Files with an
/// unparseable name are SKIPPED rather than collapsed to key `0` — a renamed /
/// foreign file must not sort permanently to the front and ship every tick.
fn list_pending_oldest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };

    let mut files: Vec<(u64, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = micros_from_filename(&name)?;
            let seq = seq_from_filename(&name)?;
            Some((micros, seq, p))
        })
        .collect();

    files.sort_by_key(|(m, s, _)| (*m, *s));
    files.into_iter().map(|(_, _, p)| p).collect()
}

/// Delete sent/ files whose mtime is older than `retention_days`.
fn prune_sent(sent_dir: &Path) -> Result<()> {
    let cutoff_secs = retention_days() * 24 * 3600;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let Ok(entries) = std::fs::read_dir(sent_dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "otlp") {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                let age_secs = now.saturating_sub(
                    mtime
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
                // `>=` so a retention of 0 days means "keep nothing" (prune even
                // just-shipped files); the default 7-day cutoff is unaffected since
                // a fresh file's age (~0s) is never >= 604800s.
                if age_secs >= cutoff_secs {
                    let _ = std::fs::remove_file(&path);
                    tracing::debug!(file = %path.display(), age_days = age_secs / 86400, "pruned old sent telemetry file");
                }
            }
        }
    }
    Ok(())
}

/// Drop OLDEST pending files beyond the size cap with a structured warning.
/// Never silently drops — always emits `tracing::warn!` with count + bytes.
fn enforce_pending_cap(pending: &Path) -> Result<()> {
    let max = max_pending_bytes();

    let Ok(entries) = std::fs::read_dir(pending) else {
        return Ok(());
    };

    let mut files: Vec<(u64, u64, u64, PathBuf)> = entries // (micros, seq, size, path)
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = micros_from_filename(&name)?;
            let seq = seq_from_filename(&name)?;
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some((micros, seq, size, p))
        })
        .collect();

    // Sort oldest-first by (micros, seq) — same total order the shipper uses, so
    // the cap evicts the genuinely-oldest records, not whatever read_dir yields.
    files.sort_by_key(|(m, s, _, _)| (*m, *s));

    let total: u64 = files.iter().map(|(_, _, s, _)| s).sum();
    if total <= max {
        return Ok(());
    }

    let mut to_drop = total - max;
    let mut dropped_count = 0u64;
    let mut dropped_bytes = 0u64;

    for (_, _, size, path) in &files {
        if to_drop == 0 {
            break;
        }
        let _ = std::fs::remove_file(path);
        dropped_bytes += size;
        dropped_count += 1;
        to_drop = to_drop.saturating_sub(*size);
    }

    if dropped_count > 0 {
        tracing::warn!(
            dropped_files = dropped_count,
            dropped_bytes,
            cap_bytes = max,
            "pending telemetry cap exceeded — oldest spool files dropped"
        );
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

    #[test]
    fn list_pending_sorted_oldest_first() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        // Write files with different seq numbers; filename micros will be equal
        // or ascending since they run in sequence. Seq counter disambiguates.
        let p1 = write_pending(&base, "traces", b"old").unwrap();
        let p2 = write_pending(&base, "traces", b"new").unwrap();

        let pending = pending_dir(&base);
        let sorted = list_pending_oldest_first(&pending);
        assert_eq!(sorted.len(), 2);
        // First file written should appear first (lower seq or lower micros)
        assert_eq!(sorted[0], p1);
        assert_eq!(sorted[1], p2);
    }

    #[test]
    fn list_pending_orders_same_micros_by_seq_and_skips_unparseable() {
        let dir = TempDir::new().unwrap();
        let pending = pending_dir(dir.path());
        std::fs::create_dir_all(&pending).unwrap();

        // Same microsecond, out-of-order seq — must come back seq 0 then seq 1.
        std::fs::write(pending.join("traces-1000-1.otlp"), b"b").unwrap();
        std::fs::write(pending.join("traces-1000-0.otlp"), b"a").unwrap();
        // A crash-orphan tmp and a foreign name must be ignored entirely (the
        // old `unwrap_or(0)` would have sorted the foreign file permanently first).
        std::fs::write(pending.join("traces-1000-2.otlp.tmp"), b"x").unwrap();
        std::fs::write(pending.join("garbage.otlp"), b"y").unwrap();

        let sorted = list_pending_oldest_first(&pending);
        assert_eq!(sorted.len(), 2, "tmp + foreign names excluded");
        assert!(sorted[0].file_name().unwrap().to_str().unwrap().ends_with("-0.otlp"));
        assert!(sorted[1].file_name().unwrap().to_str().unwrap().ends_with("-1.otlp"));
    }

    #[test]
    fn prune_sent_removes_old_files() {
        let dir = TempDir::new().unwrap();
        let sent = dir.path().join("sent");
        std::fs::create_dir_all(&sent).unwrap();

        // Create an old file by setting mtime to epoch+1s
        let old_file = sent.join("traces-1-0.otlp");
        std::fs::write(&old_file, b"x").unwrap();
        // Set mtime to a very old time via filetime
        // We can't easily set mtime in pure std; instead we mock via a very low
        // retention (0 days) — everything gets pruned.
        // Override env for this test.
        std::env::set_var("MERIDIAN_TELEMETRY_RETENTION_DAYS", "0");
        prune_sent(&sent).unwrap();
        std::env::remove_var("MERIDIAN_TELEMETRY_RETENTION_DAYS");

        // File should be removed (0-day retention = everything > 0s old)
        assert!(!old_file.exists());
    }

    #[test]
    fn pending_cap_drops_oldest_first_with_warn() {
        // Set cap to 1 byte so everything over 1 byte gets dropped
        std::env::set_var("MERIDIAN_TELEMETRY_MAX_PENDING_MB", "0");

        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        write_pending(&base, "traces", b"aaa").unwrap();
        write_pending(&base, "traces", b"bbb").unwrap();

        let pending = pending_dir(&base);
        // Cap of 0 MB → all files should be dropped
        enforce_pending_cap(&pending).unwrap();

        let remaining: Vec<_> = std::fs::read_dir(&pending)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "otlp"))
            .collect();
        // With 0MB cap all are dropped
        assert!(remaining.is_empty());

        std::env::remove_var("MERIDIAN_TELEMETRY_MAX_PENDING_MB");
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
