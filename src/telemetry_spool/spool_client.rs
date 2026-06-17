//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Custom `HttpClient` implementation that spools OTLP request bodies to disk
// instead of (or in addition to, when wired with the real client) sending them
// directly to OpenObserve.
//
// The SDK calls `send()` once per export batch.  We:
//   1. Inspect the request URI path to determine the signal ("traces" / "logs").
//   2. Write the request body bytes atomically to pending/.
//   3. Return a synthetic HTTP 200 so the SDK considers the export successful.
//
// The background shipper (shipper.rs) then POST-s the files to OO whenever a
// target is configured.  This decouples capture from delivery: we never drop
// telemetry just because OO is momentarily unreachable.

use std::fmt;

use async_trait::async_trait;
use bytes::Bytes;
use http::{Request, Response};
use opentelemetry_http::{HttpClient, HttpError};

use super::writer::{resolve_telemetry_dir, write_pending};

/// An `HttpClient` that writes every OTLP request body to the spool dir.
///
/// Derives signal from the request URI path:
///   - contains `/v1/traces` → "traces"
///   - contains `/v1/logs`   → "logs"
///   - anything else         → "traces" (safe fallback)
#[derive(Debug)]
pub struct SpoolClient {
    // The spool base dir is resolved once at construction time so we don't
    // call shellexpand on every export batch.
    base_dir: std::path::PathBuf,
}

impl SpoolClient {
    /// Build a `SpoolClient` that spools to the default telemetry directory.
    /// Returns `Err` if the telemetry dir cannot be resolved (HOME not set).
    pub fn new() -> anyhow::Result<Self> {
        let base_dir = resolve_telemetry_dir()?;
        Ok(Self { base_dir })
    }
}

impl fmt::Display for SpoolClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpoolClient({})", self.base_dir.display())
    }
}

#[async_trait]
impl HttpClient for SpoolClient {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Bytes>, HttpError> {
        let uri = request.uri().to_string();
        let signal = if uri.contains("/v1/logs") {
            "logs"
        } else {
            // Default to traces for /v1/traces and any unknown path.
            "traces"
        };

        let body = request.into_body();

        if let Err(e) = write_pending(&self.base_dir, signal, &body) {
            // Log but do NOT fail — returning an error causes the BatchSpanProcessor
            // to log a noisy export failure and retry. Instead we warn and let the
            // SDK think the export succeeded (data is lost only if write_pending
            // fails, which means the disk is full — in that case the cap enforcer
            // will have already trimmed the backlog).
            tracing::warn!(
                signal,
                error = %e,
                "telemetry spool write failed — payload dropped"
            );
        }

        // Return a synthetic 200 so the SDK considers export successful.
        let response = Response::builder()
            .status(200)
            .body(Bytes::new())
            .expect("synthetic 200 response always builds");
        Ok(response)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry_spool::writer::{pending_dir, signal_from_filename};
    use http::Request;
    use tempfile::TempDir;

    fn client_with_dir(dir: &std::path::Path) -> SpoolClient {
        SpoolClient {
            base_dir: dir.to_path_buf(),
        }
    }

    fn make_request(uri: &str, body: Vec<u8>) -> Request<Vec<u8>> {
        Request::builder().uri(uri).body(body).unwrap()
    }

    #[tokio::test]
    async fn traces_request_written_to_pending() {
        let dir = TempDir::new().unwrap();
        let client = client_with_dir(dir.path());

        let req = make_request(
            "http://localhost:5080/api/default/v1/traces",
            b"trace-payload".to_vec(),
        );
        let resp = client.send(req).await.unwrap();

        assert_eq!(resp.status(), 200);

        let pending = pending_dir(dir.path());
        let files: Vec<_> = std::fs::read_dir(&pending).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);

        let name = files[0].file_name().into_string().unwrap();
        assert_eq!(signal_from_filename(&name), Some("traces"));
        let contents = std::fs::read(files[0].path()).unwrap();
        assert_eq!(contents, b"trace-payload");
    }

    #[tokio::test]
    async fn logs_request_written_with_logs_signal() {
        let dir = TempDir::new().unwrap();
        let client = client_with_dir(dir.path());

        let req = make_request(
            "http://localhost:5080/api/default/v1/logs",
            b"log-payload".to_vec(),
        );
        let _resp = client.send(req).await.unwrap();

        let pending = pending_dir(dir.path());
        let files: Vec<_> = std::fs::read_dir(&pending).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().into_string().unwrap();
        assert_eq!(signal_from_filename(&name), Some("logs"));
    }

    #[tokio::test]
    async fn returns_200_always() {
        let dir = TempDir::new().unwrap();
        let client = client_with_dir(dir.path());
        let req = make_request("http://x/v1/traces", vec![]);
        let resp = client.send(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }
}
