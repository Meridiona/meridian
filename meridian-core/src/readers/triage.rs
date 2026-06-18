//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/triage` GET + the `decision`/`ignore` write sub-routes, ported to Rust.
//!
//! # What this is
//! The onboarding board-cleanup *working set*: the machine verdicts the daemon
//! writes into `pm_task_curation` after each PM sync, joined to the ticket and
//! ordered worst-first (`needs_detail → looks_stale → not_sure → ready`), with
//! each reason code turned into a human hint. The human *decision* writes
//! ([`record_decision`], [`set_ignored`]) record cleanup intent back into
//! `pm_task_curation` — both faithful ports of `triage/decision/route.ts` and
//! `triage/ignore/route.ts`. (`triage/apply` shells out to `meridian
//! ticket-update`, so it stays a tray command, not a DB write.)
//!
//! [`record_decision`] + [`Decision`] are the single source of truth: the daemon
//! re-exports them from `intelligence::task_triage::store` (they had no live
//! daemon caller, only tests) so the curation-decision SQL exists once.
//!
//! # Who calls this
//! - `get_triage` (read) + `triage_decision` / `triage_ignore` (writes) tray
//!   commands; the cleanup page (`ui/components/views/CleanupView.tsx`) drives the
//!   writes (the bare GET has no consumer yet — the page reads hygiene via
//!   `/api/tasks`, [`crate::tasks`]).
//!
//! # Related
//! - [`crate::hygiene`] maps reason codes too, but to *fixable* issues with a
//!   different (shorter) hint wording — do NOT share the mapping; triage shows
//!   ALL reasons and has its own [`reason_hint`]. Its `MUST_FIX` set also differs
//!   from [`IGNORE_MUST_FIX`] here — see that constant's note.
//! - [`crate::tasks`] embeds the per-ticket [`crate::hygiene::Hygiene`] verdict
//!   the cleanup UI actually renders.

use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::FromRow;
use tracing::Instrument;

/// One reason a ticket landed in its bucket, with a human-readable hint.
#[derive(Debug, Clone, Serialize)]
pub struct TriageReason {
    pub code: String,
    pub hint: String,
}

/// A ticket in the cleanup working set: the ticket identity + its machine
/// verdict (bucket, reasons) and the user's decision so far.
#[derive(Debug, Clone, Serialize)]
pub struct TriageTicket {
    pub task_key: String,
    pub provider: String,
    pub title: String,
    pub url: String,
    /// First ~160 chars of the description, ellipsised.
    pub description_excerpt: String,
    /// `ready | needs_detail | looks_stale | not_sure`.
    pub bucket: String,
    pub reasons: Vec<TriageReason>,
    pub decision: Option<String>,
    pub snoozed_until: Option<String>,
}

/// Bucket tallies for the cleanup header. `needs_attention` = everything not
/// `ready`; `undecided` = those of them with no decision yet.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TriageCounts {
    pub total: i64,
    pub ready: i64,
    pub needs_detail: i64,
    pub looks_stale: i64,
    pub not_sure: i64,
    pub needs_attention: i64,
    pub undecided: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriageResponse {
    pub items: Vec<TriageTicket>,
    pub counts: TriageCounts,
    /// `false` when `pm_task_curation` doesn't exist yet (fresh DB, no sync run).
    pub has_run: bool,
}

/// The joined `pm_task_curation`+`pm_tasks` row as read from SQLite.
#[derive(FromRow)]
struct RawRow {
    task_key: String,
    provider: Option<String>,
    title: String,
    url: Option<String>,
    description_text: String,
    bucket: String,
    reasons_json: Option<String>,
    decision: Option<String>,
    snoozed_until: Option<String>,
}

/// Reason code → human hint. A faithful mirror of the TS route's `reasonHint`,
/// which itself mirrors the Rust engine's `TriageReason::hint()`. Distinct from
/// [`crate::hygiene`]'s wording (see module docs) — kept in sync by hand. Only
/// `detail`-bearing reasons read `detail`.
fn reason_hint(code: &str, detail: Option<&Map<String, Value>>) -> String {
    // `${detail?.key}` interpolation: the number, or "undefined" when absent.
    let n = |key: &str| -> String {
        match detail.and_then(|d| d.get(key)) {
            Some(v) if v.is_number() => v.to_string(),
            _ => "undefined".to_string(),
        }
    };
    match code {
        "in_progress" => "Marked in progress on the board.".to_string(),
        "due_soon" => {
            let in_days = detail
                .and_then(|d| d.get("in_days"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            if in_days <= 0 {
                "Due today.".to_string()
            } else {
                format!("Due in {} day(s).", n("in_days"))
            }
        }
        "in_sprint" => "In the active sprint.".to_string(),
        "start_date_reached" => "Its start date has passed.".to_string(),
        "missing_description" => "No description — nothing to match your work against.".to_string(),
        "thin_description" => {
            format!(
                "Description is only {} characters — add a little detail.",
                n("chars")
            )
        }
        "vague_title" => "Title is generic — make it specific.".to_string(),
        "no_context_anchor" => "No epic or parent to anchor it.".to_string(),
        "missing_due_date" => "No due date — add one so Meridian knows when it's live.".to_string(),
        "no_activity_since" => format!("No board activity in {} days.", n("days")),
        "not_started" => "Still sitting in a not-started column.".to_string(),
        "no_due_date" => "No due date set.".to_string(),
        "overdue_long" => format!("Overdue by {} days with no movement.", n("by_days")),
        "far_future_due" => {
            format!(
                "Not due for {} days — planned, not current work.",
                n("in_days")
            )
        }
        "not_in_sprint" => "Not in any sprint.".to_string(),
        "already_done" => "Already marked done.".to_string(),
        "no_activity_signal" => "Open, but nothing yet says it's active.".to_string(),
        "unreadable_updated_at" => "Couldn't read its last-updated time.".to_string(),
        other => other.to_string(),
    }
}

/// Parse `reasons_json` (`[{code, detail?}]`) into hinted reasons; an absent or
/// malformed blob yields an empty list (matches the route's silent catch).
fn parse_reasons(reasons_json: Option<&str>) -> Vec<TriageReason> {
    let Some(raw) = reasons_json.and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok()) else {
        return Vec::new();
    };
    raw.into_iter()
        .filter_map(|r| {
            let code = r.get("code")?.as_str()?.to_string();
            let detail = r.get("detail").and_then(|d| d.as_object());
            let hint = reason_hint(&code, detail);
            Some(TriageReason { code, hint })
        })
        .collect()
}

/// Trim to the route's 160-char excerpt (157 + `…`). Char-based (vs the route's
/// UTF-16 `slice`) — identical for the common ASCII/BMP descriptions.
fn excerpt(description: &str) -> String {
    let trimmed = description.trim();
    if trimmed.chars().count() > 160 {
        let head: String = trimmed.chars().take(157).collect();
        format!("{head}…")
    } else {
        trimmed.to_string()
    }
}

/// Build the cleanup working set. `now_iso` (RFC3339) hides tickets snoozed to a
/// future moment, mirroring the daemon's `load_working_set`. Resolve `now` in
/// the caller (the tray command) so this stays deterministic/testable.
#[tracing::instrument(skip(pool))]
pub async fn get_triage(pool: &SqlitePool, now_iso: &str) -> anyhow::Result<TriageResponse> {
    // pm_task_curation only exists after migration 038 + a sync — tolerate a
    // fresh DB by reporting has_run = false (matches the route).
    let exists: Option<(i64,)> = sqlx::query_as::<_, (i64,)>(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='pm_task_curation'",
    )
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("triage.read.table_exists"))
    .await
    .context("triage: detect pm_task_curation")?;
    if exists.is_none() {
        tracing::debug!("pm_task_curation absent — triage has not run");
        return Ok(TriageResponse {
            items: Vec::new(),
            counts: TriageCounts::default(),
            has_run: false,
        });
    }

    let rows: Vec<RawRow> = sqlx::query_as::<_, RawRow>(
        r#"
        SELECT t.task_key, t.provider, t.title, t.url,
               COALESCE(t.description_text,'') AS description_text,
               c.bucket, c.reasons_json, c.decision, c.snoozed_until
        FROM pm_task_curation c
        JOIN pm_tasks t ON t.task_key = c.task_key
        WHERE c.snoozed_until IS NULL OR c.snoozed_until <= ?
        ORDER BY CASE c.bucket
          WHEN 'needs_detail' THEN 0
          WHEN 'looks_stale'  THEN 1
          WHEN 'not_sure'     THEN 2
          ELSE 3 END, t.task_key
        "#,
    )
    .bind(now_iso)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("triage.read.working_set"))
    .await
    .context("triage: fetch working set")?;
    tracing::debug!(rows = rows.len(), "triage.read.working_set");

    let mut counts = TriageCounts::default();
    let items: Vec<TriageTicket> = rows
        .into_iter()
        .map(|r| {
            counts.total += 1;
            match r.bucket.as_str() {
                "ready" => counts.ready += 1,
                "needs_detail" => counts.needs_detail += 1,
                "looks_stale" => counts.looks_stale += 1,
                "not_sure" => counts.not_sure += 1,
                _ => {}
            }
            if r.bucket != "ready" {
                counts.needs_attention += 1;
                if r.decision.is_none() {
                    counts.undecided += 1;
                }
            }
            TriageTicket {
                provider: r
                    .provider
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "jira".to_string()),
                description_excerpt: excerpt(&r.description_text),
                url: r.url.unwrap_or_default(),
                reasons: parse_reasons(r.reasons_json.as_deref()),
                bucket: r.bucket,
                decision: r.decision,
                snoozed_until: r.snoozed_until,
                task_key: r.task_key,
                title: r.title,
            }
        })
        .collect();

    tracing::info!(
        total = counts.total,
        needs_attention = counts.needs_attention,
        undecided = counts.undecided,
        "triage working set served"
    );
    Ok(TriageResponse {
        items,
        counts,
        has_run: true,
    })
}

// ── Writes (the decision + ignore sub-routes) ─────────────────────────────────

/// A human decision on a triaged ticket. `keep` returns it to the working set;
/// `excluded` drops it from classification candidates; `snoozed` defers it.
/// Re-exported by the daemon's `task_triage::store` so the decision SQL is
/// defined once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Keep,
    Excluded,
    Snoozed,
}

impl Decision {
    /// The stored string form (matches the route's allowed `decision` values).
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Keep => "keep",
            Decision::Excluded => "excluded",
            Decision::Snoozed => "snoozed",
        }
    }

    /// Parse a request string; `None` for anything outside keep/excluded/snoozed
    /// (the route's `DECISIONS` allow-set).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "keep" => Some(Decision::Keep),
            "excluded" => Some(Decision::Excluded),
            "snoozed" => Some(Decision::Snoozed),
            _ => None,
        }
    }
}

/// Hygiene codes that can NEVER be dismissed via `ignore` (the route's `MUST_FIX`
/// gate). **Deliberately distinct from [`crate::hygiene`]'s `MUST_FIX`**: that set
/// drives reader *display severity* and lists 4 codes; this set decides what is
/// *dismissible* and additionally blocks `overdue`. They are different concepts —
/// porting the route means replicating ITS 5-code set, not reusing the reader's.
/// (If they should ever be unified, that is a product decision, not a port.)
pub const IGNORE_MUST_FIX: &[&str] = &[
    "missing_description",
    "thin_description",
    "vague_title",
    "missing_due_date",
    "overdue",
];

/// A triage write rejected by a business rule (mapped to the route's 404/409).
/// Typed so callers/tests branch by variant; the command stringifies it.
#[derive(Debug)]
pub enum TriageWriteError {
    /// No `pm_task_curation` row for the key — the ticket hasn't been triaged
    /// (the route's 404). The UPDATE would match 0 rows and silently lose the
    /// decision, so this is an error, not a no-op.
    UnknownTaskKey(String),
    /// A must-fix hygiene code can't be ignored (the route's 409).
    MustFixCannotBeIgnored(String),
}

impl std::fmt::Display for TriageWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTaskKey(k) => write!(f, "unknown task_key: {k}"),
            Self::MustFixCannotBeIgnored(c) => {
                write!(f, "must-fix issues cannot be ignored: {c}")
            }
        }
    }
}
impl std::error::Error for TriageWriteError {}

/// Record a human cleanup decision on a ticket (port of `triage/decision`).
/// Idempotent UPDATE on `pm_task_curation`; `snoozed_until` is the caller-resolved
/// expiry (only set for `snoozed`). Errors with [`TriageWriteError::UnknownTaskKey`]
/// when no curation row exists — the route's 404, so a decision on an un-triaged
/// ticket can't be silently lost.
#[tracing::instrument(skip(pool), fields(decision = decision.as_str()))]
pub async fn record_decision(
    pool: &SqlitePool,
    task_key: &str,
    decision: Decision,
    snoozed_until: Option<&str>,
    now: &str,
) -> anyhow::Result<()> {
    let res = sqlx::query(
        "UPDATE pm_task_curation \
         SET decision = ?, decided_at = ?, snoozed_until = ? \
         WHERE task_key = ?",
    )
    .bind(decision.as_str())
    .bind(now)
    .bind(snoozed_until)
    .bind(task_key)
    .execute(pool)
    .instrument(tracing::debug_span!(
        "triage.write.pm_task_curation.decision"
    ))
    .await
    .with_context(|| format!("recording decision for {task_key}"))?;
    if res.rows_affected() == 0 {
        return Err(TriageWriteError::UnknownTaskKey(task_key.to_string()).into());
    }
    tracing::info!(
        task_key,
        decision = decision.as_str(),
        "triage decision recorded"
    );
    Ok(())
}

/// Toggle an optional hygiene defect's "ignored" flag on a ticket (port of
/// `triage/ignore`). Must-fix codes ([`IGNORE_MUST_FIX`]) are rejected; `undo`
/// removes the code instead of adding it. Returns the resulting ignored-code set
/// (the route's `ignored` ack field). Errors with [`TriageWriteError`] for a
/// must-fix code (409) or an unknown key (404).
#[tracing::instrument(skip(pool))]
pub async fn set_ignored(
    pool: &SqlitePool,
    task_key: &str,
    code: &str,
    undo: bool,
) -> anyhow::Result<Vec<String>> {
    if IGNORE_MUST_FIX.contains(&code) {
        return Err(TriageWriteError::MustFixCannotBeIgnored(code.to_string()).into());
    }
    let existing: Option<String> =
        sqlx::query_scalar("SELECT ignored_codes FROM pm_task_curation WHERE task_key = ?")
            .bind(task_key)
            .fetch_optional(pool)
            .instrument(tracing::debug_span!("triage.read.pm_task_curation.ignored"))
            .await
            .context("reading ignored_codes")?;
    let Some(existing) = existing else {
        return Err(TriageWriteError::UnknownTaskKey(task_key.to_string()).into());
    };

    // Mirror the route's `new Set(codes)` then add/delete EXACTLY: dedup the
    // stored array preserving first-seen order, then append `code` only if absent
    // (re-adding is a no-op, not a move-to-end), or drop it on `undo`. A malformed
    // blob resets to empty (the route's try/catch → []).
    let parsed: Vec<String> = serde_json::from_str(&existing).unwrap_or_default();
    let mut codes: Vec<String> = Vec::new();
    for c in parsed {
        if !codes.contains(&c) {
            codes.push(c);
        }
    }
    let present = codes.iter().any(|c| c == code);
    if undo {
        codes.retain(|c| c != code);
    } else if !present {
        codes.push(code.to_string());
    }

    let codes_json = serde_json::to_string(&codes).context("serialising ignored_codes")?;
    sqlx::query("UPDATE pm_task_curation SET ignored_codes = ? WHERE task_key = ?")
        .bind(&codes_json)
        .bind(task_key)
        .execute(pool)
        .instrument(tracing::debug_span!(
            "triage.write.pm_task_curation.ignored"
        ))
        .await
        .context("updating ignored_codes")?;
    tracing::info!(task_key, code, undo, "triage ignore toggled");
    Ok(codes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_curation() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE pm_task_curation (\
                task_key TEXT PRIMARY KEY, decision TEXT, decided_at TEXT, \
                snoozed_until TEXT, ignored_codes TEXT NOT NULL DEFAULT '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn decision_roundtrips_through_string() {
        for d in [Decision::Keep, Decision::Excluded, Decision::Snoozed] {
            assert_eq!(Decision::parse(d.as_str()), Some(d));
        }
        assert_eq!(Decision::parse("bogus"), None);
    }

    #[tokio::test]
    async fn record_decision_updates_or_errors_on_unknown_key() {
        let pool = pool_with_curation().await;
        sqlx::query("INSERT INTO pm_task_curation (task_key) VALUES ('T-1')")
            .execute(&pool)
            .await
            .unwrap();

        record_decision(
            &pool,
            "T-1",
            Decision::Excluded,
            None,
            "2026-06-18T00:00:00Z",
        )
        .await
        .unwrap();
        let d: Option<String> =
            sqlx::query_scalar("SELECT decision FROM pm_task_curation WHERE task_key='T-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(d.as_deref(), Some("excluded"));

        // Unknown key → UnknownTaskKey, never a silent 0-row no-op.
        let err = record_decision(&pool, "GHOST", Decision::Keep, None, "2026-06-18T00:00:00Z")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown task_key"));
    }

    #[tokio::test]
    async fn set_ignored_adds_undo_and_blocks_must_fix() {
        let pool = pool_with_curation().await;
        sqlx::query("INSERT INTO pm_task_curation (task_key, ignored_codes) VALUES ('T-1','[]')")
            .execute(&pool)
            .await
            .unwrap();

        let codes = set_ignored(&pool, "T-1", "stale_status", false)
            .await
            .unwrap();
        assert_eq!(codes, vec!["stale_status".to_string()]);
        // Adding again de-dups (Set semantics).
        let codes = set_ignored(&pool, "T-1", "stale_status", false)
            .await
            .unwrap();
        assert_eq!(codes, vec!["stale_status".to_string()]);
        // Undo removes it.
        let codes = set_ignored(&pool, "T-1", "stale_status", true)
            .await
            .unwrap();
        assert!(codes.is_empty());

        // Must-fix code (incl. the route-only `overdue`) is rejected.
        let err = set_ignored(&pool, "T-1", "overdue", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must-fix"));
        // Unknown key → error.
        let err = set_ignored(&pool, "GHOST", "stale_status", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown task_key"));
    }
}
