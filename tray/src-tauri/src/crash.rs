//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Native-crash capture via Sentry (Phase 2B). Complements the OTLP pipeline:
//! Phase 2A ships the tray's *logged* errors (`tracing::error!`/`warn!`) to the
//! central backend, but the OTLP spool structurally cannot capture a HARD crash
//! — a segfault, a panic before the logger inits, an OOM kill. Sentry's
//! out-of-process minidump reporter (`sentry-rust-minidump`, re-exported by
//! `tauri-plugin-sentry`) is the one thing that does, and it adds fingerprint
//! grouping + release health (crash-free sessions) on top. Sentry here is scoped
//! to crashes only — logged errors keep going out via OTLP, so nothing is
//! double-reported (the `sentry` dep deliberately omits the `tracing` feature).
//!
//! # Privacy
//! Gated identically to central OTLP shipping: it only initialises when the user
//! has error reporting enabled (opt-out, on by default —
//! [`meridian_core::settings::RuntimeSettings::error_reporting_enabled`]) and the
//! hard `MERIDIAN_TELEMETRY_DISABLED` kill switch is off, AND a DSN is baked into
//! the release build. A source/dev build (no baked DSN) never talks to Sentry.
//! `before_send` runs the SAME on-device scrub as the OTLP redaction path
//! ([`meridian::telemetry_spool::redact::scrub_text`]) over crash messages +
//! exception values, so `/Users/<name>`, URLs, emails, and long tokens are
//! stripped before anything leaves the machine. Gating happens at INIT (the
//! client is never created when off), not by muting sends after the fact.
//!
//! # Who calls this
//! [`crate::run`] — `init_client()` at the very top (the minidump reporter
//! relaunches this exe in reporter mode and must short-circuit before any app
//! work), the returned guard held for the process lifetime, then
//! `tauri_plugin_sentry::minidump::init(&client)` + the plugin registration.

use std::sync::Arc;

use sentry::protocol::Event;
use sentry::ClientInitGuard;

/// Sentry DSN baked at release time from `MERIDIAN_SENTRY_DSN` (see
/// `release-build.yml`). Empty in a source build → Sentry stays off, exactly
/// like the central OTLP endpoint/token constants in
/// `meridian::observability`.
const BAKED_SENTRY_DSN: Option<&str> = option_env!("MERIDIAN_SENTRY_DSN");

/// Resolve the DSN: runtime env override → release-baked → `None`.
fn dsn() -> Option<String> {
    std::env::var("MERIDIAN_SENTRY_DSN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            BAKED_SENTRY_DSN
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
}

/// Whether crash reporting is allowed: the user hasn't disabled error reporting
/// (opt-out, default on) and the hard kill switch is off. Read once at startup —
/// a mid-session toggle applies on the next launch, since the process-wide
/// Sentry client can't be cleanly uninstalled once created.
fn reporting_allowed() -> bool {
    let killed = std::env::var("MERIDIAN_TELEMETRY_DISABLED")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    if killed {
        return false;
    }
    meridian_core::settings::load_runtime_settings().error_reporting_enabled
}

/// Initialise the Sentry client if crash reporting is allowed and a DSN is baked
/// in. Returns the client guard to hold for the process lifetime, or `None` when
/// Sentry is off (no consent, kill switch set, or no DSN) — the caller then
/// skips the minidump reporter and the plugin registration entirely.
pub fn init_client() -> Option<ClientInitGuard> {
    if !reporting_allowed() {
        return None;
    }
    let dsn = dsn()?;
    let client = sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            // Never attach IP/user by default; before_send scrubs the rest.
            send_default_pii: false,
            before_send: Some(Arc::new(scrub_event)),
            // (Release-health session tracking can be layered on later via
            // sentry::start_session(); v1 is scoped to crash capture + grouping.)
            ..Default::default()
        },
    ));
    tracing::info!("sentry crash reporting initialised");
    Some(client)
}

/// On-device scrub applied to every event before it leaves the machine. Reuses
/// the OTLP redaction scrub so crash telemetry gets the identical treatment:
/// home-dir usernames, URLs, emails, and long tokens are stripped from the event
/// message and each exception value. (Stack-frame paths are build-time CI paths,
/// not user paths, so they carry no user PII.)
///
/// `server_name` needs separate handling because nothing above touches it:
/// the `sentry` crate's `contexts` feature (enabled in Cargo.toml) pulls in
/// `sentry-contexts`, whose integration auto-populates `server_name` from
/// `hostname::get()` when the app hasn't set one — and `send_default_pii:
/// false` does NOT suppress it, being a separate integration. On macOS that
/// hostname is routinely derived from the account holder's real name
/// (`Akarshs-MacBook-Pro.local`), so it would ship a likely-identifying string
/// with every crash. It gets the same pseudonym as the OTLP path's `host.name`
/// (`redact::pseudonymize_host`), so a crash here and an error log there
/// resolve to the SAME machine identifier and can still be correlated.
fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    use meridian::telemetry_spool::redact::{
        alpha_account_email_if_active, local_host_pseudonym, scrub_text,
    };
    if let Some(m) = event.message.take() {
        event.message = Some(scrub_text(&m));
    }
    for exc in event.exception.values.iter_mut() {
        if let Some(v) = exc.value.take() {
            exc.value = Some(scrub_text(&v));
        }
    }
    // Set unconditionally rather than mapping over the existing value: the OTLP
    // side now derives this from a stable hardware id rather than hashing
    // whatever hostname was present, so mapping here would produce a DIFFERENT
    // pseudonym and silently decorrelate a crash from that machine's error
    // logs. Also covers the case where `sentry-contexts` left it unset.
    event.server_name = Some(local_host_pseudonym().into());

    // ALPHA TESTING ONLY — same raw-email exception as the OTLP resource
    // attribute in `observability::init` (see `alpha_account_email_if_active`'s
    // doc), applied to Sentry's own identity field so a crash report names the
    // signed-in tester too, not just the machine pseudonym above. `send_default_pii:
    // false` never populated `event.user` in the first place, so there is
    // nothing to merge with — this is the only source of it.
    let settings = meridian_core::settings::load_runtime_settings();
    if let Some(email) = alpha_account_email_if_active(settings.account_email.as_deref()) {
        event.user = Some(sentry::User {
            email: Some(email),
            ..Default::default()
        });
    }

    // Free-text carriers that `send_default_pii: false` does NOT suppress.
    // Nothing populates these today, so CLEARING costs nothing — and clearing
    // beats scrubbing here because the failure mode is additive: the day
    // someone adds a breadcrumb or an `extra` with a window title or a file
    // path, it would otherwise egress having never passed a scrubber. This way
    // the default for anything new is "does not ship", matching the OTLP
    // allowlist's fail-closed stance.
    event.breadcrumbs = Default::default();
    event.extra.clear();
    for exc in event.exception.values.iter_mut() {
        if let Some(st) = exc.stacktrace.as_mut() {
            for frame in st.frames.iter_mut() {
                frame.vars.clear();
            }
        }
    }
    Some(event)
}
