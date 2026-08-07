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

/// The non-probing half of [`checks`]: everything decidable without a network
/// call. `Some(check)` short-circuits; `None` means "go probe the dev endpoint".
///
/// `is_otlp_configured()` is false for two unrelated reasons, and reporting them
/// with one message was actively misleading. On a packaged install it is false
/// BY DESIGN — that path is scoped to the engineer's own OpenObserve, and a
/// packaged install ships error-only redacted telemetry to the central gateway
/// instead — so "telemetry not collected" was simply wrong there. Capture to the
/// local spool is unconditional either way (the only kill switch is
/// `MERIDIAN_TELEMETRY_DISABLED`), which is what `meridian logs` and Export
/// Diagnostics read, so no install is ever truly dark.
///
/// `central_target_resolves` must be ASKED, not assumed: a fixed "errors ship to
/// the central gateway" string would assert a good state nobody checked — the
/// same fault as asserting a bad one, and harder to catch because nobody
/// investigates a green line. Two real machines it would lie to: a user who
/// opted OUT of error reporting (opt-out, default on, so a deliberate choice),
/// and a locally-built binary staged into `~/.meridian/bin`, which
/// `is_canonical_install()` classifies as canonical but whose release-injected
/// `option_env!` endpoint and token are unset, so it can never ship anything.
fn offline_verdict(
    canonical: bool,
    central_target_resolves: bool,
    dev_otlp_configured: bool,
) -> Option<Check> {
    if canonical {
        return Some(if central_target_resolves {
            Check::info(
                "openobserve",
                "obs",
                "packaged install — captured locally; errors ship to the central gateway",
            )
        } else {
            Check::info(
                "openobserve",
                "obs",
                "packaged install — captured locally; error reporting is off \
                 (or this build has no baked endpoint)",
            )
        });
    }
    if !dev_otlp_configured {
        return Some(Check::info(
            "openobserve",
            "obs",
            "local OTLP export off (no credentials in Settings) — capture still runs; read it with `meridian logs`",
        ));
    }
    None
}

/// The `openobserve` health check.
///
/// Always returns exactly one [`Check`], so callers can splice it into a report
/// without length handling. [`offline_verdict`] answers everything decidable
/// from configuration alone; only a **non-canonical** install with OTLP
/// credentials configured falls through to the live `/healthz` probe below.
///
/// That asymmetry is the non-obvious part and it is deliberate: a packaged
/// install ships to the release-baked central gateway, whose reachability is
/// not the user's problem and must never be reported to them as a fault. A dev
/// checkout ships to the engineer's own OpenObserve, where "is it up?" is
/// exactly the question worth asking.
///
/// `_cfg` is unused - the answer comes from `settings.json` and the install
/// mode, not the daemon config - and is kept for signature parity with the
/// other `checks` functions in this module's siblings.
///
/// Never fails: every error path (client build, request, non-2xx) is folded
/// into an Info or Warn [`Check`], because a health probe that errors out
/// takes the whole report down with it.
pub async fn checks(_cfg: &Config) -> Vec<Check> {
    let canonical = crate::observability::is_canonical_install();
    // `resolve_otlp_target()` costs one settings.json read — the same file the
    // `is_otlp_configured()` call would load anyway. Only consulted on the
    // canonical path, so a dev run pays nothing extra.
    let central = canonical && crate::observability::resolve_otlp_target().is_some();
    // Use the cheap helpers — the health check never needs the auth credential.
    let dev_configured = !canonical && crate::observability::is_otlp_configured();

    if let Some(check) = offline_verdict(canonical, central, dev_configured) {
        return vec![check];
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
    use super::{derive_healthz, offline_verdict};
    use crate::health::Severity;

    /// A packaged install must never CLAIM its errors are shipping without
    /// having checked.
    ///
    /// The first version returned a fixed "errors ship to the central gateway"
    /// string. That lies to two real machines: a user who deliberately opted out
    /// of error reporting (opt-out, default on), and a locally-built binary
    /// staged into `~/.meridian/bin` — classified canonical by
    /// `is_canonical_install()`, but with `option_env!` endpoint/token unset, so
    /// it can never ship anything. Asserting an unverified GOOD state erodes
    /// trust exactly like asserting a bad one, and is harder to catch because
    /// nobody investigates a green line.
    #[test]
    fn a_packaged_install_reports_shipping_only_when_a_target_resolves() {
        let shipping = offline_verdict(true, true, false).expect("canonical short-circuits");
        assert_eq!(shipping.severity, Severity::Info);
        assert!(
            shipping.detail.contains("central gateway"),
            "a resolving target should say so: {shipping:?}"
        );

        let not_shipping = offline_verdict(true, false, false).expect("canonical short-circuits");
        assert_eq!(not_shipping.severity, Severity::Info);
        assert!(
            !not_shipping.detail.contains("errors ship"),
            "claims shipping with no resolved target — the exact unverified \
             assertion this branch exists to avoid: {not_shipping:?}"
        );
        // Both cases must still make clear capture is unaffected: the spool is
        // unconditional, and it is what `meridian logs` and Export Diagnostics read.
        for c in [&shipping, &not_shipping] {
            assert!(
                c.detail.contains("captured locally"),
                "must not imply telemetry is dark: {c:?}"
            );
        }
    }

    /// The dev/local path is a different question from the packaged one, and
    /// conflating them is what produced "telemetry not collected" on installs
    /// that were shipping fine.
    #[test]
    fn the_dev_path_is_reported_separately_and_never_claims_telemetry_is_dark() {
        let c = offline_verdict(false, false, false).expect("unconfigured dev short-circuits");
        assert_eq!(c.severity, Severity::Info);
        assert!(
            !c.detail.contains("not collected"),
            "capture still runs — only the local export leg is off: {c:?}"
        );
        assert!(
            c.detail.contains("meridian logs"),
            "should point at how to read what IS captured: {c:?}"
        );

        // Dev WITH credentials must fall through to the live healthz probe.
        assert!(
            offline_verdict(false, false, true).is_none(),
            "a configured dev install must still be probed, not short-circuited"
        );
    }

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
