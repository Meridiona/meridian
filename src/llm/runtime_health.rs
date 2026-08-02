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
fn load() -> HashMap<String, RuntimeObservation> {
    let Ok(raw) = std::fs::read_to_string(record_path()) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

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
    if let Err(e) = meridian_core::fs_utils::atomic_write_json(&record_path(), &all) {
        tracing::warn!(error = %e, "failed to persist runtime provider health");
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

/// Record that `provider` answered a real call.
///
/// Clears the failure streak and stamps an `Ok` observation, so a recovered provider drops
/// the banner on the tray's next health poll rather than waiting for the user to re-test.
/// Writes only on a state CHANGE (the first success after failures) — a healthy provider
/// making an hourly call must not rewrite the file forever.
pub fn record_success(provider: LlmProvider) {
    let id = provider.as_str().to_string();
    let was_failing = clear_streak(&id);
    let stale_failure = matches!(
        load().get(&id).map(|o| &o.outcome),
        Some(ProviderTestOutcome::Failed { .. }) | Some(ProviderTestOutcome::RateLimited { .. })
    );
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

    let runtime = last_runtime.and_then(|o| {
        let at = parse(&o.observed_at)?;
        let age = now.signed_duration_since(at);
        (age < chrono::Duration::hours(OBSERVATION_MAX_AGE_HOURS)).then_some((at, &o.outcome))
    });
    let test = last_test.and_then(|t| parse(&t.tested_at).map(|at| (at, &t.outcome)));

    match (test, runtime) {
        (Some((t_at, t_out)), Some((r_at, r_out))) => Some(if r_at >= t_at {
            r_out.clone()
        } else {
            t_out.clone()
        }),
        (Some((_, out)), None) | (None, Some((_, out))) => Some(out.clone()),
        // No usable timestamp anywhere: fall back to the test result's outcome if there is
        // one at all, so a pre-existing cache with an odd timestamp still counts.
        (None, None) => last_test.map(|t| t.outcome.clone()),
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
