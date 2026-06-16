//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Centralised fault bus. The daemon calls `raise` when something breaks and
// `clear` when it recovers. The UI reads `system_notices` via the SSE stream
// and surfaces banners on every page — users never have to check terminal logs.
//
// Notice IDs follow the pattern `<subsystem>.<fault>`, e.g.:
//   pm.jira       — Jira sync failing
//   pm.linear     — Linear sync failing
//   etl.failed    — ETL pipeline error
//   mlx.down      — MLX classifier unreachable

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Raise (or refresh) a named notice. Idempotent — upserts so repeated calls
/// from the poll loop don't accumulate duplicate rows.
pub async fn raise(
    pool: &SqlitePool,
    id: &str,
    severity: &str,
    title: &str,
    detail: &str,
    remedy: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO system_notices (notice_id, severity, title, detail, remedy)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(notice_id) DO UPDATE SET
           severity  = excluded.severity,
           title     = excluded.title,
           detail    = excluded.detail,
           remedy    = excluded.remedy,
           raised_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(id)
    .bind(severity)
    .bind(title)
    .bind(detail)
    .bind(remedy)
    .execute(pool)
    .await
    .context("raising system notice")?;

    // Promote the fault to an OS-level toast (the dashboard banner already comes
    // from this table, so the notification is native-only to avoid a double
    // banner). Deduped on the notice id → one toast per fault, cleared below when
    // the fault recovers so a later re-occurrence toasts again. Best-effort:
    // never let notification delivery break the fault-bus write.
    let dedup = format!("system.fault:{id}");
    let link = if id.starts_with("pm.") {
        Some("/tasks?integrations=1")
    } else {
        Some("/logs")
    };
    let _ = crate::notifications::enqueue(
        pool,
        crate::notifications::NewNotification {
            dedup_key: &dedup,
            event_key: "system.fault",
            severity,
            title,
            body: detail,
            deep_link: link,
            channels: crate::notifications::CHANNEL_NATIVE,
            scheduled_for: None,
            expires_at: None,
        },
    )
    .await;
    Ok(())
}

/// Clear a notice — called when the daemon recovers from a fault.
pub async fn clear(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM system_notices WHERE notice_id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("clearing system notice")?;
    // Retract the paired toast so a future re-occurrence of this fault notifies
    // again instead of being deduped away.
    let _ = crate::notifications::retract(pool, &format!("system.fault:{id}")).await;
    Ok(())
}
