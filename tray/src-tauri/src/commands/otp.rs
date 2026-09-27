//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Email capture notification — the OTP Worker client.
//!
//! # What this is
//! One command backing the setup wizard's Email step
//! (`ui/app/setup/signin/OtpForm.tsx`) and Settings → Account's "Change email"
//! control (`AccountAuthControl.tsx`): a best-effort, fire-and-forget ping to
//! `infra/otp-worker`'s `POST /otp/capture`, which tells the team (via Resend)
//! that someone signed up or changed their email — the same internal
//! notification the old send/verify flow fired on a successful code check.
//!
//! There is no verification step anymore: AWS SES was never approved for
//! production sending (still sandboxed to individually-verified recipients),
//! which meant the old `/otp/send` code delivery could fail for real users'
//! arbitrary addresses — blocking sign-up outright rather than just adding
//! delay. [`capture_account_email`] therefore never blocks or fails the
//! caller on the Worker's account: [`crate::commands::account::save_account_email`]
//! (the actual local persistence) is the source of truth and has no
//! dependency on this command succeeding. The frontend calls both, but never
//! awaits this one's rejection as a reason to stop.
//!
//! # Who calls this
//! - [`capture_account_email`]: `OtpForm.tsx` (the wizard's Email step, and
//!   inline in `AccountAuthControl.tsx`'s "Change email"), invoked without
//!   awaiting its result.
//!
//! # Related
//! - `crate::commands::account` — `save_account_email`/`read_account_email`;
//!   the actual capture, never gated by this module.
//! - `crate::counter_ping` — the sibling `option_env!` → resolver →
//!   `.bearer_auth()` pattern this mirrors for `OTP_CLIENT_TOKEN`.

use std::time::Duration;

/// Compiled-in default Worker base URL — public (baked via
/// `MERIDIAN_OTP_API_URL`, mirrors `MERIDIAN_CENTRAL_OTLP_ENDPOINT`'s public
/// `vars.*` treatment, not a secret). Empty in a plain source build with no
/// CI-injected value.
const DEFAULT_OTP_API_URL: &str = match option_env!("MERIDIAN_OTP_API_URL") {
    Some(v) => v,
    None => "",
};

/// Compiled-in default bearer token — mirrors
/// `counter_ping::DEFAULT_COUNTER_API_KEY`. This proves "a genuine Meridian
/// binary sent this," not "this is a human" — it is attestation, not a strong
/// secret (extractable from the shipped binary), same honesty the Worker's own
/// docs carry. Empty in a plain source build.
const DEFAULT_OTP_CLIENT_TOKEN: &str = match option_env!("MERIDIAN_OTP_CLIENT_TOKEN") {
    Some(v) => v,
    None => "",
};

/// Priority-order env resolution, factored out as a pure function so the
/// order itself is unit-testable without mutating real process env (which is
/// process-global and racy under cargo's parallel test threads — see
/// `account.rs`'s own note on `MERIDIAN_SETTINGS_PATH` for the same problem).
/// Priority: an explicit non-blank `process_env` value, then a non-blank
/// `dotenv` value, then `default`. Blank/whitespace-only counts as absent at
/// every step, mirroring the old `clerk_publishable_key`'s treatment of a
/// `.env` line left as `KEY= `.
fn resolve_priority(process_env: Option<String>, dotenv: Option<String>, default: &str) -> String {
    if let Some(v) = process_env.filter(|s| !s.trim().is_empty()) {
        return v;
    }
    if let Some(v) = dotenv.filter(|s| !s.trim().is_empty()) {
        return v;
    }
    default.to_string()
}

/// Resolve the Worker base URL: the `OTP_API_URL` process env override (an
/// explicit override — a launcher/shell export, or local testing against a
/// different deploy) → the same key read from the current install's `.env`
/// (`install::detect_install_mode()`, exactly the branch the old
/// `clerk_publishable_key` took for `CLERK_PUBLISHABLE_KEY` — the tray doesn't
/// auto-load env the way the daemon does) → the compiled-in
/// [`DEFAULT_OTP_API_URL`]. Blank at every step means "not configured" —
/// callers must treat that as a distinct case, not attempt a request against
/// an empty URL.
fn otp_api_url() -> String {
    resolve_priority(
        std::env::var("OTP_API_URL").ok(),
        crate::install::detect_install_mode()
            .env_path()
            .and_then(|p| crate::install::env_key_from_path(p, "OTP_API_URL")),
        DEFAULT_OTP_API_URL,
    )
}

/// Resolve the bearer token, same three-step priority as [`otp_api_url`],
/// env var `OTP_CLIENT_TOKEN`.
fn otp_client_token() -> String {
    resolve_priority(
        std::env::var("OTP_CLIENT_TOKEN").ok(),
        crate::install::detect_install_mode()
            .env_path()
            .and_then(|p| crate::install::env_key_from_path(p, "OTP_CLIENT_TOKEN")),
        DEFAULT_OTP_CLIENT_TOKEN,
    )
}

/// Trim, lowercase, and reject anything without an `@` that has at least one
/// character on either side. Pure and unit-tested — the Worker does its own
/// stricter validation server-side; this only avoids sending obviously
/// malformed input and normalizes casing so `Foo@Bar.com` and `foo@bar.com`
/// hit the same KV rate-limit bucket (keyed on `sha256(normalize(email))`
/// Worker-side).
fn normalize_email(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    let at = lower.find('@')?;
    if at == 0 || at == lower.len() - 1 {
        return None;
    }
    Some(lower)
}

/// Build `/otp/capture`'s request body. `previous_email` is whatever
/// [`crate::commands::account::read_account_email`] returned at call time —
/// serialized as `null` when absent, which the Worker's `normalizeEmail`
/// treats as "no prior email" (a first-time sign-up). Pure and unit-tested
/// separately from the network call it feeds.
fn build_capture_body(email: &str, previous_email: &Option<String>) -> serde_json::Value {
    serde_json::json!({ "email": email, "previousEmail": previous_email })
}

/// Best-effort notify the team (via the Worker's `/otp/capture`, which
/// forwards to Resend — see `infra/otp-worker/src/resend.ts`) that `email`
/// was just captured or changed. Returns `Err` on any failure (not
/// configured, network error, non-2xx), but **the caller must treat this as
/// informational only** — never a reason to withhold or roll back a local
/// [`crate::commands::account::save_account_email`] call. Includes whatever
/// [`crate::commands::account::read_account_email`] returns as
/// `previousEmail`, purely so the Worker can tell a first-time sign-up apart
/// from a "Change email" — reading it here (rather than having the frontend
/// pass it) keeps the contract simple and means a caller can't forget it or
/// spoof a different value.
#[tauri::command]
#[tracing::instrument(skip(email), err)]
pub async fn capture_account_email(email: String) -> Result<(), String> {
    let email = normalize_email(&email).ok_or("invalid_email")?;
    let base = otp_api_url();
    if base.is_empty() {
        tracing::info!("capture_account_email: no OTP_API_URL configured — not_configured");
        return Err("not_configured".into());
    }
    let previous_email = crate::commands::account::read_account_email();
    let resp = reqwest::Client::new()
        .post(format!("{base}/otp/capture"))
        .bearer_auth(otp_client_token())
        .json(&build_capture_body(&email, &previous_email))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "capture_account_email: request failed");
            format!("request_failed:{e}")
        })?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("unexpected_status:{}", status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_email_trims_and_lowercases() {
        assert_eq!(
            normalize_email("  Foo@Bar.COM  "),
            Some("foo@bar.com".to_string())
        );
    }

    #[test]
    fn normalize_email_rejects_empty_or_missing_at() {
        assert_eq!(normalize_email(""), None);
        assert_eq!(normalize_email("   "), None);
        assert_eq!(normalize_email("not-an-email"), None);
    }

    #[test]
    fn normalize_email_rejects_at_on_either_edge() {
        assert_eq!(normalize_email("@bar.com"), None);
        assert_eq!(normalize_email("foo@"), None);
    }

    #[test]
    fn normalize_email_accepts_a_minimal_valid_address() {
        assert_eq!(normalize_email("a@b"), Some("a@b".to_string()));
    }

    /// Priority order: process env wins over dotenv, dotenv wins over default.
    #[test]
    fn resolve_priority_prefers_process_env_over_dotenv_over_default() {
        assert_eq!(
            resolve_priority(Some("process".into()), Some("dotenv".into()), "default"),
            "process"
        );
        assert_eq!(
            resolve_priority(None, Some("dotenv".into()), "default"),
            "dotenv"
        );
        assert_eq!(resolve_priority(None, None, "default"), "default");
    }

    /// Whitespace-only values at any step count as absent — a `.env` line left
    /// as `OTP_API_URL= ` must not shadow a real default, mirroring the old
    /// `clerk_publishable_key`'s treatment of the same case.
    #[test]
    fn resolve_priority_treats_blank_values_as_absent() {
        assert_eq!(
            resolve_priority(Some("   ".into()), Some("dotenv".into()), "default"),
            "dotenv"
        );
        assert_eq!(
            resolve_priority(Some("\t\n".into()), Some("  ".into()), "default"),
            "default"
        );
    }

    #[test]
    fn build_capture_body_includes_previous_email_when_present() {
        let body = build_capture_body("new@example.com", &Some("old@example.com".to_string()));
        assert_eq!(body["email"], "new@example.com");
        assert_eq!(body["previousEmail"], "old@example.com");
    }

    /// `None` must serialize as JSON `null`, not be omitted — the Worker's
    /// `handleCapture` reads `body.previousEmail` unconditionally and treats a
    /// missing/non-string value as "no prior email" either way, but pinning
    /// the exact shape here catches an accidental switch to `skip_serializing_if`.
    #[test]
    fn build_capture_body_serializes_absent_previous_email_as_null() {
        let body = build_capture_body("new@example.com", &None);
        assert!(body["previousEmail"].is_null());
    }
}
