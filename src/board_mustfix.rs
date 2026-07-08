//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Must-fix board-hygiene push nudge — the daemon-side half of the "must-fix"
// action card (see ui/components/timeline/useActionItems.ts). Once per local
// day, when at least one ticket on a currently-connected tracker carries a
// must-fix hygiene issue (missing description/title/due-date, or overdue) AND
// the user hasn't snoozed the "mustfix" action card, enqueue a `board.mustfix`
// notification. Idempotent via the outbox dedup key, so the poll loop can call
// this every tick without spamming — mirrors `daily_plan::maybe_nudge`.

use anyhow::Result;
use chrono::{Local, Utc};
use sqlx::{Row, SqlitePool};

use crate::notifications::{self, NewNotification};

/// Env-var / OAuth-file "is this tracker connected" check. Deliberately
/// duplicated from `tray/src-tauri/src/commands/integrations.rs`'s
/// `get_integrations` rather than shared: that logic reads files/env and lives
/// in the tray by convention (meridian-core stays DB-only), but the daemon has
/// no Tauri layer to call into. The daemon's env is already loaded via
/// `dotenvy::dotenv_override()` at startup, so a plain `std::env::var` read
/// here sees the same values the tray's `.env` parse would.
fn is_set(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => {
            let lower = v.to_lowercase();
            !lower.contains("your-") && !lower.contains("_your_") && !lower.contains("-here")
        }
        _ => false,
    }
}

fn oauth_file_exists(provider: &str) -> bool {
    std::env::var("HOME")
        .ok()
        .map(|h| {
            std::path::Path::new(&h)
                .join(".meridian/oauth")
                .join(format!("{provider}.json"))
        })
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Provider keys (matching `pm_task_curation.provider`) currently connected.
fn connected_providers() -> Vec<&'static str> {
    let mut out = Vec::new();
    let jira_basic = is_set("JIRA_BASE_URL") && is_set("JIRA_EMAIL") && is_set("JIRA_API_TOKEN");
    if oauth_file_exists("jira") || jira_basic {
        out.push("jira");
    }
    if is_set("LINEAR_API_KEY") {
        out.push("linear");
    }
    if is_set("GITHUB_TOKEN") {
        out.push("github");
    }
    if oauth_file_exists("trello") {
        out.push("trello");
    }
    let azure_devops = is_set("AZURE_DEVOPS_PAT")
        && (is_set("AZURE_DEVOPS_URL")
            || is_set("AZURE_DEVOPS_ORG")
            || is_set("AZURE_DEVOPS_ORG_URL"));
    if azure_devops {
        out.push("azure_devops");
    }
    out
}

/// Whether at least one not-yet-snoozed ticket on a connected provider carries
/// a must-fix hygiene issue. Cheap: one flat `SELECT` over `pm_task_curation`
/// (small, indexed table) — no join to `pm_tasks` needed since curation rows
/// already carry `provider`. Tolerates a pre-migration-038 DB (no table yet).
async fn has_must_fix_ticket(pool: &SqlitePool, now_iso: &str, connected: &[&str]) -> Result<bool> {
    if connected.is_empty() {
        return Ok(false);
    }

    let has_curation: Option<(i64,)> = sqlx::query_as::<_, (i64,)>(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='pm_task_curation'",
    )
    .fetch_optional(pool)
    .await?;
    if has_curation.is_none() {
        return Ok(false);
    }

    let rows = sqlx::query(
        "SELECT provider, reasons_json, ignored_codes, snoozed_until FROM pm_task_curation",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let provider: String = row.try_get("provider").unwrap_or_default();
        if !connected.contains(&provider.as_str()) {
            continue;
        }
        let snoozed_until: Option<String> = row.try_get("snoozed_until").unwrap_or(None);
        if let Some(s) = &snoozed_until {
            if s.as_str() > now_iso {
                continue;
            }
        }
        let reasons_json: Option<String> = row.try_get("reasons_json").unwrap_or(None);
        let ignored_codes: Option<String> = row.try_get("ignored_codes").unwrap_or(None);
        let issues =
            meridian_core::hygiene::parse_issues(reasons_json.as_deref(), ignored_codes.as_deref());
        if issues.iter().any(|i| i.severity == "must_fix") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Enqueue today's must-fix nudge if it's due: a connected tracker has a
/// must-fix ticket, and the "mustfix" action card isn't currently snoozed.
/// Best-effort: any DB error (e.g. a pre-migration DB) is surfaced to the
/// caller, which logs-and-ignores — matches `daily_plan::maybe_nudge`.
pub async fn maybe_notify(pool: &SqlitePool) -> Result<()> {
    let now_iso = Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        .replace("+00:00", "Z");
    let today = Local::now().format("%Y-%m-%d").to_string();

    let snoozes = meridian_core::action_card_snooze::get_active_snoozes(pool, &now_iso)
        .await
        .unwrap_or_default();
    if snoozes.iter().any(|s| s.card_kind == "mustfix") {
        return Ok(());
    }

    let connected = connected_providers();
    if !has_must_fix_ticket(pool, &now_iso, &connected).await? {
        return Ok(());
    }

    let dedup = format!("board.mustfix:{today}");
    // Log only on the real fire, not every deduped tick: `enqueue` is
    // idempotent (ON CONFLICT DO NOTHING), so an unconditional log here would
    // repeat every poll interval all day. The cheap presence check keeps this
    // to one line the day the nudge actually goes out.
    let already: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM notifications WHERE dedup_key = ?")
        .bind(&dedup)
        .fetch_optional(pool)
        .await?;
    if already.is_none() {
        tracing::info!(
            day = %today,
            connected = ?connected,
            "must-fix board-hygiene nudge fired"
        );
    }

    notifications::enqueue(
        pool,
        NewNotification::event(
            &dedup,
            "board.mustfix",
            "Tickets need must-have info",
            "Some tickets on your board are missing a due date, description, or clear title — Meridian can't track them accurately yet.",
        )
        .link("/tasks"),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    // These tests mutate process-global env (`std::env::set_var`/`remove_var`),
    // which cargo's default parallel runner shares across threads — so without
    // serialisation one test's `LINEAR_API_KEY` leaks into another's
    // "disconnected" assertion. Every env-touching test holds this lock for its
    // whole body. It's a `tokio::sync::Mutex` (not `std`) precisely because the
    // guard must survive the `.await` on `maybe_notify` — a `std` guard there
    // trips `clippy::await_holding_lock`.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn insert_curation(
        pool: &SqlitePool,
        task_key: &str,
        provider: &str,
        reasons_json: &str,
    ) {
        sqlx::query(
            "INSERT INTO pm_task_curation (task_key, provider, bucket, reasons_json, triaged_at) \
             VALUES (?, ?, 'needs_detail', ?, '2026-01-01T00:00:00Z')",
        )
        .bind(task_key)
        .bind(provider)
        .bind(reasons_json)
        .execute(pool)
        .await
        .unwrap();
    }

    fn clear_env() {
        for k in [
            "JIRA_BASE_URL",
            "JIRA_EMAIL",
            "JIRA_API_TOKEN",
            "LINEAR_API_KEY",
            "GITHUB_TOKEN",
            "AZURE_DEVOPS_PAT",
            "AZURE_DEVOPS_URL",
            "AZURE_DEVOPS_ORG",
            "AZURE_DEVOPS_ORG_URL",
            "HOME",
        ] {
            std::env::remove_var(k);
        }
    }

    #[tokio::test]
    async fn is_set_rejects_placeholder_values() {
        let _env = ENV_LOCK.lock().await;
        clear_env();
        std::env::set_var("LINEAR_API_KEY", "your-linear-key-here");
        assert!(!is_set("LINEAR_API_KEY"));
        std::env::set_var("LINEAR_API_KEY", "lin_api_real_value");
        assert!(is_set("LINEAR_API_KEY"));
        clear_env();
    }

    #[tokio::test]
    async fn no_must_fix_ticket_yields_no_notification() {
        let _env = ENV_LOCK.lock().await;
        let pool = fresh_db().await;
        clear_env();
        std::env::set_var("LINEAR_API_KEY", "lin_real");
        insert_curation(&pool, "ENG-1", "linear", r#"[{"code":"in_progress"}]"#).await;

        maybe_notify(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        clear_env();
    }

    #[tokio::test]
    async fn must_fix_on_connected_provider_notifies_once_per_day() {
        let _env = ENV_LOCK.lock().await;
        let pool = fresh_db().await;
        clear_env();
        std::env::set_var("LINEAR_API_KEY", "lin_real");
        insert_curation(&pool, "ENG-1", "linear", r#"[{"code":"missing_due_date"}]"#).await;

        maybe_notify(&pool).await.unwrap();
        maybe_notify(&pool).await.unwrap(); // idempotent — same dedup key
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        clear_env();
    }

    #[tokio::test]
    async fn must_fix_on_disconnected_provider_is_ignored() {
        let _env = ENV_LOCK.lock().await;
        let pool = fresh_db().await;
        clear_env(); // nothing connected
        insert_curation(&pool, "OLD-1", "linear", r#"[{"code":"missing_due_date"}]"#).await;

        maybe_notify(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "a stale ticket on a disconnected tracker must not nudge"
        );
        clear_env();
    }

    #[tokio::test]
    async fn snoozed_mustfix_card_suppresses_the_notification() {
        let _env = ENV_LOCK.lock().await;
        let pool = fresh_db().await;
        clear_env();
        std::env::set_var("LINEAR_API_KEY", "lin_real");
        insert_curation(&pool, "ENG-1", "linear", r#"[{"code":"missing_due_date"}]"#).await;
        meridian_core::action_card_snooze::snooze_card(&pool, "mustfix", "9999-12-31T00:00:00Z")
            .await
            .unwrap();

        maybe_notify(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        clear_env();
    }
}
