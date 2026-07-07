//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Morning "plan your day" nudge. Once per local day, when the dev has neither
// confirmed nor skipped today's plan and there are open tickets to plan against,
// enqueue a `plan.nudge` notification. Idempotent via the outbox dedup key, so
// the poll loop can call this every tick without spamming. A nudge is only
// meaningful on its own day, so it carries an `expires_at` of the next local
// midnight, and any still-live nudge from a previous day (or one whose plan has
// since been confirmed/skipped) is expired on sight — otherwise an unactioned
// banner stacks under the next day's identical one.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone, Timelike, Utc};
use sqlx::{Row, SqlitePool};

use crate::notifications::{self, NewNotification};

// Only nudge during working hours — a first poll at 3am shouldn't ping. The
// once-per-day dedup means the nudge lands on the first tick after the start hour.
const NUDGE_FROM_HOUR: u32 = 8;
const NUDGE_UNTIL_HOUR: u32 = 18;

/// Enqueue today's plan nudge if it's due and not already actioned. Best-effort:
/// any DB error (e.g. a pre-migration-041 database with no `daily_plan` tables)
/// is surfaced to the caller, which logs-and-ignores.
pub async fn maybe_nudge(pool: &SqlitePool) -> Result<()> {
    let now = Local::now();
    let hour = now.hour();
    if !(NUDGE_FROM_HOUR..NUDGE_UNTIL_HOUR).contains(&hour) {
        return Ok(());
    }
    let today = now.format("%Y-%m-%d").to_string();
    let dedup = format!("plan.nudge:{today}");
    let now_utc = utc_iso(&now);

    // Expire any still-live nudge that isn't part of *today's* nudge — a
    // previous day's unactioned banner would otherwise sit under today's
    // identical one forever. Rows written before expiry stamping existed have
    // `expires_at` NULL, so they're swept here too.
    expire_stale_nudges(pool, &dedup, &now_utc).await?;

    // Already confirmed or skipped today? Expire today's nudge (its job is
    // done — don't make the user dismiss it) and stop.
    if let Some(row) =
        sqlx::query("SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?")
            .bind(&today)
            .fetch_optional(pool)
            .await?
    {
        let confirmed: Option<String> = row.try_get("confirmed_at").unwrap_or(None);
        let skipped: i64 = row.try_get("skipped").unwrap_or(0);
        if confirmed.is_some() || skipped != 0 {
            // Prefix match (not exact) so a pending snooze re-enqueue
            // (`plan.nudge:{today}:snooze:{ts}`) is cancelled too — once the plan
            // is actioned, a deferred reminder for it is moot.
            sqlx::query(
                "UPDATE notifications SET expires_at = ?
                 WHERE dedup_key LIKE ? AND (expires_at IS NULL OR expires_at > ?)",
            )
            .bind(&now_utc)
            .bind(format!("{dedup}%"))
            .bind(&now_utc)
            .execute(pool)
            .await
            .context("expiring actioned plan nudge")?;
            return Ok(());
        }
    }

    // Nothing on the board to plan against → no nudge.
    let has_open_tasks: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pm_tasks WHERE COALESCE(is_terminal, 0) = 0)",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if has_open_tasks == 0 {
        return Ok(());
    }

    use meridian_core::notifications::categories;

    // Expiry = the next local midnight; if that's ever uncomputable (calendar
    // edge), the enqueue still goes out and the stale-nudge sweep above expires
    // the row the next day.
    let expires = next_local_midnight_utc(&now);
    let mut nudge = NewNotification::event(
        &dedup,
        "plan.nudge",
        "Plan your day",
        "Pick what you're working on today so Meridian can match your work to the right tickets.",
    )
    .link("/plan")
    .interactive(categories::PLAN_NUDGE);
    if let Some(exp) = expires.as_deref() {
        nudge = nudge.expiring(exp);
    }
    notifications::enqueue(pool, nudge).await
}

/// `t` as the UTC ISO-8601 string shape (`2026-07-05T02:30:24Z`) the
/// notifications table string-compares `expires_at` against.
fn utc_iso(t: &chrono::DateTime<Local>) -> String {
    t.with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// First instant of the next local day, as a UTC ISO string — the moment
/// today's nudge stops being meaningful. `None` only on calendar edges (last
/// representable date, a DST transition swallowing midnight).
fn next_local_midnight_utc(now: &chrono::DateTime<Local>) -> Option<String> {
    let midnight = now.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?;
    let local = Local.from_local_datetime(&midnight).earliest()?;
    Some(utc_iso(&local))
}

/// Expire every live `plan.nudge` row whose dedup key isn't part of *today's*
/// nudge. Match on today's `plan.nudge:{today}` **prefix**, not exact equality:
/// a snoozed nudge re-enqueues under `plan.nudge:{today}:snooze:{ts}` with
/// `expires_at` NULL, so an exact `<>` check would sweep the snooze row and
/// stamp its `expires_at` an hour *before* its `scheduled_for` — silently
/// killing the deferred reminder before it ever fires. The prefix keeps today's
/// canonical row AND its snooze re-enqueues alive while still expiring prior
/// days. `today_dedup` carries no LIKE metacharacters (`%`/`_`), so no `ESCAPE`
/// clause is needed.
async fn expire_stale_nudges(pool: &SqlitePool, today_dedup: &str, now_utc: &str) -> Result<()> {
    sqlx::query(
        "UPDATE notifications SET expires_at = ?
         WHERE event_key = 'plan.nudge' AND dedup_key NOT LIKE ?
           AND (expires_at IS NULL OR expires_at > ?)",
    )
    .bind(now_utc)
    .bind(format!("{today_dedup}%"))
    .bind(now_utc)
    .execute(pool)
    .await
    .context("expiring stale plan nudges")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stale_sweep_spares_today_and_snooze_but_expires_prior_days() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE notifications (id INTEGER PRIMARY KEY, event_key TEXT, \
             dedup_key TEXT, expires_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // id1: today's canonical nudge.  id2: today's snooze re-enqueue.
        // id3: a prior day's unactioned nudge.
        for (id, key) in [
            (1, "plan.nudge:2026-07-07"),
            (2, "plan.nudge:2026-07-07:snooze:2026-07-07T09:00:00Z"),
            (3, "plan.nudge:2026-07-06"),
        ] {
            sqlx::query(
                "INSERT INTO notifications (id, event_key, dedup_key) \
                 VALUES (?, 'plan.nudge', ?)",
            )
            .bind(id)
            .bind(key)
            .execute(&pool)
            .await
            .unwrap();
        }
        let now = "2026-07-07T10:00:00Z";
        expire_stale_nudges(&pool, "plan.nudge:2026-07-07", now)
            .await
            .unwrap();
        let rows: Vec<(i64, Option<String>)> =
            sqlx::query_as("SELECT id, expires_at FROM notifications ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows[0].1, None, "today's canonical nudge must survive");
        assert_eq!(rows[1].1, None, "today's snooze re-enqueue must survive");
        assert_eq!(
            rows[2].1.as_deref(),
            Some(now),
            "a prior-day nudge must be expired"
        );
    }

    #[test]
    fn expiry_is_the_next_local_midnight() {
        let now = Local::now();
        let exp = next_local_midnight_utc(&now).expect("expiry computable");
        let parsed = chrono::DateTime::parse_from_rfc3339(&exp).expect("valid ISO instant");
        let local = parsed.with_timezone(&Local);
        assert_eq!(local.date_naive(), now.date_naive().succ_opt().unwrap());
        assert_eq!((local.hour(), local.minute(), local.second()), (0, 0, 0));
        // Must sort AFTER "now" in the shared string format, or the banner
        // would be born expired.
        assert!(exp > utc_iso(&now));
    }
}
