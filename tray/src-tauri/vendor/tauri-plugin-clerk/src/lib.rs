//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Vendored, patched fork of `tauri-plugin-clerk` 0.1.1
//! (<https://github.com/Nipsuli/tauri-plugin-clerk>), pinned exact upstream
//! for supply-chain control per `tray/src-tauri/Cargo.toml`'s original
//! comment. Vendored (not forked on GitHub) so the fix lives in this repo,
//! under the same review/CI/test gates as everything else, rather than in an
//! external fork Meridian would have to maintain a build/publish pipeline
//! for.
//!
//! # The bug this fixes
//! Upstream issue: <https://github.com/Nipsuli/tauri-plugin-clerk/issues/7>
//! ("`CLERK_AUTH_EVENT -> set_client` silently fails"), with an open,
//! unmerged fix PR (#9) as of this vendoring. guest-js's client serializers
//! (the separate `tauri-plugin-clerk` npm package, not vendored here — see
//! [`backfill_clerk_client_json`] for why the fix lives Rust-side instead)
//! omit several fields `clerk-fapi-rs`'s OpenAPI-generated models require to
//! be explicitly present — a plain `Option<T>` field is optional under serde,
//! but several of these use `deserialize_with = "Option::deserialize"`,
//! which opts back into "key must be present" (accepts `null`, rejects
//! absence) — and emit timestamps as fractional-second floats where the
//! model wants an `i64`. Before this fix, deserializing the auth-change event
//! for ANY real signed-in user failed on the first such field, silently (a
//! bare `if let Ok(...)` with no `else`), so [`ClerkExt::clerk`]'s
//! `set_client` was never called and the persisted offline session cache
//! (`clerk-store.json`, via `with_tauri_store()`) never reflected a signed-in
//! user. The user-visible effect: `ensure_clerk_initialized`'s `clerk.load()`
//! falls back to that cache only when the live Clerk API fetch fails (e.g. no
//! network yet at a fresh process's cold start), and the fallback was always
//! empty — so the app would show the sign-in screen despite the user
//! genuinely having signed in before. Fixed by [`backfill_clerk_client_json`]
//! (structural, JSON-shape-driven — not a hardcoded field list) plus
//! replacing the swallowed `if let Ok` with a `match` that surfaces both the
//! outer-JSON and the post-backfill deserialize failure via `tracing::warn!`
//! (see `meridian`'s `src/observability/filter.rs::CAPTURE_TARGETS`, which
//! must include `"tauri_plugin_clerk"` for these to actually ship anywhere).
//!
//! # Un-vendoring
//! Once upstream releases a version with #9 (or an equivalent fix) merged,
//! drop this directory, restore the crates.io dependency + version pin in
//! `tray/src-tauri/Cargo.toml`, and delete this module doc.

use clerk_fapi_rs::{
    configuration::{ClientKind, Store as ClerkStateStore},
    models::{ClientClient, ClientOrganization, ClientSession, ClientUser},
    Clerk, ClerkFapiConfiguration,
};
use events::{
    emit_clerk_auth_event, ClerkAuthEvent, ClerkAuthEventPayload, CLERK_AUTH_EVENT_NAME,
    RUST_EVENT_SOURCE,
};
use log::debug;
use parking_lot::RwLock;
use std::sync::Arc;
use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Listener, Manager, Runtime,
};
use tauri_store::ClerkTauriStore;

mod commands;
mod events;
mod json_backfill;
mod tauri_store;

use json_backfill::backfill_clerk_client_json;

//
#[derive(Clone)]
pub struct ClerkStoreInternal {
    pub clerk: Clerk,
    pub publishable_key: String,
    // TODO: proxy or domain
}
pub type ClerkStore = RwLock<ClerkStoreInternal>;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the clerk APIs.
#[allow(async_fn_in_trait)]
pub trait ClerkExt<R: Runtime> {
    /// Get the whole clerk store
    fn clerk_store(&self) -> ClerkStoreInternal;
    /// Get the Clerk instance
    fn clerk(&self) -> Clerk;
    /// Init method, idempotent
    async fn ensure_clerk_initialized(&self) -> Result<(), String>;
}

fn clerk_auth_cb<R: Runtime>(
    app: AppHandle<R>,
    client: ClientClient,
    session: Option<ClientSession>,
    user: Option<ClientUser>,
    organization: Option<ClientOrganization>,
) {
    emit_clerk_auth_event(
        app,
        ClerkAuthEventPayload {
            client,
            session,
            user,
            organization,
        },
    )
}

impl<R: Runtime, T: Manager<R>> crate::ClerkExt<R> for T {
    fn clerk_store(&self) -> ClerkStoreInternal {
        let app = self.app_handle();
        let clerk_store = {
            let app_clerk = app.state::<ClerkStore>();
            let app_clerk = app_clerk.read();
            app_clerk.clone()
        };
        clerk_store
    }

    fn clerk(&self) -> Clerk {
        self.clerk_store().clerk
    }

    async fn ensure_clerk_initialized(&self) -> Result<(), String> {
        let app_handle = self.app_handle();

        // To avoid concurrent load calls, example multi window apps
        // or running init call in React.useEffect hook
        let app_clerk_init_lock = app_handle.state::<ClerkInitLock>();
        let _app_clerk_init_lock = app_clerk_init_lock.lock().await;
        //

        let clerk = self.clerk();
        if !clerk.loaded() {
            // Clerk load will default loading from API and does fall back on cached
            // resources in case of not being able to load resources from API. This
            // allows the app to work in offline mode for signedin users.
            clerk.load().await.map_err(|e| e.to_string())?;
            let app_handle_inner = app_handle.clone();
            clerk.add_listener(move |client, session, user, organization| {
                let app_handle_clone = app_handle_inner.clone();
                clerk_auth_cb(app_handle_clone, client, session, user, organization);
            });

            let app_handle_inner = app_handle.clone();
            app_handle.listen(CLERK_AUTH_EVENT_NAME, move |event| {
                let app_handle = app_handle_inner.clone();
                let raw_payload = event.payload();
                // Parse to a bare Value first (rather than straight to
                // ClerkAuthEvent) so backfill_clerk_client_json can patch the
                // fields guest-js omits before the strict, OpenAPI-generated
                // clerk-fapi-rs models ever see it. See the module doc for
                // why this exists (upstream issue #7).
                let mut value = match serde_json::from_str::<serde_json::Value>(raw_payload) {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "clerk: auth event payload was not valid JSON"
                        );
                        return;
                    }
                };
                backfill_clerk_client_json(&mut value);
                let payload = match serde_json::from_value::<ClerkAuthEvent>(value) {
                    Ok(payload) => payload,
                    Err(e) => {
                        // If this fires, a Clerk client shape appeared that
                        // backfill_clerk_client_json doesn't yet normalize —
                        // extend it (see json_backfill's tests for the
                        // pattern) rather than re-swallowing the error.
                        tracing::warn!(
                            error = %e,
                            "clerk: auth event JSON still failed to deserialize after field \
                             backfill — the offline session cache will not reflect this \
                             sign-in, so a cold start without network will show signed-out"
                        );
                        return;
                    }
                };
                if payload.source != RUST_EVENT_SOURCE {
                    debug!("Received ClerkAuthEvent: {payload:?}");
                    if let Err(e) = app_handle
                        .clerk()
                        .set_client(payload.payload.client.clone())
                    {
                        tracing::warn!(
                            error = %e,
                            "clerk: failed to persist signed-in client after auth event"
                        );
                    }
                }
            });
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct ClerkInitLockInner {}
pub type ClerkInitLock = tokio::sync::Mutex<ClerkInitLockInner>;

/// Builder for the Clerk plugin
#[derive(Default)]
pub struct ClerkPluginBuilder {
    /// Clerk publishable key
    pub publishable_key: Option<String>,
    // TODO: the proxy and domain should be
    // mutually exclusive
    /// Proxy URL
    pub proxy: Option<String>,
    /// Domain
    pub domain: Option<String>,
    /// Store
    pub store: Option<Arc<dyn ClerkStateStore>>,
    /// Flag to use tauri store, requires tauri-store-plugin
    pub with_tauri_store: bool,
}

impl ClerkPluginBuilder {
    /// Create a new builder instance
    pub fn new() -> Self {
        ClerkPluginBuilder::default()
    }

    /// Set the Clerk publishable key
    pub fn publishable_key(mut self, key: impl Into<String>) -> Self {
        self.publishable_key = Some(key.into());
        self
    }

    /// Set the proxy URL
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Set the domain
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set the store
    pub fn store(mut self, store: impl ClerkStateStore + 'static) -> Self {
        self.store = Some(Arc::new(store));
        self
    }

    /// Set the Tauri store
    pub fn with_tauri_store(mut self) -> Self {
        self.with_tauri_store = true;
        self
    }

    /// Build the Tauri plugin
    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        Builder::<R>::new("clerk")
            .invoke_handler(tauri::generate_handler![
                commands::initialize,
                commands::get_client_authorization_header,
                commands::set_client_authorization_header
            ])
            .setup(move |app, _api| {
                let publishable_key = self
                    .publishable_key
                    .ok_or("Clerk: Missing publishable_key".to_string())?;

                let mut store = self.store;
                if store.is_none() && self.with_tauri_store {
                    use tauri_plugin_store::StoreExt;
                    let clerk_store = app.store("clerk-store")?;
                    store = Some(Arc::new(ClerkTauriStore::new(clerk_store)));
                }

                let config = ClerkFapiConfiguration::new_with_store(
                    publishable_key.clone(),
                    self.proxy,
                    self.domain,
                    store,
                    None,
                    ClientKind::NonBrowser,
                )?;

                let clerk = Clerk::new(config);
                app.manage(RwLock::new(ClerkStoreInternal {
                    clerk,
                    publishable_key: publishable_key.clone(),
                }));
                app.manage(tokio::sync::Mutex::new(ClerkInitLockInner::default()));
                Ok(())
            })
            .build()
    }
}
