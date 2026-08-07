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
    // `is_otlp_configured()` is false for two unrelated reasons, and reporting
    // them with one message was actively misleading. On a packaged install it is
    // false BY DESIGN — that path is scoped to the engineer's own OpenObserve,
    // and a packaged install ships error-only redacted telemetry to the central
    // gateway instead — so "telemetry not collected" was simply wrong there.
    // Capture to the local spool is unconditional either way (the only kill
    // switch is MERIDIAN_TELEMETRY_DISABLED), which is what `meridian logs` and
    // Export Diagnostics read, so no install is ever truly dark.
    if crate::observability::is_canonical_install() {
        // Ask, do not assume. A fixed "errors ship to the central gateway"
        // string would assert a good state nobody checked — the same fault as
        // asserting a bad one, and harder to catch because nobody investigates
        // a green line. Two real machines it would lie to: a user who opted OUT
        // of error reporting (opt-out, default on, so this is a deliberate
        // choice), and a locally-built binary staged into ~/.meridian/bin, which
        // `is_canonical_install()` classifies as canonical but whose
        // release-injected `option_env!` endpoint/token are unset, so it can
        // never ship anything.
        //
        // `resolve_otlp_target()` already answers this and costs one
        // settings.json read — the same file the `is_otlp_configured()` call
        // below would have loaded anyway.
        return vec![match crate::observability::resolve_otlp_target() {
            Some(_) => Check::info(
                "openobserve",
                "obs",
                "packaged install — captured locally; errors ship to the central gateway",
            ),
            None => Check::info(
                "openobserve",
                "obs",
                "packaged install — captured locally; error reporting is off \
                 (or this build has no baked endpoint)",
            ),
        }];
    }
    // Use the cheap helpers — the health check never needs the auth credential.
    if !crate::observability::is_otlp_configured() {
        return vec![Check::info(
            "openobserve",
            "obs",
            "local OTLP export off (no credentials in Settings) — capture still runs; read it with `meridian logs`",
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
