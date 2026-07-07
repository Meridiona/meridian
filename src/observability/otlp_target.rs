//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Resolves whether/where this process is allowed to ship spooled telemetry
//! to OpenObserve. Split out of `observability/mod.rs` (which was over the
//! repo's 500-line cap) since "what's the ship target" is a self-contained
//! read of settings.json + env vars, independent of the subscriber-building
//! logic the parent module owns.
//!
//! # Who calls this
//! - `telemetry_spool::shipper::run_tick()` — `resolve_otlp_target()` each
//!   tick, to decide whether to attempt delivery.
//! - The health probe — `is_otlp_configured()` (cheap, no credential
//!   assembly) and `resolve_otlp_endpoint()` (for the `/healthz` URL).
//!
//! # Related
//! - `install_mode::is_canonical_install()` — the packaged-install gate every
//!   function here checks first; a Canonical install never resolves a target.
//! - `observability::mod::try_build_otel_providers` — the one other caller of
//!   `DEFAULT_OTLP_ENDPOINT`.

use super::install_mode::is_canonical_install;

/// Default OTLP/HTTP traces endpoint when nothing in settings.json or
/// `MERIDIAN_OTLP_ENDPOINT` overrides it.
pub(super) const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:5080/api/default/v1/traces";

/// Resolved OTLP export target: trace endpoint + Basic-auth credential.
/// `None` means export is disabled (toggle off, or no credentials anywhere).
pub struct OtlpTarget {
    pub endpoint: String,
    pub auth: String,
}

/// Cheap liveness check used by the health probe — does NOT assemble
/// credentials. Returns `true` when OTLP export would be attempted if
/// `resolve_otlp_target()` were called (toggle on + credentials present +
/// not a Canonical/packaged install).
pub fn is_otlp_configured() -> bool {
    if is_canonical_install() {
        return false;
    }
    let settings = crate::config::load_runtime_settings();
    if !settings.otlp_enabled {
        return false;
    }
    // settings.json is the single source for OO credentials — the old
    // MERIDIAN_OO_AUTH env fallback is deprecated and ignored.
    settings.oo_email.as_deref().is_some_and(|e| !e.is_empty())
        && settings
            .oo_password
            .as_deref()
            .is_some_and(|p| !p.is_empty())
}

/// Resolve the configured OTLP endpoint URL (without assembling credentials).
/// Used by the health check to derive the `/healthz` URL to ping.
pub fn resolve_otlp_endpoint() -> Option<String> {
    if is_canonical_install() {
        return None;
    }
    let settings = crate::config::load_runtime_settings();
    if !settings.otlp_enabled {
        return None;
    }
    Some(
        settings
            .otlp_endpoint
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("MERIDIAN_OTLP_ENDPOINT")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string()),
    )
}

/// Resolve the full OTLP export target: endpoint + Basic-auth header value.
/// Called only at daemon startup (inside `try_build_otel_providers`). Use
/// `is_otlp_configured()` + `resolve_otlp_endpoint()` for lighter call sites.
pub fn resolve_otlp_target() -> Option<OtlpTarget> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    // Packaged installs never ship live to OpenObserve — capture stays fully
    // local; the only path out is a user-initiated export bundle. This check
    // comes first and unconditionally, regardless of settings.json.
    if is_canonical_install() {
        return None;
    }

    let settings = crate::config::load_runtime_settings();

    if !settings.otlp_enabled {
        return None;
    }

    // Auth: settings email+password only. The MERIDIAN_OO_AUTH env fallback is
    // DEPRECATED and ignored — a dual credential store (env + settings) meant
    // the UI could show creds while the daemon used different ones (or none).
    // Credentials are set in the dashboard Settings and read from settings.json.
    let auth = match (&settings.oo_email, &settings.oo_password) {
        (Some(email), Some(pass)) if !email.is_empty() && !pass.is_empty() => {
            // Guard against HTTP header injection and malformed user:password splits.
            if email.contains(['\n', '\r']) || pass.contains(['\n', '\r']) || email.contains(':') {
                tracing::warn!(
                    "OTLP credentials contain invalid characters — OTLP export disabled"
                );
                return None;
            }
            STANDARD.encode(format!("{email}:{pass}"))
        }
        _ => {
            if std::env::var("MERIDIAN_OO_AUTH").is_ok_and(|v| !v.is_empty()) {
                tracing::warn!(
                    "MERIDIAN_OO_AUTH is set but deprecated and ignored — \
                     set OpenObserve credentials in the dashboard Settings instead"
                );
            }
            return None;
        }
    };

    let endpoint = settings
        .otlp_endpoint
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("MERIDIAN_OTLP_ENDPOINT")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string());

    // Validate scheme — only http/https are valid OTLP transports.
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        tracing::warn!(
            endpoint = %endpoint,
            "OTLP endpoint has no http/https scheme — OTLP export disabled"
        );
        return None;
    }

    Some(OtlpTarget { endpoint, auth })
}
