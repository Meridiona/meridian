//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Morning "plan your day" nudge. Once per local day, when the dev has neither
// confirmed nor skipped today's plan and there are open tickets to plan against,
// enqueue a `plan.nudge` notification. Idempotent via the outbox dedup key, so
// the poll loop can call this every tick without spamming.

use anyhow::Result;
use chrono::{Local, Timelike};
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

    // Already confirmed or skipped today? Nothing to nudge.
    if let Some(row) =
        sqlx::query("SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?")
            .bind(&today)
            .fetch_optional(pool)
            .await?
    {
        let confirmed: Option<String> = row.try_get("confirmed_at").unwrap_or(None);
        let skipped: i64 = row.try_get("skipped").unwrap_or(0);
        if confirmed.is_some() || skipped != 0 {
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

    let dedup = format!("plan.nudge:{today}");
    notifications::enqueue(
        pool,
        NewNotification::event(
            &dedup,
            "plan.nudge",
            "Plan your day",
            "Pick what you're working on today so Meridian can match your work to the right tickets.",
        )
        .link("/plan"),
    )
    .await
}
