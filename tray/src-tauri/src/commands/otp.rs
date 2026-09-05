//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! One-time email code capture — the OTP Worker client.
//!
//! # What this is
//! Two commands backing the setup wizard's Email step
//! (`ui/app/setup/signin/OtpForm.tsx`) and Settings → Account's "Change email"
//! control (`AccountAuthControl.tsx`): send a 6-digit code to an address via
//! `infra/otp-worker`'s `POST /otp/send`, then verify it via
//! `POST /otp/verify`. Neither command persists anything — [`confirm_account_otp`]
//! returns `Ok(true)`/`Ok(false)` only; the frontend calls
//! `commands::account::save_account_email` itself after a `true` result,
//! keeping that function's existing contract (idempotent plain-overwrite
//! write) untouched. There is no session, no token, no expiry to manage here
//! — the Worker is stateless from this client's point of view.
//!
//! # Error taxonomy
//! Both commands return `Err(String)` with one of a small set of stable
//! sentinel strings the frontend switches on for distinct copy (see
//! `ui/lib/otp-errors.ts`), rather than a free-form message:
//! `"not_configured"` (no Worker URL baked in or set — the fresh-clone dev
//! case), `"invalid_email"`/`"invalid_input"`, `"unauthorized"` (the
//! attestation bearer token was rejected — a build/config problem, not a
//! user-fixable one), `"blocked"` (a Turnstile challenge attached to the
//! request failed — unreachable today since this client doesn't send a
//! token yet, kept for when it does), `"rate_limited"`, `"unavailable"` (the
//! Worker/SES is down), and `"expired"` (the code was never sent, has
//! expired, or its 5-attempt verify cap was hit — the Worker deliberately
//! returns the same `410` for all three so a caller can never distinguish
//! "you ran out of guesses" from "there was nothing to guess," see
//! `infra/otp-worker/src/otp.ts`'s `VerifyOutcome` doc). Anything else
//! (a transport failure, or a status code outside this contract) carries a
//! diagnostic message but maps to the same generic "something went wrong"
//! copy client-side.
//!
//! # Verify response body
//! `/otp/verify`'s `200` response is NOT itself proof of a correct code — it
//! means "the request was well-formed and a live code exists," and carries
//! `{ ok: true, verified: bool, attemptsRemaining? }` in the body. A wrong
//! (but well-formed) guess is `200 { verified: false }`, not an HTTP error —
//! see `infra/otp-worker/src/index.ts`'s `handleVerify`. [`confirm_account_otp`]
//! parses this body rather than trusting the status code alone; treating a
//! bare `200` as "verified" would accept ANY well-formed code as correct.
//!
//! # Who calls this
//! - [`request_account_otp`] / [`confirm_account_otp`]: `OtpForm.tsx` (the
//!   wizard's Email step, and inline in `AccountAuthControl.tsx`'s
//!   "Change email").
//!
//! # Related
//! - `crate::commands::account` — `save_account_email`, called by the
//!   frontend on a `true` verify result, never by this module.
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

/// Map `/otp/send`'s response status to this command's outcome. Split out
/// from [`request_account_otp`] so the mapping itself is unit-testable
/// without a network call. `403` is a failed Turnstile challenge
/// (`infra/otp-worker/src/index.ts`'s `handleSend`) — distinct from `401`'s
/// "the bearer token itself was rejected" — kept as its own sentinel even
/// though this client doesn't attach a Turnstile token yet, so the mapping
/// stays correct the day it does.
fn map_send_status(status: u16) -> Result<(), String> {
    match status {
        200..=299 => Ok(()),
        400 => Err("invalid_email".into()),
        401 => Err("unauthorized".into()),
        403 => Err("blocked".into()),
        429 => Err("rate_limited".into()),
        503 => Err("unavailable".into()),
        other => Err(format!("unexpected_status:{other}")),
    }
}

/// Body shape of a `200` response from `/otp/verify`
/// (`infra/otp-worker/src/index.ts`'s `handleVerify`/`ok({verified, ...})`).
/// `attempts_remaining` is available but not surfaced by this command today —
/// see the module doc.
#[derive(serde::Deserialize)]
struct VerifyResponseBody {
    verified: bool,
}

/// Map `/otp/verify`'s response to this command's outcome. A `200` status is
/// NOT itself "verified" — see the module doc's "Verify response body"
/// section — so this parses `body` rather than trusting the status code
/// alone. A wrong-but-well-formed code is `200 { verified: false }`, decoded
/// here as `Ok(false)`: an expected, retryable outcome the frontend already
/// has copy for, never an `Err`. `410` covers both "the 5-attempt cap was
/// hit" and "no live code for this email" (never sent, or expired) —
/// deliberately indistinguishable Worker-side, so both map to `"expired"`.
fn map_verify_status(status: u16, body: &str) -> Result<bool, String> {
    match status {
        200 => serde_json::from_str::<VerifyResponseBody>(body)
            .map(|b| b.verified)
            .map_err(|e| format!("malformed_response:{e}")),
        400 => Err("invalid_input".into()),
        401 => Err("unauthorized".into()),
        410 => Err("expired".into()),
        429 => Err("rate_limited".into()),
        503 => Err("unavailable".into()),
        other => Err(format!("unexpected_status:{other}")),
    }
}

/// Send a fresh 6-digit code to `email` via the Worker's `/otp/send`. Returns
/// `Err("not_configured")` immediately (no network call) when no Worker URL
/// is resolved — see the module doc's error taxonomy, and `OtpForm.tsx`'s
/// handling of that specific case (a dev-notice, not a user-facing failure).
#[tauri::command]
#[tracing::instrument(skip(email), err)]
pub async fn request_account_otp(email: String) -> Result<(), String> {
    let email = normalize_email(&email).ok_or("invalid_email")?;
    let base = otp_api_url();
    if base.is_empty() {
        tracing::info!("request_account_otp: no OTP_API_URL configured — not_configured");
        return Err("not_configured".into());
    }
    let resp = reqwest::Client::new()
        .post(format!("{base}/otp/send"))
        .bearer_auth(otp_client_token())
        .json(&serde_json::json!({ "email": email }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "request_account_otp: request failed");
            format!("request_failed:{e}")
        })?;
    map_send_status(resp.status().as_u16())
}

/// Verify `code` for `email` via the Worker's `/otp/verify`. `Ok(true)` means
/// verified — the frontend is responsible for calling
/// `commands::account::save_account_email` next, this command does not.
/// `Ok(false)` means the code was wrong but the attempt was otherwise valid
/// (retryable). Returns `Err("not_configured")` immediately when no Worker
/// URL is resolved, same as [`request_account_otp`].
#[tauri::command]
#[tracing::instrument(skip(email, code), err)]
pub async fn confirm_account_otp(email: String, code: String) -> Result<bool, String> {
    let email = normalize_email(&email).ok_or("invalid_email")?;
    let code = code.trim();
    if code.is_empty() {
        return Err("invalid_input".into());
    }
    let base = otp_api_url();
    if base.is_empty() {
        tracing::info!("confirm_account_otp: no OTP_API_URL configured — not_configured");
        return Err("not_configured".into());
    }
    let resp = reqwest::Client::new()
        .post(format!("{base}/otp/verify"))
        .bearer_auth(otp_client_token())
        .json(&serde_json::json!({ "email": email, "code": code }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "confirm_account_otp: request failed");
            format!("request_failed:{e}")
        })?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    map_verify_status(status, &body)
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
    fn map_send_status_matches_the_documented_contract() {
        assert_eq!(map_send_status(200), Ok(()));
        assert_eq!(map_send_status(204), Ok(()));
        assert_eq!(map_send_status(400), Err("invalid_email".to_string()));
        assert_eq!(map_send_status(401), Err("unauthorized".to_string()));
        assert_eq!(map_send_status(403), Err("blocked".to_string()));
        assert_eq!(map_send_status(429), Err("rate_limited".to_string()));
        assert_eq!(map_send_status(503), Err("unavailable".to_string()));
        assert!(map_send_status(500).is_err());
    }

    /// The bug this pins: a `200` status is NOT itself "verified" — the
    /// Worker returns `200 { verified: false }` for a wrong-but-well-formed
    /// code (`infra/otp-worker/src/index.ts`'s `handleVerify`). Trusting the
    /// status code alone would accept ANY well-formed code as correct.
    #[test]
    fn map_verify_status_reads_the_body_not_just_the_status() {
        assert_eq!(
            map_verify_status(200, r#"{"ok":true,"verified":true}"#),
            Ok(true)
        );
        assert_eq!(
            map_verify_status(200, r#"{"ok":true,"verified":false,"attemptsRemaining":4}"#),
            Ok(false)
        );
    }

    #[test]
    fn map_verify_status_rejects_a_malformed_200_body() {
        assert!(map_verify_status(200, "not json").is_err());
        assert!(map_verify_status(200, "{}").is_err());
    }

    #[test]
    fn map_verify_status_matches_the_documented_contract() {
        assert_eq!(map_verify_status(400, ""), Err("invalid_input".to_string()));
        assert_eq!(map_verify_status(401, ""), Err("unauthorized".to_string()));
        // 410 covers BOTH the 5-attempt cap and "no live code for this
        // email" — the Worker deliberately makes these indistinguishable.
        assert_eq!(map_verify_status(410, ""), Err("expired".to_string()));
        assert_eq!(map_verify_status(429, ""), Err("rate_limited".to_string()));
        assert_eq!(map_verify_status(503, ""), Err("unavailable".to_string()));
        assert!(map_verify_status(500, "").is_err());
    }
}
