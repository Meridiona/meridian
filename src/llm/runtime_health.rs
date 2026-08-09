//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Runtime LLM provider health — what the pipeline ACTUALLY observed on real calls,
//! persisted so the dashboard's "provider unavailable" banner can see it.
//!
//! [`crate::llm::detect`]'s banner ladder reads two inputs: is the CLI on PATH, and what did
//! the last **manual** Settings → Test say. Neither notices the state that actually stalls
//! the product — a provider that is installed, tested fine an hour ago, and is now failing
//! every real call. In that state [`crate::worklog_pipeline`] leaves each hour pending, the
//! day timeline stays empty, and the banner reports "fine" because nothing ever wrote a
//! failure down. This module is the missing feedback edge.
//!
//! The record lives on disk because the two halves are in DIFFERENT PROCESSES: the daemon
//! makes the LLM calls, the tray computes the banner. An in-memory counter (like
//! [`crate::llm::resolver`]'s rate-limit backoff map) is invisible to the tray by
//! construction, which is precisely why the existing backoff state never reached the banner.
//!
//! Deliberately a SEPARATE file from `provider_test_cache.json`: that file's `tested_at` is
//! rendered as "Verified 3m ago" in Settings (`ui/components/LlmProviderDetail.tsx:111`), so
//! stamping it from a background pipeline call would claim a connectivity test the user never
//! ran. Keeping them apart lets [`most_recent_outcome`] prefer whichever signal is fresher
//! without either one lying about its own provenance.
//!
//! # Who calls this
//! - Writers: [`crate::llm::resolver`]'s call path — [`record_success`] when a provider
//!   answers, [`record_failure`] when it errors or rate-limits.
//! - Reader: [`crate::llm::detect::in_use_provider_health`], via [`most_recent_outcome`].
//!
//! # Related
//! - [`crate::llm::detect`] — the banner ladder (`classify_provider_health`) and the
//!   manual-test cache this merges with.
//! - [`crate::llm::resolver`] — the call path that produces these observations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::detect::{ProviderTestOutcome, ProviderTestResult};
use super::LlmError;
use meridian_core::LlmProvider;

/// How many CONSECUTIVE hard failures a provider must rack up before the banner alarms.
///
/// Not 1, deliberately. [`crate::llm::resolver`] already retries a `Failed` once inside a
/// single call, so one recorded failure means two attempts died — but a provider CLI can
/// still die once on a transient (a laptop resuming, a flaky socket) and recover on the next
/// tick. Alarming there trains the user to ignore the banner, which costs more than the delay
/// does. At the hourly pipeline's cadence (~3 calls/hour: two for the report, one for the
/// fold) a genuinely broken provider still trips this inside one hour.
///
/// A rate limit is NOT subject to this — see [`record_failure`].
const FAILURE_STREAK_THRESHOLD: u32 = 3;

/// How long a runtime observation stays relevant to the banner.
///
/// Bounds the one case the write path can't refresh: calls STOP (overnight, or a paused
/// daemon) right after a failure. Without a horizon the banner would still be alarming at
/// 09:00 about a 02:00 failure that nothing has had a chance to disprove. Comfortably longer
/// than the pipeline's hourly cadence, so an active daemon always re-stamps the record long
/// before it expires — expiry only ever fires on genuine inactivity.
const OBSERVATION_MAX_AGE_HOURS: i64 = 6;

/// One provider outcome as observed on a REAL pipeline call (never a manual test).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeObservation {
    /// Provider wire id, e.g. `claude` — matches [`LlmProvider::as_str`].
    pub id: String,
    /// What happened. Shares [`ProviderTestOutcome`] with the manual-test cache so
    /// `classify_provider_health`'s ladder needs no new variant to reason about.
    pub outcome: ProviderTestOutcome,
    /// RFC3339. When the daemon observed this — the recency key [`most_recent_outcome`]
    /// compares against the manual test's `tested_at`.
    pub observed_at: String,
}

/// Consecutive hard-failure counts per provider, for [`FAILURE_STREAK_THRESHOLD`].
///
/// In-process only, and that is correct: the streak is a property of one daemon's run of
/// calls, and a restart legitimately re-earns the alarm. Only the DECISION it gates is
/// persisted, because only that needs to cross into the tray.
static FAILURE_STREAKS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// Path of the runtime-health record, alongside `provider_test_cache.json` in the meridian
/// home (`MERIDIAN_HOME` when set, else `~/.meridian`).
fn record_path() -> PathBuf {
    let home = std::env::var("MERIDIAN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| meridian_core::paths::home_dir_or_cwd().join(".meridian"));
    home.join("provider_runtime_health.json")
}

/// Read the whole record. A missing/corrupt/foreign-format file degrades to "nothing
/// observed" rather than failing the banner — the next call repopulates it.
///
/// ALWAYS a real disk read, deliberately: this file has exactly one writer per install (this
/// process's own [`persist`] calls), but it has readers in TWO processes (see the module
/// header - the daemon writes, the tray's `in_use_provider_health` reads via [`latest_for`]),
/// and a tray-side cache with no invalidation would go stale forever after its first read,
/// since the tray never calls `persist` to refresh one. [`record_success`]'s hot path uses
/// [`RECORD_MIRROR`] instead of this function precisely so that a daemon-only optimisation
/// can't leak into the cross-process reader.
fn load() -> HashMap<String, RuntimeObservation> {
    let Ok(raw) = std::fs::read_to_string(record_path()) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// This process's own mirror of the record, lazily populated by [`record_success`]'s first
/// call and kept in sync by every successful [`persist`] afterward.
///
/// Consulting this instead of a fresh [`load()`] is safe ONLY because this file has exactly
/// one writer for the life of the process holding it - this process itself, via `persist`
/// (called solely from `record_success`/`record_failure`, both daemon-only per the module
/// header). A single-instance daemon guard (`main.rs`) rules out a second writer racing this
/// one, so nothing outside this process can change the file between the lazy read and the
/// next `persist`. It is deliberately NOT used by [`load()`]/[`latest_for`] - see that
/// function's doc for why a reader shared with the tray must not cache this way.
static RECORD_MIRROR: Mutex<Option<HashMap<String, RuntimeObservation>>> = Mutex::new(None);

/// Serialises the read-modify-write below. Concurrent pipeline stages (the hourly report and
/// the workstream fold can overlap across hours) would otherwise lose each other's entries —
/// the same lost-update hazard `detect::persist_test_result` guards against.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Write one observation, crash-safely (temp file + atomic rename). Logged, not propagated: a
/// failed health write must never fail the LLM call that just produced the observation.
fn persist(observation: RuntimeObservation) {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load();
    all.insert(observation.id.clone(), observation);
    match meridian_core::fs_utils::atomic_write_json(&record_path(), &all) {
        Ok(()) => {
            *RECORD_MIRROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(all);
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to persist runtime provider health");
        }
    }
}

/// Bump a provider's consecutive-failure count and return the new value.
fn bump_streak(id: &str) -> u32 {
    let mut guard = FAILURE_STREAKS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let n = map.entry(id.to_string()).or_insert(0);
    *n += 1;
    *n
}

/// Clear a provider's consecutive-failure count. Returns whether it had been non-zero.
fn clear_streak(id: &str) -> bool {
    let mut guard = FAILURE_STREAKS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(id.to_string(), 0).unwrap_or(0) > 0
}

/// True when `mirror` (populated via `loader` if empty) records `id` as `Failed` or
/// `RateLimited`. Pulled out of [`record_success`] as a pure function of its inputs so the
/// "populate once, then never call `loader` again" contract - the whole point of
/// [`RECORD_MIRROR`] - is unit-testable without a real file or the global statics.
fn has_unresolved_failure(
    id: &str,
    mirror: &mut Option<HashMap<String, RuntimeObservation>>,
    loader: impl FnOnce() -> HashMap<String, RuntimeObservation>,
) -> bool {
    let map = mirror.get_or_insert_with(loader);
    matches!(
        map.get(id).map(|o| &o.outcome),
        Some(ProviderTestOutcome::Failed { .. }) | Some(ProviderTestOutcome::RateLimited { .. })
    )
}

/// Record that `provider` answered a call made specifically to re-establish truth.
///
/// The difference from [`record_success`] is that this ALWAYS writes. `record_success` is
/// on the hot path and deliberately writes only on a state change, deciding "was there
/// anything to clear?" from [`has_unresolved_failure`] — which reads the RUNTIME record and
/// nothing else. That is correct for its own callers and wrong here: the verdict a probe
/// exists to overturn usually lives in the MANUAL TEST cache (a failed Settings → Test
/// Connection), where `has_unresolved_failure` cannot see it. `record_success` would find no
/// runtime failure, early-return, write nothing, and leave the failed test as the most
/// recent word — so the probe would succeed and change nothing, and the next call would be
/// refused by the same gate that let this one through. A manual test result carries no
/// expiry, so "nothing" here means forever.
///
/// Writing unconditionally is safe precisely because the caller is rare: `resolver`'s probe
/// exemption admits at most one call per provider per interval, so this cannot become the
/// per-call disk write that `record_success`'s short circuit exists to avoid.
pub fn record_probe_success(provider: LlmProvider) {
    let id = provider.as_str().to_string();
    clear_streak(&id);
    tracing::info!(
        provider = %id,
        "llm provider answered a health probe - clearing the failed verdict"
    );
    persist(RuntimeObservation {
        id,
        outcome: ProviderTestOutcome::Ok,
        observed_at: chrono::Utc::now().to_rfc3339(),
    });
    // `persist` alone is not enough to end the outage. `in_use_provider_health` memoises its
    // verdict for `IN_USE_HEALTH_TTL` (5 min), and the gate reads that memo — so without
    // this the recovered provider would keep being refused for up to another five minutes,
    // by a cached copy of the verdict we just disproved.
    super::detect::invalidate_in_use_health();
}

/// Record that `provider` answered a real call.
///
/// Clears the failure streak and stamps an `Ok` observation, so a recovered provider drops
/// the banner on the tray's next health poll rather than waiting for the user to re-test.
/// Writes only on a state CHANGE (the first success after failures) — a healthy provider
/// making an hourly call must not rewrite the file forever. A call made to test a REFUSED
/// provider uses [`record_probe_success`] instead, which has no such short circuit.
pub fn record_success(provider: LlmProvider) {
    let id = provider.as_str().to_string();
    let was_failing = clear_streak(&id);
    // Reads [`RECORD_MIRROR`], not a fresh `load()`: this function runs on EVERY successful
    // call - the common, healthy-provider path - and a real disk read+parse there on every
    // single call was the actual hot-path cost (a `was_failing`-only short circuit still hit
    // disk every time for an already-healthy provider, which is the overwhelmingly common
    // case). The mirror costs at most one real read per process, on whichever call happens
    // to run first, and stays accurate afterward because `persist` is this file's only writer
    // and updates the mirror in lockstep with the disk.
    let stale_failure = !was_failing && {
        let mut guard = RECORD_MIRROR.lock().unwrap_or_else(|e| e.into_inner());
        has_unresolved_failure(&id, &mut guard, load)
    };
    if !was_failing && !stale_failure {
        return;
    }
    tracing::info!(
        provider = %id,
        "llm provider recovered — clearing the runtime health flag"
    );
    persist(RuntimeObservation {
        id,
        outcome: ProviderTestOutcome::Ok,
        observed_at: chrono::Utc::now().to_rfc3339(),
    });
}

/// Record that `provider` failed a real call.
///
/// Two classes, handled differently on purpose:
///
/// - **Rate limited** — recorded IMMEDIATELY. It maps to the ladder's softer "catching up"
///   flag rather than the unavailable alarm, it is a true statement about right now, and it
///   clears itself, so there is no wolf to cry.
/// - **Hard failure** — recorded only once [`FAILURE_STREAK_THRESHOLD`] consecutive failures
///   have accrued, so a single transient never raises the alarm.
///
/// A rate limit deliberately does NOT reset the hard-failure streak: being throttled is not
/// evidence that the earlier failures were resolved. Only a success (via [`record_success`])
/// clears it.
pub fn record_failure(provider: LlmProvider, err: &LlmError) {
    let id = provider.as_str().to_string();
    match err {
        LlmError::RateLimited { message, .. } => {
            persist(RuntimeObservation {
                id,
                outcome: ProviderTestOutcome::RateLimited {
                    message: message.clone(),
                },
                observed_at: chrono::Utc::now().to_rfc3339(),
            });
        }
        LlmError::Failed(message) => {
            let streak = bump_streak(&id);
            if streak < FAILURE_STREAK_THRESHOLD {
                tracing::debug!(
                    provider = %id,
                    streak,
                    threshold = FAILURE_STREAK_THRESHOLD,
                    "llm provider failed — below the runtime health threshold, not flagging yet"
                );
                return;
            }
            // WARN, not debug: this is the transition that raises the dashboard banner and
            // explains a stalled worklog pipeline. `error` is on `redact::SAFE_STRING_KEYS`,
            // so the cause survives to the central backend.
            tracing::warn!(
                provider = %id,
                streak,
                error = %message,
                "llm provider failing repeatedly — flagging it unavailable for the dashboard"
            );
            persist(RuntimeObservation {
                id,
                outcome: ProviderTestOutcome::Failed {
                    message: message.clone(),
                },
                observed_at: chrono::Utc::now().to_rfc3339(),
            });
        }
    }
}

/// The last runtime observation for `provider`, if one is on record.
pub fn latest_for(provider: LlmProvider) -> Option<RuntimeObservation> {
    load().get(provider.as_str()).cloned()
}

/// Fold the manual test result and the runtime observation into the ONE outcome the banner
/// ladder should judge: whichever is more recent.
///
/// Recency rather than severity, so the signal is symmetric — a runtime failure supersedes an
/// older passing test (the bug this module exists to fix), and a user who fixes their auth and
/// clicks Test clears the flag immediately instead of waiting for the next pipeline call.
///
/// A runtime observation older than [`OBSERVATION_MAX_AGE_HOURS`] is ignored entirely; an
/// unparseable timestamp on either side loses to the side that has a usable one.
pub fn most_recent_outcome(
    last_test: Option<&ProviderTestResult>,
    last_runtime: Option<&RuntimeObservation>,
) -> Option<ProviderTestOutcome> {
    let now = chrono::Utc::now();
    let parse = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    };

    // Tracked separately from `runtime` below so the `(None, None)` fallback can tell WHY
    // there's nothing to compare against. `runtime` collapses "garbage timestamp" and
    // "parsed fine but past `OBSERVATION_MAX_AGE_HOURS`" into the same `None` - conflating
    // them in the fallback would resurrect an expired observation just because no test
    // exists to out-rank it, defeating the whole point of the expiry.
    let runtime_timestamp_unparseable =
        last_runtime.is_some_and(|o| parse(&o.observed_at).is_none());
    let runtime = last_runtime.and_then(|o| {
        let at = parse(&o.observed_at)?;
        let age = now.signed_duration_since(at);
        (age < chrono::Duration::hours(OBSERVATION_MAX_AGE_HOURS)).then_some((at, &o.outcome))
    });
    let test = last_test.and_then(|t| parse(&t.tested_at).map(|at| (at, &t.outcome)));

    // An ASSERTED verdict outranks any measurement, whatever the timestamps say.
    // `sticky` marks a state someone is deliberately holding - today only the
    // dev-only Disconnect button, whose entire job is to pin the signed-out state
    // so it can be worked on. A live call succeeding is exactly what that button
    // is holding the line against, and on a machine being used one arrives within
    // minutes, so without this the button reads as doing nothing at all. Only an
    // explicit Test Connection or Rescan replaces the row and clears the flag.
    if let Some(t) = last_test.filter(|t| t.sticky) {
        return Some(t.outcome.clone());
    }

    match (test, runtime) {
        (Some((t_at, t_out)), Some((r_at, r_out))) => Some(if r_at >= t_at {
            r_out.clone()
        } else {
            t_out.clone()
        }),
        (Some((_, out)), None) | (None, Some((_, out))) => Some(out.clone()),
        // No usable timestamp on either side that survived to here: fall back to the test
        // result first (consistent with the tie-break above), then to the runtime
        // observation but ONLY when its timestamp was genuinely unparseable - never when it
        // simply expired, which must stay dropped.
        (None, None) => last_test.map(|t| t.outcome.clone()).or_else(|| {
            runtime_timestamp_unparseable
                .then(|| last_runtime.map(|o| o.outcome.clone()))
                .flatten()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_result(outcome: ProviderTestOutcome, tested_at: &str) -> ProviderTestResult {
        ProviderTestResult {
            id: "claude".into(),
            outcome,
            elapsed_ms: 1,
            tested_at: tested_at.into(),
            sticky: false,
        }
    }

    fn observation(outcome: ProviderTestOutcome, observed_at: String) -> RuntimeObservation {
        RuntimeObservation {
            id: "claude".into(),
            outcome,
            observed_at,
        }
    }

    fn ago(hours: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339()
    }

    /// The bug this module exists to fix: a passing test followed by real failures must
    /// resolve to the failure, not the test.
    #[test]
    fn runtime_failure_supersedes_an_older_passing_test() {
        let test = test_result(ProviderTestOutcome::Ok, &ago(3));
        let runtime = observation(
            ProviderTestOutcome::Failed {
                message: "rate limit".into(),
            },
            ago(1),
        );
        assert!(matches!(
            most_recent_outcome(Some(&test), Some(&runtime)),
            Some(ProviderTestOutcome::Failed { .. })
        ));
    }

    /// …and the reverse, so a user who fixes the problem and re-tests isn't stuck behind a
    /// stale runtime failure.
    #[test]
    fn a_newer_passing_test_clears_an_older_runtime_failure() {
        let test = test_result(ProviderTestOutcome::Ok, &ago(1));
        let runtime = observation(
            ProviderTestOutcome::Failed {
                message: "boom".into(),
            },
            ago(3),
        );
        assert!(matches!(
            most_recent_outcome(Some(&test), Some(&runtime)),
            Some(ProviderTestOutcome::Ok)
        ));
    }

    #[test]
    fn a_stale_runtime_observation_is_ignored() {
        let test = test_result(ProviderTestOutcome::Ok, &ago(24));
        let runtime = observation(
            ProviderTestOutcome::Failed {
                message: "boom".into(),
            },
            ago(OBSERVATION_MAX_AGE_HOURS + 1),
        );
        assert!(matches!(
            most_recent_outcome(Some(&test), Some(&runtime)),
            Some(ProviderTestOutcome::Ok)
        ));
    }

    #[test]
    fn either_side_alone_is_used() {
        let runtime = observation(
            ProviderTestOutcome::RateLimited {
                message: "quota".into(),
            },
            ago(1),
        );
        assert!(matches!(
            most_recent_outcome(None, Some(&runtime)),
            Some(ProviderTestOutcome::RateLimited { .. })
        ));

        let test = test_result(ProviderTestOutcome::Ok, &ago(1));
        assert!(matches!(
            most_recent_outcome(Some(&test), None),
            Some(ProviderTestOutcome::Ok)
        ));
        assert!(most_recent_outcome(None, None).is_none());
    }

    /// An unparseable `tested_at` (hand-edited cache, older format) must still yield the
    /// test's verdict rather than silently reporting "never tested".
    #[test]
    fn an_unparseable_timestamp_falls_back_to_the_test_outcome() {
        let test = test_result(
            ProviderTestOutcome::Failed {
                message: "signed out".into(),
            },
            "not-a-timestamp",
        );
        assert!(matches!(
            most_recent_outcome(Some(&test), None),
            Some(ProviderTestOutcome::Failed { .. })
        ));
    }

    /// The mirror of the above and its own bug: a runtime observation with an unparseable
    /// timestamp and NO competing test result must still report its outcome, not `None`. The
    /// `(None, None)` match arm used to only ever fall back to `last_test`, silently dropping
    /// the one input that actually exists whenever it was the runtime side that had the bad
    /// timestamp.
    #[test]
    fn an_unparseable_runtime_timestamp_falls_back_to_the_runtime_outcome() {
        let runtime = observation(
            ProviderTestOutcome::Failed {
                message: "boom".into(),
            },
            "not-a-timestamp".into(),
        );
        assert!(matches!(
            most_recent_outcome(None, Some(&runtime)),
            Some(ProviderTestOutcome::Failed { .. })
        ));
    }

    /// The regression the fix above almost introduced: an EXPIRED runtime observation (a
    /// parseable timestamp older than `OBSERVATION_MAX_AGE_HOURS`) must stay dropped even
    /// An ASSERTED verdict is not evidence to be weighed - it outranks a measurement
    /// however fresh the measurement is.
    ///
    /// `sticky` is set by exactly one thing, the dev-only Disconnect button, whose job
    /// is to pin the signed-out state so it can be worked on. A live call succeeding is
    /// precisely what it holds the line against, and on a developer's machine (background
    /// folds, coding-agent summaries) one lands within minutes - so without this the
    /// button reads as broken. This coverage moved here with the logic: it used to live
    /// in `detect::note_live_call_success`, which this module replaced.
    #[test]
    fn an_asserted_verdict_outranks_a_newer_measurement() {
        let held = ProviderTestResult {
            sticky: true,
            ..test_result(
                ProviderTestOutcome::Failed {
                    message: "not signed in".into(),
                },
                &(chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339(),
            )
        };
        // A success observed AFTER it, which would otherwise win on recency.
        let fresh = observation(ProviderTestOutcome::Ok, chrono::Utc::now().to_rfc3339());

        assert!(matches!(
            most_recent_outcome(Some(&held), Some(&fresh)),
            Some(ProviderTestOutcome::Failed { .. })
        ));

        // Without the flag the same pair resolves the ordinary way: newest wins.
        let ordinary = ProviderTestResult {
            sticky: false,
            ..held.clone()
        };
        assert!(matches!(
            most_recent_outcome(Some(&ordinary), Some(&fresh)),
            Some(ProviderTestOutcome::Ok)
        ));
    }

    /// with no competing test - the `(None, None)` fallback must not resurrect it just
    /// because there's nothing to lose to. Only a genuinely unparseable timestamp should
    /// fall back to the observation's outcome.
    #[test]
    fn an_expired_runtime_observation_is_not_resurrected_by_the_fallback() {
        let runtime = observation(
            ProviderTestOutcome::Failed {
                message: "long gone".into(),
            },
            ago(OBSERVATION_MAX_AGE_HOURS + 1),
        );
        assert!(most_recent_outcome(None, Some(&runtime)).is_none());
    }

    /// An empty mirror populates itself from the loader and reports what the loader gave it.
    #[test]
    fn unresolved_failure_check_populates_an_empty_mirror_from_the_loader() {
        let mut mirror = None;
        let loaded = HashMap::from([(
            "claude".to_string(),
            observation(
                ProviderTestOutcome::Failed {
                    message: "boom".into(),
                },
                "irrelevant".into(),
            ),
        )]);
        assert!(has_unresolved_failure("claude", &mut mirror, || loaded.clone()));
        assert!(mirror.is_some(), "the loader's result must be cached");
    }

    /// The whole point of the mirror: once populated, the loader must never run again, even
    /// if what it WOULD return has since diverged (simulating the file changing on disk after
    /// the mirror was filled - which, per the module's safety argument, cannot happen from
    /// another process, but must also not spuriously re-trigger from this one).
    #[test]
    fn unresolved_failure_check_never_calls_the_loader_once_populated() {
        let mut mirror = Some(HashMap::from([(
            "claude".to_string(),
            observation(ProviderTestOutcome::Ok, "irrelevant".into()),
        )]));
        let result = has_unresolved_failure("claude", &mut mirror, || {
            panic!("loader must not run when the mirror is already populated")
        });
        assert!(!result, "a healthy cached entry must not read as a failure");
    }

    /// A rate limit counts as unresolved too - `record_success` must clear it just like a
    /// hard failure.
    #[test]
    fn unresolved_failure_check_counts_a_rate_limit() {
        let mut mirror = Some(HashMap::from([(
            "claude".to_string(),
            observation(
                ProviderTestOutcome::RateLimited {
                    message: "quota".into(),
                },
                "irrelevant".into(),
            ),
        )]));
        assert!(has_unresolved_failure("claude", &mut mirror, || {
            panic!("mirror already populated")
        }));
    }

    /// The debounce: a transient must not raise the banner, and the streak must survive
    /// across calls until it earns the alarm.
    #[test]
    fn hard_failures_debounce_until_the_threshold() {
        let id = "streak-test-provider";
        for expected in 1..FAILURE_STREAK_THRESHOLD {
            assert_eq!(bump_streak(id), expected);
        }
        assert_eq!(bump_streak(id), FAILURE_STREAK_THRESHOLD);
        assert!(
            clear_streak(id),
            "a bumped streak reports as previously failing"
        );
        assert!(!clear_streak(id), "clearing twice is a no-op");
        assert_eq!(bump_streak(id), 1, "a cleared streak restarts at one");
    }
}
