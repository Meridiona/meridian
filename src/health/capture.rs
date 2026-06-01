// meridian — normalises screenpipe activity into structured app sessions
//
// L1 capture-layer health checks. Meridian reads screenpipe's frames read-only;
// if capture is broken (screen-recording permission revoked → blank frames,
// accessibility permission off → empty a11y tree, screenpipe dead → stale
// frames), the classifier receives garbage and gets blamed for it. These probes
// surface that as an L1 fault. All probes are content-free: they read counts,
// timestamps, and text *presence* — never the captured text itself.

use crate::config::Config;
use crate::health::{Check, Report};
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
pub async fn run(cfg: &Config, pool: Option<&SqlitePool>) -> Report {
    let mut checks = vec![db_present(cfg)];
    match pool {
        Some(p) => {
            let frames = frames_present(p).await;
            // The ratio checks are only meaningful once frames exist.
            let have_frames = frames.severity != crate::health::Severity::Critical;
            checks.push(frames);
            if have_frames {
                checks.push(frame_freshness(p).await);
                checks.push(blank_text_rate(p).await);
                checks.push(accessibility_share(p).await);
            }
            checks.push(wal_size(cfg));
        }
        None => checks.push(Check::critical(
            "screenpipe.pool",
            "L1",
            "could not open screenpipe DB read-only (path wrong, locked, or corrupt)",
        )),
    }
    Report { checks }
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
        ),
        Err(e) => Check::critical(
            "screenpipe.db_present",
            "L1",
            format!("{path} not found ({e}) — is screenpipe installed/running?"),
        ),
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

/// Share of recent frames whose text came from the accessibility tree. A near-
/// zero share is the content-free proxy for Accessibility permission off (or an
/// Electron-heavy workload, e.g. Cursor/VS Code, that returns an empty a11y
/// tree). Warn only — OCR-only capture is degraded, not dead.
async fn accessibility_share(pool: &SqlitePool) -> Check {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN COALESCE(text_source, '') = 'accessibility'
                                  THEN 1 ELSE 0 END), 0)
         FROM (SELECT text_source FROM frames ORDER BY id DESC LIMIT ?1)",
    )
    .bind(RECENT_SAMPLE)
    .fetch_one(pool)
    .await;
    match row {
        Ok((0, _)) => Check::ok("screenpipe.a11y_tree", "L1", "no recent frames sampled"),
        Ok((total, a11y)) => {
            let pct = 100.0 * a11y as f64 / total as f64;
            let detail = format!("{a11y}/{total} recent frames via a11y ({pct:.0}%)");
            if pct < 5.0 {
                Check::warn(
                    "screenpipe.a11y_tree",
                    "L1",
                    format!("{detail} — Accessibility permission off or Electron-heavy apps"),
                )
            } else {
                Check::ok("screenpipe.a11y_tree", "L1", detail)
            }
        }
        Err(e) => Check::warn(
            "screenpipe.a11y_tree",
            "L1",
            format!("could not sample text_source ({e})"),
        ),
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
        full: Option<&str>,
        a11y: Option<&str>,
        src: &str,
        n: usize,
    ) {
        for _ in 0..n {
            sqlx::query(
                "INSERT INTO frames(app_name, timestamp, full_text, accessibility_text, text_source)
                 VALUES('A', strftime('%Y-%m-%dT%H:%M:%SZ','now'), ?1, ?2, ?3)",
            )
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
        insert(&pool, Some(""), Some(""), "ocr", 20).await;
        assert_eq!(blank_text_rate(&pool).await.severity, Severity::Critical);
        // 0% a11y share → warn (perm off or Electron-heavy).
        assert_eq!(accessibility_share(&pool).await.severity, Severity::Warn);
    }

    #[tokio::test]
    async fn healthy_a11y_capture_is_ok() {
        let pool = mem_pool().await;
        insert(&pool, None, Some("button: Save"), "accessibility", 20).await;
        assert_eq!(frames_present(&pool).await.severity, Severity::Ok);
        assert_eq!(blank_text_rate(&pool).await.severity, Severity::Ok);
        assert_eq!(accessibility_share(&pool).await.severity, Severity::Ok);
        assert_eq!(frame_freshness(&pool).await.severity, Severity::Ok);
    }
}
