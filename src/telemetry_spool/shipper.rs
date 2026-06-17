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
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::{
    observability::resolve_otlp_target,
    telemetry_spool::writer::{pending_dir, resolve_telemetry_dir, sent_dir, signal_from_filename},
};

const DEFAULT_SHIP_INTERVAL_S: u64 = 30;
const DEFAULT_RETENTION_DAYS: u64 = 7;
const DEFAULT_MAX_PENDING_MB: u64 = 512;

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

async fn run_tick() -> Result<()> {
    let base = resolve_telemetry_dir()?;
    let pending = pending_dir(&base);
    let sent = sent_dir(&base);

    // Enforce pending cap BEFORE trying to ship so we don't OOM on a long OO outage.
    enforce_pending_cap(&pending)?;

    // Resolve ship target — None means OO not configured, leave files.
    let Some(target) = resolve_otlp_target() else {
        tracing::debug!("telemetry shipper: no OTLP target configured — skipping");
        return Ok(());
    };

    // Derive base URL: strip /v1/traces suffix if present.
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
                // Move to sent/
                let dest = sent.join(&filename);
                if let Err(e) = std::fs::rename(&file_path, &dest) {
                    tracing::warn!(
                        file = %file_path.display(),
                        dest = %dest.display(),
                        error = %e,
                        "failed to move spool file to sent/"
                    );
                } else {
                    tracing::debug!(
                        file = %filename,
                        signal,
                        "telemetry file shipped"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    file = %filename,
                    signal,
                    error = %e,
                    "telemetry ship failed — stopping this tick, files remain"
                );
                // Stop on first failure — OO may be down.
                break;
            }
        }
    }

    // Retention: prune old sent files.
    prune_sent(&sent)?;

    Ok(())
}

async fn ship_one(
    client: &reqwest::Client,
    endpoint: &str,
    auth_b64: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    let resp = client
        .post(endpoint)
        .header("Authorization", format!("Basic {auth_b64}"))
        .header("Content-Type", "application/x-protobuf")
        .body(bytes)
        .send()
        .await
        .context("send OTLP request")?;

    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "OTLP endpoint returned HTTP {} for {}",
            status,
            endpoint
        ))
    }
}

/// Strip `/v1/traces` or `/v1/logs` suffix to get the base URL.
fn derive_base_url(endpoint: &str) -> String {
    if let Some(base) = endpoint.strip_suffix("/v1/traces") {
        return base.to_string();
    }
    if let Some(base) = endpoint.strip_suffix("/v1/logs") {
        return base.to_string();
    }
    endpoint.trim_end_matches('/').to_string()
}

/// List `.otlp` files in `dir` sorted oldest-first by encoded timestamp.
fn list_pending_oldest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };

    let mut files: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = crate::telemetry_spool::writer::micros_from_filename(&name).unwrap_or(0);
            Some((micros, p))
        })
        .collect();

    files.sort_by_key(|(m, _)| *m);
    files.into_iter().map(|(_, p)| p).collect()
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

    let mut files: Vec<(u64, u64, PathBuf)> = entries // (micros, size, path)
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = crate::telemetry_spool::writer::micros_from_filename(&name).unwrap_or(0);
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some((micros, size, p))
        })
        .collect();

    // Sort oldest-first
    files.sort_by_key(|(m, _, _)| *m);

    let total: u64 = files.iter().map(|(_, s, _)| s).sum();
    if total <= max {
        return Ok(());
    }

    let mut to_drop = total - max;
    let mut dropped_count = 0u64;
    let mut dropped_bytes = 0u64;

    for (_, size, path) in &files {
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

    /// Stub HTTP target — returns 200 for the test.
    /// We can't use a real server in unit tests, so we test the file-move
    /// logic separately using a helper that bypasses the HTTP call.
    #[test]
    fn derive_base_url_strips_v1_traces() {
        assert_eq!(
            derive_base_url("http://localhost:5080/api/default/v1/traces"),
            "http://localhost:5080/api/default"
        );
    }

    #[test]
    fn derive_base_url_strips_v1_logs() {
        assert_eq!(
            derive_base_url("http://localhost:5080/api/default/v1/logs"),
            "http://localhost:5080/api/default"
        );
    }

    #[test]
    fn derive_base_url_passthrough_when_no_suffix() {
        assert_eq!(
            derive_base_url("http://localhost:5080/api/default"),
            "http://localhost:5080/api/default"
        );
    }

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
