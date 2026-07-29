//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Morning "plan your day" nudge. Once per local day, when the dev has neither
// confirmed nor skipped today's plan and there are open tickets to plan against,
// enqueue a `plan.nudge` notification. If the tray's daily planner auto-open
// (`~/.meridian/plan_auto_opened`) fired within the last hour, the nudge waits
// — it is the second-chance reminder after a dismissed planner, not a
// duplicate toast over the window that just opened. Idempotent via the outbox dedup key, so
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

// How long after the tray's daily planner auto-open to hold the nudge back —
// deliberately the same grace as a "Snooze 1h" answer (`SNOOZE_SECS` in
// notification_responses.rs). The auto-open already put the planner in the
// user's face; the nudge is the second chance if they dismissed it without
// planning, not a toast over the window that just opened.
const AUTO_OPEN_GRACE_SECS: i64 = 3600;

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

    // Auto-open aware hold-back: if the tray auto-opened the planner today,
    // give the user an hour with it before reminding (the marker holds the
    // open's timestamp — see `meridian_core::plan_marker`). Once the hour has
    // passed and the plan is still unconfirmed, the nudge fires as usual, so
    // a dismissed planner still gets its reminder later.
    if let Some(meridian_dir) = meridian_core::paths::meridian_dir() {
        let path = meridian_core::plan_marker::marker_path(&meridian_dir);
        let marker = std::fs::read_to_string(path).unwrap_or_default();
        if nudge_held_back(&marker, &today, &now) {
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

/// True while the nudge should wait because the tray's planner auto-open
/// happened less than [`AUTO_OPEN_GRACE_SECS`] ago today. Pure so the three
/// regimes are unit-testable: no/stale/foreign-day marker → don't hold;
/// today's marker within the grace hour → hold; past it → fire. A today
/// marker whose timestamp can't be parsed (legacy bare-date form) → don't
/// hold — age unknown beats never reminding.
fn nudge_held_back(marker_contents: &str, today: &str, now: &chrono::DateTime<Local>) -> bool {
    if !meridian_core::plan_marker::opened_today(marker_contents, today) {
        return false;
    }
    match meridian_core::plan_marker::opened_at(marker_contents) {
        Some(opened) => {
            now.signed_duration_since(opened) < chrono::Duration::seconds(AUTO_OPEN_GRACE_SECS)
        }
        None => false,
    }
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
    fn nudge_hold_back_covers_the_grace_hour_only() {
        // Anchored to local midday rather than `Local::now()`. The `stamp_ago`
        // offsets below reach up to AUTO_OPEN_GRACE_SECS (1 h) into the past, so
        // with the real clock every run between 00:00 and 01:00 local put those
        // stamps on the PREVIOUS day - where `nudge_held_back` correctly reports
        // "not held" for a prior-day marker, failing the assertions. The subject
        // under test takes `now` as a parameter precisely so it need not depend
        // on the wall clock; this uses that.
        let now = Local::now()
            .with_hour(12)
            .and_then(|t| t.with_minute(0))
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
            .expect("local midday exists on every date");
        let today = now.format("%Y-%m-%d").to_string();
        let stamp_ago =
            |secs: i64| meridian_core::plan_marker::stamp(&(now - chrono::Duration::seconds(secs)));

        // No marker / a prior-day marker → nudge free to fire.
        assert!(!nudge_held_back("", &today, &now));
        assert!(!nudge_held_back("1999-01-01T09:00:00+00:00", &today, &now));
        // Opened 30 min ago → held. Opened 2 h ago → fires (second chance).
        assert!(nudge_held_back(&stamp_ago(1800), &today, &now));
        assert!(!nudge_held_back(&stamp_ago(7200), &today, &now));
        // Exactly at the boundary the hold ends.
        assert!(!nudge_held_back(
            &stamp_ago(AUTO_OPEN_GRACE_SECS),
            &today,
            &now
        ));
        // Legacy bare-date marker (no timestamp): age unknown → don't hold.
        assert!(!nudge_held_back(&today, &today, &now));
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
