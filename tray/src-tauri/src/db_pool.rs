//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray's swappable `meridian.db` pool handle.
//!
//! Before this module, `meridian.db`'s pool was opened once at tray startup
//! (`lib.rs`'s `open_existing_lazy` call) and handed to Tauri as bare
//! `Option<meridian_core::SqlitePool>>` managed state - held for the tray
//! process's ENTIRE lifetime, including across a daemon restart.
//! `commands::daemon::reload_daemon` SIGHUPs the daemon (macOS: exits and
//! relies on launchd, or in a dev session a human, to relaunch it) without the
//! tray's own connection ever knowing a restart happened - so the tray's pool
//! spans two different daemon process generations on the same file. That is
//! the confirmed trigger of a real `meridian.db` corruption incident
//! (2026-08-24): see PR #856, which fixed the daemon's shutdown to checkpoint
//! the WAL and made the tray's own reads detect corruption immediately - both
//! good independent hardening, but neither closes the actual gap.
//!
//! [`DbPool`] closes it: `reload_daemon` calls [`DbPool::close`] before
//! signaling and [`DbPool::reopen`] once the new daemon process is confirmed
//! up, so the tray never holds a connection spanning the boundary. Every
//! other call site is unaffected - [`DbPool::get`] returns the exact same
//! `Option<SqlitePool>` shape `State<Option<SqlitePool>>>::inner()` used to,
//! just renamed, since "the pool might legitimately be absent right now" was
//! already part of every caller's contract (a `None` during a first launch,
//! before the daemon has created the file).
//!
//! # Who calls this
//! - `lib.rs`'s setup hook constructs it and calls `app.manage`.
//! - `commands::daemon::reload_daemon` calls `close`/`reopen` around the
//!   signal - the one thing that could not be done through the old bare
//!   `Option<SqlitePool>>` state.
//! - Every dashboard/poll read that used to do
//!   `let Some(pool) = pool.inner() else { ... }` now does the same against
//!   `pool.get()`.

use meridian_core::SqlitePool;
use std::sync::{Arc, RwLock};

/// Swappable handle to the tray's `meridian.db` pool, managed as Tauri state
/// in place of a bare `Option<SqlitePool>>`.
///
/// `uri`/`key_hex` are remembered at construction so [`reopen`](Self::reopen)
/// needs no arguments at its call site - `reload_daemon` has neither the DB
/// path nor the encryption key on hand, only this handle.
#[derive(Clone)]
pub struct DbPool {
    inner: Arc<RwLock<Option<SqlitePool>>>,
    uri: String,
    key_hex: Option<String>,
}

/// Manual, not derived: several call sites take `DbPool` as a `#[tauri::command]`
/// parameter without `#[tracing::instrument(skip(...))]`, so a derived `Debug`
/// would print `key_hex` — the raw SQLCipher key — into a span every time one
/// of those commands runs. `key_hex.is_some()` is all any log ever needs.
impl std::fmt::Debug for DbPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbPool")
            .field("uri", &self.uri)
            .field("key_set", &self.key_hex.is_some())
            .finish()
    }
}

impl DbPool {
    pub fn new(pool: Option<SqlitePool>, uri: String, key_hex: Option<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(pool)),
            uri,
            key_hex,
        }
    }

    /// The pool, if one is currently open - `None` before the daemon has
    /// created `meridian.db` yet, or during the brief window between
    /// [`close`](Self::close) and [`reopen`](Self::reopen) around a daemon
    /// restart. Cheap: `sqlx::SqlitePool` is itself `Arc`-backed, so this is
    /// a shallow clone, not a new connection.
    pub fn get(&self) -> Option<SqlitePool> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Close the pool and clear the handle. Called before signaling the
    /// daemon to restart, so nothing on this side keeps a connection alive
    /// spanning the old process's shutdown and the new one's startup - see
    /// this module's header for the corruption this closes off. Every reader
    /// sees `get() == None` for the duration and behaves exactly as it
    /// already does on a cold start (empty defaults, no panic).
    pub async fn close(&self) {
        let taken = self.inner.write().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(pool) = taken {
            pool.close().await;
        }
    }

    /// Reopen against the same uri/key this handle was built with. Lazy,
    /// matching the original startup open - see that call site's doc
    /// (`lib.rs`) for why eager fails when `meridian.db` briefly does not
    /// exist. Best-effort: a failure here is logged and leaves `get()`
    /// returning `None`, same as any other reason the pool isn't open yet;
    /// the daemon's own re-creation of the file on its next write heals it
    /// exactly as a lazy pool always has.
    pub async fn reopen(&self) {
        match meridian_core::open_existing_lazy(&self.uri, self.key_hex.as_deref()).await {
            Ok(pool) => {
                *self.inner.write().unwrap_or_else(|e| e.into_inner()) = Some(pool);
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "DbPool::reopen failed - meridian.db stays unavailable until the next reload or tray restart"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn migrated_db(dir: &std::path::Path) -> (String, meridian_core::SqlitePool) {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr;

        let path = dir.join("db_pool_test.db");
        let uri = format!("sqlite://{}", path.display());
        // `open_existing`/`open_existing_lazy` both set `create_if_missing(false)`
        // (they assume the daemon already created the file) - a test fixture
        // needs its own connect path to create one from scratch.
        let opts = SqliteConnectOptions::from_str(&uri)
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .connect_with(opts)
            .await
            .expect("create db");
        sqlx::migrate!("../../src/migrations")
            .run(&pool)
            .await
            .expect("migrate");
        pool.close().await;
        // Reopen through the same lazy path `DbPool` itself uses, so the
        // handle under test behaves exactly like production.
        let pool = meridian_core::open_existing_lazy(&uri, None)
            .await
            .expect("reopen lazily");
        (uri, pool)
    }

    /// `get()` must return exactly what was passed to `new()`.
    #[tokio::test]
    async fn get_returns_the_pool_it_was_built_with() {
        let dir = tempfile::tempdir().unwrap();
        let (uri, pool) = migrated_db(dir.path()).await;
        let handle = DbPool::new(Some(pool), uri, None);
        assert!(handle.get().is_some());
    }

    /// A handle built with no pool (e.g. the DB isn't open yet) must behave
    /// exactly like the old `None` state every caller already handles.
    #[tokio::test]
    async fn get_is_none_when_built_empty() {
        let handle = DbPool::new(None, "sqlite://does-not-matter".to_string(), None);
        assert!(handle.get().is_none());
    }

    /// The exact sequence `reload_daemon` runs: close, then a read in
    /// between must see `None` (nothing races the daemon's restart), then
    /// reopen brings a working pool back without needing to be told the URI
    /// again.
    #[tokio::test]
    async fn close_then_reopen_restores_a_working_pool() {
        let dir = tempfile::tempdir().unwrap();
        let (uri, pool) = migrated_db(dir.path()).await;
        let handle = DbPool::new(Some(pool), uri, None);

        handle.close().await;
        assert!(
            handle.get().is_none(),
            "a reader between close() and reopen() must see no pool, not a stale one"
        );

        handle.reopen().await;
        let reopened = handle.get().expect("reopen must restore a pool");
        // Prove it's a genuinely live connection, not just a non-None marker.
        meridian_core::ping(&reopened)
            .await
            .expect("reopened pool must actually work");
    }

    /// `reopen` must not panic on failure, and must leave `get()` at `None`
    /// rather than propagating the error - `reopen` is deliberately
    /// best-effort (see its doc), the same shape as any other reason a lazy
    /// pool isn't open yet. A missing/wrong-shaped file is NOT enough to
    /// prove this (`open_existing_lazy` defers that check to first use, so
    /// it would return `Ok` here regardless) - an invalid key is what
    /// actually fails synchronously, at `validate_key_hex` inside
    /// `open_existing_lazy` itself, before any connection is attempted.
    #[tokio::test]
    async fn reopen_failure_leaves_the_handle_empty_not_panicked() {
        let handle = DbPool::new(
            None,
            "sqlite://does-not-matter".to_string(),
            Some("not-valid-hex".to_string()),
        );
        handle.reopen().await;
        assert!(handle.get().is_none());
    }
}
