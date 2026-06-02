// meridian — normalises screenpipe activity into structured app sessions
//
// L1 capture-layer health checks. Meridian reads screenpipe's frames read-only;
// if capture is broken (screen-recording permission revoked → blank frames,
// accessibility permission off → empty a11y tree, screenpipe dead → stale
// frames), the classifier receives garbage and gets blamed for it. These probes
// surface that as an L1 fault. All probes are content-free: they read counts,
// timestamps, and text *presence* — never the captured text itself.

use crate::config::Config;
use crate::health::Check;
use sqlx::SqlitePool;

/// Newest frame older than this ⇒ capture likely stopped (or the machine is
/// asleep). Warn, not critical — a sleeping machine is a legitimate cause.
const STALE_FRAMES_SECS: f64 = 600.0;
/// Sample size for the most-recent-frames ratio checks. By id (not wall-clock)
/// so the checks still work when the machine has been idle/asleep.
const RECENT_SAMPLE: i64 = 500;

/// Run all L1 capture checks. `pool` is `None` when the screenpipe DB could not
/// be opened read-only — the file-level check still runs and the rest report
/// the open failure.
pub async fn checks(cfg: &Config, pool: Option<&SqlitePool>) -> Vec<Check> {
    // Prerequisites first (can capture run at all?), then runtime health.
    let mut checks = vec![screenpipe_installed(), db_present(cfg)];
    match pool {
        Some(p) => {
            let frames = frames_present(p).await;
            // The ratio checks are only meaningful once frames exist.
            let have_frames = frames.severity != crate::health::Severity::Critical;
            checks.push(frames);
            if have_frames {
                // Runtime capture quality — these also serve as the permission
                // proxies (blank rate → Screen Recording, a11y share → Accessibility).
                checks.push(frame_freshness(p).await);
                checks.push(blank_text_rate(p).await);
                checks.extend(accessibility_checks(p).await);
            } else {
                // Fresh install / nothing captured: the runtime proxies are blind,
                // so surface the permission prerequisite explicitly.
                checks.push(permissions_unverified());
            }
            checks.push(wal_size(cfg));
        }
        None => checks.push(
            Check::critical(
                "screenpipe.pool",
                "L1",
                "could not open screenpipe DB read-only (path wrong, locked, or corrupt)",
            )
            .with_remedy(
                "check SCREENPIPE_DB points at ~/.screenpipe/db.sqlite and screenpipe is running",
            ),
        ),
    }
    checks
}

/// Prerequisite: the screenpipe binary is installed (on PATH). Without it there
/// is no capture at all. Content-free — a filesystem lookup only.
fn screenpipe_installed() -> Check {
    match which("screenpipe") {
        Some(path) => Check::ok("screenpipe.installed", "L1", format!("{}", path.display())),
        None => Check::critical("screenpipe.installed", "L1", "not found on PATH").with_remedy(
            "install screenpipe (e.g. brew install screenpipe) and load its launchd service",
        ),
    }
}

/// Find an executable on PATH without pulling in the `which` crate.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|cand| cand.is_file())
}

/// Prerequisite for the fresh-install / no-frames case, where the runtime
/// blank/a11y proxies cannot run. macOS TCC permission state isn't cleanly
/// readable from here, so this is an explicit "unverified — grant and re-run"
/// prompt rather than a false pass.
fn permissions_unverified() -> Check {
    Check::warn(
        "screenpipe.permissions",
        "L1",
        "no frames captured yet — Screen Recording / Accessibility may not be granted",
    )
    .with_remedy(
        "System Settings ▸ Privacy & Security ▸ grant Screen Recording AND Accessibility to screenpipe, then restart it and re-run doctor",
    )
}

/// The screenpipe DB file exists and is non-empty. File-stat only — no pool.
fn db_present(cfg: &Config) -> Check {
    let path = &cfg.screenpipe_db;
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Check::ok(
            "screenpipe.db_present",
            "L1",
            format!("{path} ({} KB)", m.len() / 1024),
        ),
        Ok(_) => Check::critical(
            "screenpipe.db_present",
            "L1",
            format!("{path} exists but is empty — screenpipe never captured"),
        )
        .with_remedy("start screenpipe and grant it Screen Recording, then re-run doctor"),
        Err(e) => Check::critical(
            "screenpipe.db_present",
            "L1",
            format!("{path} not found ({e}) — is screenpipe installed/running?"),
        )
        .with_remedy("install + start screenpipe so it creates ~/.screenpipe/db.sqlite"),
    }
}

/// The `frames` table is readable and non-empty. An unreadable table means
/// screenpipe schema drift (renamed/missing table), which breaks every ETL tick.
async fn frames_present(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM frames")
        .fetch_one(pool)
        .await
    {
        Ok(0) => Check::critical(
            "screenpipe.frames",
            "L1",
            "frames table is empty — screenpipe has captured nothing",
        ),
        Ok(n) => Check::ok("screenpipe.frames", "L1", format!("{n} frames total")),
        Err(e) => Check::critical(
            "screenpipe.frames",
            "L1",
            format!("frames table unreadable ({e}) — screenpipe schema drift?"),
        ),
    }
}

/// Age of the newest frame. Uses julianday() arithmetic (not string compare) so
/// it is robust to the timestamp format/timezone. A future timestamp surfaces
/// clock/timezone skew rather than hiding it.
async fn frame_freshness(pool: &SqlitePool) -> Check {
    let age = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT (julianday('now') - julianday(MAX(timestamp))) * 86400.0 FROM frames",
    )
    .fetch_one(pool)
    .await;
    match age {
        Ok(Some(secs)) if secs < -60.0 => Check::warn(
            "screenpipe.freshness",
            "L1",
            format!(
                "newest frame is {:.0}s in the future — clock/timezone skew",
                -secs
            ),
        ),
        Ok(Some(secs)) if secs > STALE_FRAMES_SECS => Check::warn(
            "screenpipe.freshness",
            "L1",
            format!(
                "newest frame is {:.0}s old (> {:.0}s) — capture stopped or machine asleep",
                secs, STALE_FRAMES_SECS
            ),
        ),
        Ok(Some(secs)) => Check::ok(
            "screenpipe.freshness",
            "L1",
            format!("newest frame {:.0}s ago", secs.max(0.0)),
        ),
        Ok(None) => Check::warn(
            "screenpipe.freshness",
            "L1",
            "no frame timestamps to measure",
        ),
        Err(e) => Check::warn(
            "screenpipe.freshness",
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
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN COALESCE(full_text, accessibility_text) IS NULL
                                   OR COALESCE(full_text, accessibility_text) = ''
                                  THEN 1 ELSE 0 END), 0)
         FROM (SELECT full_text, accessibility_text FROM frames ORDER BY id DESC LIMIT ?1)",
    )
    .bind(RECENT_SAMPLE)
    .fetch_one(pool)
    .await;
    match row {
        Ok((0, _)) => Check::ok("screenpipe.text_present", "L1", "no recent frames sampled"),
        Ok((total, blank)) => {
            let pct = 100.0 * blank as f64 / total as f64;
            let detail = format!("{blank}/{total} recent frames blank ({pct:.0}%)");
            if pct >= 90.0 {
                Check::critical(
                    "screenpipe.text_present",
                    "L1",
                    format!("{detail} — Screen Recording permission likely revoked"),
                )
                .with_remedy(
                    "re-grant Screen Recording to screenpipe in System Settings, then restart it",
                )
            } else if pct >= 50.0 {
                Check::warn(
                    "screenpipe.text_present",
                    "L1",
                    format!("{detail} — degraded OCR/capture"),
                )
            } else {
                Check::ok("screenpipe.text_present", "L1", detail)
            }
        }
        Err(e) => Check::warn(
            "screenpipe.text_present",
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
         FROM (SELECT app_name, text_source FROM frames ORDER BY id DESC LIMIT ?1)
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
                "screenpipe.a11y",
                "L1",
                format!("could not sample per-app a11y ({e})"),
            )]
        }
    };
    if rows.is_empty() {
        return vec![Check::ok(
            "screenpipe.a11y",
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
            "screenpipe.a11y_permission",
            "L1",
            format!("Accessibility granted — best: {best_app} {best_pct:.0}%"),
        )
    } else if best_pct > 0.0 {
        Check::warn(
            "screenpipe.a11y_permission",
            "L1",
            format!("weak a11y across all apps (best {best_app} {best_pct:.0}%) — Accessibility may be off"),
        )
        .with_remedy("grant Accessibility to screenpipe in System Settings, then restart it")
    } else {
        Check::critical(
            "screenpipe.a11y_permission",
            "L1",
            "no app is yielding any a11y — Accessibility permission likely off",
        )
        .with_remedy(
            "System Settings ▸ Privacy & Security ▸ Accessibility ▸ enable screenpipe, then restart it",
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
        "screenpipe.a11y_per_app",
        "L1",
        format!("{}{}", parts.join(" · "), suffix),
    );

    // (a) Degraded-capture note: high-usage apps that are OCR-dominant. Info,
    // not a fault — it's expected — but it tells you which sessions feed the
    // classifier lower-fidelity input (relevant when attributing a miss).
    let degraded = degraded_apps(&rows);
    let degraded_check = if degraded.is_empty() {
        Check::ok(
            "screenpipe.a11y_degraded",
            "L1",
            "active apps yield usable text",
        )
    } else {
        Check::info(
            "screenpipe.a11y_degraded",
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
    let max_id = sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(id), 0) FROM frames")
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
         FROM frames
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
                "screenpipe.a11y_regression",
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
        Check::ok("screenpipe.a11y_regression", "L1", "no a11y regressions")
    } else {
        Check::warn(
            "screenpipe.a11y_regression",
            "L1",
            format!("a11y dropped for: {}", regressed.join(" · ")),
        )
        .with_remedy("restart screenpipe to re-establish a11y capture; if the app updated it may have dropped a11y support")
    }
}

/// screenpipe's WAL file size. An unbounded WAL (stalled checkpoint) means disk
/// pressure and slow reads. Absent WAL is healthy (checkpointed).
fn wal_size(cfg: &Config) -> Check {
    let wal = format!("{}-wal", cfg.screenpipe_db);
    match std::fs::metadata(&wal) {
        Ok(m) => {
            let mb = m.len() / 1_048_576;
            if mb > 1024 {
                Check::warn(
                    "screenpipe.wal",
                    "L1",
                    format!("WAL is {mb} MB — checkpoint may be stalled"),
                )
            } else {
                Check::ok("screenpipe.wal", "L1", format!("WAL {mb} MB"))
            }
        }
        Err(_) => Check::ok("screenpipe.wal", "L1", "no WAL (checkpointed)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::Severity;
    use sqlx::sqlite::SqlitePoolOptions;

    /// In-memory screenpipe-shaped DB. max_connections(1) keeps the one
    /// in-memory database alive across queries.
    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE frames(
                id INTEGER PRIMARY KEY,
                app_name TEXT, timestamp TEXT,
                full_text TEXT, accessibility_text TEXT, text_source TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
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
                "INSERT INTO frames(app_name, timestamp, full_text, accessibility_text, text_source)
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
        assert_eq!(frames_present(&pool).await.severity, Severity::Critical);
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
        assert_eq!(frames_present(&pool).await.severity, Severity::Ok);
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
        assert!(checks.iter().any(|c| c.name == "screenpipe.a11y_per_app"));
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
    fn which_resolves_real_and_missing() {
        assert!(which("sh").is_some());
        assert!(which("definitely_not_a_real_binary_xyz123").is_none());
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
}
