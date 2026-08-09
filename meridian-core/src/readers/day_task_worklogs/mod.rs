//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The read/write side of the generated-worklog ledger (`day_task_worklogs`,
//! migration 060) behind the "Generate worklog" action on a day-task card.
//!
//! # What this is
//! Each row is one AI-drafted status update for a day-task (T1/T2…): the tickets it
//! advanced (0..N [`WorklogTarget`]s, in [`targets`]) XOR a PROPOSE of a new one,
//! plus a rich status update to post as a comment on each. This module owns the DB
//! shape and the guarded transitions; the LLM call, provider dispatch, and the
//! actual posting live daemon-side in `src/pm_worklog/generate.rs` (which reuses
//! these types so the CLI/tray/UI all serialize the identical wire contract).
//!
//! # The two branches
//! A draft either names existing tickets ([`DayTaskWorklogDraft::targets`]) or
//! proposes one new one ([`DayTaskWorklogDraft::propose`]) — never both. A proposal
//! becomes an ordinary target once approve creates the ticket ([`mark_created`]),
//! which is why targets is the only thing the posting loop reads.
//!
//! # Who calls this
//! - [`get_day_task_worklog`] → the `worklog-generate-get` CLI → the tray
//!   `get_day_task_worklog` command → the day-task detail panel (shows an existing
//!   draft on reopen). Degrades to `None` on a pre-060 DB.
//! - [`upsert_draft`] / [`mark_approved`] / [`mark_created`] / [`mark_posted`] /
//!   [`mark_error`] → the daemon `generate`/`approve` flow.
//! - [`retarget_draft`] / [`dismiss_target`] → the tray
//!   `retarget_day_task_worklog` / `dismiss_worklog_target` commands.
//!
//! # Related
//! - [`targets`] — the per-ticket rows and their independent delivery state.
//! - [`crate::day_tasks`] — the day-task cards this drafts a worklog for; its
//!   `linked_ticket` is set once a draft is approved+posted.

pub mod targets;

use crate::SqlitePool;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::Instrument;

pub use targets::{TargetInput, WorklogTarget};

/// The proposed-new-ticket branch of a draft (mutually exclusive with having any
/// [`WorklogTarget`]s).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogPropose {
    pub issue_type: String,
    pub title: String,
    pub description: String,
}

/// One labelled group of bullet points inside an update. The model names the
/// `heading` to fit the actual work (a dev's "Decisions"/"Architecture", a
/// marketer's "Campaigns", an editor's "Edits"), so the update generalises across
/// roles instead of hardcoding developer-shaped fields.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorklogSection {
    pub heading: String,
    #[serde(default)]
    pub points: Vec<String>,
}

/// The rich status update — always present. `summary` leads with the outcome and
/// `status` is the current-state one-liner (both universal); `sections` is a
/// dynamic, work-fitting set of labelled bullet groups (0..N, possibly empty).
/// This is what gets posted as a comment (rendered to plain text at post time).
///
/// Deserialized via [`UpdateWire`] so rows written before `sections` existed (the
/// fixed `decisions`/`architecture` shape) still read back with their content —
/// see that type. Serialization always emits the current `sections` shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(from = "UpdateWire")]
pub struct GeneratedWorklogUpdate {
    pub summary: String,
    #[serde(default)]
    pub sections: Vec<WorklogSection>,
    #[serde(default)]
    pub status: String,
}

/// The on-disk wire shape of an update, spanning BOTH generations of the schema.
///
/// `update_json` rows written before the dynamic-`sections` change carry a fixed
/// developer-shaped `{summary, decisions[], architecture[], status}`. Serde ignores
/// unknown fields, so those rows would otherwise deserialize with `sections: []`
/// and silently lose their bullets — the draft still parsed, so even the
/// `update_summary` fallback in [`RawWorklog::into_draft`] never fired. Reading
/// through this struct lifts the legacy arrays into the equivalent named sections,
/// so a worklog posted under the old shape still renders in the detail panel.
///
/// `sections` wins when present — a new-shape row never consults the legacy keys.
#[derive(Deserialize)]
struct UpdateWire {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    sections: Vec<WorklogSection>,
    #[serde(default)]
    status: String,
    /// Legacy (pre-`sections`) fixed groups, in the order they were posted.
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    architecture: Vec<String>,
}

impl From<UpdateWire> for GeneratedWorklogUpdate {
    fn from(w: UpdateWire) -> Self {
        let mut sections = w.sections;
        if sections.is_empty() {
            // Same headings the old renderer posted, so the panel matches the
            // comment already sitting on the ticket.
            for (heading, points) in [("Decisions", w.decisions), ("Architecture", w.architecture)]
            {
                let points: Vec<String> = points
                    .into_iter()
                    .filter(|p| !p.trim().is_empty())
                    .collect();
                if !points.is_empty() {
                    sections.push(WorklogSection {
                        heading: heading.to_string(),
                        points,
                    });
                }
            }
        }
        Self {
            summary: w.summary,
            sections,
            status: w.status,
        }
    }
}

/// One generated-worklog draft, in the exact shape the CLI prints and the tray/UI
/// consume. `targets` and `propose` are mutually exclusive; `update` is always
/// present and is posted verbatim to every target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayTaskWorklogDraft {
    /// `drafted` | `approved` | `posted`. `posted` means EVERY target posted — a
    /// partial delivery stays `approved` and is retryable.
    pub state: String,
    /// The tracker a proposal would be created on. Targets carry their own, which
    /// is what the posting loop uses; this only decides where a proposal lands.
    pub provider: String,
    /// The tickets this update posts to, model-ordered. Empty when the draft is a
    /// proposal, or once the user has dismissed every match.
    #[serde(default)]
    pub targets: Vec<WorklogTarget>,
    pub propose: Option<GeneratedWorklogPropose>,
    pub update: GeneratedWorklogUpdate,
    pub reasoning: String,
    /// The ticket approve created from [`Self::propose`], persisted BEFORE its
    /// comment posts so a retry can never double-create.
    pub created_task_key: Option<String>,
    /// The last draft-level failure (a create, or the approve as a whole).
    /// Per-target failures live on the target.
    pub error: Option<String>,
    /// When this draft was last written (generated OR regenerated) — RFC-3339,
    /// UTC. Lets the panel tell the user "this was drafted at 2:14pm" so they know
    /// to hit Regenerate if they've kept working the task since. Every write path
    /// (`upsert_draft`) stamps this, including a regenerate that overwrites a
    /// still-`drafted` row — it is NOT "first generated at", it is "as of".
    pub updated_at: String,
    /// Measured minutes worked on this task SINCE the draft was written, or
    /// `None` when that cannot be known — a row drafted before migration 079
    /// carries no baseline, and guessing one would declare every pre-upgrade
    /// draft stale by its task's whole duration.
    ///
    /// Deterministic: `day_tasks.minutes` (recomputed by the hourly fold from
    /// measured spans) minus the same figure captured at draft time. No model
    /// is involved and no timestamp is compared — see migration 079 for why a
    /// timestamp cannot carry this.
    pub stale_minutes: Option<i64>,
    /// Whether that growth is worth telling the user about, by the ONE
    /// definition ([`WORKLOG_STALE_MINUTES`]) the notifier and the dashboard
    /// both read. Always false once the draft leaves `drafted`: an approved or
    /// posted update is a record of what went out, not a draft going out of
    /// date, and offering to overwrite it would be offering to lose it.
    pub stale: bool,
}

/// How much unaccounted work makes a draft worth rewriting.
///
/// Not zero, and the reason is the cost of being wrong in each direction. A
/// draft that trails its task by two minutes is still an accurate summary, and
/// flagging it would train the user to ignore the flag — the failure mode that
/// makes every later warning useless. A draft that trails by a quarter of an
/// hour is missing something the reader of that ticket would notice.
pub const WORKLOG_STALE_MINUTES: i64 = 15;

/// The raw parent row (migration 060, less the columns 062 moved to [`targets`]).
#[derive(FromRow)]
struct RawWorklog {
    provider: String,
    propose_issue_type: Option<String>,
    propose_title: Option<String>,
    propose_description: Option<String>,
    update_summary: String,
    update_json: String,
    reasoning: String,
    state: String,
    created_task_key: Option<String>,
    last_error: Option<String>,
    updated_at: String,
    /// The task's measured minutes when this draft was written (migration 079).
    /// NULL on a row drafted before that migration.
    drafted_minutes: Option<i64>,
    /// The task's measured minutes NOW, LEFT JOINed from `day_tasks`. NULL when
    /// the task has since been dismissed or the day rewritten without it.
    task_minutes: Option<i64>,
}

/// The columns every draft read selects, including the two staleness inputs.
/// One constant because the three call sites drifted apart once already, and a
/// missing column here is a silent `None` rather than a compile error.
const DRAFT_COLUMNS: &str = "w.provider, w.propose_issue_type, w.propose_title, \
     w.propose_description, w.update_summary, w.update_json, w.reasoning, w.state, \
     w.created_task_key, w.last_error, w.updated_at, w.drafted_minutes, \
     t.minutes AS task_minutes";

/// The FROM/JOIN every draft read shares. LEFT, not INNER: a draft whose task
/// has been dismissed must still render (the user may want to read or delete
/// it) — it simply has no baseline to compare against.
const DRAFT_FROM: &str = "FROM day_task_worklogs w \
     LEFT JOIN day_tasks t ON t.day_local = w.day_local AND t.task_id = w.task_id";

/// How far a draft has fallen behind its task, in minutes, clamped at zero.
///
/// The whole staleness feature is this one expression, and it was written out
/// twice — once in [`day_draft_states`], once in [`stale_drafts`] — for the same
/// reason [`DRAFT_COLUMNS`] exists: two copies of one rule drift, and this pair
/// drifts SILENTLY, because both sides keep returning plausible numbers. The
/// clamp is the part that matters: a task's minutes can genuinely go DOWN when
/// the fold moves a segment to another workstream, and "-35 minutes of new work"
/// is not a thing to show anyone.
///
/// `MAX(…, 0)` is SQLite's two-argument scalar max, which yields NULL if either
/// argument is NULL — that is deliberate and load-bearing: a pre-079 row (no
/// baseline) or a dismissed task reads as "cannot know", not as "perfectly
/// current". Both readers map that NULL to `None`.
const STALE_MINUTES_EXPR: &str = "MAX(t.minutes - w.drafted_minutes, 0) AS stale_minutes";

impl RawWorklog {
    fn into_draft(self, targets: Vec<WorklogTarget>) -> DayTaskWorklogDraft {
        // Prefer the full JSON object; fall back to the denormalised summary so a
        // partially-written / legacy row still renders.
        let update: GeneratedWorklogUpdate = serde_json::from_str(&self.update_json)
            .unwrap_or_else(|_| GeneratedWorklogUpdate {
                summary: self.update_summary.clone(),
                ..Default::default()
            });
        let propose = match (self.propose_title, self.propose_description) {
            (Some(title), Some(description)) => Some(GeneratedWorklogPropose {
                issue_type: self
                    .propose_issue_type
                    .unwrap_or_else(|| "Task".to_string()),
                title,
                description,
            }),
            _ => None,
        };
        // Clamped at zero: the fold can revise a task's minutes DOWN (a segment
        // reassigned to another workstream, a correction applied), and "-8
        // minutes of new work" is not a thing to show anyone.
        let stale_minutes = match (self.drafted_minutes, self.task_minutes) {
            (Some(base), Some(now)) => Some((now - base).max(0)),
            _ => None,
        };
        let state = self.state;
        let stale = state == "drafted" && stale_minutes.is_some_and(|m| m >= WORKLOG_STALE_MINUTES);
        DayTaskWorklogDraft {
            state,
            provider: self.provider,
            targets,
            propose,
            update,
            reasoning: self.reasoning,
            created_task_key: self.created_task_key,
            error: self.last_error,
            updated_at: self.updated_at,
            stale_minutes,
            stale,
        }
    }
}

/// Read the current draft for `(day_local, task_id)`, or `None` if there is none.
/// A missing table (pre-060 DB) degrades to `None`, never an error.
#[tracing::instrument(skip(pool))]
pub async fn get_day_task_worklog(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> anyhow::Result<Option<DayTaskWorklogDraft>> {
    let row = sqlx::query_as::<_, RawWorklog>(&format!(
        "SELECT {DRAFT_COLUMNS} {DRAFT_FROM} WHERE w.day_local = ? AND w.task_id = ?"
    ))
    .bind(day_local)
    .bind(task_id)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.read.day_task_worklogs"
    ))
    .await;

    match row {
        Ok(Some(r)) => {
            tracing::debug!(rows = 1, "day_task_worklogs.read.day_task_worklogs");
            let targets = targets::load(pool, day_local, task_id).await?;
            Ok(Some(r.into_draft(targets)))
        }
        Ok(None) => {
            tracing::debug!(rows = 0, "day_task_worklogs.read.day_task_worklogs");
            Ok(None)
        }
        Err(e) => {
            tracing::warn!(error = %e, "day_task_worklogs: read skipped (pre-060 DB?)");
            Ok(None)
        }
    }
}

/// Read the draft back, or a clean error. The shared tail of every write path here:
/// each one returns the row as it now stands, so a caller never has to guess what
/// its write produced.
async fn read_back(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> anyhow::Result<DayTaskWorklogDraft> {
    get_day_task_worklog(pool, day_local, task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("draft vanished mid-write"))
}

/// Where one day-task's worklog stands, for a list that shows many at once.
///
/// Deliberately NOT the full [`DayTaskWorklogDraft`]: this exists to render a badge on
/// a row, and the full shape carries the update text, the reasoning and every target -
/// a payload the size of the summary itself, for a list that shows none of it.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DayDraftState {
    pub task_id: String,
    /// `drafted` | `approved` | `posted` | `error`.
    pub state: String,
    /// Measured minutes worked since the draft was written, clamped at zero. `None`
    /// when the row predates `drafted_minutes` (pre-079).
    pub stale_minutes: Option<i64>,
}

/// Every worklog row on `day_local`, one per task that has one.
///
/// # Why the summary needs this
/// The daily summary lists a day's work and, under each row, said "no ticket yet -
/// draft ready" - as a fixed string, on every row, whether or not a draft existed. It
/// was a guess dressed as a fact: a task whose draft had never been generated read
/// identically to one with a draft waiting, and a task already POSTED read as though
/// it still needed doing. The one thing the line was for - telling the user there is
/// something here to act on - was the one thing it could not do.
///
/// Absent rows are the answer for "no draft", so a caller can distinguish all three
/// states rather than folding "none" and "ready" together.
///
/// # Who calls this
/// The tray's `get_day_draft_states` → `ui/components/summary/WorkList.tsx`.
///
/// # Related
/// - [`get_day_task_worklog`] — the full draft, for the one task a user opened.
/// - [`stale_drafts`] — the notifier's narrower read (already-stale rows only).
#[tracing::instrument(skip(pool))]
pub async fn day_draft_states(
    pool: &SqlitePool,
    day_local: &str,
) -> anyhow::Result<Vec<DayDraftState>> {
    let rows = sqlx::query_as::<_, DayDraftState>(&format!(
        "SELECT w.task_id AS task_id, w.state AS state, {STALE_MINUTES_EXPR} \
         {DRAFT_FROM} \
         WHERE w.day_local = ? \
         ORDER BY w.task_id"
    ))
    .bind(day_local)
    .fetch_all(pool)
    .instrument(tracing::debug_span!(
        "day_task_worklogs.read.day_draft_states"
    ))
    .await;

    match rows {
        Ok(r) => {
            tracing::debug!(rows = r.len(), "day_task_worklogs.read.day_draft_states");
            Ok(r)
        }
        // Same degradation as every other read here: a pre-060 DB has no table and a
        // pre-079 one has no `drafted_minutes`. Neither is worth failing a summary
        // over - the rows simply render without their badge.
        Err(e) => {
            tracing::debug!(error = %e, "day_task_worklogs.read.day_draft_states unavailable");
            Ok(Vec::new())
        }
    }
}

/// One draft that has fallen behind its task, for the daemon's notifier.
#[derive(Debug, Clone, FromRow)]
pub struct StaleDraft {
    pub task_id: String,
    pub title: String,
    /// Measured minutes worked since the draft was written — already past
    /// [`WORKLOG_STALE_MINUTES`], clamped at zero.
    pub stale_minutes: i64,
}

/// Every still-`drafted` worklog on `day_local` whose task has grown by at least
/// [`WORKLOG_STALE_MINUTES`] since it was written.
///
/// `drafted` only, deliberately: an approved or posted update is a record of
/// what went out, and telling someone it is "out of date" invites them to
/// overwrite the only copy of something already live on a ticket.
///
/// # Who calls this
/// The daemon's hourly fold, which notifies once per (task, staleness step) —
/// see `src/worklog_pipeline/stale.rs`.
#[tracing::instrument(skip(pool))]
pub async fn stale_drafts(pool: &SqlitePool, day_local: &str) -> anyhow::Result<Vec<StaleDraft>> {
    // INNER JOIN here, unlike [`DRAFT_FROM`]'s LEFT: this read exists to notify about a
    // draft falling behind its task, and a draft whose task is gone has nothing to fall
    // behind. The staleness arithmetic is still the shared one - only the join differs.
    let rows = sqlx::query_as::<_, StaleDraft>(&format!(
        "SELECT w.task_id AS task_id, t.title AS title, {STALE_MINUTES_EXPR} \
         FROM day_task_worklogs w \
         JOIN day_tasks t ON t.day_local = w.day_local AND t.task_id = w.task_id \
         WHERE w.day_local = ? AND w.state = 'drafted' \
           AND w.drafted_minutes IS NOT NULL \
           AND t.minutes - w.drafted_minutes >= ? \
         ORDER BY w.task_id"
    ))
    .bind(day_local)
    .bind(WORKLOG_STALE_MINUTES)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("day_task_worklogs.read.stale_drafts"))
    .await;

    match rows {
        Ok(r) => {
            tracing::debug!(rows = r.len(), "day_task_worklogs.read.stale_drafts");
            Ok(r)
        }
        // A pre-060 DB has no table and a pre-079 one has no column. Neither is
        // an error worth failing an hourly fold over — there is simply nothing
        // to notify about.
        Err(e) => {
            tracing::debug!(error = %e, "day_task_worklogs.read.stale_drafts unavailable");
            Ok(Vec::new())
        }
    }
}

/// The fields an [`upsert_draft`] writes. Kept as a struct to stay under clippy's
/// argument limit and to read at the call site.
#[derive(Debug, Clone)]
pub struct DraftUpsert {
    /// The tracker a proposal would be created on.
    pub provider: String,
    /// The tickets the model matched — 0..N. Mutually exclusive with `propose`.
    pub targets: Vec<TargetInput>,
    pub propose: Option<GeneratedWorklogPropose>,
    pub update: GeneratedWorklogUpdate,
    pub reasoning: String,
}

/// UPSERT a `drafted` row for `(day_local, task_id)`, overwriting only a row that
/// is still `drafted`. An `approved`/`posted` row is human-owned and left intact
/// (the `WHERE state = 'drafted'` guard) — this is "regenerate overwrites the
/// draft, never approved/posted work". Returns the freshly-read draft.
///
/// The targets are replaced ONLY when the guard actually matched a row. That check
/// is load-bearing: [`targets::replace`] deletes first, so running it against a
/// preserved posted row would erase the record of comments that are live on the
/// tracker, and the next approve would post every one of them a second time.
#[tracing::instrument(skip(pool, upsert))]
pub async fn upsert_draft(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    upsert: DraftUpsert,
    now: &str,
) -> anyhow::Result<DayTaskWorklogDraft> {
    let update_json = serde_json::to_string(&upsert.update).unwrap_or_else(|_| "{}".to_string());
    let (issue_type, title, desc) = match &upsert.propose {
        Some(p) => (
            Some(p.issue_type.clone()),
            Some(p.title.clone()),
            Some(p.description.clone()),
        ),
        None => (None, None, None),
    };

    // One transaction for the guard-write + target replace: the guard INSERT below
    // takes the write lock up front, so a concurrent approve() can't slip a
    // comment-post between the guard matching and replace()'s DELETE (which would
    // erase a live posted_comment_id and let the next approve re-post it).
    let mut tx = pool
        .begin()
        .await
        .context("opening the worklog draft upsert")?;
    let res = sqlx::query(
        // `drafted_minutes` is read here rather than passed in: it is not a
        // property of the generated update, it is a property of the moment the
        // write happens, and every caller would otherwise have to fetch and
        // thread the same number through with the same chance of forgetting.
        // Inside the transaction, so the baseline can never be a value from
        // before a concurrent fold. NULL when the task row is missing, which
        // reads back as "cannot know" rather than as zero.
        "INSERT INTO day_task_worklogs \
            (day_local, task_id, provider, propose_issue_type, propose_title, \
             propose_description, update_summary, update_json, reasoning, state, \
             created_task_key, last_error, created_at, updated_at, drafted_minutes) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'drafted', NULL, NULL, ?, ?, \
             (SELECT minutes FROM day_tasks WHERE day_local = ? AND task_id = ?)) \
         ON CONFLICT(day_local, task_id) DO UPDATE SET \
            drafted_minutes = excluded.drafted_minutes, \
            provider = excluded.provider, \
            propose_issue_type = excluded.propose_issue_type, \
            propose_title = excluded.propose_title, \
            propose_description = excluded.propose_description, \
            update_summary = excluded.update_summary, \
            update_json = excluded.update_json, \
            reasoning = excluded.reasoning, \
            state = 'drafted', \
            created_task_key = NULL, \
            create_attempt_at = NULL, \
            last_error = NULL, \
            updated_at = excluded.updated_at \
         WHERE day_task_worklogs.state = 'drafted'",
    )
    .bind(day_local)
    .bind(task_id)
    .bind(&upsert.provider)
    .bind(&issue_type)
    .bind(&title)
    .bind(&desc)
    .bind(&upsert.update.summary)
    .bind(&update_json)
    .bind(&upsert.reasoning)
    .bind(now)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(&mut *tx)
    .instrument(tracing::debug_span!("day_task_worklogs.write.upsert"))
    .await?;

    if res.rows_affected() > 0 {
        targets::replace(&mut tx, day_local, task_id, &upsert.targets, now).await?;
    }
    tx.commit()
        .await
        .context("committing the worklog draft upsert")?;

    // Read back whatever now owns the key — either the fresh draft, or the
    // preserved approved/posted row the guard protected.
    read_back(pool, day_local, task_id).await
}

/// Reopen a `posted` row for regeneration: flip it back to `drafted` so the next
/// [`upsert_draft`] can overwrite it with a fresh follow-up draft.
///
/// This is the "still working on this — log an update" path AFTER a worklog has
/// already been posted. Posting appends a comment to the tracker and CANNOT be
/// undone, so this does NOT try to: the comment(s) already on the board stay
/// exactly where they are. It only clears the local `posted` lock so a new cycle
/// can begin. On the next successful generate, [`upsert_draft`]'s guard (now
/// matching, since the row is `drafted` again) runs [`targets::replace`], which
/// deletes the old target rows — including their `posted_comment_id`s — and
/// inserts fresh, unposted targets. That is intended: the follow-up approve posts
/// a NEW comment capturing the work done since, rather than editing the old one.
///
/// Scoped to `posted` ONLY. An `approved` row is mid-post (a partial or failed
/// delivery the approve-retry owns); reopening it could clear the record of
/// targets whose comments are already live and let the next approve double-post
/// them. A `drafted` row is already open, so this is a no-op there too.
///
/// Returns `true` if a `posted` row was reopened, `false` if there was nothing to
/// reopen (no row, or it was already `drafted`/`approved`).
#[tracing::instrument(skip(pool))]
pub async fn reopen_posted(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    now: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "UPDATE day_task_worklogs SET state = 'drafted', last_error = NULL, updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'posted'",
    )
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .instrument(tracing::debug_span!("day_task_worklogs.write.reopen"))
    .await
    .context("reopening a posted worklog for regeneration")?;
    Ok(res.rows_affected() > 0)
}

/// Choose which tracker a still-`drafted` PROPOSAL will create its new ticket on.
///
/// A proposal's provider is assigned at generate time as "the first configured
/// tracker" (`src/pm_worklog/generate.rs`) — a placeholder that is simply wrong half
/// the time for anyone on two boards, and until now was not overridable: the user
/// could see "New Task: …" and had no way to say it belongs in GitHub rather than
/// Jira. This is that override.
///
/// Deliberately scoped to the PROPOSE branch (`propose_title IS NOT NULL`). A match
/// draft's provider is carried per-target — each matched ticket already lives on a
/// specific board — so changing the draft-level column there would name a tracker
/// the update does not actually post to. Callers get an error rather than a silent
/// no-op, so the UI can say why.
///
/// Does NOT re-run the model, for [`retarget_draft`]'s reason: the prose describes
/// the work, and the work does not change with the board it is filed on.
///
/// `WHERE state = 'drafted'` mirrors [`upsert_draft`]'s guard — once approved, the
/// ticket may already exist on the old provider, and re-pointing the row would make
/// it name a board the ticket is not on.
///
/// # Parameters
/// * `day_local` — local day key (`YYYY-MM-DD`), half of the row's composite key.
/// * `task_id` — the day-task this draft belongs to; the other half of the key.
/// * `provider` — tracker id to file the proposed ticket on. Validated against the
///   CONNECTED trackers by the caller ([`crate::commands::set_worklog_provider`] in
///   the tray) — this fn writes whatever it is handed.
/// * `now` — RFC3339 timestamp written to `updated_at`; passed in so the core stays
///   deterministic and testable.
///
/// # Returns
/// The row as it stands after the update, re-read via `read_back`.
///
/// # Errors
/// * The row is no longer `drafted` (already approved) — the message names the
///   provider the ticket now lives on.
/// * The row is a MATCH draft (`propose_title IS NULL`) — providers are per-target
///   there, so there is nothing at draft level to set.
///
/// Both surface as an error rather than a silent no-op so the UI can explain why.
/// Also clears `last_error`: the previous failure was against the old tracker.
#[tracing::instrument(skip(pool), err)]
pub async fn set_draft_provider(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    provider: &str,
    now: &str,
) -> anyhow::Result<DayTaskWorklogDraft> {
    let res = sqlx::query(
        "UPDATE day_task_worklogs SET provider = ?, last_error = NULL, updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'drafted' \
           AND propose_title IS NOT NULL",
    )
    .bind(provider)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .instrument(tracing::debug_span!("day_task_worklogs.write.set_provider"))
    .await
    .context("choosing the tracker for a proposed worklog ticket")?;

    if res.rows_affected() == 0 {
        // Two distinct reasons, and the user can act on only one of them — so look
        // rather than emit one vague message covering both.
        let existing = read_back(pool, day_local, task_id).await?;
        if existing.state != "drafted" {
            anyhow::bail!(
                "this worklog has already been approved - the ticket exists on {} now",
                existing.provider
            );
        }
        anyhow::bail!(
            "this draft posts to tickets that already exist - each one lives on its own board"
        );
    }
    tracing::info!(
        provider,
        rows = res.rows_affected(),
        "worklog: proposal tracker chosen by the user"
    );

    read_back(pool, day_local, task_id).await
}

/// Point a still-`drafted` row at the ONE ticket the USER picked, over the whole
/// board — dropping every ticket the model chose.
///
/// The matcher only ever sees the day's planned tasks ([`crate::plan`]), which is
/// the right default and the wrong answer often enough to need an override: work on
/// something unplanned can only come back as a proposal. This is that override, and
/// it is deliberately a COLLAPSE to a single ticket rather than an addition — a user
/// reaching for the picker is correcting the model, not extending it. To keep some
/// of the model's tickets and drop others, use [`dismiss_target`].
///
/// It does NOT re-run the model. The written update describes the work, and the
/// work doesn't change based on which ticket it's filed against — so a second
/// LLM call would cost a minute to rewrite prose that was already correct.
///
/// `WHERE state = 'drafted'` mirrors [`upsert_draft`]'s guard: an approved or
/// posted row is human-owned and already delivered, and silently re-pointing one
/// would mean the comment lives on a ticket the row no longer names.
#[tracing::instrument(skip(pool))]
pub async fn retarget_draft(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
    provider: &str,
    now: &str,
) -> anyhow::Result<DayTaskWorklogDraft> {
    // One transaction for the guard-write + target replace — same reason as
    // upsert_draft: the UPDATE takes the write lock, so a concurrent approve() can't
    // slip a comment-post between the guard matching and replace()'s DELETE.
    let mut tx = pool.begin().await.context("opening the worklog retarget")?;
    let res = sqlx::query(
        "UPDATE day_task_worklogs SET \
            propose_issue_type = NULL, \
            propose_title = NULL, \
            propose_description = NULL, \
            last_error = NULL, \
            updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'drafted'",
    )
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(&mut *tx)
    .instrument(tracing::debug_span!("day_task_worklogs.write.retarget"))
    .await
    .context("retargeting the worklog draft")?;

    if res.rows_affected() == 0 {
        anyhow::bail!(
            "this worklog has already been approved - regenerate it to pick a different ticket"
        );
    }
    targets::replace(
        &mut tx,
        day_local,
        task_id,
        &[TargetInput {
            task_key: task_key.to_string(),
            provider: provider.to_string(),
            confidence: 1.0,
            manual: true,
            // A hand-picked single ticket takes the workstream's draft-level update.
            update: None,
        }],
        now,
    )
    .await?;
    tx.commit()
        .await
        .context("committing the worklog retarget")?;
    tracing::info!(task_key, provider, "worklog: draft retargeted by the user");

    read_back(pool, day_local, task_id).await
}

/// Drop ONE ticket from a still-`drafted` row's target set, leaving the rest.
///
/// The model may match several tickets; this is how the user removes the one it got
/// wrong without discarding the update or re-running anything. Dismissing the last
/// one is allowed and leaves a draft with nothing to post — the user can then pick
/// a ticket with [`retarget_draft`] or regenerate. Refuses once the row leaves
/// `drafted`; see [`targets::dismiss`] for why an already-posted target can never
/// be dismissed at all.
#[tracing::instrument(skip(pool))]
pub async fn dismiss_target(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
) -> anyhow::Result<DayTaskWorklogDraft> {
    let draft = get_day_task_worklog(pool, day_local, task_id)
        .await?
        .context("there is no draft to change - generate one first")?;
    if draft.state != "drafted" {
        anyhow::bail!("this worklog has already been approved - it cannot be changed");
    }
    targets::dismiss(pool, day_local, task_id, task_key).await?;
    read_back(pool, day_local, task_id).await
}

/// Move a `drafted` row to `approved` (idempotent — an already-approved/posted row
/// is untouched). Clears any prior `last_error` so a retry starts clean.
#[tracing::instrument(skip(pool))]
pub async fn mark_approved(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs SET state = 'approved', last_error = NULL, updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'drafted'",
    )
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Persist a created ticket's key BEFORE its comment is posted, so an approve retry
/// never re-creates the ticket, and add it as the draft's target — from here on the
/// proposal is an ordinary ticket and the posting loop treats it as one.
///
/// Guarded to `approved` rows, matching [`mark_approved`]: a `drafted` or `posted`
/// row must never have a target grafted onto it by a stray call. A `posted` row is
/// the sharp case — inserting an unposted target would drag it back out of
/// `posted`, and the next approve would comment on a ticket nobody chose.
///
/// Safe to call more than once, which the approve path relies on: this is a
/// two-step write (record the key, then add the target), and re-running it repairs
/// an attempt that died between the steps rather than stranding the row with a
/// ticket it created but can't post to.
#[tracing::instrument(skip(pool))]
pub async fn mark_created(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    created_task_key: &str,
    provider: &str,
    now: &str,
) -> anyhow::Result<()> {
    // Clears create_attempt_at alongside recording the key: the create is resolved
    // (its outcome is now on disk in created_task_key), exactly as mark_posted clears
    // post_attempt_at. See [`begin_create`].
    let res = sqlx::query(
        "UPDATE day_task_worklogs SET created_task_key = ?, create_attempt_at = NULL, updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'approved'",
    )
    .bind(created_task_key)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        tracing::debug!(
            created_task_key,
            "day_task_worklogs: mark_created skipped (row is not approved)"
        );
        return Ok(());
    }
    targets::insert(
        pool,
        day_local,
        task_id,
        &TargetInput {
            task_key: created_task_key.to_string(),
            provider: provider.to_string(),
            // The user approved this proposal outright, so the ticket is their
            // choice rather than a guess against a candidate list. `manual` keeps
            // the UI from inventing a confidence score for it.
            confidence: 1.0,
            manual: true,
            // The freshly created ticket takes the draft-level (proposal) update.
            update: None,
        },
        now,
    )
    .await
}

/// How long a `create_attempt_at` claim is honoured before it is treated as dead and
/// reclaimable. Generously beyond any real `create_ticket` call (an API POST, seconds;
/// even the slowest CLI create path is bounded well under this), so a still-live create
/// is never reclaimed — only one whose owner crashed mid-call is.
pub const CREATE_CLAIM_STALE_MINS: i64 = 15;

/// Claim the right to create a proposed row's ticket, write-ahead, immediately
/// BEFORE calling the tracker. Returns `true` iff this caller now owns the create.
///
/// The create-step analog of [`targets::begin_post`], and the guard #2b needs:
/// `create_ticket` files a REAL ticket and has no dedup marker, so without this two
/// concurrent approves (the tray shells out `worklog-generate-approve` per click, so
/// two processes can race) both see `created_task_key IS NULL` and both file one, and
/// a single retry after a crash between the create returning and [`mark_created`]
/// committing does the same. The CAS (`created_task_key IS NULL` AND the claim is
/// either unset or [stale][`CREATE_CLAIM_STALE_MINS`]) hands the create to exactly one
/// caller; every loser (concurrent approve, post-crash retry) is refused by the same
/// predicate.
///
/// The winner MUST resolve its claim: [`mark_created`] on success, [`revert_create`]
/// on a DEFINITE failure. If the owner instead CRASHES mid-create the claim is left
/// set with no key; `stale_before` then lets a much-later retry reclaim it once the
/// claim is older than the stale window (by which point any live create has long since
/// resolved), rather than wedging the row forever. Migration 065 explains why this
/// bounded auto-reclaim is chosen over post_attempt_at's permanent dead end, and the
/// narrow duplicate risk it accepts. `stale_before` is the cutoff the caller passes as
/// `now - CREATE_CLAIM_STALE_MINS`, compared via SQLite `datetime()` so RFC3339
/// formatting differences don't skew the comparison.
#[tracing::instrument(skip(pool))]
pub async fn begin_create(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    now: &str,
    stale_before: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "UPDATE day_task_worklogs SET create_attempt_at = ? \
         WHERE day_local = ? AND task_id = ? AND state = 'approved' \
           AND created_task_key IS NULL \
           AND (create_attempt_at IS NULL OR datetime(create_attempt_at) < datetime(?))",
    )
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .bind(stale_before)
    .execute(pool)
    .await
    .context("claiming the worklog ticket create")?;
    Ok(res.rows_affected() > 0)
}

/// Release a claim taken by [`begin_create`] after a DEFINITE failure — the create
/// call returned an error, so no ticket was filed and a later retry is known-safe.
///
/// Only ever called when the failure is confirmed. It must NOT be called for the
/// "we don't know what happened" case: leaving `create_attempt_at` set is what stops
/// the retry from filing a second ticket. Guarded to a still-unfilled row so a create
/// that actually landed (key recorded) is never un-claimed.
#[tracing::instrument(skip(pool))]
pub async fn revert_create(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs SET create_attempt_at = NULL \
         WHERE day_local = ? AND task_id = ? AND created_task_key IS NULL",
    )
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await
    .context("releasing a worklog create claim after a failed create")?;
    Ok(())
}

/// Record that `task_key`'s comment is live, then move the row to `posted` IF every
/// target has now landed.
///
/// Partial delivery is a real state, not an edge case: posting to three tickets can
/// succeed on two and fail on the third, and a comment cannot be un-posted. Such a
/// row stays `approved`, so the user can retry — and the retry reads each target's
/// posted flag and skips the two that already succeeded rather than commenting twice.
#[tracing::instrument(skip(pool))]
pub async fn mark_posted(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    task_key: &str,
    comment_id: &str,
    browse_url: Option<&str>,
    now: &str,
) -> anyhow::Result<()> {
    targets::mark_posted(
        pool, day_local, task_id, task_key, comment_id, browse_url, now,
    )
    .await?;

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM day_task_worklog_targets \
         WHERE day_local = ? AND task_id = ? AND posted_comment_id IS NULL",
    )
    .bind(day_local)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .context("counting the worklog's unposted targets")?;

    if pending == 0 {
        sqlx::query(
            "UPDATE day_task_worklogs SET state = 'posted', last_error = NULL, updated_at = ? \
             WHERE day_local = ? AND task_id = ?",
        )
        .bind(now)
        .bind(day_local)
        .bind(task_id)
        .execute(pool)
        .await
        .context("marking the worklog posted")?;
    } else {
        tracing::info!(
            pending,
            "worklog: posted to some targets, others still pending"
        );
    }
    Ok(())
}

/// Record a draft-level failure without changing its lifecycle state — leaves it
/// retry-safe (a `drafted` row stays drafted, an `approved` row stays approved).
/// A failure against one ticket goes to [`targets::mark_error`] instead, so the
/// other tickets' outcomes aren't buried under it.
#[tracing::instrument(skip(pool))]
pub async fn mark_error(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    error: &str,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE day_task_worklogs SET last_error = ?, updated_at = ? \
         WHERE day_local = ? AND task_id = ?",
    )
    .bind(error)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
