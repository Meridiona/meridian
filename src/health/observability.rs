//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Observability sink health. If OpenObserve is down, traces/logs silently drop
// — which blinds the very fault-attribution this layer depends on, so it is
// worth a check of its own. Export is gated on credentials being present in
// settings.json (resolved by observability::resolve_otlp_target; the old
// MERIDIAN_OO_AUTH env fallback is deprecated and ignored).

use crate::config::Config;
use crate::health::Check;
use std::time::Duration;

pub async fn checks(_cfg: &Config) -> Vec<Check> {
    // Use the cheap helpers — the health check never needs the auth credential.
    if !crate::observability::is_otlp_configured() {
        return vec![Check::info(
            "openobserve",
            "obs",
            "OTLP export disabled (no credentials in Settings) — telemetry not collected",
        )];
    }
    let endpoint = crate::observability::resolve_otlp_endpoint()
        .unwrap_or_else(|| "http://localhost:5080/api/default/v1/traces".to_string());
    let healthz = derive_healthz(&endpoint);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return vec![Check::info(
                "openobserve",
                "obs",
                format!("client error ({e})"),
            )]
        }
    };

    vec![match client.get(&healthz).send().await {
        Ok(resp) if resp.status().is_success() => Check::ok("openobserve", "obs", "reachable"),
        Ok(resp) => Check::warn(
            "openobserve",
            "obs",
            format!(
                "HTTP {} — traces/logs may be dropping",
                resp.status().as_u16()
            ),
        )
        .with_remedy("check the openobserve launchd agent (port 5080)"),
        Err(_) => Check::warn("openobserve", "obs", "not reachable — traces/logs dropping")
            .with_remedy("start OpenObserve (port 5080)"),
    }]
}

/// `http://host:port/api/...` → `http://host:port/healthz`.
fn derive_healthz(endpoint: &str) -> String {
    if let Some(scheme_end) = endpoint.find("://") {
        let rest = &endpoint[scheme_end + 3..];
        let host_port = rest.split('/').next().unwrap_or(rest);
        return format!("{}://{}/healthz", &endpoint[..scheme_end], host_port);
    }
    "http://localhost:5080/healthz".to_string()
}

#[cfg(test)]
mod tests {
    use super::derive_healthz;

    #[test]
    fn healthz_derived_from_otlp_endpoint() {
        assert_eq!(
            derive_healthz("http://localhost:5080/api/default/v1/traces"),
            "http://localhost:5080/healthz"
        );
        assert_eq!(
            derive_healthz("https://127.0.0.1:9000/x"),
            "https://127.0.0.1:9000/healthz"
        );
        assert_eq!(derive_healthz("garbage"), "http://localhost:5080/healthz");
    }
}
