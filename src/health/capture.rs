//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
    // Direct Accessibility-grant probe from screenpipe's own log (ground truth,
    // unlike the DB a11y-yield proxy below). Catches a grant silently dropped by
    // a reinstall/update before it shows up as OCR-only sessions.
    if let Some(c) = screenpipe_accessibility_permission(cfg) {
        checks.push(c);
    }
    checks.push(a11y_helper_status(cfg));
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
                checks.push(capture_coverage(p, "-1 day").await);
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

/// Direct Accessibility-permission probe from screenpipe's own log. macOS TCC
/// state isn't readable from our process, but screenpipe logs
/// `permission monitor started … accessibility=true|false` on every (re)start,
/// and that line is ground truth. A reinstall/update can silently drop the grant
/// (it attaches to a binary path/signature that changes), flipping a11y capture
/// to OCR-only with no other symptom — this surfaces it loudly. Returns `None`
/// when the log or the line can't be found (nothing to assert).
fn screenpipe_accessibility_permission(cfg: &Config) -> Option<Check> {
    let dir = std::path::Path::new(&cfg.screenpipe_db).parent()?;
    // Newest screenpipe.<date>.N.log by mtime — that's the live one.
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        let is_sp_log = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("screenpipe.") && n.ends_with(".log"))
            .unwrap_or(false);
        if !is_sp_log {
            continue;
        }
        if let Ok(m) = entry.metadata().and_then(|md| md.modified()) {
            if newest.as_ref().is_none_or(|(t, _)| m > *t) {
                newest = Some((m, p));
            }
        }
    }
    let log_path = newest?.1;
    let content = std::fs::read_to_string(&log_path).ok()?;
    accessibility_verdict_from_log(&content)
}

/// Pure verdict from screenpipe log text: scan for the LAST `permission monitor
/// started …` line (the current state after the most recent restart) and report
/// the Accessibility grant. Split out from the file I/O so it is unit-testable.
fn accessibility_verdict_from_log(content: &str) -> Option<Check> {
    let last = content
        .lines()
        .rfind(|l| l.contains("permission monitor started"))?;

    if last.contains("accessibility=false") {
        Some(
            Check::critical(
                "screenpipe.accessibility_grant",
                "L1",
                "screenpipe reports accessibility=false — a11y tree capture is OFF (OCR-only); the grant was likely dropped by a reinstall/update",
            )
            .with_remedy(
                "System Settings ▸ Privacy & Security ▸ Accessibility ▸ enable screenpipe, then restart it (meridian restart)",
            ),
        )
    } else if last.contains("accessibility=true") {
        Some(Check::ok(
            "screenpipe.accessibility_grant",
            "L1",
            "screenpipe reports accessibility=true (a11y capture enabled)",
        ))
    } else {
        None
    }
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

/// State of the meridian a11y-helper — the agent that enables accessibility on
/// Electron/Chromium apps (Claude, Codex, Slack, …) so screenpipe can capture
/// them. Without a working helper those apps are invisible to capture: they
/// ship with their AX tree disabled, never register with the AX focus tracker,
/// and their frames get misattributed or dedup-dropped. The helper logs its
/// trust state on every start/change; the last `AX trusted:` line is ground
/// truth (macOS TCC state isn't readable from this process).
fn a11y_helper_status(cfg: &Config) -> Check {
    let logs = std::path::Path::new(&cfg.meridian_db)
        .parent()
        .map(|d| d.join("logs/a11y-helper.log"));
    let content = logs.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    match content {
        None => Check::warn(
            "a11y_helper.installed",
            "L1",
            "a11y-helper log not found — Electron apps (Claude, Codex, …) may be invisible to capture",
        )
        .with_remedy("run scripts/install-a11y-helper-daemon.sh, then grant ~/.meridian/bin/meridian-a11y-helper Accessibility in System Settings"),
        Some(content) => a11y_helper_verdict_from_log(&content),
    }
}

/// Pure verdict from the helper log: the LAST `AX trusted:` line wins (the
/// helper re-logs on every state change). Split from the file I/O for tests.
fn a11y_helper_verdict_from_log(content: &str) -> Check {
    let last = content.lines().rfind(|l| l.contains("AX trusted:"));
    match last {
        Some(l) if l.contains("AX trusted: false") => Check::critical(
            "a11y_helper.trusted",
            "L1",
            "a11y-helper is running but NOT granted Accessibility — Electron apps (Claude, Codex, …) are invisible to capture",
        )
        .with_remedy(
            "System Settings ▸ Privacy & Security ▸ Accessibility ▸ add ~/.meridian/bin/meridian-a11y-helper and toggle it on",
        ),
        Some(_) => Check::ok(
            "a11y_helper.trusted",
            "L1",
            "a11y-helper trusted — Electron apps get their accessibility enabled on focus",
        ),
        None => Check::warn(
            "a11y_helper.trusted",
            "L1",
            "a11y-helper log has no trust-state line — helper may not be running",
        )
        .with_remedy("launchctl kickstart -k gui/$(id -u)/com.meridiona.a11y-helper, then re-run doctor"),
    }
}

/// Capture-coverage cross-check: every app the user focuses must produce
/// frames. `ui_events` records `app_switch`/`window_focus` with app names from
/// screenpipe's notification observer (a fresh, push-based source), so it keeps
/// seeing an app even when the frame pipeline has gone blind to it — exactly
/// the Electron "ghost app" failure: a Chromium/Electron app with accessibility
/// disabled never registers with the AX focus tracker, the walker attributes
/// its frames to the previously focused app (or dedup drops them), and the
/// app's activity silently vanishes from the timeline. Focused-but-frameless is
/// the content-free signature of that whole failure class.
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
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT u.app_name, u.f, COALESCE(fr.n, 0)
         FROM (SELECT app_name, COUNT(*) AS f
               FROM ui_events
               WHERE event_type IN ('app_switch', 'window_focus')
                 AND timestamp > datetime('now', ?1)
                 AND app_name IS NOT NULL AND app_name != ''
               GROUP BY app_name
               HAVING f >= ?2) u
         LEFT JOIN (SELECT app_name, COUNT(*) AS n
                    FROM frames
                    WHERE timestamp > datetime('now', ?1)
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
                "screenpipe.capture_coverage",
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
            "screenpipe.capture_coverage",
            "L1",
            format!(
                "focused but producing NO frames: {} — their activity is invisible to the timeline",
                ghosted.join(" · ")
            ),
        )
        .with_remedy(
            "quit and reopen the affected app (Electron apps build their accessibility tree only at launch); if it persists, restart screenpipe (meridian restart) and re-run doctor",
        )
    } else if !degraded.is_empty() {
        Check::warn(
            "screenpipe.capture_coverage",
            "L1",
            format!("low frame yield for: {}", degraded.join(" · ")),
        )
        .with_remedy("restart the affected app, then re-run doctor; persistent low yield means captures are being dropped or misattributed")
    } else {
        Check::ok(
            "screenpipe.capture_coverage",
            "L1",
            format!("all {} actively-used apps are producing frames", rows.len()),
        )
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
        sqlx::query(
            "CREATE TABLE ui_events(
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
                "INSERT INTO ui_events(timestamp, event_type, app_name)
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

    #[test]
    fn a11y_helper_verdict_parses_trust_states() {
        // Untrusted → critical with remedy.
        let c = a11y_helper_verdict_from_log(
            "2026-06-05T20:00:00Z a11y-helper: started (poll 3.0s)\n2026-06-05T20:00:00Z a11y-helper: AX trusted: false — grant Accessibility…\n",
        );
        assert_eq!(c.severity, Severity::Critical);
        assert!(c.remedy.is_some());

        // A later trusted:true overrides an earlier false — grants arrive mid-run.
        let c = a11y_helper_verdict_from_log(
            "a11y-helper: AX trusted: false — grant…\na11y-helper: AX trusted: true — poking enabled\n",
        );
        assert_eq!(c.severity, Severity::Ok);

        // No trust line at all → helper likely not running.
        let c = a11y_helper_verdict_from_log("unrelated noise\n");
        assert_eq!(c.severity, Severity::Warn);
    }

    #[test]
    fn accessibility_log_verdict_latest_line_wins() {
        // No permission-monitor line at all → nothing to assert.
        assert!(accessibility_verdict_from_log("nothing relevant here").is_none());

        // accessibility=false → critical, with a remedy.
        let c = accessibility_verdict_from_log(
            "startup\npermission monitor started screen=true mic=false accessibility=false keychain=true\n",
        )
        .expect("verdict");
        assert_eq!(c.severity, Severity::Critical);
        assert!(c.remedy.is_some());

        // A later true overrides an earlier false — the most-recent restart wins.
        let c = accessibility_verdict_from_log(
            "permission monitor started accessibility=false\nnoise\npermission monitor started accessibility=true\n",
        )
        .expect("verdict");
        assert_eq!(c.severity, Severity::Ok);
    }
}
