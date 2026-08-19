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
//! # ALPHA TESTING ONLY — per-user Support ID (expires 2026-12-31)
//! [`save_account_email`] also derives a domain-separated, one-way hash of the email
//! (`meridian::telemetry_spool::redact::pseudonymize_account`) and mirrors
//! ONLY that hash into `settings.json`'s `account_pseudonym` — never the raw
//! email, which stays confined to this module's own `account.json`. The
//! daemon has no other way to see who's signed in (`settings.json` is the one
//! file both processes read), and it's what
//! `telemetry_spool::redact::local_host_pseudonym` reads to seed the Support
//! ID/shipped `host.name` per account instead of per machine, until that
//! function's hardcoded expiry passes. [`clear_account_email`] clears it back
//! out on sign-out. See that function's doc for the full rationale — this is
//! gated on the wall clock, NOT the release channel, because the hand-picked
//! alpha testers install the same `stable` build as everyone else.
//!
//! # Related
//! - `crate::analytics` — the PostHog capture this feeds (email = `distinct_id`).
//! - `commands::setup` — `mark_setup_complete`/`is_first_run`, the same
//!   `~/.meridian/*` marker-file pattern this module follows.

use anyhow::Context;
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

/// Whether the app must be signed in to be usable — the single switch behind
/// BOTH sign-in gates (`RequireSignIn`, which hides the whole dashboard, and
/// the setup wizard's sign-in step, whose Next button is otherwise dead).
/// They read one answer rather than each re-deriving it, so the two can never
/// disagree and strand a user in a wizard they cannot leave.
///
/// **False only in a DEBUG build with no publishable key configured.** That is
/// the contributor case: `git clone` → `dev-start.sh` with no `.env`, where
/// requiring sign-in means requiring a Clerk key we cannot ship, and the app is
/// simply unusable.
///
/// # Why `debug_assertions` and not [`crate::install::detect_install_mode`]
///
/// The obvious-looking `InstallMode::Dev` check is WRONG here, and quietly so:
/// `detect_install_mode` returns `Dev` only when a `.env` **already exists**, so
/// a fresh clone — precisely the case this exists for — is `Bare`, and `Bare` is
/// also what a real install with no `.env` reports. Keying on it would both miss
/// the contributor and hand a shipped build a path to unlock itself.
///
/// `debug_assertions` cannot do either. A packaged build is compiled in release,
/// so this whole branch is compiled OUT of the shipped binary — sign-in there is
/// not "enforced at runtime", it is the only code that exists. There is no
/// setting, env var, or file a user can reach that turns it off.
///
/// Setting `CLERK_PUBLISHABLE_KEY` in a dev `.env` flips this back to `true`, so
/// a maintainer's dev build exercises the real production sign-in path end to
/// end. The key's presence IS the mode switch — nothing else to remember.
#[tauri::command]
#[tracing::instrument]
pub async fn sign_in_required() -> bool {
    sign_in_required_for(&clerk_publishable_key())
}

/// The decision itself, taking the resolved key rather than looking it up.
///
/// Split this way ON PURPOSE: [`clerk_publishable_key`] consults the process env
/// AND the checkout's `.env`, so a test calling it would answer differently on a
/// maintainer's machine (key present) than in CI (absent) — the "no key" case
/// would silently stop being tested by whoever most needs it to hold. Passing
/// the key in makes both branches deterministic everywhere.
fn sign_in_required_for(key: &str) -> bool {
    // Release builds compile this out entirely — see [`sign_in_required`].
    if !cfg!(debug_assertions) {
        return true;
    }
    !key.trim().is_empty()
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

    // ALPHA TESTING ONLY — see the module doc. Mirrors ONLY the hash, never
    // the email, into settings.json for the daemon to seed the per-user
    // Support ID from.
    write_account_pseudonym(Some(&email))
        .await
        .map_err(|e| format!("persist account pseudonym: {e:#}"))?;

    Ok(())
}

/// ALPHA TESTING ONLY (revert target: ~1 month from 2026-07-28) — mirror the
/// signed-in account's hashed pseudonym into `settings.json`'s
/// `account_pseudonym`, or clear it (`email = None`) on sign-out. Never
/// writes the raw email — only
/// [`meridian::telemetry_spool::redact::pseudonymize_account`]'s one-way
/// hash, which is all `telemetry_spool::redact::local_host_pseudonym` needs.
/// Clearing promptly on sign-out matters: a lingering hash would keep
/// grouping error reports under someone no longer signed in on this machine.
/// Mirror the account pseudonym (a one-way hash, never the raw email) into
/// `settings.json`, or clear it on sign-out.
///
/// Instrumented because it does real I/O whose failure is otherwise invisible:
/// the mapped `String` error stops at the Tauri command boundary and never
/// reaches `meridian logs` or the telemetry backend, so a persistently failing
/// settings write would silently leave the Support ID stale - the one value
/// support uses to find this user's error rows.
#[tracing::instrument(skip(email), fields(signed_in = email.is_some()))]
async fn write_account_pseudonym(email: Option<&str>) -> anyhow::Result<()> {
    let hash = email.map(meridian::telemetry_spool::redact::pseudonymize_account);
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut v = meridian_core::settings::read_settings_value();
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "account_pseudonym".to_string(),
                match &hash {
                    Some(h) => serde_json::Value::String(h.clone()),
                    None => serde_json::Value::Null,
                },
            );
        }
        meridian_core::settings::write_settings_value(&v)
    })
    .await
    .context("join settings write task")
    .and_then(|inner| inner);
    if let Err(e) = &result {
        // The failure boundary. No email and no hash in the field set - the
        // hash IS the pseudonymous identifier, so it stays out of logs.
        tracing::error!(error = %format!("{e:#}"), "writing the account pseudonym failed");
    }
    result
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
    // ALPHA TESTING ONLY (see module doc) — clear the mirrored pseudonym
    // FIRST, so a crash between the two writes fails toward "back to the
    // per-machine Support ID", never toward "still grouped under the old
    // account" with no email left to explain why.
    write_account_pseudonym(None)
        .await
        .map_err(|e| format!("clear account pseudonym: {e:#}"))?;

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

#[cfg(test)]
mod sign_in_gate_tests {
    use super::sign_in_required_for;

    /// A key configured means the real gate, even in dev — this is how a
    /// maintainer exercises the production sign-in path locally.
    #[test]
    fn a_configured_key_always_requires_sign_in() {
        assert!(sign_in_required_for("pk_test_abc123"));
        assert!(sign_in_required_for("pk_live_abc123"));
    }

    /// The contributor case, and the ONLY case that may bypass: a debug build
    /// with nothing configured. Whitespace counts as nothing — a `.env` line
    /// left as `CLERK_PUBLISHABLE_KEY= ` must not register as a real key and
    /// lock a contributor out of a gate they have no way to satisfy.
    #[test]
    fn an_unset_key_bypasses_only_in_debug() {
        let expected = !cfg!(debug_assertions);
        assert_eq!(sign_in_required_for(""), expected);
        assert_eq!(sign_in_required_for("   "), expected);
        assert_eq!(sign_in_required_for("\t\n"), expected);
    }

    /// THE SHIPPING INVARIANT. A release build gates unconditionally — no key
    /// state, env var, or file can unlock it. If this ever fails, a packaged
    /// build can be opened without an account.
    ///
    /// Only meaningful under `cargo test --release`; in a debug run it asserts
    /// the (true) claim that the release branch is compiled out. The guard is
    /// deliberate rather than `#[cfg(not(debug_assertions))]` so the test is
    /// visible in both modes instead of silently vanishing from the default run.
    #[test]
    fn a_release_build_can_never_bypass() {
        if cfg!(debug_assertions) {
            return;
        }
        for key in ["", "   ", "pk_test_abc123"] {
            assert!(
                sign_in_required_for(key),
                "release build bypassed sign-in for key {key:?}"
            );
        }
    }
}
