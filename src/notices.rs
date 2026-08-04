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

/// Notice id raised when `meridian.db` is structurally damaged.
///
/// Lives here rather than beside the raiser in `main.rs` because THREE parties
/// must agree on it byte-for-byte: the daemon raises it (latched - see the
/// usage-site doc in `main.rs`), the UI banners it, and `db::repair` clears it
/// from the rebuilt file so the banner does not survive its own fix. It is
/// also its own `event_key` (not the shared `system.fault`), so clearing must
/// pass it for both.
pub const DB_CORRUPT: &str = "db.corrupt";

/// Raise (or refresh) a named notice. Idempotent — upserts so repeated calls
/// from the poll loop don't accumulate duplicate rows. The paired toast is
/// always stamped `system.fault` — use [`raise_typed`] when the caller needs
/// its own distinct `event_key` instead of sharing the `system.fault` bucket.
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
    /// The paired toast's event_key, distinct from the shared `system.fault`
    /// bucket [`raise`] uses.
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
    clear_typed_reporting(pool, id, event_key).await?;
    Ok(())
}

/// Same as [`clear_typed`], but reports whether a notice actually existed and
/// was deleted. Used by callers that must tell "a real fault was cleared"
/// apart from "there was nothing to clear" — e.g. `refresh_health`'s
/// startup reconciliation, which clears a `tray.daemon_quiet` notice that may
/// be stale from a *previous* process instance (a fresh tray process can
/// never observe that instance's down→up transition itself) and should only
/// fire the "back online" toast when something was actually there.
pub async fn clear_typed_reporting(pool: &SqlitePool, id: &str, event_key: &str) -> Result<bool> {
    let mut conn = pool.acquire().await.context("acquiring for clear")?;
    clear_typed_on(&mut conn, id, event_key).await
}

/// [`clear_typed_reporting`] for callers holding a single `SqliteConnection`
/// rather than a pool.
///
/// Exists for `db::repair`'s rebuild, which must run EVERY write on one
/// explicitly-closed connection: a pooled acquire there is handed back to the
/// pool from a spawned task on drop, `pool.close()` does not reliably wait for
/// that hand-back, and the surviving connection holds the WAL open — tripping
/// the rebuild's "un-checkpointed write-ahead log" guard and failing the whole
/// repair (intermittently, under load). See the closing comments in
/// `db::repair::rebuild::build_replacement`.
pub async fn clear_typed_on(
    conn: &mut sqlx::SqliteConnection,
    id: &str,
    event_key: &str,
) -> Result<bool> {
    let result = sqlx::query("DELETE FROM system_notices WHERE notice_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await
        .context("clearing system notice")?;
    // Retract the paired toast so a future re-occurrence of this fault notifies
    // again instead of being deduped away. Best-effort, matching
    // `notifications::retract` — same statement, same dedup-key shape.
    let _ = sqlx::query("DELETE FROM notifications WHERE dedup_key = ?")
        .bind(format!("{event_key}:{id}"))
        .execute(&mut *conn)
        .await;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn clear_typed_reporting_is_true_only_when_a_row_actually_existed() {
        let pool = fresh_db().await;

        // Nothing raised yet — clearing is a no-op, reported as such.
        let cleared_nothing = clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        assert!(!cleared_nothing);

        raise_typed(
            &pool,
            Notice {
                id: "tray.daemon_quiet",
                severity: "warning",
                title: "Meridian went quiet.",
                detail: "Tap to check what happened.",
                remedy: None,
                event_key: "system.health",
                deep_link: Some("/logs"),
            },
        )
        .await
        .unwrap();

        // A real notice exists — clearing it reports true, exactly once.
        let cleared_real = clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        assert!(cleared_real);

        let cleared_again = clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        assert!(!cleared_again);
    }
}
