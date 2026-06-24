//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// L1 capture-layer health checks. Since the slice-4b cutover, capture runs
// in-process (the tray writes `capture_frames` / `capture_ui_events` in
// meridian.db) — these probes read those tables. If capture is broken
// (Screen-Recording revoked → blank frames, Accessibility off → empty a11y
// tree, the tray not running → stale frames), the classifier receives garbage
// and gets blamed for it; these surface that as an L1 fault. All probes are
// content-free: they read counts, timestamps, and text *presence* — never the
// captured text itself.

use crate::health::Check;
use sqlx::SqlitePool;

/// Newest frame older than this ⇒ capture likely stopped (or the machine is
/// asleep). Warn, not critical — a sleeping machine is a legitimate cause.
const STALE_FRAMES_SECS: f64 = 600.0;
/// Sample size for the most-recent-frames ratio checks. By id (not wall-clock)
/// so the checks still work when the machine has been idle/asleep.
const RECENT_SAMPLE: i64 = 500;

/// Run all L1 capture checks against meridian's own `capture_frames` /
/// `capture_ui_events` tables. Since the slice-4b cutover the tray captures
/// in-process and writes these tables directly — screenpipe is no longer
/// involved, so the old screenpipe binary/DB-file/log/WAL prerequisite probes
/// are gone. `pool` is the meridian pool (the daemon's own DB, always open).
pub async fn checks(pool: &SqlitePool) -> Vec<Check> {
    let (frames, state) = frames_present(pool).await;
    let mut checks = vec![frames];
    match state {
        // Frames exist — the runtime quality proxies are meaningful (blank rate →
        // Screen Recording, a11y share → Accessibility).
        FramesState::Present => {
            checks.push(frame_freshness(pool).await);
            checks.push(blank_text_rate(pool).await);
            checks.extend(accessibility_checks(pool).await);
            checks.push(capture_coverage(pool, "-1 day").await);
        }
        // Fresh install / nothing captured yet: the runtime proxies are blind,
        // so surface the permission prerequisite explicitly.
        FramesState::Empty => checks.push(permissions_unverified()),
        // The table is unreadable (schema/migration fault) — NOT a macOS
        // permission problem, so a permission remedy would mislead. The critical
        // `capture.frames` message already names the real cause; stand alone.
        FramesState::Unreadable => {}
    }
    checks
}

/// Why `capture_frames` is not yielding rows — distinguishes the cases that look
/// identical at `Severity::Critical` but need different remediation: an empty
/// table (grant permissions) vs an unreadable one (fix the schema/migration).
enum FramesState {
    Present,
    Empty,
    Unreadable,
}

/// Prerequisite for the fresh-install / no-frames case, where the runtime
/// blank/a11y proxies cannot run. macOS TCC permission state isn't cleanly
/// readable from here, so this is an explicit "unverified — grant and re-run"
/// prompt rather than a false pass.
fn permissions_unverified() -> Check {
    Check::warn(
        "capture.permissions",
        "L1",
        "no frames captured yet — Screen Recording / Accessibility may not be granted to Meridian",
    )
    .with_remedy(
        "System Settings ▸ Privacy & Security ▸ grant Screen Recording AND Accessibility to Meridian, then re-run doctor",
    )
}

/// The `capture_frames` table is readable and non-empty. An unreadable table
/// means a schema/migration problem, which breaks every ETL tick. Returns the
/// [`FramesState`] alongside the [`Check`] so the caller can tell an empty table
/// (permission prerequisite) apart from an unreadable one (schema fault).
async fn frames_present(pool: &SqlitePool) -> (Check, FramesState) {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM capture_frames")
        .fetch_one(pool)
        .await
    {
        Ok(0) => (
            Check::critical(
                "capture.frames",
                "L1",
                "capture_frames is empty — the tray has captured nothing yet",
            ),
            FramesState::Empty,
        ),
        Ok(n) => (
            Check::ok("capture.frames", "L1", format!("{n} frames total")),
            FramesState::Present,
        ),
        Err(e) => (
            Check::critical(
                "capture.frames",
                "L1",
                format!("capture_frames unreadable ({e}) — schema/migration problem?"),
            ),
            FramesState::Unreadable,
        ),
    }
}

/// Age of the newest frame. Uses julianday() arithmetic (not string compare) so
/// it is robust to the timestamp format/timezone. A future timestamp surfaces
/// clock/timezone skew rather than hiding it.
async fn frame_freshness(pool: &SqlitePool) -> Check {
    let age = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT (julianday('now') - julianday(MAX(timestamp))) * 86400.0 FROM capture_frames",
    )
    .fetch_one(pool)
    .await;
    match age {
        Ok(Some(secs)) if secs < -60.0 => Check::warn(
            "capture.freshness",
            "L1",
            format!(
                "newest frame is {:.0}s in the future — clock/timezone skew",
                -secs
            ),
        ),
        Ok(Some(secs)) if secs > STALE_FRAMES_SECS => Check::warn(
            "capture.freshness",
            "L1",
            format!(
                "newest frame is {:.0}s old (> {:.0}s) — capture stopped or machine asleep",
                secs, STALE_FRAMES_SECS
            ),
        ),
        Ok(Some(secs)) => Check::ok(
            "capture.freshness",
            "L1",
            format!("newest frame {:.0}s ago", secs.max(0.0)),
        ),
        Ok(None) => Check::warn("capture.freshness", "L1", "no frame timestamps to measure"),
        Err(e) => Check::warn(
            "capture.freshness",
            "L1",
            format!("could not read frame freshness ({e})"),
        ),
    }
}

/// Fraction of the most-recent frames with no extractable text. A high blank
/// rate is the content-free proxy for Screen-Recording permission revoked
/// (screenpipe captures black frames → OCR yields nothing).
async fn blank_text_rate(pool: &SqlitePool) -> Check {
    let row = sqlx::query_as::<_, (i64, i64)>(
        // NULLIF('') treats an empty string as absent, so an empty full_text does
        // not mask a non-empty accessibility_text (and vice versa) — only a frame
        // with NO usable text in either column counts as blank.
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN COALESCE(NULLIF(full_text, ''), NULLIF(accessibility_text, '')) IS NULL
                                  THEN 1 ELSE 0 END), 0)
         FROM (SELECT full_text, accessibility_text FROM capture_frames ORDER BY id DESC LIMIT ?1)",
    )
    .bind(RECENT_SAMPLE)
    .fetch_one(pool)
    .await;
    match row {
        Ok((0, _)) => Check::ok("capture.text_present", "L1", "no recent frames sampled"),
        Ok((total, blank)) => {
            let pct = 100.0 * blank as f64 / total as f64;
            let detail = format!("{blank}/{total} recent frames blank ({pct:.0}%)");
            if pct >= 90.0 {
                Check::critical(
                    "capture.text_present",
                    "L1",
                    format!("{detail} — Screen Recording permission likely revoked"),
                )
                .with_remedy(
                    "re-grant Screen Recording to Meridian in System Settings, then restart it",
                )
            } else if pct >= 50.0 {
                Check::warn(
                    "capture.text_present",
                    "L1",
                    format!("{detail} — degraded OCR/capture"),
                )
            } else {
                Check::ok("capture.text_present", "L1", detail)
            }
        }
        Err(e) => Check::warn(
            "capture.text_present",
            "L1",
            format!("could not sample frame text ({e})"),
        ),
    }
}

/// Per-app accessibility-tree yield. Accessibility *permission* is global (all
/// apps lose a11y if it is off), but a11y *yield* is per-app — some apps simply
/// expose little tree even with permission granted. A single blended average
/// conflates the two: an app-mix heavy in low-yield apps reads as "permission
/// off". So we split it:
///   - permission verdict = "is ANY well-sampled app yielding healthy a11y?"
///     (a robust oracle, immune to app mix); critical only if none is.
///   - a per-app breakdown (Info) so the operator sees which apps are OCR-only
///     — that is itself a fault-attribution signal (those sessions are degraded
///     input to the classifier, not model errors).
const A11Y_HEALTHY_PCT: f64 = 30.0;
const A11Y_MIN_APP_FRAMES: i64 = 20;
const A11Y_RECENT_WINDOW: i64 = 2000;
const A11Y_MAX_APPS_SHOWN: usize = 8;
/// (a) Degraded-capture note: an app at/under this a11y share is OCR-dominant.
const A11Y_OCR_DOMINANT_PCT: f64 = 20.0;
/// ...and needs this many frames in the window to count as high-usage (you
/// actually spend time there, so its OCR-only sessions matter).
const A11Y_SIGNIFICANT_FRAMES: i64 = 50;
/// (b) Regression: baseline window of older frames to compare the recent one to.
const A11Y_BASELINE_WINDOW: i64 = 30000;
/// An app whose a11y was at least this healthy in the baseline...
const A11Y_BASELINE_HEALTHY_PCT: f64 = 50.0;
/// ...but has fallen under this recently ⇒ a regression worth a warning.
const A11Y_REGRESSED_PCT: f64 = 20.0;

fn share_pct(n: i64, a11y: i64) -> f64 {
    if n > 0 {
        100.0 * a11y as f64 / n as f64
    } else {
        0.0
    }
}

async fn accessibility_checks(pool: &SqlitePool) -> Vec<Check> {
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT app_name, COUNT(*) AS n,
                COALESCE(SUM(CASE WHEN COALESCE(text_source, '') = 'accessibility'
                                  THEN 1 ELSE 0 END), 0) AS a11y
         FROM (SELECT app_name, text_source FROM capture_frames ORDER BY id DESC LIMIT ?1)
         WHERE app_name IS NOT NULL AND app_name != ''
         GROUP BY app_name
         HAVING n >= ?2
         ORDER BY n DESC",
    )
    .bind(A11Y_RECENT_WINDOW)
    .bind(A11Y_MIN_APP_FRAMES)
    .fetch_all(pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            return vec![Check::warn(
                "capture.a11y",
                "L1",
                format!("could not sample per-app a11y ({e})"),
            )]
        }
    };
    if rows.is_empty() {
        return vec![Check::ok(
            "capture.a11y",
            "L1",
            "no app with a meaningful recent sample",
        )];
    }

    let share = |n: i64, a11y: i64| {
        if n > 0 {
            100.0 * a11y as f64 / n as f64
        } else {
            0.0
        }
    };
    let (best_app, best_pct) = rows
        .iter()
        .map(|(app, n, a)| (app.as_str(), share(*n, *a)))
        .fold(("", 0.0_f64), |acc, x| if x.1 > acc.1 { x } else { acc });

    // Permission verdict: driven by the best-yielding app, not the average.
    let verdict = if best_pct >= A11Y_HEALTHY_PCT {
        Check::ok(
            "capture.a11y_permission",
            "L1",
            format!("Accessibility granted — best: {best_app} {best_pct:.0}%"),
        )
    } else if best_pct > 0.0 {
        Check::warn(
            "capture.a11y_permission",
            "L1",
            format!("weak a11y across all apps (best {best_app} {best_pct:.0}%) — Accessibility may be off"),
        )
        .with_remedy("grant Accessibility to Meridian in System Settings, then restart it")
    } else {
        Check::critical(
            "capture.a11y_permission",
            "L1",
            "no app is yielding any a11y — Accessibility permission likely off",
        )
        .with_remedy(
            "System Settings ▸ Privacy & Security ▸ Accessibility ▸ enable Meridian, then restart it",
        )
    };

    // Per-app breakdown (most-used first) — diagnostic, never a fault.
    let parts: Vec<String> = rows
        .iter()
        .take(A11Y_MAX_APPS_SHOWN)
        .map(|(app, n, a)| format!("{app} {:.0}%", share(*n, *a)))
        .collect();
    let more = rows.len().saturating_sub(A11Y_MAX_APPS_SHOWN);
    let suffix = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    let per_app = Check::info(
        "capture.a11y_per_app",
        "L1",
        format!("{}{}", parts.join(" · "), suffix),
    );

    // (a) Degraded-capture note: high-usage apps that are OCR-dominant. Info,
    // not a fault — it's expected — but it tells you which sessions feed the
    // classifier lower-fidelity input (relevant when attributing a miss).
    let degraded = degraded_apps(&rows);
    let degraded_check = if degraded.is_empty() {
        Check::ok(
            "capture.a11y_degraded",
            "L1",
            "active apps yield usable text",
        )
    } else {
        Check::info(
            "capture.a11y_degraded",
            "L1",
            format!("OCR-only (degraded input): {}", degraded.join(" · ")),
        )
    };

    // (b) Regression: an app that *used to* yield a11y but recently dropped to
    // OCR-only — a real change (capture broke / app updated), unlike an
    // always-low app. Needs the baseline window, so it queries again.
    let regression = a11y_regression(pool, A11Y_RECENT_WINDOW, A11Y_BASELINE_WINDOW).await;

    vec![verdict, per_app, degraded_check, regression]
}

/// High-usage apps whose recent capture is OCR-dominant. Returns "App pct%".
fn degraded_apps(rows: &[(String, i64, i64)]) -> Vec<String> {
    rows.iter()
        .filter(|(_, n, a)| {
            *n >= A11Y_SIGNIFICANT_FRAMES && share_pct(*n, *a) < A11Y_OCR_DOMINANT_PCT
        })
        .map(|(app, n, a)| format!("{app} {:.0}%", share_pct(*n, *a)))
        .collect()
}

/// Compare each app's a11y share in the recent window against an older baseline;
/// flag apps that were healthy then but OCR-only now. `recent_window`/
/// `base_window` are frame counts (parameterised for testing).
async fn a11y_regression(pool: &SqlitePool, recent_window: i64, base_window: i64) -> Check {
    let max_id = sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(id), 0) FROM capture_frames")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let recent_start = max_id - recent_window;
    let base_start = max_id - recent_window - base_window;

    let rows = sqlx::query_as::<_, (String, i64, i64, i64, i64)>(
        "SELECT app_name,
                SUM(CASE WHEN id > ?1 THEN 1 ELSE 0 END) AS recent_n,
                SUM(CASE WHEN id > ?1 AND COALESCE(text_source,'')='accessibility' THEN 1 ELSE 0 END) AS recent_a,
                SUM(CASE WHEN id <= ?1 THEN 1 ELSE 0 END) AS base_n,
                SUM(CASE WHEN id <= ?1 AND COALESCE(text_source,'')='accessibility' THEN 1 ELSE 0 END) AS base_a
         FROM capture_frames
         WHERE id > ?2 AND app_name IS NOT NULL AND app_name != ''
         GROUP BY app_name
         HAVING recent_n >= ?3 AND base_n >= 50",
    )
    .bind(recent_start)
    .bind(base_start)
    .bind(A11Y_MIN_APP_FRAMES)
    .fetch_all(pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            return Check::info(
                "capture.a11y_regression",
                "L1",
                format!("not available ({e})"),
            )
        }
    };
    let regressed: Vec<String> = rows
        .iter()
        .filter_map(|(app, rn, ra, bn, ba)| {
            let recent = share_pct(*rn, *ra);
            let base = share_pct(*bn, *ba);
            (base >= A11Y_BASELINE_HEALTHY_PCT && recent < A11Y_REGRESSED_PCT)
                .then(|| format!("{app} {base:.0}%→{recent:.0}%"))
        })
        .collect();
    if regressed.is_empty() {
        Check::ok("capture.a11y_regression", "L1", "no a11y regressions")
    } else {
        Check::warn(
            "capture.a11y_regression",
            "L1",
            format!("a11y dropped for: {}", regressed.join(" · ")),
        )
        .with_remedy("quit and reopen the affected app to rebuild its accessibility tree (Electron apps build it only at launch); an app update can also drop a11y support")
    }
}

/// Capture-coverage cross-check: every app the user focuses must produce
/// frames. `capture_ui_events` records `app_switch`/`window_focus` from the
/// in-process input recorder (a push-based source independent of the frame
/// pipeline), so it keeps seeing an app even when frame capture has gone blind
/// to it — the Electron "ghost app" failure: a Chromium/Electron app whose AX
/// tree never built, whose frames get misattributed or dedup-dropped, so its
/// activity silently vanishes from the timeline. Focused-but-frameless is the
/// content-free signature of that whole failure class.
///
/// Calibration (live data): healthy apps run ≥ 1 frame per focus event (each
/// window_focus trigger writes a frame, plus click/typing captures); ghosted
/// apps sit at ~0. Apps with fewer than `COVERAGE_MIN_FOCUS` focus events are
/// skipped — too little usage to judge.
const COVERAGE_MIN_FOCUS: i64 = 5;
const COVERAGE_WARN_RATIO: f64 = 0.5;
/// System processes that legitimately take focus without producing frames
/// (lock screen, permission dialogs, notification banners).
const COVERAGE_IGNORE_APPS: &[&str] = &[
    "loginwindow",
    "ScreenSaverEngine",
    "UserNotificationCenter",
    "universalAccessAuthWarn",
    "Dock",
    "Spotlight",
    "Control Center",
    "Notification Center",
    "CoreServicesUIAgent",
];

async fn capture_coverage(pool: &SqlitePool, window: &str) -> Check {
    // Use julianday() for both sides of the comparison: `datetime('now', ?1)`
    // produces '2026-06-23 17:00:00' (space separator, no Z), but
    // insert_capture_frame writes RFC 3339 '2026-06-24T16:00:00.000000Z'
    // (T + Z). String comparison at index 10 would have 'T' (0x54) > ' '
    // (0x20), so every row passes the filter regardless of date — the window
    // never excludes anything. julianday() normalises both formats internally.
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT u.app_name, u.f, COALESCE(fr.n, 0)
         FROM (SELECT app_name, COUNT(*) AS f
               FROM capture_ui_events
               WHERE event_type IN ('app_switch', 'window_focus')
                 AND julianday(timestamp) > julianday('now', ?1)
                 AND app_name IS NOT NULL AND app_name != ''
               GROUP BY app_name
               HAVING f >= ?2) u
         LEFT JOIN (SELECT app_name, COUNT(*) AS n
                    FROM capture_frames
                    WHERE julianday(timestamp) > julianday('now', ?1)
                    GROUP BY app_name) fr
           ON fr.app_name = u.app_name",
    )
    .bind(window)
    .bind(COVERAGE_MIN_FOCUS)
    .fetch_all(pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        // ui_events may not exist on older screenpipe versions — nothing to
        // assert, not a fault.
        Err(_) => {
            return Check::info(
                "capture.capture_coverage",
                "L1",
                "ui_events not available — coverage cross-check skipped",
            )
        }
    };

    let mut ghosted: Vec<String> = Vec::new();
    let mut degraded: Vec<String> = Vec::new();
    for (app, focus, frames) in &rows {
        if COVERAGE_IGNORE_APPS.contains(&app.as_str()) {
            continue;
        }
        if *frames == 0 {
            ghosted.push(format!("{app} ({focus} focus events, 0 frames)"));
        } else if (*frames as f64) < (*focus as f64) * COVERAGE_WARN_RATIO {
            degraded.push(format!("{app} ({focus} focus → {frames} frames)"));
        }
    }

    if !ghosted.is_empty() {
        Check::critical(
            "capture.capture_coverage",
            "L1",
            format!(
                "focused but producing NO frames: {} — their activity is invisible to the timeline",
                ghosted.join(" · ")
            ),
        )
        .with_remedy(
            "quit and reopen the affected app (Electron apps build their accessibility tree only at launch); if it persists, restart Meridian (meridian restart) and re-run doctor",
        )
    } else if !degraded.is_empty() {
        Check::warn(
            "capture.capture_coverage",
            "L1",
            format!("low frame yield for: {}", degraded.join(" · ")),
        )
        .with_remedy("restart the affected app, then re-run doctor; persistent low yield means captures are being dropped or misattributed")
    } else {
        Check::ok(
            "capture.capture_coverage",
            "L1",
            format!("all {} actively-used apps are producing frames", rows.len()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::Severity;
    use sqlx::sqlite::SqlitePoolOptions;

    /// In-memory capture-table DB. max_connections(1) keeps the one
    /// in-memory database alive across queries.
    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE capture_frames(
                id INTEGER PRIMARY KEY,
                app_name TEXT, timestamp TEXT,
                full_text TEXT, accessibility_text TEXT, text_source TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE capture_ui_events(
                id INTEGER PRIMARY KEY,
                timestamp TEXT, event_type TEXT, app_name TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_focus(pool: &SqlitePool, app: &str, n: usize) {
        for _ in 0..n {
            sqlx::query(
                "INSERT INTO capture_ui_events(timestamp, event_type, app_name)
                 VALUES(strftime('%Y-%m-%dT%H:%M:%SZ','now'), 'window_focus', ?1)",
            )
            .bind(app)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    async fn insert(
        pool: &SqlitePool,
        app: &str,
        full: Option<&str>,
        a11y: Option<&str>,
        src: &str,
        n: usize,
    ) {
        for _ in 0..n {
            sqlx::query(
                "INSERT INTO capture_frames(app_name, timestamp, full_text, accessibility_text, text_source)
                 VALUES(?1, strftime('%Y-%m-%dT%H:%M:%SZ','now'), ?2, ?3, ?4)",
            )
            .bind(app)
            .bind(full)
            .bind(a11y)
            .bind(src)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn empty_frames_is_critical() {
        let pool = mem_pool().await;
        let (check, state) = frames_present(&pool).await;
        assert_eq!(check.severity, Severity::Critical);
        assert!(matches!(state, FramesState::Empty));
    }

    #[tokio::test]
    async fn all_blank_flags_screen_recording() {
        let pool = mem_pool().await;
        insert(&pool, "A", Some(""), Some(""), "ocr", 20).await;
        assert_eq!(blank_text_rate(&pool).await.severity, Severity::Critical);
    }

    #[tokio::test]
    async fn healthy_a11y_capture_is_ok() {
        let pool = mem_pool().await;
        insert(&pool, "A", None, Some("button: Save"), "accessibility", 20).await;
        let (check, state) = frames_present(&pool).await;
        assert_eq!(check.severity, Severity::Ok);
        assert!(matches!(state, FramesState::Present));
        assert_eq!(blank_text_rate(&pool).await.severity, Severity::Ok);
        assert_eq!(frame_freshness(&pool).await.severity, Severity::Ok);
        let checks = accessibility_checks(&pool).await;
        assert_eq!(checks[0].severity, Severity::Ok); // permission verdict
    }

    #[tokio::test]
    async fn permission_ok_when_any_app_has_a11y() {
        // The whole point of per-app: a low-a11y app (Chrome) must NOT trigger a
        // false "permission off" when another app (Code) clearly has a11y.
        let pool = mem_pool().await;
        insert(
            &pool,
            "Code",
            None,
            Some("editor tree"),
            "accessibility",
            30,
        )
        .await;
        insert(&pool, "Chrome", Some("page text"), None, "ocr", 30).await;
        let checks = accessibility_checks(&pool).await;
        assert_eq!(checks[0].severity, Severity::Ok); // best app drives the verdict
        assert!(checks.iter().any(|c| c.name == "capture.a11y_per_app"));
    }

    #[tokio::test]
    async fn no_app_with_a11y_flags_permission_off() {
        let pool = mem_pool().await;
        insert(&pool, "Code", Some("text"), None, "ocr", 30).await;
        insert(&pool, "Chrome", Some("text"), None, "ocr", 30).await;
        let checks = accessibility_checks(&pool).await;
        assert_eq!(checks[0].severity, Severity::Critical);
        assert!(checks[0].remedy.is_some());
    }

    #[test]
    fn permissions_prompt_is_actionable() {
        // The no-frames prerequisite must carry a remedy, not a silent pass.
        let c = permissions_unverified();
        assert_eq!(c.severity, Severity::Warn);
        assert!(c.remedy.is_some());
    }

    #[test]
    fn degraded_apps_flags_high_usage_ocr_only() {
        let rows = vec![
            ("Code".to_string(), 100, 95),  // 95% a11y → not degraded
            ("DBeaver".to_string(), 80, 8), // 10% a11y, high usage → degraded
            ("Quick".to_string(), 10, 0),   // 0% but < 50 frames → excluded
        ];
        let d = degraded_apps(&rows);
        assert_eq!(d.len(), 1);
        assert!(d[0].contains("DBeaver"));
    }

    #[tokio::test]
    async fn a11y_regression_flags_a_drop_from_baseline() {
        let pool = mem_pool().await;
        // 60 older frames with a11y, then 30 recent frames OCR-only — same app.
        insert(&pool, "Code", None, Some("tree"), "accessibility", 60).await;
        insert(&pool, "Code", Some("pixels"), None, "ocr", 30).await;
        // small windows so the recent 30 vs older 60 split cleanly
        let c = a11y_regression(&pool, 30, 60).await;
        assert_eq!(c.severity, Severity::Warn);
        assert!(c.remedy.is_some());
    }

    #[tokio::test]
    async fn a11y_regression_quiet_when_stable() {
        let pool = mem_pool().await;
        insert(&pool, "Code", None, Some("tree"), "accessibility", 60).await;
        insert(&pool, "Code", None, Some("tree"), "accessibility", 30).await;
        let c = a11y_regression(&pool, 30, 60).await;
        assert_eq!(c.severity, Severity::Ok);
    }

    #[tokio::test]
    async fn coverage_flags_focused_but_frameless_app() {
        // The Electron ghost signature: the user focuses the app (ui_events
        // sees it) but no frames carry its name.
        let pool = mem_pool().await;
        insert_focus(&pool, "Codex", 10).await;
        insert(&pool, "Code", None, Some("tree"), "accessibility", 30).await;
        insert_focus(&pool, "Code", 10).await;
        let c = capture_coverage(&pool, "-1 day").await;
        assert_eq!(c.severity, Severity::Critical);
        assert!(c.detail.contains("Codex"));
        assert!(c.remedy.is_some());
    }

    #[tokio::test]
    async fn coverage_warns_on_low_frame_yield() {
        let pool = mem_pool().await;
        insert_focus(&pool, "Claude", 10).await;
        insert(&pool, "Claude", None, Some("t"), "accessibility", 2).await; // 0.2 < 0.5
        let c = capture_coverage(&pool, "-1 day").await;
        assert_eq!(c.severity, Severity::Warn);
        assert!(c.detail.contains("Claude"));
    }

    #[tokio::test]
    async fn coverage_quiet_when_healthy_and_below_threshold() {
        let pool = mem_pool().await;
        // Healthy app: frames ≥ focus events.
        insert_focus(&pool, "Code", 10).await;
        insert(&pool, "Code", None, Some("t"), "accessibility", 30).await;
        // Frameless but under COVERAGE_MIN_FOCUS — too little usage to judge.
        insert_focus(&pool, "Briefly", 2).await;
        // Frameless but a known system process — ignored.
        insert_focus(&pool, "loginwindow", 10).await;
        let c = capture_coverage(&pool, "-1 day").await;
        assert_eq!(c.severity, Severity::Ok);
    }

    // NOTE: `a11y_helper_verdict_parses_trust_states` and
    // `accessibility_log_verdict_latest_line_wins` were removed in the slice-4b
    // cutover along with the screenpipe/a11y-helper log probes they covered —
    // capture is in-process now (no screenpipe log, no separate a11y-helper).
}
