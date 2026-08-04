//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray's daemon watchdog: decide first, act second.
//!
//! # Why this module exists (READ THIS BEFORE RESTORING `-k`)
//!
//! This watchdog corrupted `meridian.db` on every macOS install, repeatedly,
//! for days. The mechanism, measured on 2026-08-04:
//!
//! 1. [`crate::commands::daemon_control::probe`] gives the daemon **800 ms** to
//!    answer on `~/.meridian/daemon.sock`.
//! 2. A daemon mid-ETL-batch or mid-WAL-checkpoint routinely misses that.
//! 3. Two consecutive misses (`WATCHDOG_STRIKES = 2`, ≈10 s) were treated as
//!    "down", and the watchdog ran `launchctl kickstart -k` — **SIGTERM to a
//!    perfectly healthy process, mid-write**.
//! 4. `WATCHDOG_COOLDOWN` paced that at one kill every ~45 s, forever: ~80
//!    hard kills per hour, while the tray stayed attached to the same WAL
//!    database. Damage landed in the `app_sessions` b-tree tail — the table the
//!    daemon writes.
//!
//! Observed directly: `daemon watchdog: endpoint down, restarting` → `SIGTERM
//! received` → `meridian daemon starting` **3.4 s later**. It was never
//! unhealthy. Quitting the tray froze the launchd `runs` counter instantly;
//! nothing else changed.
//!
//! # The three independent guards
//!
//! Any one of these stops the incident. All three are here deliberately,
//! because the failure is silent data corruption and a single mistaken gate
//! must not be enough to restart the storm.
//!
//! 1. **Never signal a live process.** [`decide`] returns [`Action::Wait`]
//!    whenever the daemon process is known to be alive, however slow it is to
//!    answer. A slow daemon is a performance problem; killing it mid-write is a
//!    data-loss problem.
//! 2. **`kickstart` without `-k`.** Per `man launchctl`, `-k` is precisely and
//!    only what kills a running instance. Dropping it makes the worst case a
//!    no-op instead of a SIGTERM. See
//!    [`crate::commands::daemon_control::start_if_stopped`].
//! 3. **A storm cap.** At most [`MAX_STARTS_IN_WINDOW`] start attempts per
//!    [`START_WINDOW`]; past that [`decide`] returns [`Action::GiveUp`] and the
//!    watchdog stops acting. This bounds the blast radius of *any* future
//!    mistake in the two guards above — the original incident would have cost
//!    one restart instead of hundreds.
//!
//! # What this watchdog is actually for
//!
//! Very little, and that is the honest answer. The daemon's launchd plist sets
//! `KeepAlive = true`, so **launchd already restarts a dead daemon** without
//! being asked. This loop is a backstop for the case launchd does not cover —
//! a service that is loaded but stopped — and a place to notice and report a
//! daemon that will not come back. It is not, and must not become, a liveness
//! killer: it has no way to tell "busy" from "wedged", and it proved willing to
//! act on that ambiguity ~80 times an hour.
//!
//! Recovering a genuinely *wedged* (alive but unresponsive) daemon is
//! deliberately out of scope. Doing it safely needs a signal this loop does not
//! have — a heartbeat the daemon advances, or a multi-minute unresponsive
//! window. Note that `etl_runs` is **not** a usable heartbeat: the daemon
//! latches and stops all ETL when it detects `db.corrupt`, so an `etl_runs`
//! heartbeat would kill a daemon that is idle on purpose, building a second
//! corruption path on top of the one this module exists to close.
//!
//! # Related
//! - [`crate::commands::daemon_control`] — [`probe`] and the launchd verbs.
//! - [`super::refresh`] — the slower, user-facing went-quiet/back-online
//!   notices. It reports; this module acts.

use std::time::{Duration, Instant};
use tracing::Instrument;

/// How often the watchdog probes the daemon's IPC endpoint.
const TICK: Duration = Duration::from_secs(5);

/// Consecutive missed probes before the endpoint is considered unresponsive.
///
/// This no longer decides whether anything gets *killed* — [`decide`]'s
/// liveness gate does — so it is only the "is the socket answering" filter.
const STRIKES: u32 = 2;

/// Quiet window after a start attempt, so a daemon that is merely mid-startup
/// is not started on top of itself.
const COOLDOWN: Duration = Duration::from_secs(45);

/// Ceiling on start attempts within [`START_WINDOW`] before the watchdog stops
/// acting entirely. Deliberately small: with `KeepAlive = true` doing the real
/// work, needing more than a handful of nudges an hour means something is wrong
/// that restarting will not fix.
const MAX_STARTS_IN_WINDOW: u32 = 5;

/// The window [`MAX_STARTS_IN_WINDOW`] is counted over. **Tumbling, not
/// rolling**: the count resets wholesale once the window elapses, rather than
/// ageing out individual attempts. Cruder, but it cannot drift, and the only
/// consequence of the imprecision is when the cap lifts — never whether a live
/// process gets signalled.
const START_WINDOW: Duration = Duration::from_secs(3600);

/// What the watchdog should do on this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Do nothing.
    Wait,
    /// Ask the service manager to start the daemon if it is stopped. Never
    /// kills, never signals — see guard 2 in the module docs.
    Start,
    /// The storm cap tripped. Stop acting and report instead.
    GiveUp,
}

/// Everything [`decide`] is allowed to look at.
///
/// A struct rather than positional arguments so that removing a rule from
/// [`decide`] is a change to its *body*, not its signature — an unused
/// parameter would otherwise fail the `-D warnings` build before the
/// regression test could run, masking the very revert the test exists to catch.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Inputs {
    /// The daemon answered its IPC endpoint this tick.
    pub probe_ok: bool,
    /// Whether the daemon *process* exists, independent of whether it answered.
    /// `None` means "could not determine" — see [`daemon_process_alive`].
    pub process_alive: Option<bool>,
    /// Consecutive ticks the endpoint has failed to answer.
    pub consecutive_failures: u32,
    /// Start attempts already made inside the current [`START_WINDOW`].
    pub starts_in_window: u32,
    /// Whether the post-start quiet window is still open.
    pub in_cooldown: bool,
}

/// The whole watchdog policy, as one pure function.
///
/// Pure and free of `cfg` on purpose: the policy is identical on every
/// platform, so it compiles and its tests run everywhere — including the
/// Windows CI job, where past `cfg`-gated helpers have twice broken the build
/// in ways invisible from macOS.
///
/// Rules, in order:
/// 1. endpoint answered → nothing to do
/// 2. **the process is alive → never act** (the guard that closes the
///    corruption incident; a slow daemon is not a dead one)
/// 3. not yet [`STRIKES`] consecutive failures → too early to call it
/// 4. still inside the post-start cooldown → let it finish starting
/// 5. storm cap reached → [`Action::GiveUp`]
/// 6. otherwise → [`Action::Start`]
pub(crate) fn decide(i: Inputs) -> Action {
    if i.probe_ok {
        return Action::Wait;
    }
    // A live-but-slow process must never be signalled. `None` ("cannot tell")
    // deliberately does NOT block: on a platform without a liveness check this
    // preserves the pre-existing behaviour rather than silently disabling the
    // backstop. Guards 2 and 3 still apply there.
    if i.process_alive == Some(true) {
        return Action::Wait;
    }
    if i.consecutive_failures < STRIKES {
        return Action::Wait;
    }
    if i.in_cooldown {
        return Action::Wait;
    }
    if i.starts_in_window >= MAX_STARTS_IN_WINDOW {
        return Action::GiveUp;
    }
    Action::Start
}

/// Whether the daemon process exists, independent of whether it answered.
///
/// `None` means "could not determine", which [`decide`] treats as
/// non-blocking. Kept as the single `cfg`-bearing seam in this module so
/// [`decide`] itself stays portable.
async fn daemon_process_alive() -> Option<bool> {
    crate::commands::daemon_control::process_alive().await
}

/// Probe every [`TICK`]; act only when [`decide`] says to.
///
/// See the module docs for why this is far more conservative than it looks like
/// it should be.
pub async fn run_daemon_watchdog() {
    let mut consecutive_failures: u32 = 0;
    let mut cooldown_until: Option<Instant> = None;
    let mut window_started = Instant::now();
    let mut starts_in_window: u32 = 0;
    let mut gave_up = false;

    loop {
        tokio::time::sleep(TICK).await;

        if window_started.elapsed() >= START_WINDOW {
            window_started = Instant::now();
            starts_in_window = 0;
            gave_up = false;
        }

        let probe_ok = crate::commands::daemon_control::probe().await.running;
        if probe_ok {
            consecutive_failures = 0;
            cooldown_until = None;
        } else {
            consecutive_failures = consecutive_failures.saturating_add(1);
        }

        let in_cooldown = cooldown_until.is_some_and(|until| Instant::now() < until);

        // `daemon_process_alive` spawns a subprocess, so only ask when the
        // answer could actually change the outcome — i.e. only when every other
        // guard would otherwise permit [`Action::Start`]. In every case skipped
        // here [`decide`] reaches a verdict before it reads `process_alive`
        // (`probe_ok` → `Wait`; under [`STRIKES`] → `Wait`; in cooldown →
        // `Wait`; past the cap → `GiveUp`), so passing `None` cannot change the
        // result. Without this, a daemon left stopped past the storm cap would
        // fork `launchctl` every 5 s forever.
        let could_start = !probe_ok && consecutive_failures >= STRIKES && !in_cooldown && !gave_up;
        let process_alive = if could_start {
            daemon_process_alive().await
        } else {
            None
        };

        let action = decide(Inputs {
            probe_ok,
            process_alive,
            consecutive_failures,
            starts_in_window,
            in_cooldown,
        });

        match action {
            Action::Wait => {
                // A silent endpoint on a live process is the case that used to
                // trigger a kill. Record it — it is a real (performance)
                // symptom worth seeing in telemetry — but do not act on it.
                if !probe_ok && process_alive == Some(true) {
                    tracing::debug!(
                        consecutive_failures,
                        "daemon watchdog: endpoint slow but the process is alive - not restarting"
                    );
                }
            }
            Action::Start => {
                async {
                    tracing::info!("daemon watchdog: daemon not running, starting it");
                    if let Err(e) = crate::commands::daemon_control::start_if_stopped().await {
                        // Mark the span itself ERROR, not just the log line: a
                        // failed automatic recovery is the case worth finding in
                        // OpenObserve, and span status is what an error-only
                        // query filters on.
                        tracing::Span::current().record("otel.status_code", "ERROR");
                        tracing::warn!(error = %e, "daemon watchdog: start attempt failed");
                    }
                }
                .instrument(tracing::info_span!(
                    "daemon_watchdog.start",
                    down_after_s = (consecutive_failures as u64) * TICK.as_secs(),
                    otel.status_code = tracing::field::Empty,
                ))
                .await;
                cooldown_until = Some(Instant::now() + COOLDOWN);
                consecutive_failures = 0;
                starts_in_window = starts_in_window.saturating_add(1);
            }
            Action::GiveUp => {
                // Log once per window, not every tick.
                if !gave_up {
                    gave_up = true;
                    tracing::warn!(
                        starts_in_window,
                        "daemon watchdog: too many start attempts this hour - giving up until the window resets"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silent_endpoint() -> Inputs {
        Inputs {
            probe_ok: false,
            process_alive: None,
            consecutive_failures: STRIKES,
            starts_in_window: 0,
            in_cooldown: false,
        }
    }

    /// **The regression test for the corruption incident.**
    ///
    /// Two consecutive 800 ms probe timeouts against a daemon that is alive and
    /// merely busy is precisely the state that produced ~80 SIGTERMs an hour
    /// and corrupted `app_sessions`. It must not produce an action, no matter
    /// how many probes are missed.
    #[test]
    fn a_live_but_slow_daemon_is_never_restarted() {
        let busy = Inputs {
            process_alive: Some(true),
            ..silent_endpoint()
        };
        assert_eq!(
            decide(busy),
            Action::Wait,
            "a live daemon that is slow to answer must never be signalled - \
             this is what corrupted meridian.db"
        );

        // Still true after a long outage: time does not turn "busy" into
        // "dead", and this loop has no signal that could tell the difference.
        for failures in [STRIKES, STRIKES + 10, 1_000] {
            assert_eq!(
                decide(Inputs {
                    consecutive_failures: failures,
                    ..busy
                }),
                Action::Wait,
                "no number of missed probes justifies signalling a live process"
            );
        }
    }

    /// The backstop must still work, or the fix is just a disabled watchdog.
    #[test]
    fn a_dead_daemon_is_still_started() {
        assert_eq!(
            decide(Inputs {
                process_alive: Some(false),
                ..silent_endpoint()
            }),
            Action::Start
        );
        // "Cannot tell" preserves the old behaviour rather than disabling the
        // backstop on platforms without a liveness check.
        assert_eq!(decide(silent_endpoint()), Action::Start);
    }

    /// A healthy endpoint short-circuits everything, including the storm cap.
    #[test]
    fn a_healthy_endpoint_is_always_left_alone() {
        assert_eq!(
            decide(Inputs {
                probe_ok: true,
                process_alive: Some(false),
                consecutive_failures: 99,
                starts_in_window: MAX_STARTS_IN_WINDOW + 1,
                in_cooldown: false,
            }),
            Action::Wait
        );
    }

    /// Debounce and cooldown, unchanged in spirit from the original loop.
    #[test]
    fn a_single_blip_and_the_startup_window_are_both_tolerated() {
        assert_eq!(
            decide(Inputs {
                consecutive_failures: STRIKES - 1,
                process_alive: Some(false),
                ..silent_endpoint()
            }),
            Action::Wait,
            "one missed probe is a blip, not an outage"
        );
        assert_eq!(
            decide(Inputs {
                in_cooldown: true,
                process_alive: Some(false),
                ..silent_endpoint()
            }),
            Action::Wait,
            "a daemon mid-startup must not be started on top of itself"
        );
    }

    /// The loop skips the (subprocess-spawning) liveness query whenever another
    /// guard already settles the tick, and passes `None` instead. That is only
    /// sound if `None` yields the same verdict in exactly those cases — so pin
    /// it, because the optimisation would otherwise silently turn into a
    /// behaviour change the first time [`decide`]'s rule order is edited.
    #[test]
    fn skipping_the_liveness_query_cannot_change_the_verdict() {
        let unknown = Inputs {
            process_alive: None,
            ..silent_endpoint()
        };
        assert_eq!(
            decide(Inputs {
                consecutive_failures: STRIKES - 1,
                ..unknown
            }),
            Action::Wait,
            "under the strike count, liveness is not consulted"
        );
        assert_eq!(
            decide(Inputs {
                in_cooldown: true,
                ..unknown
            }),
            Action::Wait,
            "in cooldown, liveness is not consulted"
        );
        assert_eq!(
            decide(Inputs {
                starts_in_window: MAX_STARTS_IN_WINDOW,
                ..unknown
            }),
            Action::GiveUp,
            "past the cap, liveness is not consulted"
        );
        assert_eq!(
            decide(Inputs {
                probe_ok: true,
                ..unknown
            }),
            Action::Wait,
            "on a healthy endpoint, liveness is not consulted"
        );
    }

    /// The blast-radius cap. Independent of whether any single decision above
    /// is right — this is what would have held the original incident to a
    /// handful of restarts instead of hundreds per hour.
    #[test]
    fn repeated_starts_are_capped_within_the_window() {
        let dead = Inputs {
            process_alive: Some(false),
            ..silent_endpoint()
        };
        for n in 0..MAX_STARTS_IN_WINDOW {
            assert_eq!(
                decide(Inputs {
                    starts_in_window: n,
                    ..dead
                }),
                Action::Start,
                "attempt {n} is within the cap"
            );
        }
        for n in [MAX_STARTS_IN_WINDOW, MAX_STARTS_IN_WINDOW + 50] {
            assert_eq!(
                decide(Inputs {
                    starts_in_window: n,
                    ..dead
                }),
                Action::GiveUp,
                "past the cap the watchdog must stop acting, not keep trying"
            );
        }
    }
}
