//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Atomic file writer for OTLP telemetry spool.
//
// Writes raw OTLP/HTTP protobuf request bodies to
// `~/.meridian/telemetry/pending/` using an atomic tmp-then-rename strategy so
// a partial write is never visible to the shipper.
//
// Filename scheme: `<signal>-<unix_micros>-<seq>.otlp`
//   signal = "traces" | "logs"
//   unix_micros = microseconds since Unix epoch (monotonic within a process)
//   seq = per-process counter to disambiguate same-microsecond writes

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

/// Global sequence counter — unique per process lifetime.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Resolve the telemetry spool base directory.
///
/// Precedence: `MERIDIAN_TELEMETRY_DIR` env → `~/.meridian/telemetry`.
pub fn resolve_telemetry_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("MERIDIAN_TELEMETRY_DIR") {
        return Ok(PathBuf::from(shellexpand::tilde(&dir).into_owned()));
    }
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".meridian").join("telemetry"))
}

/// `~/.meridian/telemetry/pending/`
pub fn pending_dir(base: &Path) -> PathBuf {
    base.join("pending")
}

/// `~/.meridian/telemetry/sent/`
pub fn sent_dir(base: &Path) -> PathBuf {
    base.join("sent")
}

/// Build a spool filename for the given signal.
///
/// `signal` must be `"traces"` or `"logs"`.
pub fn make_filename(signal: &str) -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{signal}-{micros}-{seq}.otlp")
}

/// Write `bytes` atomically to `pending_dir`.
///
/// Creates both `pending/` and the parent `base/` if absent.
/// Returns the path of the written file on success.
pub fn write_pending(base: &Path, signal: &str, bytes: &[u8]) -> Result<PathBuf> {
    let pending = pending_dir(base);
    std::fs::create_dir_all(&pending)
        .with_context(|| format!("create pending dir {}", pending.display()))?;

    let filename = make_filename(signal);
    let final_path = pending.join(&filename);
    let tmp_path = pending.join(format!("{filename}.tmp"));

    std::fs::write(&tmp_path, bytes)
        .with_context(|| format!("write tmp spool file {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("rename spool file to {}", final_path.display()))?;

    tracing::debug!(
        signal,
        path = %final_path.display(),
        bytes = bytes.len(),
        "spooled telemetry payload"
    );
    Ok(final_path)
}

/// Determine the OTLP signal type from a spool filename.
///
/// Returns `Some("traces")` / `Some("logs")` / `None` for unknown.
pub fn signal_from_filename(name: &str) -> Option<&str> {
    if name.starts_with("traces-") {
        Some("traces")
    } else if name.starts_with("logs-") {
        Some("logs")
    } else {
        None
    }
}

/// Parse the unix-micros timestamp from a spool filename.
///
/// Filename format: `<signal>-<unix_micros>-<seq>.otlp`
pub fn micros_from_filename(name: &str) -> Option<u64> {
    // Strip the signal prefix
    let after_signal = if let Some(s) = name.strip_prefix("traces-") {
        s
    } else if let Some(s) = name.strip_prefix("logs-") {
        s
    } else {
        return None;
    };
    // First segment up to the next `-` is unix_micros
    let micros_str = after_signal.split('-').next()?;
    micros_str.parse().ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_pending_creates_file_with_correct_signal() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        let payload = b"hello traces";

        let path = write_pending(&base, "traces", payload).unwrap();
        assert!(path.exists());
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("traces-"));
        assert_eq!(std::fs::read(&path).unwrap(), payload);
    }

    #[test]
    fn write_pending_logs_signal() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        let path = write_pending(&base, "logs", b"log bytes").unwrap();
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("logs-"));
    }

    #[test]
    fn filenames_are_unique() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        let p1 = write_pending(&base, "traces", b"a").unwrap();
        let p2 = write_pending(&base, "traces", b"b").unwrap();
        assert_ne!(p1, p2);
    }

    #[test]
    fn signal_from_filename_roundtrips() {
        assert_eq!(signal_from_filename("traces-1234-0.otlp"), Some("traces"));
        assert_eq!(signal_from_filename("logs-1234-1.otlp"), Some("logs"));
        assert_eq!(signal_from_filename("metrics-1234-0.otlp"), None);
    }

    #[test]
    fn micros_from_filename_parses() {
        assert_eq!(
            micros_from_filename("traces-1718000000000000-42.otlp"),
            Some(1718000000000000u64)
        );
        assert_eq!(micros_from_filename("logs-9999-0.otlp"), Some(9999u64));
        assert_eq!(micros_from_filename("unknown.otlp"), None);
    }
}
