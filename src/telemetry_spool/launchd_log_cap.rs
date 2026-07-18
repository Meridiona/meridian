//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Size-caps the launchd-redirected raw stdout/stderr log files via
//! "copytruncate" — split out of `shipper.rs` (which was over the repo's
//! 500-line cap) since capping these crash-safety-net files is a
//! self-contained concern, independent of the OTLP spool ship/retention
//! logic the rest of that module owns.
//!
//! # Who calls this
//! `shipper::run_tick()`, once per shipper tick (every
//! `MERIDIAN_TELEMETRY_SHIP_INTERVAL_S`, default 30s), alongside the OTLP
//! spool's own retention pruning.
//!
//! # Related
//! - `observability::resolve_log_dir()` — where these files live
//!   (`~/.meridian/logs/`, launchd's `StandardOutPath`/`StandardErrorPath`
//!   target; see each service's `com.meridiona.*.plist`).
//! - `telemetry_spool::mod::build_export_bundle` — folds these (now capped)
//!   files into diagnostics export bundles alongside the OTLP spool.

use std::path::Path;

/// launchd-redirected raw stdout/stderr files (see each service's
/// `com.meridiona.*.plist` `StandardOutPath`/`StandardErrorPath`) — the crash
/// safety net `tracing`/`logging` no longer mirrors into (the OTel spool is
/// the sole application-log sink; see `observability.rs`'s module doc). These
/// have no built-in rotation, so they're capped here instead.
pub(crate) const LAUNCHD_LOG_NAMES: &[&str] = &[
    "daemon.log",
    "daemon-error.log",
    "tray.log",
    "tray-error.log",
    "a11y-helper.log",
    "a11y-helper-error.log",
];

const DEFAULT_LAUNCHD_LOG_MAX_MB: u64 = 10;

fn launchd_log_max_bytes() -> u64 {
    let mb = std::env::var("MERIDIAN_LAUNCHD_LOG_MAX_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_LAUNCHD_LOG_MAX_MB);
    mb * 1024 * 1024
}

/// Cap each known launchd-redirected log file to `MERIDIAN_LAUNCHD_LOG_MAX_MB`
/// (default 10MB) via a "copytruncate" strategy: keep only the last
/// `max_bytes` of content, truncate the rest. Safe for a file an unrelated
/// process (launchd, holding the service's stdout/stderr fd open for its
/// lifetime) is actively appending to — POSIX re-seeks to true EOF on every
/// `write()` when the fd was opened `O_APPEND` (which launchd's
/// `StandardOutPath`/`StandardErrorPath` redirection uses), so truncating out
/// from under it just shortens where "EOF" is; the writer keeps appending
/// correctly from there. This is the same strategy `logrotate`'s
/// `copytruncate` option uses for exactly this "can't ask the writer to
/// reopen its fd" case.
pub(crate) fn cap_launchd_logs() {
    let Ok(log_dir) = crate::observability::resolve_log_dir() else {
        return;
    };
    let max_bytes = launchd_log_max_bytes();

    for name in LAUNCHD_LOG_NAMES {
        let path = log_dir.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() <= max_bytes {
            continue;
        }
        match copytruncate(&path, max_bytes) {
            Ok(()) => {
                tracing::debug!(file = %path.display(), kept_bytes = max_bytes, "capped oversized launchd log file");
            }
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "failed to cap oversized launchd log file");
            }
        }
    }
}

/// Keep only the last `keep_bytes` of `path`, in place. Reads the tail,
/// truncates to zero, writes the tail back — a single fd, no rename, so it
/// works even while another process holds the same path open for appending.
///
/// Same known race `logrotate --copytruncate` has: bytes an O_APPEND writer
/// appends between the `read_to_end` and `set_len(0)` below are lost (read
/// too early to be in `tail`, then zeroed by the truncate). We can't close
/// that window without an advisory lock the writer doesn't take, but we CAN
/// detect it after the fact — re-check the length right before truncating and
/// warn if it grew, so a lost burst leaves a trace instead of vanishing
/// silently.
fn copytruncate(path: &Path, keep_bytes: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let len = file.metadata()?.len();
    let skip = len.saturating_sub(keep_bytes);

    file.seek(SeekFrom::Start(skip))?;
    let mut tail = Vec::with_capacity(keep_bytes.min(len) as usize);
    file.read_to_end(&mut tail)?;

    // Race check: if the file grew between the read above and now, a
    // concurrent O_APPEND writer landed bytes we didn't capture in `tail` —
    // they're about to be destroyed by `set_len(0)`.
    if let Ok(meta) = file.metadata() {
        if meta.len() > len {
            tracing::warn!(
                file = %path.display(),
                pre_read_len = len,
                pre_truncate_len = meta.len(),
                "copytruncate: file grew during the read — some in-flight log \
                 bytes were lost to this truncation (known copytruncate race)"
            );
        }
    }

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&tail)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn copytruncate_keeps_only_the_tail() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("daemon.log");
        std::fs::write(&path, b"0123456789abcdefghij").unwrap(); // 20 bytes

        copytruncate(&path, 5).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"fghij");
    }

    #[test]
    fn copytruncate_noop_when_keep_exceeds_len() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("daemon.log");
        std::fs::write(&path, b"short").unwrap();

        copytruncate(&path, 100).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"short");
    }
}
