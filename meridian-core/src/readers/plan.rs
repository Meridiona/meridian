//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/plan` GET + POST ported to Rust — the daily "what I'm working on today"
//! board: the dev's committed set, ranked suggestions, and the full scored board.
//!
//! A faithful port of `ui/app/api/plan/route.ts` + its scoring lib
//! `ui/lib/daily-plan.ts`. Scoring is additive over pure board signals (no LLM):
//! carry-over, in-progress, due-soon, recently-worked, plus a small `updated_at`
//! tiebreaker — sorted so the most-likely-today tickets float to the top.
//!
//! This is the FIRST data-writer in `meridian-core`. The reads and the six write
//! actions co-locate here because a POST returns the freshly-scored response
//! ([`build_plan_response`]) — splitting the write SQL into the tray would force
//! re-exposing the whole scoring surface anyway. Writes go through the same
//! shared pool the readers use (see [`crate::db::open_existing`]); the daemon
//! still solely owns the SCHEMA (migrations) — we only touch `daily_plan` rows.
//!
//! # Who calls this
//! - Commands: `get_plan` (read) + `plan_action` (write), registered in the
//!   tray's `lib.rs`.
//! - Frontend: `ui/components/views/PlanView.tsx` via `ui/lib/bridge.ts`
//!   (`load` for the GET, `mutate` for the POST).
//!
//! # Related
//! - [`crate::task_detail`] — the per-ticket detail dialog drilled into from a card.
//! - [`crate::date`] — `due_days_from` / `local_day_bounds` / `today_string`, reused here.
//! - [`crate::tasks`] — the per-task time + hygiene page over the same `pm_tasks`.

use crate::SqlitePool;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row};
use std::collections::{HashMap, HashSet};
use tracing::Instrument;

// ── Tunables (mirror lib/daily-plan.ts) ──────────────────────────────────────
/// "Worked recently" lookback in days. Public because the command resolves
/// `now` and derives the `recent_since_day` bound from it (kept single-sourced).
pub const RECENT_WORK_DAYS: i64 = 3;
const DUE_SOON_DAYS: i64 = 14; // due within this many days counts as a soon signal
const SUGGESTION_CAP: usize = 5; // how many tasks to pre-fill in the morning
const EXCERPT_LEN: usize = 130; // description excerpt length for card display

// ── Types (field names match the TS interfaces byte-for-byte) ─────────────────

/// One committed plan row, joined with its LIVE `pm_tasks` state (snapshot
/// fallback when the ticket has since left the active board). Mirrors `PlanItem`.
#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub task_key: String,
    pub position: i64,
    pub origin: String,
    pub title: String,
    pub provider: String,
    pub url: String,
    pub status: String,
    pub is_terminal: bool,
    pub due_date: Option<String>,
    pub due_days: Option<i64>,
    // TaskMeta (flattened, as the TS interface extends it):
    pub description: String,
    pub epic: Option<String>,
    pub priority: Option<String>,
    pub issue_type: String,
    pub story_points: Option<String>,
}

/// One scored, candidate board ticket. Mirrors `AvailableTask`.
#[derive(Debug, Clone, Serialize)]
pub struct AvailableTask {
    pub key: String,
    pub title: String,
    pub provider: String,
    pub url: String,
    pub status: String,
    pub is_terminal: bool,
    pub due_date: Option<String>,
    pub due_days: Option<i64>,
    pub started: bool,
    pub carryover: bool,
    pub worked_recently: bool,
    pub score: i64,
    pub origin: String,
    pub reason: String,
    // TaskMeta:
    pub description: String,
    pub epic: Option<String>,
    pub priority: Option<String>,
    pub issue_type: String,
    pub story_points: Option<String>,
}

/// The full `/api/plan` payload for a day. Mirrors `PlanResponse`.
#[derive(Debug, Clone, Serialize)]
pub struct PlanResponse {
    pub date: String,
    pub has_table: bool,
    pub confirmed: bool,
    pub skipped: bool,
    pub plan: Vec<PlanItem>,
    pub suggestions: Vec<AvailableTask>,
    pub available: Vec<AvailableTask>,
}

/// The POST body (`{ action, date?, task_key?, task_keys? }`). A single payload
/// object — not separate `invoke` args — so the Tauri (camelCase→snake_case) and
/// browser (`JSON.stringify`) paths send one identical snake_case shape.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanBody {
    pub action: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub task_key: Option<String>,
    /// `None` = key absent (a 400 for confirm/set); `Some([])` = explicit "clear".
    #[serde(default)]
    pub task_keys: Option<Vec<String>>,
}

// ── Small helpers ─────────────────────────────────────────────────────────────

/// One blank-to-`None` trimmed string (mirrors the TS `(x)?.trim() || null`).
fn trimmed(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Short single-line excerpt of a description for card display. Mirrors the TS
/// `excerpt`: collapse whitespace runs to one space, trim, and ellipsise past
/// `EXCERPT_LEN`. JS slices by UTF-16 code units; we slice by `char`, which
/// agrees for BMP text and differs only on astral codepoints (accepted edge).
fn excerpt(s: Option<&str>) -> String {
    let collapsed = s
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() > EXCERPT_LEN {
        let head: String = chars[..EXCERPT_LEN - 1].iter().collect();
        format!("{}…", head.trim_end())
    } else {
        collapsed
    }
}

/// Lightweight in-progress heuristic, for SCORING ONLY (the Rust triage engine
/// owns authoritative startedness). Word-ish contains, mirrors `looksStarted`.
const STARTED_HINTS: &[&str] = &[
    "progress",
    "doing",
    "wip",
    "review",
    "qa",
    "testing",
    "dev",
    "implement",
    "active",
    "building",
    "ongoing",
    "started",
];
fn looks_started(status: &str) -> bool {
    let s = status.to_lowercase();
    STARTED_HINTS.iter().any(|h| s.contains(h))
}

/// Due-date score component (mirrors `dueComponent`). Overdue is the strongest.
fn due_component(due_days: Option<i64>) -> i64 {
    match due_days {
        None => 0,
        Some(d) if d < 0 => 400,
        Some(d) if d <= 2 => 350,
        Some(d) if d <= 7 => 250,
        Some(d) if d <= DUE_SOON_DAYS => 120,
        Some(d) if d <= 30 => 40,
        Some(_) => 0,
    }
}

/// Friendly due label (mirrors `dueReason`); `None` when far-future / no date.
fn due_reason(due_days: Option<i64>) -> Option<String> {
    match due_days {
        None => None,
        Some(d) if d < 0 => Some(format!("Overdue {}d", -d)),
        Some(0) => Some("Due today".to_string()),
        Some(1) => Some("Due tomorrow".to_string()),
        Some(d) if d <= DUE_SOON_DAYS => Some(format!("Due in {d}d")),
        Some(_) => None,
    }
}

/// Whether `name` is a real table in this DB (mirrors `tableExists`).
async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

// ── Loaders ───────────────────────────────────────────────────────────────────

struct MetaRow {
    confirmed_at: Option<String>,
    skipped: i64,
}

async fn load_meta(pool: &SqlitePool, date: &str) -> anyhow::Result<MetaRow> {
    if !table_exists(pool, "daily_plan_meta").await {
        return Ok(MetaRow {
            confirmed_at: None,
            skipped: 0,
        });
    }
    let row = sqlx::query("SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?")
        .bind(date)
        .fetch_optional(pool)
        .instrument(tracing::debug_span!("plan.read.daily_plan_meta"))
        .await?;
    Ok(match row {
        Some(r) => MetaRow {
            confirmed_at: r.try_get("confirmed_at").unwrap_or(None),
            skipped: r.try_get("skipped").unwrap_or(0),
        },
        None => MetaRow {
            confirmed_at: None,
            skipped: 0,
        },
    })
}

/// task_keys committed on the most recent planned day strictly before `date`.
async fn carryover_keys(pool: &SqlitePool, date: &str) -> anyhow::Result<HashSet<String>> {
    if !table_exists(pool, "daily_plan").await {
        return Ok(HashSet::new());
    }
    let prior: Option<String> =
        sqlx::query_scalar("SELECT MAX(plan_date) FROM daily_plan WHERE plan_date < ?")
            .bind(date)
            .fetch_one(pool)
            .await
            .unwrap_or(None);
    let Some(prior) = prior else {
        return Ok(HashSet::new());
    };
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT task_key FROM daily_plan WHERE plan_date = ?")
            .bind(&prior)
            .fetch_all(pool)
            .instrument(tracing::debug_span!("plan.read.daily_plan.carryover"))
            .await?;
    Ok(keys.into_iter().collect())
}

/// task_key → most recent worked timestamp within the lookback window.
/// `recent_since_day` is the LOCAL `YYYY-MM-DD` of `now − RECENT_WORK_DAYS`; we
/// take its `local_day_bounds().start` as the `started_at >=` bound (mirrors
/// `recentWorkedKeys`, which does the same local-day rounding).
async fn recent_worked_keys(
    pool: &SqlitePool,
    recent_since_day: &str,
) -> anyhow::Result<HashMap<String, String>> {
    let (start, _) = crate::date::local_day_bounds(recent_since_day);
    let rows = sqlx::query(
        r#"SELECT task_key, MAX(started_at) AS last_at
           FROM app_sessions
           WHERE task_key IS NOT NULL AND task_session_type = 'task' AND started_at >= ?
           GROUP BY task_key"#,
    )
    .bind(&start)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("plan.read.app_sessions.recent"))
    .await?;
    let mut map = HashMap::new();
    for r in rows {
        let key: String = r.try_get("task_key").unwrap_or_default();
        let last_at: Option<String> = r.try_get("last_at").unwrap_or(None);
        if let Some(last_at) = last_at {
            map.insert(key, last_at);
        }
    }
    Ok(map)
}

#[derive(FromRow)]
struct BoardRow {
    task_key: String,
    title: Option<String>,
    provider: Option<String>,
    url: Option<String>,
    status_raw: String,
    is_terminal: i64,
    due_date: Option<String>,
    updated_at: Option<String>,
    description_text: Option<String>,
    epic_title: Option<String>,
    parent_key: Option<String>,
    priority: Option<String>,
    issue_type: Option<String>,
    story_points: Option<String>,
    decision: Option<String>,
}

/// Every candidate board ticket (non-excluded, non-terminal), scored & sorted
/// top-first. Mirrors `buildAvailable`. `today` drives the calendar due-days
/// math; `now_ms` drives the FRACTIONAL-day recency/`updated_at` components
/// (a different mechanism — do not conflate with the calendar diff).
#[tracing::instrument(skip(pool), fields(date = %date))]
pub async fn build_available(
    pool: &SqlitePool,
    date: &str,
    today: NaiveDate,
    now_ms: i64,
    recent_since_day: &str,
) -> anyhow::Result<Vec<AvailableTask>> {
    let has_curation = table_exists(pool, "pm_task_curation").await;
    let sql = format!(
        r#"SELECT t.task_key, t.title, t.provider, t.url,
                  COALESCE(t.status_raw,'') AS status_raw,
                  COALESCE(t.is_terminal,0) AS is_terminal,
                  t.due_date, t.updated_at,
                  t.description_text, t.epic_title, t.parent_key,
                  t.priority, t.issue_type, t.story_points,
                  {} AS decision
           FROM pm_tasks t
           {}"#,
        if has_curation { "c.decision" } else { "NULL" },
        if has_curation {
            "LEFT JOIN pm_task_curation c ON c.task_key = t.task_key"
        } else {
            ""
        },
    );
    let rows: Vec<BoardRow> = sqlx::query_as::<_, BoardRow>(&sql)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("plan.read.pm_tasks"))
        .await?;
    tracing::debug!(rows = rows.len(), "plan.read.pm_tasks");

    let carry = carryover_keys(pool, date).await?;
    let worked = recent_worked_keys(pool, recent_since_day).await?;

    let mut items: Vec<AvailableTask> = Vec::new();
    for r in rows {
        if r.decision.as_deref() == Some("excluded") {
            continue; // honour board cleanup
        }
        let is_terminal = r.is_terminal != 0;
        if is_terminal {
            continue; // done tickets aren't today's work
        }
        let due_days = crate::date::due_days_from(r.due_date.as_deref(), today);
        let started = looks_started(&r.status_raw);
        let carryover = carry.contains(&r.task_key);
        let worked_at = worked.get(&r.task_key).cloned();
        let worked_recently = worked_at.is_some();

        // recency-of-work component (fractional elapsed days)
        let mut recent_comp = 0i64;
        if let Some(ref wa) = worked_at {
            if let Some(ms) = crate::intervals::parse_ms(wa) {
                let age_days = (now_ms - ms) as f64 / 86_400_000.0;
                recent_comp = if age_days < 1.0 {
                    200
                } else if age_days < 2.0 {
                    150
                } else {
                    80
                };
            }
        }
        // small updated_at tiebreaker — replicates `max(0, 30 - min(30, floor(age)))`.
        // (A future `updated_at` yields a negative age whose floor is < 0, so the
        // inner `min(30, …)` keeps the negative and 30−neg can exceed 30 — faithfully
        // reproduced rather than clamped to the comment's nominal 0..30.)
        let mut upd_comp = 0i64;
        if let Some(ref ua) = r.updated_at {
            if let Some(ms) = crate::intervals::parse_ms(ua) {
                let age_days = (now_ms - ms) as f64 / 86_400_000.0;
                let floored = age_days.floor() as i64;
                upd_comp = (30 - floored.min(30)).max(0);
            }
        }

        let score = (if carryover { 500 } else { 0 })
            + (if started { 300 } else { 0 })
            + due_component(due_days)
            + recent_comp
            + upd_comp;

        // primary origin + friendly reason (highest-weight signal wins)
        let dr = due_reason(due_days);
        let (origin, reason) = if carryover {
            ("carryover", "Carried over".to_string())
        } else if started {
            ("in_progress", "In progress".to_string())
        } else if let Some(dr) = dr {
            ("due_soon", dr)
        } else if worked_recently {
            (
                "recent",
                if recent_comp >= 150 {
                    "Worked recently".to_string()
                } else {
                    "Worked this week".to_string()
                },
            )
        } else {
            ("manual", "On your board".to_string())
        };

        items.push(AvailableTask {
            key: r.task_key,
            // pm_tasks.title is non-null in practice (the provider sync always sets a
            // summary); null → "" rather than the TS `null` so consumers never crash.
            title: r.title.unwrap_or_default(),
            provider: trimmed(r.provider).unwrap_or_else(|| "jira".to_string()),
            url: r.url.unwrap_or_default(),
            status: r.status_raw,
            is_terminal,
            due_date: r.due_date,
            due_days,
            started,
            carryover,
            worked_recently,
            score,
            origin: origin.to_string(),
            reason,
            description: excerpt(r.description_text.as_deref()),
            epic: trimmed(r.epic_title).or_else(|| trimmed(r.parent_key)),
            priority: trimmed(r.priority),
            issue_type: trimmed(r.issue_type).unwrap_or_default(),
            story_points: trimmed(r.story_points),
        });
    }

    // Highest score first; stable tiebreak on key so order is deterministic.
    // NOTE: Rust `str::cmp` is byte-ordinal; JS `localeCompare` is locale-aware.
    // They agree for ASCII task keys (PROJ-123) — the only keys in practice.
    items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.key.cmp(&b.key)));
    Ok(items)
}

#[derive(FromRow)]
struct PlanJoinRow {
    task_key: String,
    position: i64,
    origin: String,
    task_snapshot: Option<String>,
    on_board: i64,
    title: Option<String>,
    provider: Option<String>,
    url: Option<String>,
    status_raw: String,
    is_terminal: i64,
    due_date: Option<String>,
    description_text: Option<String>,
    epic_title: Option<String>,
    parent_key: Option<String>,
    priority: Option<String>,
    issue_type: Option<String>,
    story_points: Option<String>,
}

/// A ticket's board fields captured onto the `daily_plan` row at write time
/// (the 044 `task_snapshot` JSON blob). Field names mirror the `pm_tasks`
/// columns the snapshot SELECT projects, so a JSON round-trip is lossless.
#[derive(Debug, Clone, Serialize, Deserialize, Default, FromRow)]
struct TaskSnapshot {
    title: Option<String>,
    provider: Option<String>,
    url: Option<String>,
    status_raw: Option<String>,
    is_terminal: Option<i64>,
    due_date: Option<String>,
    description_text: Option<String>,
    epic_title: Option<String>,
    parent_key: Option<String>,
    priority: Option<String>,
    issue_type: Option<String>,
    story_points: Option<String>,
}

fn parse_snapshot(s: Option<&str>) -> Option<TaskSnapshot> {
    serde_json::from_str(s?).ok()
}

/// Committed plan rows joined with their LIVE `pm_tasks` state; an off-board
/// planned ticket falls back to its captured snapshot and is treated as
/// completed (it left the active board, almost always by being Done). Mirrors
/// `loadPlan`. Guards the 041-but-not-044 case (no `task_snapshot` column).
async fn load_plan(
    pool: &SqlitePool,
    date: &str,
    today: NaiveDate,
) -> anyhow::Result<Vec<PlanItem>> {
    if !table_exists(pool, "daily_plan").await {
        return Ok(Vec::new());
    }
    // 041 created daily_plan; 044 added task_snapshot. A DB stuck between them
    // lacks the column — select a NULL literal instead of erroring on it.
    let has_snapshot = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM pragma_table_info('daily_plan') WHERE name='task_snapshot'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    let sql = format!(
        r#"SELECT p.task_key, p.position, p.origin,
                  {},
                  (t.task_key IS NOT NULL) AS on_board,
                  t.title, t.provider, t.url,
                  COALESCE(t.status_raw,'') AS status_raw,
                  COALESCE(t.is_terminal,0) AS is_terminal,
                  t.due_date, t.description_text, t.epic_title, t.parent_key,
                  t.priority, t.issue_type, t.story_points
           FROM daily_plan p
           LEFT JOIN pm_tasks t ON t.task_key = p.task_key
           WHERE p.plan_date = ?
           ORDER BY p.position ASC, p.task_key ASC"#,
        if has_snapshot {
            "p.task_snapshot"
        } else {
            "NULL AS task_snapshot"
        },
    );
    let rows: Vec<PlanJoinRow> = sqlx::query_as::<_, PlanJoinRow>(&sql)
        .bind(date)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("plan.read.daily_plan.join"))
        .await?;
    tracing::debug!(rows = rows.len(), "plan.read.daily_plan.join");

    Ok(rows
        .into_iter()
        .map(|r| {
            let on_board = r.on_board != 0;
            // Live board row wins; otherwise fall back to the captured snapshot.
            let snap = if on_board {
                None
            } else {
                parse_snapshot(r.task_snapshot.as_deref())
            };
            let s = snap.as_ref();
            // pick: live column when on-board, else the snapshot's field.
            let pick = |live: Option<String>, snap_val: Option<String>| {
                if on_board {
                    live
                } else {
                    snap_val
                }
            };
            let due_date = pick(r.due_date, s.and_then(|x| x.due_date.clone()));
            let status = if on_board {
                r.status_raw
            } else {
                s.and_then(|x| x.status_raw.clone()).unwrap_or_default()
            };
            PlanItem {
                title: pick(r.title, s.and_then(|x| x.title.clone()))
                    .unwrap_or_else(|| r.task_key.clone()),
                provider: pick(r.provider, s.and_then(|x| x.provider.clone()))
                    .unwrap_or_else(|| "jira".to_string()),
                url: pick(r.url, s.and_then(|x| x.url.clone())).unwrap_or_default(),
                status,
                // Off the active board ⇒ completed for the day's plan; on board ⇒ live flag.
                is_terminal: if on_board { r.is_terminal != 0 } else { true },
                due_days: crate::date::due_days_from(due_date.as_deref(), today),
                due_date,
                description: excerpt(
                    pick(
                        r.description_text,
                        s.and_then(|x| x.description_text.clone()),
                    )
                    .as_deref(),
                ),
                epic: trimmed(pick(r.epic_title, s.and_then(|x| x.epic_title.clone())))
                    .or_else(|| trimmed(pick(r.parent_key, s.and_then(|x| x.parent_key.clone())))),
                priority: trimmed(pick(r.priority, s.and_then(|x| x.priority.clone()))),
                issue_type: trimmed(pick(r.issue_type, s.and_then(|x| x.issue_type.clone())))
                    .unwrap_or_default(),
                story_points: trimmed(pick(r.story_points, s.and_then(|x| x.story_points.clone()))),
                task_key: r.task_key,
                position: r.position,
                origin: r.origin,
            }
        })
        .collect())
}

/// Full plan payload for a day. `available` is supplied by the caller (the
/// command scores the board once and reuses it for both the read and the POST
/// response), so this never re-scores. Mirrors `buildPlanResponse`.
#[tracing::instrument(skip(pool, available), fields(date = %date))]
pub async fn build_plan_response(
    pool: &SqlitePool,
    date: &str,
    today: NaiveDate,
    available: Vec<AvailableTask>,
) -> anyhow::Result<PlanResponse> {
    let has_table = table_exists(pool, "daily_plan").await;
    let meta = load_meta(pool, date).await?;
    let plan = load_plan(pool, date, today).await?;

    let committed: HashSet<String> = plan.iter().map(|p| p.task_key.clone()).collect();
    let suggestions: Vec<AvailableTask> = available
        .iter()
        .filter(|a| !committed.contains(&a.key) && a.score > 0)
        .take(SUGGESTION_CAP)
        .cloned()
        .collect();

    tracing::info!(
        date,
        has_table,
        confirmed = meta.confirmed_at.is_some(),
        skipped = meta.skipped == 1,
        plan = plan.len(),
        suggestions = suggestions.len(),
        available = available.len(),
        "plan served"
    );

    Ok(PlanResponse {
        date: date.to_string(),
        has_table,
        confirmed: meta.confirmed_at.is_some(),
        skipped: meta.skipped == 1,
        plan,
        suggestions,
        available,
    })
}

// ── Writes (the six POST actions) ─────────────────────────────────────────────

/// A write was rejected before touching the DB — surfaced to the command as a
/// human-readable error (the browser path still gets the route's HTTP status).
#[derive(Debug)]
pub enum PlanWriteError {
    /// `task_keys` array required (confirm/set) — a missing/malformed body must
    /// not silently wipe the day.
    TaskKeysRequired,
    /// `task_key` string required (add/remove).
    TaskKeyRequired,
    /// The `daily_plan` table doesn't exist yet (pre-migration-041 DB).
    StorageNotReady,
    /// An unknown action string.
    UnknownAction(String),
}

impl std::fmt::Display for PlanWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TaskKeysRequired => write!(f, "task_keys array required"),
            Self::TaskKeyRequired => write!(f, "task_key required"),
            Self::StorageNotReady => {
                write!(f, "plan storage not ready — restart the meridian daemon")
            }
            Self::UnknownAction(a) => write!(f, "unknown action: {a}"),
        }
    }
}
impl std::error::Error for PlanWriteError {}

/// JSON snapshot of a ticket's board fields, or `None` when it isn't on the
/// board (so an `add`/`confirm` of an off-board key keeps any earlier snapshot —
/// see the `COALESCE(excluded.task_snapshot, …)` in the UPSERTs). Mirrors the
/// route's `snapshotFor`; the projected columns + JSON keys match `TaskSnapshot`.
async fn snapshot_for<'e, E>(executor: E, key: &str) -> anyhow::Result<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    // Run on the CALLER's executor (the open transaction inside `replace_plan`,
    // the pool in `add`) — never re-acquire from the pool mid-transaction, which
    // would deadlock a single-connection pool.
    let row = sqlx::query_as::<_, TaskSnapshot>(
        r#"SELECT title, provider, url, COALESCE(status_raw,'') AS status_raw,
                  COALESCE(is_terminal,0) AS is_terminal, due_date,
                  description_text, epic_title, parent_key, priority, issue_type, story_points
           FROM pm_tasks WHERE task_key = ?"#,
    )
    .bind(key)
    .fetch_optional(executor)
    .await?;
    Ok(match row {
        Some(snap) => Some(serde_json::to_string(&snap)?),
        None => None,
    })
}

/// Apply a write action, then return the freshly-scored plan response (reusing
/// the already-scored `available`). Mirrors the route's POST switch + final
/// `buildPlanResponse`. `now` is seconds-precision UTC (matches `nowIso`).
#[tracing::instrument(skip(pool, body, available), fields(action = %body.action, date = %date))]
#[allow(clippy::too_many_arguments)]
pub async fn apply_plan_action(
    pool: &SqlitePool,
    body: &PlanBody,
    date: &str,
    today: NaiveDate,
    now: &str,
    available: Vec<AvailableTask>,
) -> anyhow::Result<PlanResponse> {
    // Schema owned by Rust migration 041; fail clearly if the daemon hasn't applied it.
    if !table_exists(pool, "daily_plan").await {
        return Err(PlanWriteError::StorageNotReady.into());
    }

    // origin lookup uses the scored board so a committed task keeps a meaningful
    // origin label ("carried over" / …) instead of bare "manual".
    let origin_map: HashMap<&str, &str> = available
        .iter()
        .map(|a| (a.key.as_str(), a.origin.as_str()))
        .collect();
    let origin_for = |key: &str| origin_map.get(key).copied().unwrap_or("manual").to_string();

    match body.action.as_str() {
        "confirm" => {
            // task_keys MUST be present — an explicit [] clears the plan, but a
            // missing/malformed body must error, not wipe the day silently.
            let keys = body
                .task_keys
                .as_ref()
                .ok_or(PlanWriteError::TaskKeysRequired)?;
            let mut tx = pool.begin().await?;
            replace_plan(&mut tx, date, keys, &origin_for, now).await?;
            upsert_meta(&mut tx, date, Some(now), 0, now).await?;
            tx.commit().await?;
        }
        "set" => {
            // Live edit while confirmed — replace rows, leave meta untouched.
            let keys = body
                .task_keys
                .as_ref()
                .ok_or(PlanWriteError::TaskKeysRequired)?;
            let mut tx = pool.begin().await?;
            replace_plan(&mut tx, date, keys, &origin_for, now).await?;
            tx.commit().await?;
        }
        "add" => {
            let key = body
                .task_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or(PlanWriteError::TaskKeyRequired)?;
            let max: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(position), -1) FROM daily_plan WHERE plan_date = ?",
            )
            .bind(date)
            .fetch_one(pool)
            .await?;
            let snapshot = snapshot_for(pool, key).await?;
            sqlx::query(
                r#"INSERT INTO daily_plan (plan_date, task_key, position, origin, task_snapshot, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?)
                   ON CONFLICT(plan_date, task_key) DO NOTHING"#,
            )
            .bind(date)
            .bind(key)
            .bind(max + 1)
            .bind(origin_for(key))
            .bind(snapshot)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await?;
        }
        "remove" => {
            let key = body
                .task_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or(PlanWriteError::TaskKeyRequired)?;
            sqlx::query("DELETE FROM daily_plan WHERE plan_date = ? AND task_key = ?")
                .bind(date)
                .bind(key)
                .execute(pool)
                .await?;
        }
        "reorder" => {
            // Absent/malformed task_keys → empty (no-op), matching the route's filter.
            let keys = body.task_keys.clone().unwrap_or_default();
            let mut tx = pool.begin().await?;
            for (i, key) in keys.iter().enumerate() {
                sqlx::query("UPDATE daily_plan SET position = ?, updated_at = ? WHERE plan_date = ? AND task_key = ?")
                    .bind(i as i64)
                    .bind(now)
                    .bind(date)
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        "skip" => {
            let mut tx = pool.begin().await?;
            upsert_meta(&mut tx, date, Some(now), 1, now).await?;
            tx.commit().await?;
        }
        "reopen" => {
            let mut tx = pool.begin().await?;
            upsert_meta(&mut tx, date, None, 0, now).await?;
            tx.commit().await?;
        }
        other => return Err(PlanWriteError::UnknownAction(other.to_string()).into()),
    }

    // Return the fresh state (plan writes don't change pm_tasks scoring → reuse).
    build_plan_response(pool, date, today, available).await
}

/// Replace the day's committed set with `ordered` (idempotent UPSERT + prune of
/// dropped keys), within the caller's transaction. Mirrors the route's
/// `replacePlan`. The snapshot read runs on the same transaction connection.
async fn replace_plan(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    date: &str,
    ordered: &[String],
    origin_for: &impl Fn(&str) -> String,
    now: &str,
) -> anyhow::Result<()> {
    let keep: HashSet<&str> = ordered.iter().map(|s| s.as_str()).collect();
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT task_key FROM daily_plan WHERE plan_date = ?")
            .bind(date)
            .fetch_all(&mut **tx)
            .await?;
    for key in existing {
        if !keep.contains(key.as_str()) {
            sqlx::query("DELETE FROM daily_plan WHERE plan_date = ? AND task_key = ?")
                .bind(date)
                .bind(&key)
                .execute(&mut **tx)
                .await?;
        }
    }
    for (i, key) in ordered.iter().enumerate() {
        let snapshot = snapshot_for(&mut **tx, key).await?;
        sqlx::query(
            r#"INSERT INTO daily_plan (plan_date, task_key, position, origin, task_snapshot, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(plan_date, task_key) DO UPDATE SET
                 position      = excluded.position,
                 updated_at    = excluded.updated_at,
                 task_snapshot = COALESCE(excluded.task_snapshot, daily_plan.task_snapshot)"#,
        )
        .bind(date)
        .bind(key)
        .bind(i as i64)
        .bind(origin_for(key))
        .bind(snapshot)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Upsert the day's meta row (confirmed/skipped). Mirrors `upsertMeta`.
async fn upsert_meta(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    date: &str,
    confirmed_at: Option<&str>,
    skipped: i64,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO daily_plan_meta (plan_date, confirmed_at, skipped, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?)
           ON CONFLICT(plan_date) DO UPDATE SET
             confirmed_at = excluded.confirmed_at,
             skipped      = excluded.skipped,
             updated_at   = excluded.updated_at"#,
    )
    .bind(date)
    .bind(confirmed_at)
    .bind(skipped)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_component_buckets() {
        assert_eq!(due_component(None), 0);
        assert_eq!(due_component(Some(-1)), 400);
        assert_eq!(due_component(Some(0)), 350);
        assert_eq!(due_component(Some(2)), 350);
        assert_eq!(due_component(Some(5)), 250);
        assert_eq!(due_component(Some(10)), 120);
        assert_eq!(due_component(Some(20)), 40);
        assert_eq!(due_component(Some(60)), 0);
    }

    #[test]
    fn due_reason_labels() {
        assert_eq!(due_reason(Some(-3)).as_deref(), Some("Overdue 3d"));
        assert_eq!(due_reason(Some(0)).as_deref(), Some("Due today"));
        assert_eq!(due_reason(Some(1)).as_deref(), Some("Due tomorrow"));
        assert_eq!(due_reason(Some(5)).as_deref(), Some("Due in 5d"));
        assert_eq!(due_reason(Some(30)), None);
        assert_eq!(due_reason(None), None);
    }

    #[test]
    fn excerpt_collapses_and_ellipsises() {
        assert_eq!(excerpt(Some("  a   b\tc  ")), "a b c");
        assert_eq!(excerpt(None), "");
        let long = "x".repeat(200);
        let out = excerpt(Some(&long));
        assert_eq!(out.chars().count(), EXCERPT_LEN); // 129 chars + ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn looks_started_matches_hints() {
        assert!(looks_started("In Progress"));
        assert!(looks_started("In Review"));
        assert!(!looks_started("To Do"));
        assert!(!looks_started("Backlog"));
    }
}
