//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Resolves whether/where this process is allowed to ship spooled telemetry,
//! and in which of two modes. Split out of `observability/mod.rs` (which was
//! over the repo's 500-line cap) since "what's the ship target" is a
//! self-contained read of settings.json + env vars, independent of the
//! subscriber-building logic the parent module owns.
//!
//! # Two shipping modes
//! - **Dev/Bare checkout** (`target/debug|release`): ships FULL-FIDELITY
//!   telemetry to the engineer's OWN OpenObserve, HTTP Basic auth, gated on
//!   `otlp_enabled` + `oo_email`/`oo_password`. Unchanged from before. No
//!   redaction — a developer debugging against their own instance wants
//!   everything.
//! - **Canonical (packaged) install** (`~/.meridian/bin/meridian`): ships a
//!   REDACTED, ERROR-ONLY stream to Meridian's central ingest gateway, Bearer
//!   write-token auth, gated on the user's explicit `error_reporting_consent`.
//!   The endpoint + token are release-baked constants (never user settings);
//!   the shipper runs [`crate::telemetry_spool::redact`] before POSTing.
//!
//! # Who calls this
//! - `telemetry_spool::shipper::run_tick()` — `resolve_otlp_target()` each
//!   tick, to decide whether/what/where to deliver.
//! - The health probe — `is_otlp_configured()` (cheap, no credential
//!   assembly) and `resolve_otlp_endpoint()` (for the `/healthz` URL). These
//!   two intentionally cover ONLY the dev/local path (see their docs): a
//!   packaged install must not health-ping the remote central gateway.
//!
//! # Related
//! - `install_mode::is_canonical_install()` — the packaged-vs-dev split that
//!   selects the mode.
//! - `observability::mod::try_build_otel_providers` — the one other caller of
//!   `DEFAULT_OTLP_ENDPOINT`.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::install_mode::is_canonical_install;

/// Default OTLP/HTTP traces endpoint (dev path) when nothing in settings.json
/// or `MERIDIAN_OTLP_ENDPOINT` overrides it.
pub(super) const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:5080/api/default/v1/traces";

/// Central ingest-gateway endpoint, baked at release time from the
/// `MERIDIAN_CENTRAL_OTLP_ENDPOINT` **build** env (CI sets it — see
/// `scripts/package-updater.sh`/release workflow). Empty in a plain source
/// build, exactly like Dayflow's release-injected telemetry secrets, so a dev
/// build can never ship to production by accident. A runtime env var of the
/// same name overrides it, letting an engineer point a local build at a test
/// gateway. See [`central_endpoint`].
const BAKED_CENTRAL_ENDPOINT: Option<&str> = option_env!("MERIDIAN_CENTRAL_OTLP_ENDPOINT");

/// Write-only central ingest token, baked the same way as
/// [`BAKED_CENTRAL_ENDPOINT`]. Never a user setting — it is a distributable,
/// rotatable, write-scoped credential the gateway validates. See
/// [`central_token`].
const BAKED_CENTRAL_TOKEN: Option<&str> = option_env!("MERIDIAN_CENTRAL_TELEMETRY_TOKEN");

/// Authorization credential for the OTLP ship leg, carrying its scheme so the
/// shipper and the `telemetry import` CLI can't disagree on `Basic` vs
/// `Bearer`.
pub enum AuthCredential {
    /// `base64(email:password)` — the value that follows `Basic ` (dev path).
    Basic(String),
    /// A write-only ingest token — the value that follows `Bearer ` (central
    /// path).
    Bearer(String),
}

impl AuthCredential {
    /// The full `Authorization` header value, scheme prefix included.
    pub fn header_value(&self) -> String {
        match self {
            AuthCredential::Basic(b64) => format!("Basic {b64}"),
            AuthCredential::Bearer(token) => format!("Bearer {token}"),
        }
    }
}

/// Resolved OTLP export target: endpoint + auth + whether the shipper must
/// redact before sending. `None` from [`resolve_otlp_target`] means export is
/// disabled for this process (mode off, no consent, or missing config).
pub struct OtlpTarget {
    pub endpoint: String,
    pub auth: AuthCredential,
    /// True on the packaged-consented central path: the shipper MUST run
    /// [`crate::telemetry_spool::redact::redact_and_filter`] before POSTing.
    /// False on the dev path (full-fidelity telemetry to the engineer's own
    /// OO).
    pub redact: bool,
}

/// Central endpoint: runtime env override → release-baked constant → `None`.
fn central_endpoint() -> Option<String> {
    std::env::var("MERIDIAN_CENTRAL_OTLP_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            BAKED_CENTRAL_ENDPOINT
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
}

/// Central write token: runtime env override → release-baked constant → `None`.
fn central_token() -> Option<String> {
    std::env::var("MERIDIAN_CENTRAL_TELEMETRY_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            BAKED_CENTRAL_TOKEN
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
}

fn is_http(endpoint: &str) -> bool {
    endpoint.starts_with("http://") || endpoint.starts_with("https://")
}

/// Cheap liveness check used by the health probe — does NOT assemble
/// credentials. Returns `true` when the DEV/local OTLP export would be
/// attempted (toggle on + credentials present + not a packaged install).
///
/// Intentionally scoped to the dev/local path only: it drives the `/healthz`
/// probe against the engineer's own OpenObserve. A packaged install's central
/// error shipping is deliberately excluded so prod installs never health-ping
/// the remote gateway.
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

/// Resolve the configured DEV/local OTLP endpoint URL (without assembling
/// credentials). Used by the health check to derive the `/healthz` URL to ping.
/// Returns `None` on a packaged install (see [`is_otlp_configured`] for why the
/// central path is excluded here).
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

/// Resolve the full OTLP export target: endpoint + auth + redaction mode.
/// Called every shipper tick. Branches on install type:
/// - packaged install → [`resolve_central_target`] (consent-gated, redacted);
/// - dev checkout → [`resolve_dev_target`] (full-fidelity, own OO).
pub fn resolve_otlp_target() -> Option<OtlpTarget> {
    let settings = crate::config::load_runtime_settings();
    if is_canonical_install() {
        resolve_central_target(&settings)
    } else {
        resolve_dev_target(&settings)
    }
}

/// Packaged-install path: ship a redacted, error-only stream to the central
/// gateway, but ONLY with explicit user consent and only when the release-baked
/// endpoint + token are present.
fn resolve_central_target(
    settings: &meridian_core::settings::RuntimeSettings,
) -> Option<OtlpTarget> {
    // The sole gate for central shipping. Absent consent → nothing leaves the
    // machine (capture-to-spool + `meridian logs` continue unaffected).
    if !settings.error_reporting_consent {
        return None;
    }

    let endpoint = central_endpoint()?;
    if !is_http(&endpoint) {
        tracing::warn!(
            endpoint = %endpoint,
            "central OTLP endpoint has no http/https scheme — error shipping disabled"
        );
        return None;
    }

    let token = central_token()?;
    // Guard against HTTP header injection via a malformed baked/overridden token.
    if token.contains(['\n', '\r']) {
        tracing::warn!("central telemetry token contains CR/LF — error shipping disabled");
        return None;
    }

    Some(OtlpTarget {
        endpoint,
        auth: AuthCredential::Bearer(token),
        redact: true,
    })
}

/// Dev/Bare checkout path: ship full-fidelity telemetry to the engineer's own
/// OpenObserve. Unchanged behaviour from before the central path existed —
/// `otlp_enabled` + `oo_email`/`oo_password`, Basic auth, no redaction.
fn resolve_dev_target(settings: &meridian_core::settings::RuntimeSettings) -> Option<OtlpTarget> {
    if !settings.otlp_enabled {
        return None;
    }

    // Auth: settings email+password only. The MERIDIAN_OO_AUTH env fallback is
    // DEPRECATED and ignored — a dual credential store (env + settings) meant
    // the UI could show creds while the daemon used different ones (or none).
    let auth = match (&settings.oo_email, &settings.oo_password) {
        (Some(email), Some(pass)) if !email.is_empty() && !pass.is_empty() => {
            // Guard against HTTP header injection and malformed user:password splits.
            if email.contains(['\n', '\r']) || pass.contains(['\n', '\r']) || email.contains(':') {
                tracing::warn!(
                    "OTLP credentials contain invalid characters — OTLP export disabled"
                );
                return None;
            }
            AuthCredential::Basic(STANDARD.encode(format!("{email}:{pass}")))
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
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("MERIDIAN_OTLP_ENDPOINT")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string());

    // Validate scheme — only http/https are valid OTLP transports.
    if !is_http(&endpoint) {
        tracing::warn!(
            endpoint = %endpoint,
            "OTLP endpoint has no http/https scheme — OTLP export disabled"
        );
        return None;
    }

    Some(OtlpTarget {
        endpoint,
        auth,
        redact: false,
    })
}
