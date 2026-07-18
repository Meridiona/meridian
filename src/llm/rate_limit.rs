//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Proactive pacing for metered endpoints — spend the quota slowly enough that the 429
//! never happens.
//!
//! # Why this exists alongside the backoff that was already here
//! [`super::resolver`] already handles a rate limit REACTIVELY: eat the 429, park the
//! backend, fall back to the on-device model. That is the right backstop and it stays. But
//! it is strictly after-the-fact — something must fail before anything slows down, and on a
//! free tier the first thing that fails is the endpoint probe, which fires up to
//! `schemas × rungs` (16) requests back-to-back. A 15-RPM key cannot survive that, so a
//! free-tier endpoint could never finish being measured, and an unmeasured endpoint is
//! refused by the production gate ([`meridian_core::CustomLlmProvider::effective_rung`]).
//! The endpoint was usable; we just never let it prove it. This module is what makes that
//! first probe complete.
//!
//! # Custom endpoints only
//! `rpm` is a field on a [`meridian_core::CustomLlmProvider`] row and nothing else has one.
//! The four CLI providers are subprocesses on the user's own flat-rate subscription — a 429
//! there means "your plan is exhausted for the next few hours", which is a different
//! resource with a different recovery time, already handled by the per-backend backoff. The
//! on-device model has no quota at all, only a GPU permit ([`crate::llm_gate`]). Pacing
//! those would serialise work for nothing.
//!
//! # Even spacing, not a burst bucket
//! A classic token bucket refills to a burst allowance, which is exactly the shape that
//! trips a small RPM: 15 tokens banked over a quiet hour still leave in one instant, and
//! providers measure a sliding window, not our intentions. So each call reserves the NEXT
//! evenly-spaced slot (`60 / rpm` apart) and sleeps until it. Slower in the best case,
//! but the best case was never the problem.
//!
//! # What this does NOT do
//! - **No token (TPM) pacing.** Most free tiers cap tokens-per-minute too, and on long
//!   worklog prompts that often binds first. Pacing it needs a token estimate before the
//!   call, which we do not have; until then TPM is handled reactively from the
//!   `x-ratelimit-reset-tokens` header (see [`super::openai_compat`]).
//! - **No cross-process coordination.** The reservations live in this process's memory, so
//!   two daemons sharing one key each pace to `rpm` and together spend `2 × rpm`. This
//!   matches the existing in-memory backoff's limitation and is accepted for now; the fix
//!   would be a file lock, as the Jira OAuth refresh race needed.
//! - **No adaptive learning.** A 429 does not shrink the configured rate. The reactive
//!   backoff is the backstop for a wrong `rpm`; inferring the real one would first have to
//!   disambiguate a mis-configured ceiling from a shared key from a TPM cap.
//!
//! # Who calls this
//! [`super::resolver::complete_inner`] before a [`meridian_core::LlmProvider::Custom`] call,
//! and [`super::probe`] between attempts. The LLM Lab deliberately does NOT call it: it
//! measures rate limits as data, and pacing would corrupt the measurement.
//!
//! # Related
//! - [`super::resolver`] — the reactive half: backoff, fallback, and the priority chain that
//!   turns a `Retry-After` header into a park duration.
//! - [`meridian_core::CustomLlmProvider::rpm`] — where the ceiling is configured.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// When the next request against a given backend may be sent, keyed by backend IDENTITY —
/// the same [`super::resolver::backoff_key`] the reactive backoff uses, so two custom
/// endpoints pace independently and neither is slowed by the other's ceiling.
///
/// In-memory and deliberately not persisted: a reservation is meaningless across a restart
/// (the window it referred to has long since rolled), and writing pacing state to
/// `settings.json` would put a transient in a file the user edits.
static NEXT_SLOT: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());

/// The pacing/backoff key for a custom endpoint id.
///
/// Defined HERE and used by [`super::resolver::backoff_key`] too, so the proactive and
/// reactive maps can never disagree about what identifies a backend — if they did, an
/// endpoint could be paced under one name and parked under another.
pub fn custom_key(id: &str) -> String {
    format!("custom:{id}")
}

/// Reserve the next slot for `key` and sleep until it is due.
///
/// `rpm` of `0` means unmetered — returns immediately, having reserved nothing. This is the
/// default for every endpoint, so pacing is strictly opt-in and an unconfigured setup
/// behaves exactly as it did before.
///
/// Returns `false` when the wait would exceed `max_wait`, in which case **no slot is
/// consumed** and the caller proceeds unpaced. That matters: a user-driven call (drafting a
/// task, synthesising a worklog) must not hang for a minute behind a background job, and
/// the reactive backoff is still there to catch it if it does 429. Pass `None` for a
/// background caller that would rather wait than fail — the probe, notably, where waiting is
/// the entire point.
///
/// # Why the lock is released before sleeping
/// The reservation is computed and committed under the lock, then the guard is dropped and
/// the sleep happens outside it. Holding a `std::sync::Mutex` across an `.await` would both
/// risk a deadlock and serialise every other endpoint behind this one's wait.
pub async fn acquire(key: &str, rpm: u32, max_wait: Option<Duration>) -> bool {
    if rpm == 0 {
        return true;
    }
    let interval = Duration::from_secs_f64(60.0 / f64::from(rpm));

    let wait = {
        let mut guard = NEXT_SLOT.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        // A key that has never been used, or whose last slot is in the past, goes now.
        let slot = guard.get(key).copied().filter(|t| *t > now).unwrap_or(now);
        let wait = slot.saturating_duration_since(now);
        if max_wait.is_some_and(|m| wait > m) {
            // Refuse WITHOUT committing, so a caller that declined to wait does not leave a
            // reservation behind that would delay the next caller for a request never sent.
            tracing::debug!(
                key,
                wait_s = wait.as_secs_f64(),
                "llm rate limit: slot too far out, proceeding unpaced"
            );
            return false;
        }
        guard.insert(key.to_string(), slot + interval);
        wait
    };

    if !wait.is_zero() {
        tracing::debug!(
            key,
            wait_s = wait.as_secs_f64(),
            rpm,
            "llm rate limit: pacing"
        );
        tokio::time::sleep(wait).await;
    }
    true
}

/// Drop the pacing state for a backend. Called when an endpoint is removed or its `rpm`
/// changes, so a stale reservation from the old ceiling cannot delay the first call under
/// the new one.
pub fn forget(key: &str) {
    NEXT_SLOT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(key);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0` is the default on every row, so this is the path almost every user takes.
    #[tokio::test]
    async fn zero_rpm_never_waits() {
        let t0 = Instant::now();
        assert!(acquire("unmetered", 0, None).await);
        assert!(acquire("unmetered", 0, None).await);
        assert!(t0.elapsed() < Duration::from_millis(50));
    }

    /// The first call against a fresh key must not pay for a slot nobody used.
    #[tokio::test]
    async fn first_call_is_immediate() {
        let t0 = Instant::now();
        assert!(acquire("fresh-key", 60, None).await);
        assert!(t0.elapsed() < Duration::from_millis(50));
    }

    /// The actual promise: N calls occupy N-1 intervals. At 600 rpm the interval is 100ms,
    /// so three calls must take at least 200ms — the burst is genuinely spread.
    #[tokio::test]
    async fn consecutive_calls_are_spaced() {
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(acquire("spaced-key", 600, None).await);
        }
        assert!(
            t0.elapsed() >= Duration::from_millis(200),
            "3 calls at 600rpm should span >=2 intervals, took {:?}",
            t0.elapsed()
        );
    }

    /// Two endpoints must not pace each other — one on a slow free tier cannot be allowed to
    /// stall an unrelated fast one.
    #[tokio::test]
    async fn keys_are_independent() {
        assert!(acquire("slow-endpoint", 6, None).await);
        let t0 = Instant::now();
        assert!(acquire("fast-endpoint", 6, None).await);
        assert!(
            t0.elapsed() < Duration::from_millis(50),
            "a different key should not inherit another's reservation"
        );
    }

    /// A declined wait must leave no reservation behind, or a user-driven call that chose not
    /// to wait would still push the next background call out by a full interval.
    #[tokio::test]
    async fn refusing_to_wait_consumes_no_slot() {
        // Occupy the slot, pushing the next one a full 10s out (6 rpm).
        assert!(acquire("impatient-key", 6, None).await);
        // Declines — 10s is well over the cap.
        assert!(!acquire("impatient-key", 6, Some(Duration::from_millis(1))).await);
        // The refusal did not move the slot, so this still sees the ORIGINAL reservation.
        assert!(!acquire("impatient-key", 6, Some(Duration::from_millis(1))).await);
    }

    #[tokio::test]
    async fn forget_clears_the_reservation() {
        assert!(acquire("forgettable", 6, None).await);
        forget("forgettable");
        let t0 = Instant::now();
        assert!(acquire("forgettable", 6, None).await);
        assert!(t0.elapsed() < Duration::from_millis(50));
    }
}
