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

pub mod cli;
pub mod shipper;
pub mod spool_client;
pub mod writer;

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
