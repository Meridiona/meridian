//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
use clerk_fapi_rs::models::{ClientClient, ClientOrganization, ClientSession, ClientUser};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

pub const CLERK_AUTH_EVENT_NAME: &str = "plugin-clerk-auth-cb";
pub const RUST_EVENT_SOURCE: &str = "rust";

/// Need to be in sync with ClerkAuthEventPayload in
/// guest-js/sync.ts
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClerkAuthEventPayload {
    pub client: ClientClient,
    pub session: Option<ClientSession>,
    pub user: Option<ClientUser>,
    pub organization: Option<ClientOrganization>,
}
/// Need to be in sync with ClerkAuthEvent in
/// guest-js/sync.ts
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClerkAuthEvent {
    // Window name or "rust" to identify sender
    pub source: String,
    pub payload: ClerkAuthEventPayload,
}

pub fn emit_clerk_auth_event<R: Runtime>(app: AppHandle<R>, payload: ClerkAuthEventPayload) {
    // MERIDIAN PATCH: upstream logged the whole payload here with `{payload:?}`.
    // `ClientSession` carries `last_active_token.jwt` — a live session
    // credential — so that line writes a bearer token into whatever sink the
    // `log` facade is wired to. It is inert today (nothing installs a `log`
    // logger, and no `LogTracer` bridge exists), but this crate is now on
    // `CAPTURE_TARGETS` in `src/observability/filter.rs`, so the day anyone adds
    // `tauri-plugin-log` or a bridge, those tokens land in the full-fidelity
    // local telemetry spool. Log presence, never contents.
    tracing::debug!(
        has_session = payload.session.is_some(),
        has_user = payload.user.is_some(),
        has_organization = payload.organization.is_some(),
        "clerk: emitting auth event"
    );

    if let Err(e) = app.emit(
        CLERK_AUTH_EVENT_NAME,
        ClerkAuthEvent {
            source: RUST_EVENT_SOURCE.to_string(),
            payload,
        },
    ) {
        tracing::error!(error = %e, "clerk: failed to emit auth change event");
    }
}
