//! Bounded retry with linear backoff for transient, self-clearing failures —
//! chiefly Windows **os error 32** ("used by another process"), where an
//! antivirus scan, the Search Indexer, or a not-yet-released file handle briefly
//! blocks a rename/overwrite that would succeed a moment later.
//!
//! Two variants of one schedule (`base_delay × attempt` between tries, the final
//! attempt's error surfaced): [`retry_transient`] for async call sites
//! (`tokio::fs`, the tray's backend-binary install) and
//! [`retry_transient_blocking`] for sync ones (`std::fs`, the encrypt-in-place
//! file swap in [`crate::db_crypto`]). Both live here so the daemon and the
//! Tauri tray share a single implementation rather than each carrying its own
//! copy — the shape recurred across the Windows sync-failure fixes and earned a
//! home in the shared crate.
//!
//! On non-Windows platforms these are effectively inert: the guarded ops don't
//! hit transient sharing violations there, so the first attempt always succeeds.
//! Keeping them platform-neutral means the logic compiles and is unit-tested on
//! CI (macOS) — the one platform CI runs tests on — even though the failures
//! they absorb are Windows-specific.

use std::time::Duration;

/// Retry an async fallible operation up to `attempts` times with linear backoff
/// (`base_delay × the attempt number` slept between tries), surfacing the last
/// error only after every attempt fails. `attempts == 1` means a single try with
/// no retry; a first-attempt success returns immediately without sleeping.
///
/// See the [module docs](self) for the transient-failure class this exists for.
pub async fn retry_transient<F, Fut, T, E>(
    attempts: u32,
    base_delay: Duration,
    mut op: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt = 1;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt >= attempts => return Err(e),
            Err(_) => {
                tokio::time::sleep(base_delay * attempt).await;
                attempt += 1;
            }
        }
    }
}

/// Synchronous counterpart to [`retry_transient`]: same schedule, but
/// `std::thread::sleep` instead of `tokio::time::sleep`, for blocking call sites
/// with no async runtime on the path (e.g. the encrypt-in-place file swap
/// reached from the tray's `block_on(migrate)` startup gate). `attempts == 1`
/// means a single try with no retry; a first-attempt success returns immediately
/// without sleeping.
pub fn retry_transient_blocking<F, T, E>(
    attempts: u32,
    base_delay: Duration,
    mut op: F,
) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
{
    let mut attempt = 1;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if attempt >= attempts => return Err(e),
            Err(_) => {
                std::thread::sleep(base_delay * attempt);
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── async: retry_transient ──────────────────────────────────────────────
    // Deterministic (zero-delay backoff, call-counting fakes) — no real timing.

    /// A first-attempt success returns straight through and never re-invokes the
    /// operation — the common, fast path.
    #[tokio::test]
    async fn retry_transient_returns_on_first_success_without_retrying() {
        let calls = AtomicU32::new(0);
        let out: Result<u32, ()> = retry_transient(5, Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(7) }
        })
        .await;
        assert_eq!(out, Ok(7));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a first-attempt success must not retry"
        );
    }

    /// Retries across transient failures and returns the eventual success — the
    /// shape of an AV/indexer hold that lets go after a moment.
    #[tokio::test]
    async fn retry_transient_recovers_after_transient_failures() {
        let calls = AtomicU32::new(0);
        let out: Result<&str, &str> = retry_transient(5, Duration::ZERO, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 3 {
                    Err("locked")
                } else {
                    Ok("done")
                }
            }
        })
        .await;
        assert_eq!(out, Ok("done"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "three failures, then success on the fourth try"
        );
    }

    /// A file locked for every attempt surfaces the last error (not swallowed)
    /// after exactly `attempts` tries — so a genuinely stuck file still fails.
    #[tokio::test]
    async fn retry_transient_surfaces_error_after_exhausting_attempts() {
        let calls = AtomicU32::new(0);
        let out: Result<(), &str> = retry_transient(4, Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err("still locked") }
        })
        .await;
        assert_eq!(out, Err("still locked"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "exactly `attempts` calls — no more, no fewer"
        );
    }

    /// `attempts == 1` means "try once, no retry" — the degenerate bound must not
    /// off-by-one into zero or an extra attempt.
    #[tokio::test]
    async fn retry_transient_with_single_attempt_does_not_retry() {
        let calls = AtomicU32::new(0);
        let out: Result<(), &str> = retry_transient(1, Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err("locked") }
        })
        .await;
        assert_eq!(out, Err("locked"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ── sync: retry_transient_blocking ──────────────────────────────────────
    // The same four properties for the blocking variant (db_crypto's path).

    #[test]
    fn retry_transient_blocking_returns_on_first_success_without_retrying() {
        let mut calls = 0u32;
        let out: Result<u32, ()> = retry_transient_blocking(5, Duration::ZERO, || {
            calls += 1;
            Ok(7)
        });
        assert_eq!(out, Ok(7));
        assert_eq!(calls, 1, "a first-attempt success must not retry");
    }

    #[test]
    fn retry_transient_blocking_recovers_after_transient_failures() {
        let mut calls = 0u32;
        let out: Result<&str, &str> = retry_transient_blocking(5, Duration::ZERO, || {
            calls += 1;
            if calls <= 3 {
                Err("locked")
            } else {
                Ok("done")
            }
        });
        assert_eq!(out, Ok("done"));
        assert_eq!(calls, 4, "three failures, then success on the fourth try");
    }

    #[test]
    fn retry_transient_blocking_surfaces_error_after_exhausting_attempts() {
        let mut calls = 0u32;
        let out: Result<(), &str> = retry_transient_blocking(4, Duration::ZERO, || {
            calls += 1;
            Err("still locked")
        });
        assert_eq!(out, Err("still locked"));
        assert_eq!(calls, 4, "exactly `attempts` calls — no more, no fewer");
    }

    #[test]
    fn retry_transient_blocking_with_single_attempt_does_not_retry() {
        let mut calls = 0u32;
        let out: Result<(), &str> = retry_transient_blocking(1, Duration::ZERO, || {
            calls += 1;
            Err("locked")
        });
        assert_eq!(out, Err("locked"));
        assert_eq!(calls, 1);
    }
}
