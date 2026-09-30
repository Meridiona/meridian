//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Detects capture producing no frames for a sustained window — an
//! **output-side** backstop for "reinstalled and it's not tracking" reports,
//! independent of what the OS permission APIs claim.
//!
//! # Why the permission checks in [`super::permissions`] aren't enough
//!
//! `accessibility_trusted()`/`screen_recording_trusted()` read the TCC grant
//! for the calling process. That grant is supposed to track the app's
//! code-signing identity, which should survive a delete-and-reinstall of the
//! same signed build — but Screen Recording in particular has a
//! well-documented macOS quirk where WindowServer caches which *process* is
//! authorized independently of the TCC database, and doesn't always notice a
//! replaced binary until the Mac reboots or the grant is toggled off/on by
//! hand. So the OS can keep answering "granted" while capture is getting
//! nothing — a permission-only check cannot see that at all. This module
//! checks the thing users actually mean by "not tracking": is anything
//! actually landing in `capture_frames`.
//!
//! Users who never run the in-app uninstall wizard before deleting
//! `Meridian.app` (see `src/uninstall.rs`'s module doc) are the population
//! most likely to hit this — nothing in a plain Trash delete touches the TCC
//! grant, so a stale WindowServer cache entry is the one thing standing
//! between "worked before" and "reinstalled and it's not tracking".
//!
//! # A quiet table only means something while the user is at the machine
//!
//! Capture writes nothing while the screen is locked, the display is off, or
//! the user is simply away, so "no frame for 20 minutes" is the normal state
//! of every lunch break and every night. Judging by elapsed time alone raised
//! the red "Tracking has stopped" error (and its OS toast) for every user who
//! stepped away, then cleared itself once they came back and a frame landed.
//!
//! So elapsed time is necessary but not sufficient. [`StallTracker`] also
//! counts health ticks on which the user was demonstrably active (see
//! [`crate::sys::seconds_since_last_input`]) and still no new frame had
//! landed, and the notice needs [`ACTIVE_STALL_TICKS`] of them. The input
//! signal comes from the OS, not from `capture_ui_events`, because that
//! recorder shares the accessibility grant the frames depend on and would go
//! blind in exactly the failure being diagnosed.
//!
//! This can only ever raise LATER or not at all compared with the elapsed-time
//! rule alone, never sooner, so it cannot introduce a new false positive.
//!
//! [`STARTUP_GRACE`] covers the launch case: a fresh launch's newest frame is
//! very often from a PRIOR session, so the first tick after launch would
//! otherwise read as a huge, entirely normal gap. It only has to outlast
//! startup lag (plugin registration, the capture engine's first tick).
//!
//! # Related
//! - [`super::permissions`] — the input-side (TCC boolean) check this
//!   complements, not replaces; both can be raised independently.
//! - `meridian-core/src/capture.rs` — `capture_frames`'s writer, ~2s cadence
//!   (`capture/screenpipe.rs::CAPTURE_INTERVAL`) whenever capture is running.
//! - `crate::state::PauseSource` — legitimate reasons capture is silent; this
//!   check no-ops while any pause is active rather than flag it as broken.

use meridian_core::SqlitePool;
use std::time::{Duration, Instant};

/// `system_notices` id / dedup key.
const NOTICE_ID: &str = "tray.capture_stalled";
/// Shares `permissions.rs`'s event key so this groups with the other
/// capture-health notices in the dashboard banner — `notice_id` alone is
/// still what makes each row unique, so there is no collision.
const EVENT_KEY: &str = "system.capture_permission";

/// How long `capture_frames` must go without a new row before this is
/// believed. Generously above capture's own ~2s tick cadence and above any
/// ordinary startup/resume lag — sized to catch a genuinely dead pipeline,
/// not a slow one.
const STALL_THRESHOLD: Duration = Duration::from_secs(20 * 60);

/// How long after this process starts before a stale `MAX(timestamp)` is
/// trusted. See the module doc — this is what stops "the newest frame is
/// from yesterday" from reading as an instant false positive at every launch.
const STARTUP_GRACE: Duration = Duration::from_secs(3 * 60);

/// A reading of [`crate::sys::seconds_since_last_input`] under this means the
/// user touched an input device during the last health tick (~60s), with
/// margin for tick jitter, so capture had something to record.
const ACTIVE_IDLE_LIMIT_S: f64 = 90.0;

/// Health ticks with the user active and not one new frame before the notice
/// is believed. Ten is ten minutes of real use, comfortably past the point
/// where a healthy pipeline has written hundreds of frames, and still inside
/// [`STALL_THRESHOLD`], so a user who is working the whole time is told at the
/// same 20 minutes as before, not later.
const ACTIVE_STALL_TICKS: u32 = 10;

/// What the poll loop remembers between health ticks. Lives in the loop, not a
/// static, so it dies with the process and every launch starts from zero (the
/// same reasoning as [`super::permissions::PermissionDebounce`]).
#[derive(Default)]
pub(super) struct StallTracker {
    /// Newest frame timestamp seen on the previous tick; a change means at
    /// least one frame landed since, i.e. capture is alive.
    last_frame_ts: Option<chrono::DateTime<chrono::Utc>>,
    /// Ticks since that frame on which the user was active. Cleared whenever a
    /// new frame lands or a pause starts, so it never spans either.
    active_ticks: u32,
    /// Whether this episode has already been logged, so a long stall writes
    /// one WARN instead of one a minute.
    warned: bool,
}

/// Re-check whether capture is actually producing frames, and raise/clear
/// [`NOTICE_ID`] accordingly. Runs on the same `do_health` cadence as
/// [`super::permissions::check_permissions`] (~60s).
///
/// `loop_started` and `paused` are passed in rather than read from a shared
/// state so the decision stays a pure function of its inputs — mirrors
/// [`super::permissions::PermissionDebounce::observe`]'s own reasoning.
/// `loop_started.elapsed()` and the OS idle reading are taken here, in the one
/// production call site — never in a test — for the same reason described on
/// [`check_capture_health_inner`]'s `since_launch` parameter.
pub(super) async fn check_capture_health(
    pool: &SqlitePool,
    loop_started: Instant,
    paused: bool,
    tracker: &mut StallTracker,
) {
    if !crate::sys::is_bundled() {
        return;
    }
    check_capture_health_inner(
        pool,
        loop_started.elapsed(),
        paused,
        tracker,
        crate::sys::seconds_since_last_input(),
    )
    .await;
}

/// The `is_bundled()`-gated body of [`check_capture_health`], split out so
/// tests can drive it directly — `cargo test` never runs from inside a
/// `.app` bundle, so a test calling the outer function would always hit that
/// gate and silently exercise nothing (same reasoning as
/// [`super::permissions`]'s pure-function split).
///
/// Takes `since_launch` as an already-computed [`Duration`], not an
/// [`Instant`] to call `.elapsed()` on internally. `Instant` only ever
/// counts forward safely when every value in play was itself produced by
/// `Instant::now()` on the same clock; a test that backdates one by
/// subtracting hours (`Instant::now() - Duration::from_secs(3600)`) can
/// underflow and panic on Windows, where `Instant` counts from system boot —
/// a CI runner up for less than that duration cannot represent an instant
/// that far in the past. This bit `poll::watchdog`'s throttle test once
/// already (see `d1baab3b`'s fix). Taking a plain `Duration` here removes
/// the hazard at its root instead of re-guarding it with `checked_sub`:
/// tests construct arbitrary durations directly, with no `Instant`
/// arithmetic anywhere in this module's test suite.
///
/// `idle_secs` is the OS's seconds-since-last-input reading, passed in for the
/// same testability reason. `None` (the platform cannot say) counts as
/// "active": an unreadable signal must fall back to the old elapsed-time
/// behaviour, never silence a real stall.
async fn check_capture_health_inner(
    pool: &SqlitePool,
    since_launch: Duration,
    paused: bool,
    tracker: &mut StallTracker,
    idle_secs: Option<f64>,
) {
    // Capture is legitimately silent for a pause the user or a schedule
    // chose — that is not "stalled", and clearing here would be wrong too:
    // a genuine stall discovered just before a pause starts should still
    // read as raised when the pause ends, not be silently forgotten. The
    // activity count restarts so it never spans the pause.
    if paused {
        tracker.active_ticks = 0;
        return;
    }
    if since_launch < STARTUP_GRACE {
        return;
    }

    let last_ts = match meridian_core::last_frame_timestamp(pool).await {
        Ok(Some(ts)) => ts,
        Ok(None) => {
            // No frame has ever been written (table empty/missing — a very
            // fresh install mid-onboarding, or a build with the `capture`
            // feature off). Nothing to compare against; not this check's
            // job to diagnose that.
            return;
        }
        Err(e) => {
            tracing::debug!(error = %e, "capture_health: could not read capture_frames — skipping this tick");
            return;
        }
    };

    let new_frame = tracker.last_frame_ts != Some(last_ts);
    tracker.last_frame_ts = Some(last_ts);
    if new_frame {
        tracker.active_ticks = 0;
        tracker.warned = false;
    } else if idle_secs.is_none_or(|s| s < ACTIVE_IDLE_LIMIT_S) {
        tracker.active_ticks = tracker.active_ticks.saturating_add(1);
    }

    let gap = chrono::Utc::now().signed_duration_since(last_ts);
    let quiet_too_long = gap > chrono::Duration::from_std(STALL_THRESHOLD).unwrap();

    let result = if !quiet_too_long {
        meridian::notices::clear_typed(pool, NOTICE_ID, EVENT_KEY).await
    } else if tracker.active_ticks < ACTIVE_STALL_TICKS {
        // Quiet for a long time, but the user was mostly away: the normal
        // state of a lunch break or a night. Leave any existing notice as it
        // is; the next frame clears it.
        tracing::debug!(
            active_ticks = tracker.active_ticks,
            ?idle_secs,
            "capture_health: no recent frame, but too little user activity to call it a stall"
        );
        return;
    } else {
        if !tracker.warned {
            tracker.warned = true;
            tracing::warn!(
                last_ts = %last_ts,
                active_ticks = tracker.active_ticks,
                "capture_health: user active but no new frame within the stall threshold"
            );
        }
        meridian::notices::raise_typed(
            pool,
            meridian::notices::Notice {
                id: NOTICE_ID,
                severity: "error",
                title: "Tracking has stopped",
                detail: "Meridian hasn't captured any activity in a while, even though it's running.",
                remedy: Some(
                    "Quit and reopen Meridian. If that doesn't help, open System Settings \u{2192} \
                     Privacy & Security, turn Meridian's Accessibility and Screen Recording access \
                     off and back on, then reopen the app.",
                ),
                event_key: EVENT_KEY,
                deep_link: Some(meridian_core::notifications::deep_links::SETTINGS),
            },
        )
        .await
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, id = NOTICE_ID, "capture_health: notice write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real schema-migrated in-memory DB — same pattern as
    /// `poll::refresh`'s own `fresh_db`, so `capture_frames` (migration 046)
    /// and `system_notices` (037) are the genuine tables, not a hand-rolled
    /// stand-in.
    async fn fresh_db() -> SqlitePool {
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("../../src/migrations")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    async fn seeded_pool(last_frame_offset: Option<chrono::Duration>) -> SqlitePool {
        let pool = fresh_db().await;
        if let Some(offset) = last_frame_offset {
            let ts = (chrono::Utc::now() - offset).to_rfc3339();
            sqlx::query(
                "INSERT INTO capture_frames (timestamp, app_name, text_source) \
                 VALUES (?1, 'TestApp', 'ocr')",
            )
            .bind(ts)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    /// Well past [`STARTUP_GRACE`] — a plain [`Duration`], never an
    /// [`Instant`], so no test in this module can hit the Windows
    /// boot-epoch underflow described on [`check_capture_health_inner`].
    const LONG_AGO: Duration = Duration::from_secs(3600);

    /// User at the keyboard right now.
    const ACTIVE: Option<f64> = Some(1.0);
    /// User away for an hour.
    const AWAY: Option<f64> = Some(3600.0);

    /// Drive `n` health ticks with a fixed idle reading.
    async fn ticks(
        pool: &SqlitePool,
        tracker: &mut StallTracker,
        n: u32,
        idle: Option<f64>,
        paused: bool,
    ) {
        for _ in 0..n {
            check_capture_health_inner(pool, LONG_AGO, paused, tracker, idle).await;
        }
    }

    async fn raised_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
            .bind(NOTICE_ID)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_user_active_the_whole_time_still_gets_the_alarm() {
        // The case this module exists for: the user is working, nothing lands.
        // The first tick only records the newest frame; the next
        // ACTIVE_STALL_TICKS are the evidence.
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        let mut tracker = StallTracker::default();
        ticks(&pool, &mut tracker, ACTIVE_STALL_TICKS, ACTIVE, false).await;
        assert_eq!(
            raised_count(&pool).await,
            0,
            "one tick short of the evidence"
        );
        ticks(&pool, &mut tracker, 1, ACTIVE, false).await;
        assert_eq!(raised_count(&pool).await, 1, "a real stall must raise");
    }

    #[tokio::test]
    async fn a_long_absence_never_raises() {
        // Overnight or a long lunch: the newest frame is hours old and the
        // user has not touched the machine. Capture is silent by design.
        let pool = seeded_pool(Some(chrono::Duration::hours(11))).await;
        let mut tracker = StallTracker::default();
        ticks(&pool, &mut tracker, 200, AWAY, false).await;
        assert_eq!(raised_count(&pool).await, 0, "being away is not a stall");
    }

    #[tokio::test]
    async fn only_active_ticks_count_toward_the_alarm() {
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        let mut tracker = StallTracker::default();
        ticks(&pool, &mut tracker, 40, AWAY, false).await;
        ticks(&pool, &mut tracker, 5, ACTIVE, false).await;
        assert_eq!(raised_count(&pool).await, 0, "5 active ticks is not enough");
        ticks(&pool, &mut tracker, 6, ACTIVE, false).await;
        assert_eq!(
            raised_count(&pool).await,
            1,
            "activity accumulates across idle gaps"
        );
    }

    #[tokio::test]
    async fn an_unreadable_idle_signal_counts_as_active() {
        // Falling back to the old elapsed-time rule beats hiding a real stall.
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        let mut tracker = StallTracker::default();
        ticks(&pool, &mut tracker, ACTIVE_STALL_TICKS + 1, None, false).await;
        assert_eq!(raised_count(&pool).await, 1);
    }

    #[tokio::test]
    async fn a_new_frame_resets_the_count_and_clears_the_notice() {
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        let mut tracker = StallTracker::default();
        ticks(&pool, &mut tracker, ACTIVE_STALL_TICKS + 1, ACTIVE, false).await;
        assert_eq!(raised_count(&pool).await, 1);
        assert!(tracker.warned);

        sqlx::query(
            "INSERT INTO capture_frames (timestamp, app_name, text_source) \
             VALUES (?1, 'TestApp', 'ocr')",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();
        ticks(&pool, &mut tracker, 1, ACTIVE, false).await;

        assert_eq!(raised_count(&pool).await, 0, "a landed frame must clear it");
        assert_eq!(tracker.active_ticks, 0);
        assert!(!tracker.warned, "the next episode must be logged again");
    }

    #[tokio::test]
    async fn a_pause_restarts_the_activity_count() {
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        let mut tracker = StallTracker::default();
        ticks(&pool, &mut tracker, ACTIVE_STALL_TICKS, ACTIVE, false).await;
        ticks(&pool, &mut tracker, 1, ACTIVE, true).await;
        assert_eq!(tracker.active_ticks, 0);
        ticks(&pool, &mut tracker, ACTIVE_STALL_TICKS - 1, ACTIVE, false).await;
        assert_eq!(
            raised_count(&pool).await,
            0,
            "the count must not span a pause"
        );
    }

    #[tokio::test]
    async fn a_healthy_pipeline_clears_any_stale_notice() {
        let pool = seeded_pool(Some(chrono::Duration::seconds(5))).await;
        // Seed a stale notice as if a prior tick had raised it.
        meridian::notices::raise_typed(
            &pool,
            meridian::notices::Notice {
                id: NOTICE_ID,
                severity: "error",
                title: "t",
                detail: "d",
                remedy: None,
                event_key: EVENT_KEY,
                deep_link: None,
            },
        )
        .await
        .unwrap();

        check_capture_health_inner(&pool, LONG_AGO, false, &mut StallTracker::default(), ACTIVE)
            .await;
        let raised: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
                .bind(NOTICE_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(raised, 0, "a recovered pipeline must clear the notice");
    }

    #[tokio::test]
    async fn a_stale_frame_within_the_startup_grace_period_does_not_raise() {
        // The classic false positive this module exists to avoid: the newest
        // frame is from a prior session (hours old), but the process itself
        // just started — capture hasn't had a chance to write yet.
        let pool = seeded_pool(Some(chrono::Duration::hours(6))).await;
        check_capture_health_inner(
            &pool,
            Duration::ZERO,
            false,
            &mut StallTracker::default(),
            ACTIVE,
        )
        .await;
        let raised: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
                .bind(NOTICE_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            raised, 0,
            "a launch still inside the startup grace period must not raise"
        );
    }

    #[tokio::test]
    async fn a_stalled_pipeline_while_paused_does_not_raise() {
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        check_capture_health_inner(&pool, LONG_AGO, true, &mut StallTracker::default(), ACTIVE)
            .await;
        let raised: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
                .bind(NOTICE_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(raised, 0, "a deliberate pause must never read as a stall");
    }

    #[tokio::test]
    async fn no_frames_ever_written_does_not_raise() {
        let pool = seeded_pool(None).await;
        check_capture_health_inner(&pool, LONG_AGO, false, &mut StallTracker::default(), ACTIVE)
            .await;
        let raised: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
                .bind(NOTICE_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(raised, 0, "an empty table has nothing to diagnose here");
    }
}
