//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Clerk email sign-in — publishable-key resolution + the captured account email.
//!
//! # What this is
//! The setup wizard's Sign-in step and Settings → Account's control
//! (`ui/app/setup/signin.tsx` / the `ui/app/setup/signin/` module — see
//! `SignInWidget.tsx`/`AccountAuthControl.tsx`) drive Clerk entirely
//! client-side via `tauri-plugin-clerk` (see `lib.rs`'s plugin registration)
//! — email one-time-code only, no OAuth
//! (the plugin doesn't support it) and no password (disabled on the Clerk
//! instance itself; see the manual dashboard step below). Once a session
//! exists, the frontend calls [`save_account_email`] so the Rust side also
//! knows who's signed in, since Clerk's own session lives only in the
//! webview's persisted store.
//!
//! **Manual prerequisite (not code):** the Clerk application's email
//! auth strategy must have `sign_in_strategies: ["email_code"]` and
//! `auth_password.enabled = false` (Clerk dashboard → Configure → Email,
//! phone, username) so sign-up never asks for a password. Verify with
//! `clerk config pull` after linking the app (`clerk link --app <id>`).
//!
//! # Who calls this
//! - [`clerk_publishable_key`]: `lib.rs`'s builder, to construct the
//!   `tauri_plugin_clerk::ClerkPluginBuilder`.
//! - [`save_account_email`]: `ui/app/setup/signin/SignInWidget.tsx` and
//!   `AccountAuthControl.tsx` on successful sign-in.
//! - [`read_account_email`]: `crate::analytics`, the identity gate — no
//!   PostHog event is sent at all until this returns `Some` (the email
//!   becomes the event's `distinct_id` directly, never an anonymous id).
//!
//! # Related
//! - `crate::analytics` — the PostHog capture this feeds (email = `distinct_id`).
//! - `commands::setup` — `mark_setup_complete`/`is_first_run`, the same
//!   `~/.meridian/*` marker-file pattern this module follows.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Clerk publishable key — a **public** identifier (safe to embed in a shipped
/// client, unlike a secret key), baked in at compile time mirroring
/// `analytics.rs`'s `DEFAULT_POSTHOG_API_KEY`. Both release workflows
/// (`release.yml` and `release-staging.yml`) set `MERIDIAN_CLERK_PUBLISHABLE_KEY`
/// to the **production** Clerk instance's key (`meridiona.com`) — every
/// packaged, downloadable build uses production, staging channel included,
/// per the rule: only a bare source checkout gets the dev instance. A local
/// `cargo run`/`tauri dev` build compiles this constant empty (no CI env var
/// set), but never actually falls through to it — [`clerk_publishable_key`]
/// resolves the **dev** instance's key first from the repo root `.env`
/// (`CLERK_PUBLISHABLE_KEY=pk_test_...`) via the `InstallMode::Dev` branch
/// below, same as the tracker keys `commands::integrations` reads. This
/// constant is really only reached by `InstallMode::Canonical`/`Bare` (a real
/// install) when, for whatever reason, no CI-baked key made it into the
/// binary. The frontend never needs its own copy — `initClerk()`
/// (`tauri-plugin-clerk`) returns the key from this Rust-side config, so
/// `ClerkProvider` reads `clerk.publishableKey` off that, not a
/// `NEXT_PUBLIC_*` env var.
const DEFAULT_CLERK_PUBLISHABLE_KEY: &str = match option_env!("MERIDIAN_CLERK_PUBLISHABLE_KEY") {
    Some(k) => k,
    None => "",
};

/// Resolve the publishable key, in priority order: the `CLERK_PUBLISHABLE_KEY`
/// process env (an explicit override — a launcher/shell export, or local
/// testing against a different instance), then the same key read straight out
/// of [`crate::install::detect_install_mode`]'s `.env` file (the tray doesn't
/// auto-load env the way the daemon does — see `install.rs`'s
/// `env_key_from_path` doc; this is where a source checkout picks up the repo
/// root `.env`'s **dev** instance key), then the compiled-in
/// [`DEFAULT_CLERK_PUBLISHABLE_KEY`] (the **production** instance key, for
/// any real install with no `.env` override).
pub fn clerk_publishable_key() -> String {
    if let Some(v) = std::env::var("CLERK_PUBLISHABLE_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return v;
    }
    if let Some(v) = crate::install::detect_install_mode()
        .env_path()
        .and_then(|p| crate::install::env_key_from_path(p, "CLERK_PUBLISHABLE_KEY"))
    {
        return v;
    }
    DEFAULT_CLERK_PUBLISHABLE_KEY.to_string()
}

/// Persisted account state — deliberately its own file (`~/.meridian/account.json`),
/// never merged into `settings.json` (which the dashboard reads/writes/displays),
/// same rationale as `analytics.rs`'s `analytics_state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountState {
    email: String,
}

fn account_path() -> Option<PathBuf> {
    meridian_core::paths::home_dir().map(|h| h.join(".meridian/account.json"))
}

/// Persist the signed-in user's email after the setup wizard's sign-in step
/// completes. Crash-safe write (temp + rename), mirroring
/// `analytics.rs::save_state`. Idempotent — safe to call again (e.g. a
/// relaunch that re-verifies the persisted Clerk session). The next
/// scheduled poll tick picks up the email and starts sending
/// `app_installed`/`daily_usage` identified by it (see `crate::analytics`).
#[tauri::command]
// skip `email` too: it's PII and must never land in a span field / log line.
// `err` records the failure + marks the span status ERROR on the `Err` path.
#[tracing::instrument(skip(email), err)]
pub async fn save_account_email(email: String) -> Result<(), String> {
    let email = email.trim().to_string();
    if email.is_empty() || !email.contains('@') {
        return Err("save_account_email: not a valid email address".into());
    }
    let path = account_path().ok_or("save_account_email: home directory could not be resolved")?;
    // Crash-safe write via the shared helper. It's sync, so run it on the
    // blocking pool rather than stalling the async command's executor thread.
    let state = AccountState {
        email: email.clone(),
    };
    tokio::task::spawn_blocking(move || meridian_core::fs_utils::atomic_write_json(&path, &state))
        .await
        .map_err(|e| format!("join account write task: {e}"))?
        .map_err(|e| format!("persist account file: {e:#}"))?;
    tracing::info!("account: email captured");
    Ok(())
}

/// Read the persisted account email, if any. `None` before the sign-in step
/// has ever completed (or on a corrupt/missing file — never fabricated).
pub(crate) fn read_account_email() -> Option<String> {
    let path = account_path()?;
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<AccountState>(&s)
        .ok()
        .map(|a| a.email)
}

/// Frontend-facing read of the persisted account email — backs the Settings →
/// Account panel (`ui/components/timeline/settings/AccountSection.tsx`) so it
/// can show who's signed in without waiting on Clerk's own (async) session
/// check first.
#[tauri::command]
#[tracing::instrument]
pub async fn get_account_email() -> Option<String> {
    read_account_email()
}

/// Clear the persisted account email on sign-out. Called by the Settings →
/// Account panel right after the webview's Clerk session itself is torn down
/// (`useClerk().signOut()` in `AccountAuthControl.tsx`'s `AccountStatus`) — this file
/// is the Rust-side mirror of that session, so it must be cleared in lockstep
/// or a stale email would keep surfacing in Settings/analytics after logout.
/// Not an error if never signed in (no file to remove).
#[tauri::command]
#[tracing::instrument]
pub async fn clear_account_email() -> Result<(), String> {
    let Some(path) = account_path() else {
        return Ok(());
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::info!("account: email cleared");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove account file: {e}")),
    }
}
