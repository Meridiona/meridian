use clerk_fapi_rs::models::{ClientClient, ClientOrganization, ClientSession, ClientUser};
use log::debug;
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
    debug!("Emiting auth event: {payload:?}");

    if let Err(e) = app.emit(
        CLERK_AUTH_EVENT_NAME,
        ClerkAuthEvent {
            source: RUST_EVENT_SOURCE.to_string(),
            payload,
        },
    ) {
        tracing::error!("Failed to emit clerk auth change: {e}");
    }
}
