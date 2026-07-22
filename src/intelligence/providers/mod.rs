//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

pub mod azure_devops;
pub mod cdm;
pub mod github;
pub mod jira;
pub mod linear;
pub mod status;
pub mod trello;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Write a sync error for a provider. Writes to both `pm_sync_state.last_error`
/// (for the connect-status indicators) and `system_notices` (for the global
/// UI fault bus that surfaces banners on every page).
pub async fn stamp_sync_error(pool: &SqlitePool, provider: &str, error: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES (?, '1970-01-01T00:00:00Z', ?)
         ON CONFLICT(provider) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(provider)
    .bind(error)
    .execute(pool)
    .await?;

    let (title, remedy): (&str, Option<&str>) = match provider {
        "jira" => (
            "Jira sync failing",
            Some("Set JIRA_API_TOKEN and JIRA_BASE_URL in .env"),
        ),
        "linear" => ("Linear sync failing", Some("Set LINEAR_API_KEY in .env")),
        "trello" => (
            "Trello sync failing",
            Some("Run: meridian oauth-login trello"),
        ),
        "github" => ("GitHub sync failing", Some("Set GITHUB_TOKEN in .env")),
        "azure_devops" => (
            "Azure DevOps sync failing",
            Some("Set AZURE_DEVOPS_PAT in .env"),
        ),
        _ => ("PM sync failing", None),
    };
    let _ = crate::notices::raise(
        pool,
        &format!("pm.{provider}"),
        "error",
        title,
        error,
        remedy,
    )
    .await;
    Ok(())
}

/// Clear the last error for a provider after a successful sync.
pub async fn clear_sync_error(pool: &SqlitePool, provider: &str) -> Result<()> {
    sqlx::query("UPDATE pm_sync_state SET last_error = NULL WHERE provider = ?")
        .bind(provider)
        .execute(pool)
        .await?;
    let _ = crate::notices::clear(pool, &format!("pm.{provider}")).await;
    Ok(())
}

/// Flag worklog- or daily-plan-retained rows that fell out of the active-task
/// fetch as off-board (`is_terminal = 1`).
///
/// Every provider's `prune` deletes `pm_tasks` rows the active-task fetch no
/// longer returns — EXCEPT those with `pm_worklogs` history or `daily_plan`
/// membership, which are kept forever so the timeline still has their title
/// and the plan checkbox's Undo (reopen) still has a row to act on. The gap: a
/// kept row's `is_terminal` was frozen at whatever it was when the task last
/// appeared in the fetch, so a ticket that goes Done or is reassigned away
/// while retained lingers on the board as "active" indefinitely (the board is
/// `WHERE is_terminal = 0`). The active-task fetch is, by construction, exactly
/// "assigned to me AND not-done AND in-scope-type", so any row NOT in
/// `fetched_keys` is no longer one of those — it belongs off the board. We stamp
/// it terminal (kept for the timeline, hidden from the board and from worklog
/// candidate matching, which also filters `is_terminal = 0`; the plan checkbox
/// itself reads `is_terminal` straight off this row). If the ticket is
/// reassigned back / reopened it returns in the next fetch and the upsert resets
/// `is_terminal` from its real status, so this self-corrects.
///
/// `fetched_keys` empty (you have zero active tasks) → every retained row for
/// this provider is stamped. Returns the number of rows flagged.
pub async fn mark_retained_offboard(
    pool: &SqlitePool,
    provider: &str,
    fetched_keys: &[String],
) -> Result<u64> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE pm_tasks SET is_terminal = 1 \
         WHERE provider = ? AND is_terminal = 0 \
           AND task_key NOT IN ({placeholders}) \
           AND (task_key IN (SELECT DISTINCT task_key FROM pm_worklogs WHERE provider = ?) \
                OR task_key IN (SELECT DISTINCT task_key FROM daily_plan))"
    );
    let mut q = sqlx::query(&sql).bind(provider);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q = q.bind(provider);
    let result = q
        .execute(pool)
        .await
        .with_context(|| format!("flagging retained off-board {provider} tasks"))?;
    Ok(result.rows_affected())
}
