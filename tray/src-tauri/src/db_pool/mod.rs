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
//! - Anything that touches this pool and can fail calls
//!   [`raise_if_corrupt`] on the error - see that function's doc.

use meridian_core::SqlitePool;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// The managed [`DbPool`] handle, from an app handle.
///
/// For code that must reach the pool but is not a `#[tauri::command]` (so it
/// cannot take `State<'_, DbPool>`): the capture consumers, the poll loop's
/// guards, `resume_capture`.
///
/// # Take the HANDLE, never a pool snapshot
///
/// The point of returning `DbPool` rather than `Option<SqlitePool>` is that
/// callers must resolve [`DbPool::get`] **at each use**. A long-lived task that
/// clones the `SqlitePool` once and keeps it is the exact bug this exists to
/// prevent: [`close`](DbPool::close) can only reach the pool inside this
/// handle, so an escaped clone keeps its connections - and its WAL-index
/// (`-shm`) mapping - alive across the daemon restart the close/reopen dance
/// exists to fence off. Measured on 1.91.0-staging.2: the capture consumers
/// held such a clone for the whole process lifetime and wrote through it every
/// ~2.5 s, and after a reconnect-triggered daemon restart every write failed
/// with `(code: 11) database disk image is malformed` **while the file itself
/// was healthy** (`db check`: 40 tables clean) and reads kept succeeding. Only
/// writes break, because reads can still be served from the main file while the
/// WAL write path cannot.
pub(crate) fn from_app(app: &tauri::AppHandle) -> Option<DbPool> {
    use tauri::Manager;
    app.try_state::<DbPool>().map(|s| s.inner().clone())
}

/// If `err` indicates `meridian.db` is corrupt, raise the SAME `db.corrupt`
/// notice `main.rs`'s `etl_tick` raises on the daemon side - immediately,
/// from whichever side of the app noticed first.
///
/// The daemon already had this covered for its own queries, but the tray holds
/// its own independent, long-lived pool on the same file (opened once at
/// startup, `lib.rs`'s `app.manage(db_pool)`) and touches different tables on
/// its own cadence. In the incident this was written for, the tray's poll-loop
/// reads hit `(code: 11) database disk image is malformed` a full 5+ minutes
/// before any daemon-side query happened to touch the same damage - and until
/// this function existed, that whole window was silent `tracing::warn!` noise
/// with no banner, because nothing on this side of the process ever called
/// `raise_typed`. Idempotent (`raise_typed` upserts), so calling it on every
/// failing tick is safe and cheap - it does not need its own latch the way the
/// daemon's ETL loop does, because a tick that keeps failing just keeps
/// refreshing the same notice row rather than retrying a query with side
/// effects.
///
/// # Why this lives here and not in `poll::refresh`
///
/// It started as a private helper wrapping that loop's four dashboard READS,
/// which quietly made "the tray noticed corruption" mean "one of four specific
/// reads noticed corruption". `commands::tasks`' PM-sync outbox writes are on
/// this same pool and outside all of it, so on a staging machine whose
/// `meridian.db` was damaged they were the only code to find the damage - and
/// reported it as a raw SQL string in a settings panel, with no banner and no
/// Repair button, because the three detectors that DO know what corruption
/// means each had a scope that excluded them:
///
/// - `repair_boot`'s startup probe is skipped entirely while a daemon answers
///   (its own comment defers to "the notice banner instead");
/// - the daemon latches only when ITS queries reach a damaged page;
/// - this helper only covered `poll::refresh`.
///
/// Living on the pool module is what lets any of them call it - both `poll` and
/// `commands` already depend on this module for `DbPool` itself.
///
/// # Coverage is still partial - do not read this as an invariant
///
/// The rule this SHOULD enforce is "a failure on the tray's pool goes through
/// here". It does not yet. Wired today: `poll::refresh`'s four reads and
/// `commands::tasks`' two outbox queries. **Not** wired: `commands::dashboard`,
/// which has ~24 `cmd_err!` sites reading `pm_tasks`, triage, week and
/// coding-agent tables - so damage confined to those pages is still found
/// without raising the banner.
///
/// That gap is narrower than it looks, because `poll::refresh` re-reads the
/// active-session/today/worklogs tables every ~30 s and the daemon latches on
/// its own ETL path, so most real damage is reached by something that does
/// raise. It is not zero, though, and the honest statement is that this is a
/// convention being adopted rather than one already held everywhere. Anything
/// added here should also be added to those sites rather than assuming they are
/// already covered.
pub(crate) async fn raise_if_corrupt(pool: &SqlitePool, err: &anyhow::Error) {
    if !meridian::db::integrity::is_corrupt_error(err) {
        return;
    }
    let _ = meridian::notices::raise_typed(
        pool,
        meridian::notices::Notice {
            id: meridian::notices::DB_CORRUPT,
            severity: "error",
            title: "Meridian's database is damaged",
            // Full chain, not `err.to_string()` - same reasoning as
            // `crate::cmd_err!`'s doc comment: `anyhow::Error`'s `Display`
            // renders only the outermost `.context()` and would otherwise
            // drop the SQLite code a reader needs.
            detail: &format!("{err:#}"),
            remedy: Some("Quit Meridian, then run 'meridian db repair' in a terminal"),
            event_key: meridian::notices::DB_CORRUPT,
            deep_link: Some(meridian_core::notifications::deep_links::LOGS),
        },
    )
    .await;
}

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
    /// Serialises [`recover_if_corrupt`](Self::recover_if_corrupt) and remembers
    /// when it last ran, so concurrent failing writers recycle the pool once
    /// between them instead of each racing their own close/reopen.
    recycle: Arc<tokio::sync::Mutex<Option<Instant>>>,
}

/// Minimum gap between two pool recycles.
///
/// Without it, a wedged pool would recycle on EVERY failing write - the capture
/// consumers alone write every ~2.5 s - so a fault the recycle cannot fix (real
/// file damage, a revoked key) would turn into a close/reopen storm on the file
/// the daemon is also using. One attempt per window, then the banner and the
/// error stand.
const RECYCLE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

/// Ceiling on the close half of a recycle.
///
/// `SqlitePool::close` waits for checked-out connections to come back, and the
/// whole reason we are here is that something is wrong with this pool. Bounded so
/// a connection that never returns cannot hold the recycle lock - and therefore
/// every future recovery attempt - forever. `close` clears the handle
/// synchronously before it awaits, so a timeout still leaves `get()` at `None`
/// and the reopen below still installs a fresh pool.
const RECYCLE_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
            recycle: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Recover from a write that failed because this pool's view of the database is
    /// broken: drop every connection and open a fresh pool. Returns whether the
    /// caller now has a working pool and may retry.
    ///
    /// # Why the app must heal itself here
    ///
    /// On 1.91.0-staging.2 a reconnect-triggered daemon restart left the tray's
    /// connections with a desynced WAL index (`-shm`). Reads kept working, every
    /// write failed with `(code: 11) database disk image is malformed`, and `db
    /// check` reported all 40 tables healthy - the data was never damaged, only this
    /// process's bookkeeping. The only cure was quitting and relaunching the app,
    /// which **no user has any way of knowing**. They just saw sync stop working.
    ///
    /// Closing and reopening the pool is what that relaunch did for the database
    /// handle, so doing it here removes the need for the user to be told anything.
    /// It is deliberately independent of *why* the view broke: the ordering fixes
    /// that shipped alongside this close the mechanism we identified, but this is
    /// what makes a mechanism we did NOT identify survivable rather than permanent.
    ///
    /// Non-corrupt errors return `false` immediately - a locked database or a
    /// missing table must not trigger a reconnect.
    ///
    /// # Caller contract
    ///
    /// **Call this only after your query has returned**, never while holding a
    /// connection from this pool. The close half waits for checked-out connections
    /// to be returned, so a caller still holding one would wait on itself. Every
    /// current caller is on an error path, where the connection is already back.
    /// Exclusive access to this handle's close/reopen lifecycle.
    ///
    /// **Every close+reopen pair must hold this**, not just the recycle path.
    /// `commands::daemon::reload_with_pool_cycle` had its own private lock, which
    /// serialised reloads against each other but not against
    /// [`recover_if_corrupt`](Self::recover_if_corrupt) - and interleaving those two
    /// reintroduces the exact hazard the close/reopen dance exists to remove:
    ///
    /// 1. reload closes, handle is `None`;
    /// 2. a recycle sees `None`, treats it as nothing to close, and REOPENS;
    /// 3. reload then signals the daemon restart - with that fresh pool open
    ///    across it.
    ///
    /// Step 3 is the 2026-08-24 corruption profile (a tray connection spanning two
    /// daemon generations with no WAL checkpoint between them). The lock lives on the
    /// handle rather than in either caller so a third close/reopen site cannot be
    /// added without one.
    ///
    /// The guard's value is the last recycle instant, which is also what makes the
    /// cooldown check-and-set atomic.
    pub(crate) async fn lock_cycle(&self) -> tokio::sync::MutexGuard<'_, Option<Instant>> {
        self.recycle.lock().await
    }

    pub(crate) async fn recover_if_corrupt(&self, err: &anyhow::Error) -> bool {
        if !meridian::db::integrity::is_corrupt_error(err) {
            return false;
        }

        // Held across the close/reopen on purpose: it serialises concurrent
        // recyclers, excludes a `reload_daemon` cycle (see `lock_cycle`), and makes
        // the cooldown check-and-set atomic.
        let mut last = self.lock_cycle().await;
        if let Some(at) = *last {
            if at.elapsed() < RECYCLE_COOLDOWN {
                tracing::debug!(
                    "skipping meridian.db pool recycle - one ran less than {}s ago",
                    RECYCLE_COOLDOWN.as_secs()
                );
                return false;
            }
        }
        *last = Some(Instant::now());

        tracing::warn!("recycling the meridian.db pool after a corrupt-view write failure");
        if tokio::time::timeout(RECYCLE_CLOSE_TIMEOUT, self.close())
            .await
            .is_err()
        {
            // `close` already took the handle before awaiting, so this is safe to
            // proceed through - the old pool is unreachable either way.
            tracing::warn!(
                timeout_s = RECYCLE_CLOSE_TIMEOUT.as_secs() as i64,
                "pool close did not finish during recycle - reopening anyway"
            );
        }
        self.reopen().await;

        let recovered = self.get().is_some();
        if recovered {
            tracing::info!("meridian.db pool recycled - writes should work again");
        } else {
            tracing::warn!("meridian.db pool recycle did not yield a usable pool");
        }
        recovered
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
mod tests;
