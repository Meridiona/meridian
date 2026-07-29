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
//!
//! Each retried attempt emits a `debug`-level trace (`attempt`, `attempts`)
//! before backing off, so a live `meridian logs` tail shows the retry cadence —
//! the diagnosability this failure class needs, since it can only be reproduced
//! on a real Windows box, never in CI.

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
                tracing::debug!(
                    attempt,
                    attempts,
                    "retry: transient failure, backing off before the next attempt"
                );
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
                tracing::debug!(
                    attempt,
                    attempts,
                    "retry: transient failure, backing off before the next attempt"
                );
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

    // ── boundary + schedule (both variants) ─────────────────────────────────
    // The cases the four-property pair above doesn't pin: success landing on
    // the *last allowed* attempt (off-by-one either way), which error is
    // surfaced when they differ, the `attempts == 0` degenerate floor, and that
    // the backoff is actually linear (`base × attempt`), not constant.

    /// Success on exactly the final allowed attempt returns `Ok` — the retry
    /// must not give up one attempt early. Distinct from the "recovers" test,
    /// which has slack (5 attempts, succeeds on the 4th); here `attempts == 3`
    /// and success is on the 3rd, so an off-by-one would wrongly return `Err`.
    #[tokio::test]
    async fn retry_transient_succeeds_on_the_final_allowed_attempt() {
        let calls = AtomicU32::new(0);
        let out: Result<&str, ()> = retry_transient(3, Duration::ZERO, || {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if n < 3 {
                    Err(())
                } else {
                    Ok("made it")
                }
            }
        })
        .await;
        assert_eq!(out, Ok("made it"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "no extra attempt past the success"
        );
    }

    /// When every attempt fails with a *different* error, the one surfaced is
    /// the final attempt's — not the first, not an arbitrary one.
    #[tokio::test]
    async fn retry_transient_returns_the_last_error_not_the_first() {
        let calls = AtomicU32::new(0);
        let out: Result<(), u32> = retry_transient(4, Duration::ZERO, || {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move { Err(n) }
        })
        .await;
        assert_eq!(
            out,
            Err(4),
            "the fourth (final) attempt's error is surfaced"
        );
    }

    /// `attempts == 0` degenerates to a single try, never zero — the operation
    /// is always run at least once (a "retry 0 times" caller still gets one go).
    #[tokio::test]
    async fn retry_transient_with_zero_attempts_still_makes_one_call() {
        let calls = AtomicU32::new(0);
        let out: Result<(), &str> = retry_transient(0, Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err("x") }
        })
        .await;
        assert_eq!(out, Err("x"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "attempts==0 still runs the op once"
        );
    }

    /// The backoff is linear (`base × attempt`), not constant: with all attempts
    /// failing, the total time slept is `base × (1 + 2 + … + (attempts-1))`.
    /// Uses tokio's paused clock so this is exact and instant — no real waiting.
    #[tokio::test(start_paused = true)]
    async fn retry_transient_uses_linear_backoff_schedule() {
        let base = Duration::from_millis(100);
        let start = tokio::time::Instant::now();
        let out: Result<(), ()> = retry_transient(4, base, || async { Err(()) }).await;
        let elapsed = start.elapsed();
        assert_eq!(out, Err(()));
        // Sleeps after attempts 1, 2, 3 (attempt 4 is final, no sleep): 100+200+300.
        assert_eq!(
            elapsed,
            base + base * 2 + base * 3,
            "linear backoff: base×1 + base×2 + base×3 = 600ms of virtual time"
        );
    }

    #[test]
    fn retry_transient_blocking_succeeds_on_the_final_allowed_attempt() {
        let mut calls = 0u32;
        let out: Result<&str, ()> = retry_transient_blocking(3, Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                Err(())
            } else {
                Ok("made it")
            }
        });
        assert_eq!(out, Ok("made it"));
        assert_eq!(calls, 3, "no extra attempt past the success");
    }

    #[test]
    fn retry_transient_blocking_returns_the_last_error_not_the_first() {
        let mut calls = 0u32;
        let out: Result<(), u32> = retry_transient_blocking(4, Duration::ZERO, || {
            calls += 1;
            Err(calls)
        });
        assert_eq!(
            out,
            Err(4),
            "the fourth (final) attempt's error is surfaced"
        );
    }

    #[test]
    fn retry_transient_blocking_with_zero_attempts_still_makes_one_call() {
        let mut calls = 0u32;
        let out: Result<(), &str> = retry_transient_blocking(0, Duration::ZERO, || {
            calls += 1;
            Err("x")
        });
        assert_eq!(out, Err("x"));
        assert_eq!(calls, 1, "attempts==0 still runs the op once");
    }

    /// The blocking variant sleeps for real, so verify the linear schedule via
    /// elapsed wall-clock. Small `base` keeps it quick; asserting a *lower*
    /// bound (`std::thread::sleep` only ever oversleeps) keeps it non-flaky,
    /// while the sum still distinguishes linear (5+10+15=30ms) from constant
    /// (5+5+5=15ms).
    #[test]
    fn retry_transient_blocking_uses_linear_backoff_schedule() {
        let base = Duration::from_millis(5);
        let start = std::time::Instant::now();
        let out: Result<(), ()> = retry_transient_blocking(4, base, || Err(()));
        let elapsed = start.elapsed();
        assert_eq!(out, Err(()));
        assert!(
            elapsed >= base + base * 2 + base * 3,
            "linear backoff sleeps at least base×1 + base×2 + base×3 (30ms), got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "sanity upper bound, got {elapsed:?}"
        );
    }
}
