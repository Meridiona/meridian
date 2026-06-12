//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Persistence for ticket triage: read the raw board out of `pm_tasks`, write the
// machine verdicts into `pm_task_curation`, and read/write the human decision.
//
// Two invariants this layer guarantees:
//   1. Re-triage NEVER overwrites a human decision. The verdict UPSERT touches
//      only `bucket` / `reasons_json` / `triaged_at`; `decision`, `decided_at`,
//      `snoozed_until`, and `enriched_description` are preserved (CLAUDE.md: PM
//      writes are idempotent UPSERTs, never DELETE+INSERT).
//   2. Curation rows for tickets that left the board (pruned from `pm_tasks`) are
//      cleaned up, so the working set never references a vanished ticket.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::{TicketSignals, TriageVerdict};

/// One board row plus its owning provider — the input to a triage pass.
pub struct CurationInput {
    pub provider: String,
    pub signals: TicketSignals,
}

/// A ticket joined with its curation state — what the onboarding UI/API reads.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CuratedTask {
    pub task_key: String,
    pub provider: String,
    pub title: String,
    pub url: String,
    pub bucket: String,
    pub reasons_json: String,
    pub decision: Option<String>,
    pub snoozed_until: Option<String>,
    pub enriched_description: Option<String>,
}

/// A human decision on a triaged ticket. `keep` returns it to the working set;
/// `excluded` drops it from classification candidates; `snoozed` defers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Keep,
    Excluded,
    Snoozed,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Keep => "keep",
            Decision::Excluded => "excluded",
            Decision::Snoozed => "snoozed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "keep" => Some(Decision::Keep),
            "excluded" => Some(Decision::Excluded),
            "snoozed" => Some(Decision::Snoozed),
            _ => None,
        }
    }
}

/// Read every cached ticket out of `pm_tasks` as triage input.
pub async fn load_board(pool: &SqlitePool) -> Result<Vec<CurationInput>> {
    let rows = sqlx::query(
        "SELECT task_key, provider, title, description_text, status_raw, is_terminal, \
                sprint_name, due_date, start_date, updated_at, epic_title, parent_key \
         FROM pm_tasks",
    )
    .fetch_all(pool)
    .await
    .context("loading pm_tasks for triage")?;

    Ok(rows
        .into_iter()
        .map(|r| CurationInput {
            provider: r.get("provider"),
            signals: TicketSignals {
                task_key: r.get("task_key"),
                title: r.get("title"),
                description_text: r.get("description_text"),
                status_raw: r.get("status_raw"),
                is_terminal: r.get::<i64, _>("is_terminal") != 0,
                sprint_name: r.get("sprint_name"),
                due_date: r.get("due_date"),
                start_date: r.get("start_date"),
                updated_at: r.get("updated_at"),
                epic_title: r.get("epic_title"),
                parent_key: r.get("parent_key"),
            },
        })
        .collect())
}

/// Upsert one machine verdict. Preserves any existing human decision.
pub async fn save_verdict(
    pool: &SqlitePool,
    provider: &str,
    verdict: &TriageVerdict,
    now: &str,
) -> Result<()> {
    let reasons_json =
        serde_json::to_string(&verdict.reasons).context("serialising triage reasons")?;
    sqlx::query(
        "INSERT INTO pm_task_curation (task_key, provider, bucket, reasons_json, triaged_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(task_key) DO UPDATE SET \
           provider     = excluded.provider, \
           bucket       = excluded.bucket, \
           reasons_json = excluded.reasons_json, \
           triaged_at   = excluded.triaged_at",
    )
    .bind(&verdict.task_key)
    .bind(provider)
    .bind(verdict.bucket.as_str())
    .bind(&reasons_json)
    .bind(now)
    .execute(pool)
    .await
    .with_context(|| format!("saving triage verdict for {}", verdict.task_key))?;
    Ok(())
}

/// Record a human decision on a ticket. Written once per user action; idempotent.
pub async fn record_decision(
    pool: &SqlitePool,
    task_key: &str,
    decision: Decision,
    snoozed_until: Option<&str>,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE pm_task_curation \
         SET decision = ?, decided_at = ?, snoozed_until = ? \
         WHERE task_key = ?",
    )
    .bind(decision.as_str())
    .bind(now)
    .bind(snoozed_until)
    .bind(task_key)
    .execute(pool)
    .await
    .with_context(|| format!("recording decision for {task_key}"))?;
    Ok(())
}

/// Delete curation rows whose ticket is no longer in `pm_tasks`. Returns the count
/// removed. Keeps the working set from pointing at tickets that left the board.
pub async fn prune_orphans(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query(
        "DELETE FROM pm_task_curation \
         WHERE task_key NOT IN (SELECT task_key FROM pm_tasks)",
    )
    .execute(pool)
    .await
    .context("pruning orphaned curation rows")?;
    Ok(res.rows_affected())
}

/// Read the working set for the onboarding UI: tickets joined with curation,
/// worst-first (needs_detail / looks_stale before not_sure before ready), and
/// hiding snoozed-until-future rows.
pub async fn load_working_set(pool: &SqlitePool, now: &str) -> Result<Vec<CuratedTask>> {
    let rows = sqlx::query(
        "SELECT t.task_key, t.provider, t.title, t.url, \
                c.bucket, c.reasons_json, c.decision, c.snoozed_until, c.enriched_description \
         FROM pm_task_curation c \
         JOIN pm_tasks t ON t.task_key = c.task_key \
         WHERE c.snoozed_until IS NULL OR c.snoozed_until <= ? \
         ORDER BY CASE c.bucket \
           WHEN 'needs_detail' THEN 0 \
           WHEN 'looks_stale'  THEN 1 \
           WHEN 'not_sure'     THEN 2 \
           ELSE 3 END, t.task_key",
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .context("loading triage working set")?;

    Ok(rows
        .into_iter()
        .map(|r| CuratedTask {
            task_key: r.get("task_key"),
            provider: r.get("provider"),
            title: r.get("title"),
            url: r.get("url"),
            bucket: r.get("bucket"),
            reasons_json: r.get("reasons_json"),
            decision: r.get("decision"),
            snoozed_until: r.get("snoozed_until"),
            enriched_description: r.get("enriched_description"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::task_triage::run_triage;
    use chrono::{TimeZone, Utc};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    /// Insert a minimal pm_tasks row. `extra` is appended SQL for optional columns.
    async fn insert_task(
        pool: &SqlitePool,
        key: &str,
        status_raw: &str,
        is_terminal: i64,
        desc: &str,
        due: Option<&str>,
        updated_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO pm_tasks (task_key, provider, title, description_text, status_raw, \
                is_terminal, url, due_date, updated_at) \
             VALUES (?, 'jira', ?, ?, ?, ?, 'http://x', ?, ?)",
        )
        .bind(key)
        .bind(format!("Title for {key} which is plenty specific"))
        .bind(desc)
        .bind(status_raw)
        .bind(is_terminal)
        .bind(due)
        .bind(updated_at)
        .execute(pool)
        .await
        .unwrap();
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn run_triage_buckets_and_persists() {
        let pool = db().await;
        let long = "A".repeat(120);
        // Active + detailed → ready.
        insert_task(
            &pool,
            "READY-1",
            "In Progress",
            0,
            &long,
            None,
            "2026-06-11T00:00:00Z",
        )
        .await;
        // Not started, no due, very old → looks_stale.
        insert_task(
            &pool,
            "STALE-1",
            "Backlog",
            0,
            &long,
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        // Active but empty description → needs_detail.
        insert_task(
            &pool,
            "THIN-1",
            "In Progress",
            0,
            "",
            None,
            "2026-06-11T00:00:00Z",
        )
        .await;
        // Done → looks_stale.
        insert_task(
            &pool,
            "DONE-1",
            "Done",
            1,
            &long,
            None,
            "2026-06-11T00:00:00Z",
        )
        .await;

        let s = run_triage(&pool, now()).await.unwrap();
        assert_eq!(s.ready, 1, "ready");
        assert_eq!(s.needs_detail, 1, "needs_detail");
        assert_eq!(s.looks_stale, 2, "looks_stale (stale + done)");
        assert_eq!(s.needs_attention(), 3);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pm_task_curation")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 4);
    }

    #[tokio::test]
    async fn retriage_preserves_human_decision() {
        let pool = db().await;
        let long = "A".repeat(120);
        insert_task(
            &pool,
            "STALE-1",
            "Backlog",
            0,
            &long,
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        run_triage(&pool, now()).await.unwrap();

        // User says keep.
        record_decision(
            &pool,
            "STALE-1",
            Decision::Keep,
            None,
            "2026-06-12T12:00:00Z",
        )
        .await
        .unwrap();

        // A later sync re-triages — the decision must survive.
        run_triage(&pool, now()).await.unwrap();
        let decision: Option<String> =
            sqlx::query_scalar("SELECT decision FROM pm_task_curation WHERE task_key = 'STALE-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(decision.as_deref(), Some("keep"));
    }

    #[tokio::test]
    async fn prune_removes_orphans_on_retriage() {
        let pool = db().await;
        let long = "A".repeat(120);
        insert_task(
            &pool,
            "T-1",
            "Backlog",
            0,
            &long,
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        insert_task(
            &pool,
            "T-2",
            "Backlog",
            0,
            &long,
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        run_triage(&pool, now()).await.unwrap();

        // T-2 leaves the board (provider prune).
        sqlx::query("DELETE FROM pm_tasks WHERE task_key = 'T-2'")
            .execute(&pool)
            .await
            .unwrap();
        let s = run_triage(&pool, now()).await.unwrap();
        assert_eq!(s.pruned, 1);

        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT task_key FROM pm_task_curation ORDER BY task_key")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec!["T-1".to_string()]);
    }

    #[tokio::test]
    async fn working_set_is_worst_first_and_hides_future_snooze() {
        let pool = db().await;
        let long = "A".repeat(120);
        insert_task(
            &pool,
            "READY-1",
            "In Progress",
            0,
            &long,
            None,
            "2026-06-11T00:00:00Z",
        )
        .await;
        insert_task(
            &pool,
            "STALE-1",
            "Backlog",
            0,
            &long,
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        insert_task(
            &pool,
            "THIN-1",
            "In Progress",
            0,
            "",
            None,
            "2026-06-11T00:00:00Z",
        )
        .await;
        run_triage(&pool, now()).await.unwrap();

        let ws = load_working_set(&pool, "2026-06-12T12:00:00+00:00")
            .await
            .unwrap();
        // needs_detail before looks_stale before ready.
        let order: Vec<&str> = ws.iter().map(|c| c.task_key.as_str()).collect();
        assert_eq!(order, vec!["THIN-1", "STALE-1", "READY-1"]);

        // Snooze the stale one into the future — it drops out of the working set.
        record_decision(
            &pool,
            "STALE-1",
            Decision::Snoozed,
            Some("2026-07-01T00:00:00+00:00"),
            "2026-06-12T12:00:00+00:00",
        )
        .await
        .unwrap();
        let ws2 = load_working_set(&pool, "2026-06-12T12:00:00+00:00")
            .await
            .unwrap();
        let keys: Vec<&str> = ws2.iter().map(|c| c.task_key.as_str()).collect();
        assert_eq!(keys, vec!["THIN-1", "READY-1"]);
    }

    #[test]
    fn decision_roundtrips() {
        for d in [Decision::Keep, Decision::Excluded, Decision::Snoozed] {
            assert_eq!(Decision::parse(d.as_str()), Some(d));
        }
        assert_eq!(Decision::parse("bogus"), None);
    }
}
