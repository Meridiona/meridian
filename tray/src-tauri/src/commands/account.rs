//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! One-time email capture — the captured account email + its ALPHA telemetry mirror.
//!
//! # What this is
//! The setup wizard's Email step and Settings → Account's control
//! (`ui/app/setup/signin.tsx` / the `ui/app/setup/signin/` module — see
//! `OtpForm.tsx`/`AccountAuthControl.tsx`) send/verify a one-time code via
//! `crate::commands::otp` (a small Cloudflare Worker + AWS SES, no client-side
//! auth library, no session). Once a code verifies, the frontend calls
//! [`save_account_email`] to persist the address — there is no session to mirror;
//! this file's `account.json` IS the whole record of who's signed in.
//!
//! # Who calls this
//! - [`save_account_email`]: `ui/app/setup/signin/OtpForm.tsx` (on a verified
//!   code) and `AccountAuthControl.tsx`'s "Change email" control.
//! - [`read_account_email`]: `crate::analytics`, the identity gate — no
//!   PostHog event is sent at all until this returns `Some` (the email
//!   becomes the event's `distinct_id` directly, never an anonymous id).
//!
//! # ALPHA TESTING ONLY — per-user Support ID + raw-email telemetry (expires after 2026-12-31)
//! [`save_account_email`] also derives a domain-separated, one-way hash of the email
//! (`meridian::telemetry_spool::redact::pseudonymize_account`) and mirrors
//! BOTH that hash AND the raw email into `settings.json` (`account_pseudonym`
//! and `account_email` respectively) — the raw email otherwise stays confined
//! to this module's own `account.json`. The daemon has no other way to see
//! who's signed in (`settings.json` is the one file both processes read), and
//! `account_pseudonym` is what `telemetry_spool::redact::local_host_pseudonym`
//! reads to seed the Support ID/shipped `host.name` per account instead of
//! per machine; `account_email` is what lets an OpenObserve resource
//! attribute (`observability::init`) and Sentry's `user.email` (`crash.rs`)
//! name the actual signed-in tester, via
//! `telemetry_spool::redact::alpha_account_email_if_active`, gated at read
//! time by both consumers against the same hardcoded expiry. This is a
//! deliberate, explicitly-approved exception to "never write the raw email"
//! — see `docs/privacy.md`'s alpha section. [`clear_account_email`] clears it back
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

/// Persist the captured user's email after the setup wizard's Email step (or
/// Settings → Account's "Change email") verifies a code. Crash-safe write
/// (temp + rename), mirroring `analytics.rs::save_state`. Idempotent — a plain
/// overwrite, safe to call again for the same or a changed address. The next
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
/// signed-in account's identity into `settings.json`'s `account_pseudonym`
/// (a one-way hash) AND, as of the raw-email telemetry change, its sibling
/// `account_email` (the RAW address), or clear both (`email = None`) on
/// sign-out. `account_pseudonym` is what
/// `telemetry_spool::redact::local_host_pseudonym` needs; `account_email` is
/// what lets an OpenObserve resource attribute / Sentry `user.email` name the
/// actual signed-in tester, gated at READ time by every consumer against
/// `redact::ALPHA_ACCOUNT_OVERRIDE_EXPIRES_UNIX` (see
/// `redact::alpha_account_email_if_active`) — this function itself does not
/// time-gate the write, exactly like the pseudonym half never did. Clearing
/// promptly on sign-out matters: a lingering value would keep attributing
/// error reports to someone no longer signed in on this machine.
///
/// Instrumented because it does real I/O whose failure is otherwise invisible:
/// the mapped `String` error stops at the Tauri command boundary and never
/// reaches `meridian logs` or the telemetry backend, so a persistently failing
/// settings write would silently leave the Support ID stale - the one value
/// support uses to find this user's error rows.
#[tracing::instrument(skip(email), fields(signed_in = email.is_some()))]
async fn write_account_pseudonym(email: Option<&str>) -> anyhow::Result<()> {
    let hash = email.map(meridian::telemetry_spool::redact::pseudonymize_account);
    // GATE THE WRITE, not just every read. Consumers already refuse to ship the
    // address past `ALPHA_ACCOUNT_OVERRIDE_EXPIRES_UNIX`, but nothing stopped it
    // being STORED - so a tester who signs in after the expiry would still put
    // raw PII in `settings.json`, for a window that had already closed. The
    // pseudonym half is unaffected: it is a one-way hash and is what the Support
    // ID needs after the exception lapses.
    let raw_email = email
        .and_then(|e| meridian::telemetry_spool::redact::alpha_account_email_if_active(Some(e)));
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        // Under the shared settings lock - see
        // `meridian_core::settings::mutate_settings_value`. Without it a sign-in
        // racing a Settings save silently discarded one of the two.
        meridian_core::settings::mutate_settings_value(|v| {
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "account_pseudonym".to_string(),
                    match &hash {
                        Some(h) => serde_json::Value::String(h.clone()),
                        None => serde_json::Value::Null,
                    },
                );
                obj.insert(
                    "account_email".to_string(),
                    match &raw_email {
                        Some(e) => serde_json::Value::String(e.clone()),
                        None => serde_json::Value::Null,
                    },
                );
            }
            Ok(())
        })
    })
    .await
    .context("join settings write task")
    .and_then(|inner| inner);
    if let Err(e) = &result {
        // The failure boundary. No email and no hash in the field set - the
        // hash IS the pseudonymous identifier, so it stays out of logs.
        tracing::error!(error = %format!("{e:#}"), "writing the account identity failed");
    }
    result
}

/// Whether a stored `account_email` must be removed now - the whole decision
/// [`purge_expired_account_email`] makes, split out so it is testable without
/// `MERIDIAN_SETTINGS_PATH`, which is process-global and racy under cargo's
/// test threads (see `meridian_core::settings`'s own note on it).
///
/// True only when a non-empty address is stored AND the ALPHA window has
/// closed. An absent, null, or blank value is already fine, and a value inside
/// the window is legitimately there.
fn account_email_should_be_purged(stored: Option<&str>) -> bool {
    let Some(stored) = stored else { return false };
    if stored.trim().is_empty() {
        return false;
    }
    meridian::telemetry_spool::redact::alpha_account_email_if_active(Some(stored)).is_none()
}

/// Remove a stored raw `account_email` once the ALPHA exception has lapsed.
///
/// # Why gating the write is not enough
/// [`write_account_pseudonym`] runs on sign-in and sign-out. A tester who signs
/// in before [`meridian::telemetry_spool::redact::ALPHA_ACCOUNT_OVERRIDE_EXPIRES_UNIX`]
/// and simply STAYS signed in never calls it again, so their raw address would
/// sit in `settings.json` indefinitely - past the window the exception was
/// approved for, and with no user-visible sign it is still there.
///
/// The exception's whole premise is that it lapses automatically with no deploy
/// (see `redact::ALPHA_ACCOUNT_OVERRIDE_EXPIRES_UNIX`). Shipping stops on its
/// own; retention did not, until this.
///
/// Only the raw email is touched. `account_pseudonym` is a one-way hash and is
/// what the Support ID falls back to afterwards, so it must survive.
///
/// # Who calls this
/// `lib.rs`'s setup, once per launch. One settings read on a healthy install
/// where the key is already absent, and a single write on the one launch that
/// crosses the boundary.
#[tracing::instrument]
pub(crate) fn purge_expired_account_email() {
    // Under the shared lock like every other writer. This was written as a bare
    // read-modify-write and had the same lost-update race it removes elsewhere:
    // it runs during tray startup, alongside the first settings reads and writes.
    let outcome = meridian_core::settings::mutate_settings_value(|v| {
        let Some(obj) = v.as_object_mut() else {
            return Ok(false);
        };
        let stored = obj.get("account_email").and_then(|e| e.as_str());
        if !account_email_should_be_purged(stored) {
            return Ok(false);
        }
        obj.insert("account_email".to_string(), serde_json::Value::Null);
        Ok(true)
    });
    // Nothing to purge is the overwhelmingly common path and must not count as
    // a write - `mutate_settings_value` persists unconditionally, so the
    // no-op rewrite is accepted here in exchange for one lock discipline
    // rather than two.
    match outcome {
        // Never log the address itself - that would defeat the removal by
        // copying it into the telemetry spool on its way out.
        Ok(true) => tracing::info!(
            "the alpha raw-email window has closed - removed the stored account email"
        ),
        Ok(false) => {}
        Err(e) => tracing::warn!(
            error = %format!("{e:#}"),
            "could not remove the expired account email - will retry next launch"
        ),
    }
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

/// Frontend-facing read of the persisted account email — backs
/// `RequireEmailCapture`'s one-time gate check and the Settings → Account
/// panel (`ui/components/timeline/settings/AccountSection.tsx`), both of which
/// need to know synchronously (no session object to inspect) whether an
/// address has ever been captured.
#[tauri::command]
#[tracing::instrument]
pub async fn get_account_email() -> Option<String> {
    read_account_email()
}

/// Clear the persisted account email. There is no session to sign out of, so
/// nothing in the shipped UI calls this today (`AccountAuthControl.tsx` offers
/// "Change email" — a plain overwrite via [`save_account_email`] — rather than
/// a sign-out that would need this). Kept as a real command rather than
/// deleted: it is the only way to fully reset a machine's captured identity
/// (e.g. for local testing, or a future account-reset affordance), and
/// deleting it would mean reinventing the ALPHA-pseudonym-clearing ordering
/// below from scratch if that need ever comes back. Not an error if never
/// captured (no file to remove).
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
mod account_email_retention_tests {
    use super::*;

    /// The retention half of the ALPHA raw-email exception.
    ///
    /// Consumers already refuse to SHIP the address past its expiry and the
    /// write is gated too, but a tester who signs in before the boundary and
    /// simply stays signed in never re-runs that write - so without a purge
    /// their raw address sits in `settings.json` indefinitely, past the window
    /// the exception was approved for.
    #[test]
    fn a_stored_address_is_purged_only_once_the_window_has_closed() {
        let inside = meridian::telemetry_spool::redact::alpha_account_email_if_active(Some(
            "tester@example.com",
        ))
        .is_some();
        assert_eq!(
            account_email_should_be_purged(Some("tester@example.com")),
            !inside,
            "purge exactly when the alpha window is closed - never while it is open"
        );
    }

    /// Nothing to remove must never mean a settings write. `purge_expired_account_email`
    /// rewrites the whole document, so a false positive here would rewrite
    /// `settings.json` on every single launch.
    #[test]
    fn nothing_to_purge_is_never_a_write() {
        assert!(!account_email_should_be_purged(None));
        assert!(!account_email_should_be_purged(Some("")));
        assert!(!account_email_should_be_purged(Some("   ")));
    }
}
