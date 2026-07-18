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

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Raise (or refresh) a named notice. Idempotent — upserts so repeated calls
/// from the poll loop don't accumulate duplicate rows. The paired toast is
/// always stamped `system.fault` — use [`raise_typed`] when the caller needs
/// its own per-type settings toggle instead of sharing `notify_system_fault`.
pub async fn raise(
    pool: &SqlitePool,
    id: &str,
    severity: &str,
    title: &str,
    detail: &str,
    remedy: Option<&str>,
) -> Result<()> {
    let link = if id.starts_with("pm.") {
        Some("/tasks?integrations=1")
    } else {
        Some("/logs")
    };
    raise_typed(
        pool,
        Notice {
            id,
            severity,
            title,
            detail,
            remedy,
            event_key: "system.fault",
            deep_link: link,
        },
    )
    .await
}

/// A notice to raise via [`raise_typed`] — grouped into a struct so callers
/// read clearly and to stay under clippy's argument limit (see `BlockBounds`
/// in `src/etl/runner.rs` for the same convention elsewhere in this repo).
pub struct Notice<'a> {
    pub id: &'a str,
    /// `info` | `warning` | `error`.
    pub severity: &'a str,
    pub title: &'a str,
    pub detail: &'a str,
    pub remedy: Option<&'a str>,
    /// The paired toast's event_key — its own `notify_<type>` settings toggle,
    /// instead of the shared `system.fault` bucket [`raise`] uses.
    pub event_key: &'a str,
    pub deep_link: Option<&'a str>,
}

/// Raise (or refresh) a named notice whose paired toast is stamped with
/// `n.event_key` instead of the shared `system.fault` bucket [`raise`] uses.
/// Same upsert/idempotency semantics as `raise`; pair with [`clear_typed`]
/// using the same `event_key` so the retract dedup key matches the enqueue.
pub async fn raise_typed(pool: &SqlitePool, n: Notice<'_>) -> Result<()> {
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
    .bind(n.id)
    .bind(n.severity)
    .bind(n.title)
    .bind(n.detail)
    .bind(n.remedy)
    .execute(pool)
    .await
    .context("raising system notice")?;

    // Promote the fault to an OS-level toast (the dashboard banner already comes
    // from this table, so the notification is native-only to avoid a double
    // banner). Deduped on `<event_key>:<id>` → one toast per fault, cleared below
    // when the fault recovers so a later re-occurrence toasts again. Best-effort:
    // never let notification delivery break the fault-bus write.
    let dedup = format!("{}:{}", n.event_key, n.id);
    // Native-only (the dashboard banner already comes from this table).
    // `.interactive()` pairs category + actions from the one source of truth, so
    // the [View] button set can't drift from the other producers.
    use meridian_core::notifications::categories;
    let mut note =
        crate::notifications::NewNotification::event(&dedup, n.event_key, n.title, n.detail)
            .via(crate::notifications::CHANNEL_NATIVE)
            .interactive(categories::SYSTEM_FAULT);
    note.severity = n.severity;
    note.deep_link = n.deep_link;
    let _ = crate::notifications::enqueue(pool, note).await;
    Ok(())
}

/// Clear a notice raised via [`raise`] — called when the daemon recovers from
/// a fault.
pub async fn clear(pool: &SqlitePool, id: &str) -> Result<()> {
    clear_typed(pool, id, "system.fault").await
}

/// Clear a notice raised via [`raise_typed`] with the matching `event_key`.
pub async fn clear_typed(pool: &SqlitePool, id: &str, event_key: &str) -> Result<()> {
    sqlx::query("DELETE FROM system_notices WHERE notice_id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("clearing system notice")?;
    // Retract the paired toast so a future re-occurrence of this fault notifies
    // again instead of being deduped away.
    let _ = crate::notifications::retract(pool, &format!("{event_key}:{id}")).await;
    Ok(())
}
