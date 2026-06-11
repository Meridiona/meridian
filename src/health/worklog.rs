//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// PM-worklog health: drafts awaiting human review, hours stuck unprocessed, and
// worklogs failing to post to Jira (the retry-forever case the daemon never
// surfaces). Content-free — counts and timestamps from meridian.db.

use crate::config::Config;
use crate::health::Check;
use sqlx::SqlitePool;

/// A pending hour older than this (minutes) is wedged, not just settling.
const STUCK_HOUR_MIN: f64 = 90.0;

pub async fn checks(_cfg: &Config, pool: Option<&SqlitePool>) -> Vec<Check> {
    let p = match pool {
        Some(p) => p,
        None => return Vec::new(),
    };
    vec![
        drafts_pending(p).await,
        stuck_hours(p).await,
        post_failures(p).await,
    ]
}

async fn drafts_pending(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pm_worklogs WHERE state = 'drafted'")
        .fetch_one(pool)
        .await
    {
        Ok(0) => Check::ok("drafts", "L4", "none awaiting review"),
        Ok(n) => Check::info(
            "drafts",
            "L4",
            format!("{n} awaiting review in the dashboard"),
        ),
        Err(e) => Check::info("drafts", "L4", format!("not available ({e})")),
    }
}

async fn stuck_hours(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pm_worklog_hours
         WHERE status = 'pending'
           AND (julianday('now') - julianday(hour_end)) * 1440.0 > ?1",
    )
    .bind(STUCK_HOUR_MIN)
    .fetch_one(pool)
    .await
    {
        Ok(0) => Check::ok("hour ledger", "L4", "no stuck hours"),
        Ok(n) => Check::warn("hour ledger", "L4", format!("{n} hours stuck unprocessed"))
            .with_remedy("a summariser/classifier stall upstream — check those queues"),
        Err(e) => Check::info("hour ledger", "L4", format!("not available ({e})")),
    }
}

async fn post_failures(pool: &SqlitePool) -> Check {
    match sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT COUNT(*), MAX(post_attempt_count) FROM pm_worklogs
         WHERE last_post_error IS NOT NULL AND state = 'approved'",
    )
    .fetch_one(pool)
    .await
    {
        Ok((0, _)) => Check::ok("jira posting", "L2", "no post failures"),
        Ok((n, attempts)) => Check::warn(
            "jira posting",
            "L2",
            format!(
                "{n} approved worklogs failing to post (≤{} attempts each)",
                attempts.unwrap_or(0)
            ),
        )
        .with_remedy("check jira.auth and the worklog's last_post_error in the dashboard"),
        Err(e) => Check::info("jira posting", "L2", format!("not available ({e})")),
    }
}
