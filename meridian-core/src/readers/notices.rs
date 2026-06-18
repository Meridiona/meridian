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
//! The notice *read* is still served over SSE (`/api/notices/stream`); when that
//! folds into a Tauri event, its query lands in this module too.
//!
//! # Who calls this
//! The tray `delete_notice` command → `ui/components/views/TasksView.tsx` (on a
//! successful provider connect). The route's SSE `refresh()` push is a separate
//! concern handled by the notices stream port.
//!
//! # Related
//! - [`crate::integrations`] — the provider-connection state the connect flow reads.

use crate::SqlitePool;
use tracing::Instrument;

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
}
