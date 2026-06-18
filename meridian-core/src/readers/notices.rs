//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/notices/[id]` DELETE ported to Rust.
//!
//! # What this is
//! `system_notices` holds active fault banners (e.g. "couldn't reach Jira") that
//! the daemon raises and auto-clears on the next healthy poll. [`delete_notice`]
//! clears one immediately — the UI calls it the moment a provider reconnects, so
//! the banner disappears without waiting for the ETL cycle. A faithful port of
//! `ui/app/api/notices/[id]/route.ts` (which deletes by `notice_id`).
//!
//! The notice *read* ([`read_notices`]) is the live snapshot the tray's poll
//! loop emits as the `notices-update` Tauri event (the ported `/api/notices/stream`
//! SSE) and that the `get_notices` command serves on first paint — porting the
//! query out of the Node `notices-store.ts` singleton.
//!
//! # Who calls this
//! - [`delete_notice`] — the tray `delete_notice` command → `TasksView.tsx`
//!   (on a successful provider connect).
//! - [`read_notices`] — the tray `get_notices` command + the poll loop's
//!   `notices-update` emit → `ui/components/NoticeBar.tsx`.
//!
//! # Related
//! - [`crate::integrations`] — the provider-connection state the connect flow reads.

use crate::SqlitePool;
use sqlx::FromRow;
use tracing::Instrument;

/// One active fault banner (the shape `NoticeBar.tsx` renders). `severity` is
/// `'error'|'warning'`; `remedy` is the optional fix hint.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct Notice {
    pub notice_id: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub remedy: Option<String>,
    pub raised_at: String,
}

/// All active notices, newest first (port of `notices-store.ts`'s `queryNotices`).
/// Returns empty — never errors — when the daemon hasn't created `system_notices`
/// yet (pre-migration DB) or on a transient read error, matching the route's
/// `catch → []`.
#[tracing::instrument(skip(pool))]
pub async fn read_notices(pool: &SqlitePool) -> Vec<Notice> {
    let rows = sqlx::query_as::<_, Notice>(
        "SELECT notice_id, severity, title, detail, remedy, raised_at \
         FROM system_notices ORDER BY raised_at DESC",
    )
    .fetch_all(pool)
    .instrument(tracing::debug_span!("notices.read.all"))
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "notices: read failed, treating as empty");
        Vec::new()
    });
    tracing::debug!(rows = rows.len(), "notices.read.all");
    rows
}

/// Clear one notice from `system_notices` by `notice_id` (port of the DELETE
/// route). Idempotent — deleting an absent notice is a no-op. The daemon owns
/// the table; this only removes a row, never the schema.
#[tracing::instrument(skip(pool))]
pub async fn delete_notice(pool: &SqlitePool, notice_id: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM system_notices WHERE notice_id = ?")
        .bind(notice_id)
        .execute(pool)
        .instrument(tracing::debug_span!("notices.write.delete"))
        .await?;
    tracing::info!(notice_id, "notice cleared");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn delete_removes_only_the_matching_notice() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE system_notices (notice_id TEXT PRIMARY KEY, title TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        for nid in ["pm.jira", "pm.linear"] {
            sqlx::query("INSERT INTO system_notices (notice_id, title) VALUES (?, 'x')")
                .bind(nid)
                .execute(&pool)
                .await
                .unwrap();
        }

        delete_notice(&pool, "pm.jira").await.unwrap();
        // Deleting again is a harmless no-op (idempotent).
        delete_notice(&pool, "pm.jira").await.unwrap();

        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT notice_id FROM system_notices ORDER BY notice_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec!["pm.linear".to_string()]);
    }

    #[tokio::test]
    async fn read_notices_orders_newest_first_and_tolerates_missing_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // No table yet → empty, not an error (pre-migration DB).
        assert!(read_notices(&pool).await.is_empty());

        sqlx::query(
            "CREATE TABLE system_notices (notice_id TEXT PRIMARY KEY, severity TEXT, \
             title TEXT, detail TEXT, remedy TEXT, raised_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (nid, raised) in [
            ("pm.jira", "2026-06-18T09:00:00Z"),
            ("a11y", "2026-06-18T10:00:00Z"),
        ] {
            sqlx::query(
                "INSERT INTO system_notices (notice_id, severity, title, detail, remedy, raised_at) \
                 VALUES (?, 'warning', 't', 'd', NULL, ?)",
            )
            .bind(nid)
            .bind(raised)
            .execute(&pool)
            .await
            .unwrap();
        }

        let notices = read_notices(&pool).await;
        // ORDER BY raised_at DESC → the 10:00 row first.
        assert_eq!(notices[0].notice_id, "a11y");
        assert_eq!(notices[1].notice_id, "pm.jira");
        assert!(notices[0].remedy.is_none());
    }
}
