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
//! # Debounce is unnecessary here, unlike `permissions.rs`
//!
//! [`super::permissions`] must debounce because a single boolean TCC read can
//! be a launch-time phantom (see its module doc's 2026-08-16 incident). This
//! check instead measures elapsed time since the last real frame — the
//! evidence already accumulates in the query itself. A single query result
//! IS the accumulated signal; there is nothing to smooth.
//!
//! [`STARTUP_GRACE`] plays the equivalent role: a fresh launch's newest frame
//! is very often from a PRIOR session (last night, last week), so the very
//! first health tick after launch would otherwise read as a huge, entirely
//! normal gap and raise instantly. The grace period only has to outlast
//! startup lag (plugin registration, the capture engine's first tick), not
//! [`STALL_THRESHOLD`] itself — once one fresh frame lands, the gap collapses
//! to seconds.
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

/// Re-check whether capture is actually producing frames, and raise/clear
/// [`NOTICE_ID`] accordingly. Runs on the same `do_health` cadence as
/// [`super::permissions::check_permissions`] (~60s).
///
/// `loop_started` and `paused` are passed in rather than read from a shared
/// state so the decision stays a pure function of its inputs — mirrors
/// [`super::permissions::PermissionDebounce::observe`]'s own reasoning.
pub(super) async fn check_capture_health(pool: &SqlitePool, loop_started: Instant, paused: bool) {
    if !crate::sys::is_bundled() {
        return;
    }
    check_capture_health_inner(pool, loop_started, paused).await;
}

/// The `is_bundled()`-gated body of [`check_capture_health`], split out so
/// tests can drive it directly — `cargo test` never runs from inside a
/// `.app` bundle, so a test calling the outer function would always hit that
/// gate and silently exercise nothing (same reasoning as
/// [`super::permissions`]'s pure-function split).
async fn check_capture_health_inner(pool: &SqlitePool, loop_started: Instant, paused: bool) {
    // Capture is legitimately silent for a pause the user or a schedule
    // chose — that is not "stalled", and clearing here would be wrong too:
    // a genuine stall discovered just before a pause starts should still
    // read as raised when the pause ends, not be silently forgotten.
    if paused {
        return;
    }
    if loop_started.elapsed() < STARTUP_GRACE {
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

    let gap = chrono::Utc::now().signed_duration_since(last_ts);
    let stalled = gap > chrono::Duration::from_std(STALL_THRESHOLD).unwrap();

    let result = if stalled {
        tracing::warn!(last_ts = %last_ts, "capture_health: no new frame within the stall threshold");
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
    } else {
        meridian::notices::clear_typed(pool, NOTICE_ID, EVENT_KEY).await
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

    fn long_ago() -> Instant {
        // `Instant` cannot be constructed in the past directly; subtracting
        // from `now()` is the standard way to backdate one in a test.
        Instant::now() - Duration::from_secs(3600)
    }

    #[tokio::test]
    async fn a_stalled_pipeline_past_the_grace_period_raises() {
        let pool = seeded_pool(Some(chrono::Duration::hours(1))).await;
        check_capture_health_inner(&pool, long_ago(), false).await;
        let raised: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
                .bind(NOTICE_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(raised, 1, "a real stall past the grace period must raise");
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

        check_capture_health_inner(&pool, long_ago(), false).await;
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
        check_capture_health_inner(&pool, Instant::now(), false).await;
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
        check_capture_health_inner(&pool, long_ago(), true).await;
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
        check_capture_health_inner(&pool, long_ago(), false).await;
        let raised: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM system_notices WHERE notice_id = ?1")
                .bind(NOTICE_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(raised, 0, "an empty table has nothing to diagnose here");
    }
}
