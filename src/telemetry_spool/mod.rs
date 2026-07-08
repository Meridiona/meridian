//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Telemetry spool module.
//
// Provides durable OTLP telemetry delivery across OpenObserve downtime:
//
//   writer.rs       — atomic file writer: pending/<signal>-<micros>-<seq>.otlp
//   spool_client.rs — `HttpClient` impl that intercepts OTLP export calls and
//                     spools request bodies instead of posting directly
//   shipper.rs      — background tokio task: drains pending/ → OO when online
//   cli.rs          — `meridian telemetry status|export|import` subcommands
//
// The two delivery callers (the background shipper and the `telemetry import`
// CLI) share `derive_base_url` + `ship_one` from this root so the URL-derivation
// and HTTP-status classification can never drift between them.
//
// `build_export_bundle` (below) is the single implementation of "package up
// local telemetry into a tar.gz" — shared by `cli.rs`'s `telemetry export` and
// the tray's `export_diagnostics_bundle` Tauri command (a Canonical/packaged
// install's only path to a developer's OpenObserve: hand-export, hand-import).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

pub mod cli;
pub(crate) mod launchd_log_cap;
pub mod render;
pub(crate) mod retention;
pub mod shipper;
pub mod spool_client;
pub mod writer;

/// Bundle local telemetry into a `.tar.gz`: every `.otlp` file in `pending/`
/// and `sent/` (optionally filtered to `micros >= since_micros`) — the OTel
/// spool, sole source of application logs/traces — plus, when
/// `include_launchd_logs` is set, the launchd-redirected raw stdout/stderr
/// files (`daemon.log`, `mlx-server.log`, etc. — the crash safety net; see
/// `observability.rs`'s module doc) so a single bundle also carries whatever
/// OTel structurally can't: a panic/crash from before or outside the logger.
///
/// Returns the output path and the number of files archived. Writes to `out`
/// if given, else `<telemetry_dir>/export-<unix_micros>.tar.gz`.
pub fn build_export_bundle(
    out: Option<&Path>,
    since_micros: Option<u64>,
    include_launchd_logs: bool,
) -> Result<(PathBuf, usize)> {
    let base = writer::resolve_telemetry_dir()?;

    let mut all_files: Vec<PathBuf> = Vec::new();
    for dir in [writer::pending_dir(&base), writer::sent_dir(&base)] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_none_or(|e| e != "otlp") {
                    continue;
                }
                if let Some(thresh) = since_micros {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let file_micros = writer::micros_from_filename(name).unwrap_or(0);
                    if file_micros < thresh {
                        continue;
                    }
                }
                all_files.push(p);
            }
        }
    }

    if include_launchd_logs {
        if let Ok(log_dir) = crate::observability::resolve_log_dir() {
            for name in launchd_log_cap::LAUNCHD_LOG_NAMES {
                let p = log_dir.join(name);
                if p.is_file() {
                    all_files.push(p);
                }
            }
        }
    }

    let out_path = if let Some(p) = out {
        p.to_path_buf()
    } else {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        base.join(format!("export-{micros}.tar.gz"))
    };

    let file = std::fs::File::create(&out_path)
        .with_context(|| format!("create {}", out_path.display()))?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    let file_count = all_files.len();
    for file_path in &all_files {
        let name = file_path.file_name().unwrap_or_default();
        tar.append_path_with_name(file_path, name)
            .with_context(|| format!("add {} to archive", file_path.display()))?;
    }

    tar.finish().context("finish tar archive")?;

    Ok((out_path, file_count))
}

/// Strip a `/v1/traces` or `/v1/logs` suffix to recover the OO base URL.
/// Shared by the shipper and the `telemetry import` CLI.
pub fn derive_base_url(endpoint: &str) -> String {
    if let Some(base) = endpoint.strip_suffix("/v1/traces") {
        return base.to_string();
    }
    if let Some(base) = endpoint.strip_suffix("/v1/logs") {
        return base.to_string();
    }
    endpoint.trim_end_matches('/').to_string()
}

/// Why a single ship attempt failed — drives whether the caller quarantines the
/// file (terminal) or stops the tick and retries later (retryable).
#[derive(Debug)]
pub enum ShipError {
    /// The server rejected THIS payload and retrying the same bytes can never
    /// succeed (HTTP 400 malformed/truncated protobuf, 413 too large, 422
    /// unprocessable). Quarantine it so one poison file can't head-of-line-block
    /// every newer record behind it in the oldest-first queue.
    Terminal(String),
    /// Transient: network error, HTTP 5xx, 401/403 (creds may be fixed), or 429
    /// (rate limit). Stop the tick and retry next time — OO recovery or a creds
    /// fix drains the backlog without dropping anything.
    Retryable(String),
}

impl std::fmt::Display for ShipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShipError::Terminal(m) => write!(f, "terminal: {m}"),
            ShipError::Retryable(m) => write!(f, "retryable: {m}"),
        }
    }
}

/// POST one OTLP payload to `endpoint`, classifying any failure so the caller can
/// quarantine a permanently-rejected payload without stalling the whole queue.
pub async fn ship_one(
    client: &reqwest::Client,
    endpoint: &str,
    auth_b64: &str,
    bytes: Vec<u8>,
) -> Result<(), ShipError> {
    let resp = client
        .post(endpoint)
        .header("Authorization", format!("Basic {auth_b64}"))
        .header("Content-Type", "application/x-protobuf")
        .body(bytes)
        .send()
        .await
        .map_err(|e| ShipError::Retryable(format!("send OTLP request: {e}")))?;

    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else if matches!(status.as_u16(), 400 | 413 | 422) {
        // Payload-level rejection — the same bytes will fail forever.
        Err(ShipError::Terminal(format!("HTTP {status} for {endpoint}")))
    } else {
        // 401/403 (creds), 429 (rate limit), 5xx, anything else → transient.
        Err(ShipError::Retryable(format!(
            "HTTP {status} for {endpoint}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `MERIDIAN_TELEMETRY_DIR`/`MERIDIAN_LOG_DIR` are process-global env vars;
    // Rust runs tests in parallel threads by default, so any two tests here
    // that set them concurrently would race. Serialize this module's tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn build_export_bundle_archives_pending_and_sent_otlp_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        std::env::set_var("MERIDIAN_TELEMETRY_DIR", dir.path());

        writer::write_pending(dir.path(), "traces", b"a").unwrap();
        let sent = writer::sent_dir(dir.path());
        std::fs::create_dir_all(&sent).unwrap();
        std::fs::write(sent.join("logs-1-0.otlp"), b"b").unwrap();

        let out = dir.path().join("bundle.tar.gz");
        let (path, count) = build_export_bundle(Some(&out), None, false).unwrap();

        std::env::remove_var("MERIDIAN_TELEMETRY_DIR");

        assert_eq!(path, out);
        assert_eq!(count, 2);
        assert!(out.exists());
    }

    #[test]
    fn build_export_bundle_folds_in_launchd_logs_when_requested() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        std::env::set_var("MERIDIAN_TELEMETRY_DIR", dir.path());
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::env::set_var("MERIDIAN_LOG_DIR", &log_dir);

        writer::write_pending(dir.path(), "traces", b"a").unwrap();
        std::fs::write(log_dir.join("daemon.log"), b"crash text").unwrap();
        std::fs::write(log_dir.join("some-other-file.txt"), b"ignore me").unwrap();

        let out = dir.path().join("bundle.tar.gz");
        let (_, count) = build_export_bundle(Some(&out), None, true).unwrap();

        std::env::remove_var("MERIDIAN_TELEMETRY_DIR");
        std::env::remove_var("MERIDIAN_LOG_DIR");

        // 1 otlp file + daemon.log (a known launchd log name) — the unrelated
        // .txt file must NOT be swept in.
        assert_eq!(count, 2);
    }

    #[test]
    fn derive_base_url_strips_suffixes_and_trailing_slash() {
        assert_eq!(
            derive_base_url("http://localhost:5080/api/default/v1/traces"),
            "http://localhost:5080/api/default"
        );
        assert_eq!(
            derive_base_url("http://localhost:5080/api/default/v1/logs"),
            "http://localhost:5080/api/default"
        );
        assert_eq!(
            derive_base_url("http://localhost:5080/api/default/"),
            "http://localhost:5080/api/default"
        );
    }
}
