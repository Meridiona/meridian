//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The apps Meridian has actually captured recently — the picker source for the
//! Settings "Ignore an app" control. No route; new work for the capture-privacy
//! feature.
//!
//! # What this is
//! Distinct `app_name`s seen in `capture_frames` over a recent window, each with
//! a frame count (a rough "how much have I seen this" proxy — capture samples on
//! a fixed cadence, so count ≈ time), most-seen first. This lets the Settings UI
//! offer a one-tap chip list of the user's real apps instead of a blank text box.
//!
//! Meridian's own windows are never captured (`is_self_app` in the tray engine),
//! so the tray process never appears here — nothing to filter out.
//!
//! # Who calls this
//! The tray `get_recent_capture_apps` command →
//! `ui/components/timeline/settings/CaptureSection.tsx` (the ignore-apps picker).
//!
//! # Related
//! - [`crate::settings`] — where the chosen apps land (`ignored_apps`), enforced
//!   at capture time by `tray::capture_ignore`.

use crate::SqlitePool;
use serde::Serialize;
use tracing::Instrument;

/// One recently-captured app and how many frames carried it.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureApp {
    /// The `app_name` exactly as captured (what `ignored_apps` matches against).
    pub app: String,
    /// Frame count over the window — a recency/volume hint for the UI, not a
    /// precise duration.
    pub frames: i64,
}

#[derive(sqlx::FromRow)]
struct Row {
    app_name: String,
    frames: i64,
}

/// The apps seen in `capture_frames` over the last `days` days, most-frequent
/// first, capped at `limit`. NULL/empty `app_name` rows are excluded (the
/// reader already ignores them everywhere else).
///
/// A DB that predates `capture_frames` (migration 046) has no such table; that
/// degrades to an empty list rather than erroring — an un-migrated DB simply has
/// no capture history to offer, it is not a failure.
#[tracing::instrument(skip(pool))]
pub async fn recent_capture_apps(
    pool: &SqlitePool,
    days: i64,
    limit: i64,
) -> anyhow::Result<Vec<CaptureApp>> {
    // `timestamp` is RFC3339 UTC with constant width, so a lexical `>=` against
    // an ISO cutoff is a correct time comparison (same posture as every other
    // reader). `datetime('now', '-N days')` yields 'YYYY-MM-DD HH:MM:SS'; the
    // stored form is 'YYYY-MM-DDTHH:MM:SS.ffffffZ' — both sort correctly under
    // string comparison for the date prefix, and we only need day granularity.
    let rows = sqlx::query_as::<_, Row>(
        "SELECT app_name, COUNT(*) AS frames \
         FROM capture_frames \
         WHERE app_name IS NOT NULL AND app_name != '' \
           AND timestamp >= strftime('%Y-%m-%dT00:00:00.000000Z', datetime('now', ?)) \
         GROUP BY app_name \
         ORDER BY frames DESC \
         LIMIT ?",
    )
    .bind(format!("-{days} days"))
    .bind(limit)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("capture_apps.read.capture_frames"))
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "capture_apps: read skipped (pre-046 DB?)");
            return Ok(Vec::new());
        }
    };
    tracing::debug!(rows = rows.len(), "capture_apps.read.capture_frames");

    Ok(rows
        .into_iter()
        .map(|r| CaptureApp {
            app: r.app_name,
            frames: r.frames,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_frames() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE capture_frames (id INTEGER PRIMARY KEY AUTOINCREMENT, \
             timestamp TEXT NOT NULL, app_name TEXT, window_name TEXT, browser_url TEXT, \
             full_text TEXT, accessibility_text TEXT, text_source TEXT, capture_trigger TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert(pool: &SqlitePool, ts: &str, app: Option<&str>) {
        sqlx::query("INSERT INTO capture_frames (timestamp, app_name) VALUES (?, ?)")
            .bind(ts)
            .bind(app)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ranks_recent_apps_by_frame_count_and_drops_blanks() {
        let pool = pool_with_frames().await;
        // Two recent frames for Chrome, one for Code, plus a blank/NULL that must
        // not surface. Use a far-future-safe recent stamp via `now`.
        let now = "2999-01-01T10:00:00.000000Z";
        insert(&pool, now, Some("Google Chrome")).await;
        insert(&pool, now, Some("Google Chrome")).await;
        insert(&pool, now, Some("Code")).await;
        insert(&pool, now, Some("")).await;
        insert(&pool, now, None).await;

        // A very wide window so the fixed test stamps are always inside it.
        let apps = recent_capture_apps(&pool, 400_000, 50).await.unwrap();
        assert_eq!(apps.len(), 2, "blank + NULL app_name rows are excluded");
        assert_eq!(apps[0].app, "Google Chrome");
        assert_eq!(apps[0].frames, 2);
        assert_eq!(apps[1].app, "Code");
    }

    #[tokio::test]
    async fn a_pre_046_database_degrades_to_empty() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        assert!(recent_capture_apps(&pool, 30, 50).await.unwrap().is_empty());
    }
}
