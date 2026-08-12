use crate::ClerkExt;
use clerk_fapi_rs::models::{ClientClient, ClientEnvironment};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

/// Need to keep in sync with ClerkInitResponse in
/// guest-js/sync.ts
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClerkInitResponse {
    environment: ClientEnvironment,
    client: ClientClient,
    publishable_key: String,
    // TODO: DomainOrProxyUrl
    /* ts side
    export type DomainOrProxyUrl =
      | {
          /**
           * **Required for applications that run behind a reverse proxy**. The URL that Clerk will proxy requests to. Can be either a relative path (`/__clerk`) or a full URL (`https://<your-domain>/__clerk`).
           */
          proxyUrl?: never;
          /**
           * **Required if your application is a satellite application**. Sets the domain of the satellite application.
           */
          domain?: string | ((url: URL) => string);
        }
      | {
          proxyUrl?: string | ((url: URL) => string);
          domain?: never;
        };
    */
}

/// Authorization header to be injected in clerk-js __unstable__onBeforeRequest
#[tauri::command]
pub(crate) async fn get_client_authorization_header<R: Runtime>(
    app: AppHandle<R>,
) -> Option<String> {
    app.clerk().get_client_authorization_header()
}

/// Authorization header read in __unstable__onAfterResponse
#[tauri::command]
pub(crate) async fn set_client_authorization_header<R: Runtime>(
    app: AppHandle<R>,
    header: Option<String>,
) -> () {
    app.clerk().set_client_authorization_header(header)
}

#[tauri::command]
pub(crate) async fn initialize<R: Runtime>(app: AppHandle<R>) -> Result<ClerkInitResponse, String> {
    app.ensure_clerk_initialized().await?;
    let client = app.clerk().client().map_err(|e| e.to_string())?;
    let environment = app.clerk().environment().map_err(|e| e.to_string())?;
    let publishable_key = app.clerk_store().publishable_key;

    Ok(ClerkInitResponse {
        environment,
        client,
        publishable_key,
    })
}
