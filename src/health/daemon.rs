//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// meridian-daemon health: ETL liveness, frame-cursor progress, classification
// queue depth, and the subprocess-error sentinel. All content-free (statuses,
// counts, timestamps from meridian.db, opened read-only).

use crate::config::Config;
use crate::health::Check;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;

/// No ETL run in this long ⇒ the poll loop is probably stalled.
const STALE_RUN_SECS: f64 = 1800.0;
/// A pending_* queue this deep ⇒ a downstream stage is wedged.
const QUEUE_WARN: i64 = 50;

/// Open meridian.db read-only for inspection. Returns None if it cannot be
/// opened (missing / not yet created / corrupt) — the caller surfaces that.
pub async fn open_meridian_ro(cfg: &Config) -> Option<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(&cfg.meridian_db_uri())
        .ok()?
        .read_only(true);
    SqlitePool::connect_with(opts).await.ok()
}

pub async fn checks(_cfg: &Config, pool: Option<&SqlitePool>) -> Vec<Check> {
    let p = match pool {
        Some(p) => p,
        None => {
            return vec![Check::critical(
                "meridian DB",
                "L1",
                "meridian.db not readable (missing, locked, or not yet created)",
            )
            .with_remedy("start the daemon once to create ~/.meridian/meridian.db")]
        }
    };
    vec![
        Check::ok("meridian DB", "L1", "readable"),
        etl_last_run(p).await,
        etl_freshness(p).await,
        etl_cursor(p).await,
        queue_depth(p, "pending_summariser", "summariser queue").await,
        queue_depth(p, "pending_classifier", "classifier queue").await,
        subprocess_errors(p).await,
    ]
}

async fn etl_last_run(pool: &SqlitePool) -> Check {
    match sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, error FROM etl_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(Some((status, err))) => match status.as_str() {
            "success" | "skipped" => Check::ok("etl last run", "L1", format!("status: {status}")),
            "running" => Check::info("etl last run", "L1", "a run is in progress"),
            "failed" => Check::critical(
                "etl last run",
                "L1",
                format!("failed: {}", err.unwrap_or_default()),
            )
            .with_remedy("check ~/.meridian/logs — usually a screenpipe read error"),
            other => Check::warn("etl last run", "L1", format!("status: {other}")),
        },
        Ok(None) => Check::info("etl last run", "L1", "no ETL runs yet"),
        Err(e) => Check::warn(
            "etl last run",
            "L1",
            format!("could not read etl_runs ({e})"),
        ),
    }
}

async fn etl_freshness(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, Option<f64>>(
        "SELECT (julianday('now') - julianday(MAX(started_at))) * 86400.0 FROM etl_runs",
    )
    .fetch_one(pool)
    .await
    {
        Ok(Some(age)) if age > STALE_RUN_SECS => Check::warn(
            "etl freshness",
            "L1",
            format!("last run {:.0}m ago — poll loop may be stalled", age / 60.0),
        )
        .with_remedy("meridian restart"),
        Ok(Some(age)) => Check::ok(
            "etl freshness",
            "L1",
            format!("last run {:.0}s ago", age.max(0.0)),
        ),
        Ok(None) => Check::info("etl freshness", "L1", "no runs yet"),
        Err(e) => Check::warn(
            "etl freshness",
            "L1",
            format!("could not read run time ({e})"),
        ),
    }
}

async fn etl_cursor(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, i64>("SELECT last_frame_id FROM etl_cursor WHERE id = 1")
        .fetch_optional(pool)
        .await
    {
        Ok(Some(id)) => Check::ok("etl cursor", "L1", format!("at frame {id}")),
        Ok(None) => Check::info("etl cursor", "L1", "not initialised yet"),
        Err(e) => Check::warn("etl cursor", "L1", format!("could not read cursor ({e})")),
    }
}

async fn queue_depth(pool: &SqlitePool, method: &str, label: &'static str) -> Check {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM app_sessions WHERE task_method = ?1")
        .bind(method)
        .fetch_one(pool)
        .await
    {
        Ok(0) => Check::ok(label, "L2", "empty"),
        Ok(n) if n >= QUEUE_WARN => Check::warn(label, "L2", format!("{n} sessions backed up"))
            .with_remedy("check the MLX server / claude+codex CLIs are reachable"),
        Ok(n) => Check::info(label, "L2", format!("{n} pending")),
        Err(e) => Check::warn(label, "L2", format!("could not read queue ({e})")),
    }
}

async fn subprocess_errors(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM app_sessions WHERE task_method = 'subprocess_error'",
    )
    .fetch_one(pool)
    .await
    {
        Ok(0) => Check::ok("classify errors", "L2", "no sentinel errors"),
        Ok(n) => Check::warn(
            "classify errors",
            "L2",
            format!("{n} sessions sentinelled — usually a sustained MLX outage, not the model"),
        )
        .with_remedy("verify the MLX server, then re-classify those sessions"),
        Err(e) => Check::warn("classify errors", "L2", format!("could not read ({e})")),
    }
}
