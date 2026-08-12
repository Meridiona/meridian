use clerk_fapi_rs::configuration::Store as ClerkStateStore;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tauri::Runtime;
use tauri_plugin_store::Store as TauriStore;

#[derive(Clone)]
pub struct ClerkTauriStore<R: Runtime> {
    inner: Arc<TauriStore<R>>,
}
impl<R: Runtime> ClerkTauriStore<R> {
    pub fn new(store: Arc<TauriStore<R>>) -> Self {
        ClerkTauriStore { inner: store }
    }
}
impl<R: Runtime> std::fmt::Debug for ClerkTauriStore<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClerkTauriStore").finish()
    }
}
impl<R: Runtime> ClerkStateStore for ClerkTauriStore<R> {
    fn set(&self, key: &str, value: JsonValue) {
        self.inner.set(key, value);
    }
    fn get(&self, key: &str) -> Option<JsonValue> {
        self.inner.get(key)
    }
    fn has(&self, key: &str) -> bool {
        self.inner.has(key)
    }
    fn delete(&self, key: &str) -> bool {
        self.inner.delete(key)
    }
}
