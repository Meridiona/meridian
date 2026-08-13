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

/// The most tasks a day's plan may hold.
///
/// A product limit, not a technical one. A plan is a statement of intent for one
/// day, and a day that claims dozens of tasks isn't a plan — it's a backlog, which
/// makes the plan useless as the prior the worklog matcher now leans on
/// ([`load_plan_candidates`]). Most focused days land on 1-3; the existing
/// advisory nag in `PlanTodayColumn.tsx` fires at 5. This is the hard stop.
pub const MAX_PLAN_TASKS: usize = 20;

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

/// Whether `table` has `column` — used to stay graceful on a DB that predates a
/// migration (here, `pm_tasks.deleted_at` from 075). Same swallow-on-error shape
/// as [`table_exists`]: a probe failure reads as "absent", so the caller falls
/// back to the pre-migration query instead of erroring.
async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1 FROM pragma_table_info(?) WHERE name=?")
        .bind(table)
        .bind(column)
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

/// True when `date`'s plan is already handled — confirmed or explicitly
/// skipped in `daily_plan_meta`. The tray's daily auto-open gate: an
/// already-planned day must not pop the planner again. Missing table or no
/// row for `date` (and any read error) → `false`.
///
/// # Who calls this
/// The tray poll loop (`tray/src-tauri/src/poll/plan_auto_open.rs`).
#[tracing::instrument(skip(pool))]
pub async fn plan_handled(pool: &SqlitePool, date: &str) -> bool {
    load_meta(pool, date)
        .await
        .map(|m| m.confirmed_at.is_some() || m.skipped == 1)
        .unwrap_or(false)
}

/// A day's plan as the *review* side needs it: the committed rows and whether
/// the dev actually stood behind them.
///
/// Distinct from [`PlanResponse`] on purpose. That one is the planner screen's
/// payload and carries `suggestions` + `available`, which means scoring the whole
/// board — real work that the daily summary has no use for. This is the read for
/// anyone asking "what did they say they would do today?".
#[derive(Debug, Clone, Serialize)]
pub struct DayPlan {
    /// The committed rows, in plan order. Empty when nothing was committed.
    pub items: Vec<PlanItem>,
    /// `daily_plan_meta.confirmed_at IS NOT NULL`. Note this is ALSO true after a
    /// skip — see [`plan_handled`] — which is why `skipped` must be checked too.
    pub confirmed: bool,
    /// The ritual was dismissed rather than answered.
    pub skipped: bool,
}

impl DayPlan {
    /// The canonical "they planned this day" test, matching what the planner UI
    /// (`PlanView.tsx`) and the focus checklist (`OverviewPanel.tsx`) both use:
    /// confirmed, not skipped, and actually holding something. A confirmed but
    /// empty plan is a day with no plan, not a day that planned nothing.
    pub fn is_planned(&self) -> bool {
        self.confirmed && !self.skipped && !self.items.is_empty()
    }
}

/// Read one day's committed plan without scoring the board.
///
/// # Who calls this
/// [`crate::day_evidence::collect`] — the daily summary's planned-vs-actual side.
#[tracing::instrument(skip(pool))]
pub async fn plan_for_day(
    pool: &SqlitePool,
    date: &str,
    today: NaiveDate,
) -> anyhow::Result<DayPlan> {
    let meta = load_meta(pool, date).await?;
    let items = load_plan(pool, date, today).await?;
    tracing::debug!(rows = items.len(), "plan.read.day_plan");
    Ok(DayPlan {
        items,
        confirmed: meta.confirmed_at.is_some(),
        skipped: meta.skipped == 1,
    })
}

/// Whether `date` may have yesterday's unfinished work written into it automatically.
///
/// Carry-over is a convenience, and the cost of getting it wrong is putting tasks in
/// someone's plan that they took out on purpose — so it fires only into a day nobody has
/// expressed any intention about yet. Three guards, each closing a different way to be
/// wrong:
///
/// - **only today.** A past date is a record of what was planned then. Writing into it
///   would rewrite history, and the timeline renders past days from exactly these rows.
/// - **only an untouched day.** The presence of a `daily_plan_meta` row is the marker,
///   and it is what makes this idempotent in the sense that matters: not "runs once" but
///   "never contradicts the user". Confirming, skipping, or clearing the plan all leave a
///   row, so a user who deletes a carried-over task and closes the modal does not find it
///   back the next time anything reads the plan. An empty plan with a meta row is a
///   DECISION; an empty plan with no meta row is a day not started.
/// - **only with a prior planned day to carry from**, which `carry_over_unfinished`
///   establishes by finding candidates at all.
async fn carry_over_is_due(
    pool: &SqlitePool,
    date: &str,
    today: NaiveDate,
) -> anyhow::Result<bool> {
    if date != today.format("%Y-%m-%d").to_string().as_str() {
        return Ok(false);
    }
    if !table_exists(pool, "daily_plan_meta").await {
        // No meta table means no way to tell an untouched day from a cleared one, and
        // guessing wrong here re-adds work the user deliberately removed.
        return Ok(false);
    }
    let touched: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM daily_plan_meta WHERE plan_date = ?")
            .bind(date)
            .fetch_optional(pool)
            .instrument(tracing::debug_span!("plan.read.daily_plan_meta.touched"))
            .await?;
    Ok(touched.is_none())
}

/// Write the carried-over tasks from `available` into `date`'s plan, and mark the day
/// confirmed. Returns how many were written.
///
/// Marking it CONFIRMED is the part worth stating plainly, because it is what makes the
/// feature visible at all. `confirmed` is what the timeline's "Today's focus" reads to
/// decide it has a plan to show (`OverviewPanel`), so carrying tasks over without it would
/// write rows that only the plan modal ever renders — reproducing, exactly, the split this
/// fixes: a modal listing today's tasks behind a panel insisting there are none. A plan
/// the app composed on the user's behalf is still a plan; they can empty it, and the meta
/// row that leaves behind stops it coming back.
async fn carry_over_unfinished(
    pool: &SqlitePool,
    date: &str,
    available: &[AvailableTask],
) -> anyhow::Result<usize> {
    // `carryover` already means "was on the last planned day AND is still open" - the flag
    // is computed against `available`, which holds only non-terminal tasks. So a task
    // finished yesterday is absent here rather than filtered out again.
    let keys: Vec<String> = available
        .iter()
        .filter(|a| a.carryover)
        .take(MAX_PLAN_TASKS)
        .map(|a| a.key.clone())
        .collect();
    if keys.is_empty() {
        return Ok(0);
    }

    let now = chrono::Local::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    // Origin "carryover" rather than "manual": the card's label ("Carried over") is read
    // from this, so a task the user did not put there says where it came from.
    replace_plan(&mut tx, date, &keys, &|_| "carryover".to_string(), &now).await?;
    upsert_meta(&mut tx, date, Some(&now), 0, &now).await?;
    tx.commit().await?;
    Ok(keys.len())
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
    // `t.deleted_at IS NULL` (migration 075) — a task the user deleted from the
    // composer must not be offered back as a suggestion. Gated on the column
    // existing so a DB not yet migrated to 075 still reads (graceful missing
    // schema, like the curation join above).
    let has_deleted_at = column_exists(pool, "pm_tasks", "deleted_at").await;
    let sql = format!(
        r#"SELECT t.task_key, t.title, t.provider, t.url,
                  COALESCE(t.status_raw,'') AS status_raw,
                  COALESCE(t.is_terminal,0) AS is_terminal,
                  t.due_date, t.updated_at,
                  t.description_text, t.epic_title, t.parent_key,
                  t.priority, t.issue_type, t.story_points,
                  {} AS decision
           FROM pm_tasks t
           {}
           {}"#,
        if has_curation { "c.decision" } else { "NULL" },
        if has_curation {
            "LEFT JOIN pm_task_curation c ON c.task_key = t.task_key"
        } else {
            ""
        },
        if has_deleted_at {
            "WHERE t.deleted_at IS NULL"
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
    /// Set (migration 075) when the user deleted this personal task from the
    /// composer. Unlike `on_board = false` (the ticket's `pm_tasks` row is simply
    /// gone, e.g. pruned off a tracker's board — shown as completed via its
    /// snapshot), a deleted task must vanish outright: filtered out in
    /// [`load_plan`] / [`load_plan_candidates`] before [`resolve_row`] ever sees it.
    deleted_at: Option<String>,
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

/// One planned task resolved to its best-known board fields — the shape the
/// worklog matcher compares a day's work against.
///
/// Deliberately NOT [`PlanItem`]. `PlanItem` is a card, so its `description` is
/// [`excerpt`]-ed to ~130 chars for display; handing that to the matcher would
/// silently shrink its prompt context to under half of what `render_doc` allows,
/// and nothing anywhere would say so. This carries the FULL description, and the
/// card shape is derived from it — so the two can't drift.
#[derive(Debug, Clone)]
pub struct PlanCandidate {
    pub task_key: String,
    pub position: i64,
    pub origin: String,
    pub provider: String,
    pub url: String,
    pub title: String,
    pub issue_type: String,
    pub epic: Option<String>,
    /// Full, untruncated — see the type doc.
    pub description: String,
    pub status: String,
    pub is_terminal: bool,
    pub due_date: Option<String>,
    pub priority: Option<String>,
    pub story_points: Option<String>,
}

/// Resolve one joined row to its best-known fields: the live `pm_tasks` columns
/// when the ticket is still on the board, else the snapshot captured onto the
/// plan row at write time (044).
///
/// `on_board` here means only "a `pm_tasks` row still exists for this key" — it
/// is NOT [`crate::board::is_on_board`], which asks whether a ticket is open and
/// not cleanup-excluded. Same words, different question; don't substitute one for
/// the other.
fn resolve_row(r: PlanJoinRow) -> PlanCandidate {
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
    PlanCandidate {
        title: pick(r.title, s.and_then(|x| x.title.clone())).unwrap_or_else(|| r.task_key.clone()),
        provider: pick(r.provider, s.and_then(|x| x.provider.clone()))
            .unwrap_or_else(|| "jira".to_string()),
        url: pick(r.url, s.and_then(|x| x.url.clone())).unwrap_or_default(),
        status: if on_board {
            r.status_raw
        } else {
            s.and_then(|x| x.status_raw.clone()).unwrap_or_default()
        },
        // Off the active board ⇒ completed for the day's plan; on board ⇒ live flag.
        is_terminal: if on_board { r.is_terminal != 0 } else { true },
        due_date: pick(r.due_date, s.and_then(|x| x.due_date.clone())),
        description: pick(
            r.description_text,
            s.and_then(|x| x.description_text.clone()),
        )
        .unwrap_or_default(),
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
}

/// The day's committed plan, resolved. This is the worklog matcher's candidate
/// set: what the dev actually said they'd work on today, not the whole board.
///
/// Two things it deliberately does NOT filter:
/// - **Terminal tasks stay.** Checking a task off the plan closes the real ticket
///   (`OverviewPanel.tsx`'s `toggleDone` → `apply_ticket_fix`), which drops it off
///   the board — so filtering terminal here would delete exactly the task the dev
///   just finished from the set used to log the work they finished it with. The
///   `task_snapshot` fallback is what keeps its text available afterwards.
/// - **Personal (`'local'`) tasks stay.** They belong to the plan. It's the
///   *matcher* that can't use them (nothing to post a comment to), so that filter
///   lives at its call site, not here.
///
/// # Who calls this
/// `meridian::pm_worklog::generate::fetch_plan_candidates` — the matcher.
#[tracing::instrument(skip(pool))]
pub async fn load_plan_candidates(
    pool: &SqlitePool,
    date: &str,
) -> anyhow::Result<Vec<PlanCandidate>> {
    if !table_exists(pool, "daily_plan").await {
        return Ok(Vec::new());
    }
    Ok(plan_join_rows(pool, date)
        .await?
        .into_iter()
        .filter(|r| r.deleted_at.is_none())
        .map(resolve_row)
        .collect())
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
    // The card shape is the candidate shape, excerpted for display + dated.
    Ok(plan_join_rows(pool, date)
        .await?
        .into_iter()
        // A deleted personal task must vanish outright, unlike a merely off-board
        // one (`on_board = false`, still shown via its snapshot as completed).
        .filter(|r| r.deleted_at.is_none())
        .map(resolve_row)
        .map(|c| PlanItem {
            due_days: crate::date::due_days_from(c.due_date.as_deref(), today),
            description: excerpt(Some(&c.description)),
            task_key: c.task_key,
            position: c.position,
            origin: c.origin,
            title: c.title,
            provider: c.provider,
            url: c.url,
            status: c.status,
            is_terminal: c.is_terminal,
            due_date: c.due_date,
            epic: c.epic,
            priority: c.priority,
            issue_type: c.issue_type,
            story_points: c.story_points,
        })
        .collect())
}

/// The raw `daily_plan LEFT JOIN pm_tasks` for one day. The single place this
/// join is written — [`load_plan`] (cards) and [`load_plan_candidates`] (the
/// matcher) both resolve from it via [`resolve_row`].
async fn plan_join_rows(pool: &SqlitePool, date: &str) -> anyhow::Result<Vec<PlanJoinRow>> {
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

    // 075 added pm_tasks.deleted_at; a DB that predates it lacks the column, so
    // select a NULL literal instead of erroring on `t.deleted_at` (the join's
    // consumers treat a NULL as "not deleted" — see the `deleted_at.is_none()`
    // filter in load_plan).
    let has_deleted_at = column_exists(pool, "pm_tasks", "deleted_at").await;

    let sql = format!(
        r#"SELECT p.task_key, p.position, p.origin,
                  {},
                  (t.task_key IS NOT NULL) AS on_board,
                  t.title, t.provider, t.url,
                  COALESCE(t.status_raw,'') AS status_raw,
                  COALESCE(t.is_terminal,0) AS is_terminal,
                  t.due_date, t.description_text, t.epic_title, t.parent_key,
                  t.priority, t.issue_type, t.story_points, {}
           FROM daily_plan p
           LEFT JOIN pm_tasks t ON t.task_key = p.task_key
           WHERE p.plan_date = ?
           ORDER BY p.position ASC, p.task_key ASC"#,
        if has_snapshot {
            "p.task_snapshot"
        } else {
            "NULL AS task_snapshot"
        },
        if has_deleted_at {
            "t.deleted_at"
        } else {
            "NULL AS deleted_at"
        },
    );
    let rows: Vec<PlanJoinRow> = sqlx::query_as::<_, PlanJoinRow>(&sql)
        .bind(date)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("plan.read.daily_plan.join"))
        .await?;
    tracing::debug!(rows = rows.len(), "plan.read.daily_plan.join");
    Ok(rows)
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
    let mut meta = load_meta(pool, date).await?;
    let mut plan = load_plan(pool, date, today).await?;

    // Yesterday's unfinished work becomes today's plan, without being asked.
    if has_table && plan.is_empty() && carry_over_is_due(pool, date, today).await? {
        match carry_over_unfinished(pool, date, &available).await {
            Ok(0) => {}
            Ok(n) => {
                // Re-read rather than synthesising the rows we just wrote: `load_plan`
                // resolves each task's CURRENT status/terminal state, which is the whole
                // reason a carried-over task can be shown as already done.
                meta = load_meta(pool, date).await?;
                plan = load_plan(pool, date, today).await?;
                tracing::info!(
                    date,
                    carried = n,
                    "plan: carried yesterday's unfinished work over"
                );
            }
            // Never fatal. A day with no plan is a worse outcome than a day the user
            // plans by hand, so a failure here degrades to the empty state it replaced.
            Err(e) => tracing::warn!(date, error = %e, "plan: carry-over failed"),
        }
    }

    let committed: HashSet<String> = plan.iter().map(|p| p.task_key.clone()).collect();
    // Carryover (yesterday's confirmed, still-not-Done) tasks are never subject to
    // the suggestion cap — a dev who leaves 7 tasks unfinished must see all 7
    // pre-filled tomorrow, not just however many fit in SUGGESTION_CAP. Only the
    // non-carryover "you might also want this" suggestions are capped, and only
    // to fill out whatever room is left under SUGGESTION_CAP.
    let uncommitted: Vec<&AvailableTask> = available
        .iter()
        .filter(|a| !committed.contains(&a.key) && a.score > 0)
        .collect();
    let (carryover, rest): (Vec<&AvailableTask>, Vec<&AvailableTask>) =
        uncommitted.into_iter().partition(|a| a.carryover);
    let remaining_slots = SUGGESTION_CAP.saturating_sub(carryover.len());
    let suggestions: Vec<AvailableTask> = carryover
        .into_iter()
        .chain(rest.into_iter().take(remaining_slots))
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
    /// The write would push the day's plan past [`MAX_PLAN_TASKS`].
    TooManyTasks(usize),
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
            // App text - surfaces verbatim in the planner. Plain hyphens only.
            Self::TooManyTasks(n) => write!(
                f,
                "You can plan up to {MAX_PLAN_TASKS} tasks for a day - this would make {n}. \
                 Remove one before adding another. Most focused days land on 1-3."
            ),
            Self::UnknownAction(a) => write!(f, "unknown action: {a}"),
        }
    }
}
impl std::error::Error for PlanWriteError {}

/// Reject a whole-list write that would exceed [`MAX_PLAN_TASKS`].
///
/// Counts DISTINCT keys: `replace_plan` UPSERTs, so a payload repeating a key
/// writes one row, and counting the raw array would refuse a plan that is
/// actually legal.
///
/// This is the live path — the planner never sends `add`/`remove`, it edits its
/// list locally and persists the whole thing as `set`/`confirm`
/// (`PlanView.tsx`). The UI stops the user at ten first; this is the guard that
/// actually holds.
fn check_plan_size(keys: &[String]) -> Result<(), PlanWriteError> {
    let distinct: HashSet<&String> = keys.iter().collect();
    if distinct.len() > MAX_PLAN_TASKS {
        return Err(PlanWriteError::TooManyTasks(distinct.len()));
    }
    Ok(())
}

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
            check_plan_size(keys)?;
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
            check_plan_size(keys)?;
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
            let snapshot = snapshot_for(pool, key).await?;
            // Cap check, position (MAX+1) and INSERT are ONE atomic statement, so two
            // concurrent adds can't both read count=9 and both land (a TOCTOU that
            // breached the cap and duplicated positions — the sibling arms avoid it by
            // being pure writes; this one decides on a COUNT, so it must decide and write
            // under the same write lock). SQLite serialises writers, so the aggregate the
            // second add sees already includes the first. `HAVING COUNT(*) < cap` gates
            // the row on the current size; the aggregate SELECT over zero rows still
            // yields one row (NULL max, 0 count), so an empty plan inserts at position 0.
            // `ON CONFLICT DO NOTHING` keeps re-adding an existing key a no-op even at the
            // cap (HAVING may exclude it, but it's already present, so nothing is lost).
            let res = sqlx::query(
                r#"INSERT INTO daily_plan (plan_date, task_key, position, origin, task_snapshot, created_at, updated_at)
                   SELECT ?, ?, COALESCE(MAX(position), -1) + 1, ?, ?, ?, ?
                     FROM daily_plan
                    WHERE plan_date = ?
                   HAVING COUNT(*) < ?
                   ON CONFLICT(plan_date, task_key) DO NOTHING"#,
            )
            .bind(date)
            .bind(key)
            .bind(origin_for(key))
            .bind(snapshot)
            .bind(now)
            .bind(now)
            .bind(date)
            .bind(MAX_PLAN_TASKS as i64)
            .execute(pool)
            .await?;

            // No row inserted means EITHER the key was already in the plan (a no-op by
            // design, even at the cap) OR the cap blocked a NEW key. Only the latter is an
            // error, so disambiguate with an existence check — never insert past the cap.
            if res.rows_affected() == 0 {
                let already: bool = sqlx::query_scalar::<_, i64>(
                    "SELECT 1 FROM daily_plan WHERE plan_date = ? AND task_key = ?",
                )
                .bind(date)
                .bind(key)
                .fetch_optional(pool)
                .await?
                .is_some();
                if !already {
                    return Err(PlanWriteError::TooManyTasks(MAX_PLAN_TASKS + 1).into());
                }
            }

            // AUTHORING A TASK FOR A DAY IS COMMITTING TO IT.
            //
            // Every other arm that puts rows in `daily_plan` also writes
            // `daily_plan_meta`; this one did not, and `confirmed_at` stayed
            // NULL. That matters because `confirmed` is what every reader gates
            // on — the dashboard's "Today's focus" renders
            // `plan.confirmed ? plan.plan : []` — so a task added this way was
            // written to the database correctly and then displayed nowhere.
            //
            // It hit hardest exactly where the product is trying hardest to make
            // a good impression:
            //   * the onboarding walkthrough, whose planner beat is driven
            //     entirely by the composer. It says "Saved" and "today's tasks
            //     are in", and then the dashboard it hands over to was empty.
            //   * every solo / no-tracker user, for whom the composer is the ONLY
            //     way to build a plan (there is no board to drag from), so their
            //     plan never appeared at all.
            // Dragging a board ticket across was unaffected, which is why this
            // survived: that path goes through "confirm".
            //
            // `confirmed_at` is only stamped when NULL, so re-adding to a day
            // that was committed hours ago does not move its timestamp. `skipped`
            // is cleared unconditionally: authoring a task for a day you had
            // skipped is an explicit change of mind, and leaving the flag set
            // would hide the task behind the same gate again.
            sqlx::query(
                r#"INSERT INTO daily_plan_meta (plan_date, confirmed_at, skipped, created_at, updated_at)
                   VALUES (?, ?, 0, ?, ?)
                   ON CONFLICT(plan_date) DO UPDATE SET
                     confirmed_at = COALESCE(daily_plan_meta.confirmed_at, excluded.confirmed_at),
                     skipped      = 0,
                     updated_at   = excluded.updated_at"#,
            )
            .bind(date)
            .bind(now)
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
