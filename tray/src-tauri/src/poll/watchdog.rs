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
    /// Whether the user has deliberately paused the daemon from the tray menu.
    ///
    /// A paused daemon is `bootout`ed, so it looks exactly like a dead one to
    /// every other input here: the endpoint is silent and the process is gone.
    /// Without this flag the watchdog would spend its whole start budget
    /// `kickstart`ing a label launchd no longer has loaded, then report a
    /// daemon that "will not come back" - about a daemon the user switched off
    /// on purpose.
    pub daemon_paused: bool,
    /// Whether the installer is mid-swap of the daemon binary
    /// ([`crate::daemon_lifecycle::is_staging`]).
    ///
    /// Like `daemon_paused`, the daemon is down *by instruction* — the
    /// installer killed it so it could overwrite the file, and starting it back
    /// up re-locks the very binary being written. Unlike a pause it lasts
    /// seconds, not until the user says otherwise.
    pub staging_in_progress: bool,
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
///
/// Rule 0 sits above all of them: a daemon that is down *by instruction* — the
/// user paused it, or the installer is replacing its binary — stays down, and
/// no evidence this loop can gather outranks that.
pub(crate) fn decide(i: Inputs) -> Action {
    // Ahead of the endpoint check, and ahead of the storm cap, on purpose.
    // Pause `bootout`s the agent, so a paused daemon presents exactly as a
    // crashed one; every rule below would read it as an outage to recover
    // from, and rule 5 would eventually escalate it to a user-visible "gave
    // up" notice. Returning here is what keeps `Pause` meaning something.
    if i.daemon_paused {
        return Action::Wait;
    }
    // Same shape, different instruction: the installer stopped the daemon so it
    // could overwrite `~/.meridian/bin/meridian.exe`, and a silent endpoint is
    // the INTENDED state for those few seconds. Starting it here re-locks the
    // binary mid-swap and fails the install outright - see
    // [`crate::daemon_lifecycle::begin_staging`] for the incident this closes.
    //
    // It must sit above the `process_alive` guard, not below it: that guard is
    // `None` on Windows (`daemon_control::process_alive`), which `decide`
    // treats as non-blocking, so on the one platform where this race actually
    // occurs there is nothing further down that would stop it.
    if i.staging_in_progress {
        return Action::Wait;
    }
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

        // Read fresh each tick, so a pause or resume takes effect on the next.
        let paused = crate::daemon_lifecycle::is_paused();
        // Likewise fresh: the staging window opens and closes inside a single
        // install, so a value cached across ticks would be wrong for most of it.
        let staging = crate::daemon_lifecycle::is_staging();

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
        // `!paused` belongs in this set for the same reason as the rest: while
        // paused [`decide`] returns before it reads `process_alive`, so asking
        // would only fork `launchctl` every 5 s for an answer that cannot
        // change the verdict.
        // `!staging` joins this set for the same reason as `!paused`: while it
        // is set `decide` returns before it reads `process_alive`, so asking
        // would only fork a subprocess for an answer that cannot change the
        // verdict.
        let could_start = !paused
            && !staging
            && !probe_ok
            && consecutive_failures >= STRIKES
            && !in_cooldown
            && !gave_up;
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
            daemon_paused: paused,
            staging_in_progress: staging,
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
                // RE-READ the flag at the last instant before acting.
                //
                // `decide` was handed a value sampled at the top of this tick,
                // and between then and here the loop has awaited a socket probe
                // and, on the path that reaches `Start`, forked a subprocess for
                // the liveness check - hundreds of milliseconds during which an
                // install can begin. That is a check-then-act window on the one
                // decision that must not be wrong, and it is invisible from
                // `decide`, which is pure and cannot know its input went stale.
                //
                // A re-read rather than [`crate::daemon_lifecycle::LIFECYCLE`]:
                // the mutex would serialise properly, but the watchdog would
                // then block for the length of an install and start the daemon
                // the instant it was released, and a 5 s-budgeted `stop_for_quit`
                // would queue behind the same install. The reasoning is recorded
                // on `begin_staging`; this closes the window that reasoning left
                // open rather than reversing it.
                //
                // Not airtight, and deliberately so: `begin_staging` can still
                // land during `start_if_stopped` itself. The installer's own
                // re-kill loop (`stop_running_daemon_before_stage`) is the
                // backstop for that residue - it kills anything that appears
                // mid-wait, which is exactly what this would be.
                if crate::daemon_lifecycle::is_staging() {
                    tracing::debug!(
                        "daemon watchdog: install started during this tick - standing down"
                    );
                    continue;
                }
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
            daemon_paused: false,
            staging_in_progress: false,
        }
    }

    /// **The regression test for the broken-Windows-update incident.**
    ///
    /// The installer kills the daemon to overwrite `meridian.exe`, which takes
    /// long enough for this loop to strike out and start it again — from the
    /// very path being staged. The installer's post-kill poll then reported
    /// that process as an un-killable holder of the binary and failed the whole
    /// install ("still running after stop attempts"), so every Windows update
    /// after the first install died here.
    ///
    /// `silent_endpoint()` is deliberately the base: `process_alive: None` is
    /// the Windows reality (`daemon_control::process_alive`), and `decide`
    /// treats `None` as non-blocking — so on the one platform where this race
    /// happens, this rule is the ONLY thing standing between the watchdog and
    /// the installer.
    #[test]
    fn a_daemon_the_installer_stopped_is_never_started() {
        let staging = Inputs {
            staging_in_progress: true,
            ..silent_endpoint()
        };
        // The control: without the flag this exact input starts the daemon,
        // which is what made the race possible. So the assertion below is
        // testing the rule and not some other guard that happens to cover it.
        assert_eq!(
            decide(silent_endpoint()),
            Action::Start,
            "sanity: a silent endpoint with no liveness answer does start"
        );
        assert_eq!(
            decide(staging),
            Action::Wait,
            "starting the daemon mid-swap re-locks the binary and fails the install"
        );

        // A long install must not wear the rule down - staging outranks the
        // failure count, the cooldown, and the storm cap alike. Every one of
        // these reaches a verdict AFTER the staging check, so if the rule is
        // ever moved below them this fails.
        for extra in [
            Inputs {
                consecutive_failures: 1_000,
                ..staging
            },
            Inputs {
                in_cooldown: true,
                ..staging
            },
            Inputs {
                starts_in_window: MAX_STARTS_IN_WINDOW,
                ..staging
            },
            Inputs {
                process_alive: Some(false),
                ..staging
            },
        ] {
            assert_eq!(
                decide(extra),
                Action::Wait,
                "no amount of downtime turns an install-held daemon into an outage"
            );
        }
    }

    /// The half `decide` structurally cannot cover: the flag going true AFTER
    /// its input was sampled.
    ///
    /// `decide` is pure, so it can only ever judge a snapshot. The loop takes
    /// that snapshot at the top of the tick and then awaits a socket probe,
    /// plus (on exactly the path that reaches `Start`) a forked subprocess for
    /// the liveness check - ample time for an install to begin. A verdict of
    /// `Start` computed before `begin_staging` is therefore not evidence that
    /// starting is still safe, and nothing inside `decide` can tell.
    ///
    /// Source-scanned because the window is between two awaits in
    /// `run_daemon_watchdog`, which polls a real socket forever - there is no
    /// seam to drive it through. Truncated at the first `#[cfg(test)]` so the
    /// needles cannot match their own literals in this module.
    #[test]
    fn the_start_arm_re_reads_the_flag_before_acting() {
        let whole = include_str!("watchdog.rs");
        let src = &whole[..whole
            .find("#[cfg(test)]")
            .expect("watchdog.rs lost its test module marker")];
        let arm = src
            .split_once("Action::Start => {")
            .expect("the Start arm is gone")
            .1;
        let arm = &arm[..arm.find("Action::GiveUp").unwrap_or(arm.len())];
        assert!(
            arm.contains("if crate::daemon_lifecycle::is_staging() {"),
            "the Start arm acts on a staging value sampled before two awaits - \
             an install that begins mid-tick starts the daemon it just killed"
        );
        // Standing down must SKIP the start, not merely log before it.
        let gate = arm
            .split_once("if crate::daemon_lifecycle::is_staging() {")
            .expect("gate")
            .1;
        let gate = &gate[..gate.find("start_if_stopped").unwrap_or(gate.len())];
        assert!(
            gate.contains("continue;"),
            "the re-read does not skip the start, so it only narrates the race"
        );
    }

    /// Staging is a few seconds inside one install, so the rule must not
    /// outlive it — a flag that stuck would leave the machine with no
    /// supervisor at all on Windows, which is a worse failure than the one it
    /// was added to fix (there, this loop IS the KeepAlive).
    #[test]
    fn the_backstop_returns_the_moment_staging_ends() {
        assert_eq!(
            decide(Inputs {
                staging_in_progress: false,
                ..silent_endpoint()
            }),
            Action::Start
        );
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
                daemon_paused: false,
                staging_in_progress: false,
            }),
            Action::Wait
        );
    }

    /// A paused daemon must be left alone, however dead it looks.
    ///
    /// Pause `bootout`s the agent, which is indistinguishable from a crash on
    /// every other input: silent endpoint, no process, and (unlike a crash)
    /// nothing that will ever bring it back on its own. So this is the one
    /// state where the backstop is exactly wrong - it would undo the user's
    /// explicit "stop working" the moment [`STRIKES`] elapsed, and `Pause`
    /// would go back to being the no-op it was when it ran `launchctl stop`.
    #[test]
    fn a_paused_daemon_is_never_started() {
        let paused = Inputs {
            process_alive: Some(false),
            daemon_paused: true,
            ..silent_endpoint()
        };
        assert_eq!(
            decide(paused),
            Action::Wait,
            "the watchdog must not resurrect a daemon the user paused"
        );

        // And it must not merely delay: no amount of elapsed outage turns a
        // deliberate pause into something to recover from.
        for failures in [STRIKES, STRIKES + 10, 1_000] {
            assert_eq!(
                decide(Inputs {
                    consecutive_failures: failures,
                    ..paused
                }),
                Action::Wait
            );
        }

        // Nor may it burn the start budget and then report the pause as a
        // failure to come back - `GiveUp` is a notice-raising state.
        assert_eq!(
            decide(Inputs {
                starts_in_window: MAX_STARTS_IN_WINDOW + 1,
                ..paused
            }),
            Action::Wait,
            "a pause must not surface as the watchdog giving up"
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
        // The combination the loop actually produces while paused. `!paused`
        // is part of `could_start`, so a paused tick never queries liveness
        // and always reaches `decide` with `process_alive: None` - the one
        // skip condition the paused-daemon test above cannot cover, because it
        // pins `Some(false)` to isolate the pause rule itself.
        assert_eq!(
            decide(Inputs {
                daemon_paused: true,
                ..unknown
            }),
            Action::Wait,
            "while paused, liveness is not consulted"
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
