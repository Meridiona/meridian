//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Deterministic ticket triage — the "board clean-up" brain used at onboarding and
// on every later sync. It takes the raw set of tickets a provider fetched and,
// using only fields already on `pm_tasks` (no LLM, no session evidence), sorts
// each into one of four buckets so the user can clean their board in a fast,
// worst-first pass before any classification runs.
//
//   ✅ Ready        — looks active and is detailed enough to attribute work to
//   ✏️ NeedsDetail  — likely active, but too thin for the classifier to match
//   🗑️ LooksStale   — abandoned signature; propose excluding / closing
//   ❓ NotSure       — open and reasonable, but no signal either way (quick keep/skip)
//
// SAFETY CONTRACT: this module only *proposes*. It never mutates a ticket, never
// deletes, never excludes on its own. A wrong verdict costs the user one glance,
// never lost data. Thresholds are conservative on purpose — we would rather leave
// a dead ticket in the pool (runtime evidence demotes it later) than wrongly flag
// a live one as stale.

mod rules;
pub mod store;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rules::Startedness;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// The four onboarding buckets a ticket can land in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageBucket {
    Ready,
    NeedsDetail,
    LooksStale,
    NotSure,
}

impl TriageBucket {
    /// The action the onboarding UI should pre-select for this bucket.
    pub fn suggested_action(&self) -> &'static str {
        match self {
            TriageBucket::Ready => "keep",
            TriageBucket::NeedsDetail => "add_detail",
            TriageBucket::LooksStale => "review_for_close",
            TriageBucket::NotSure => "confirm",
        }
    }

    /// Stable wire/storage string. Matches the serde `snake_case` rename.
    pub fn as_str(&self) -> &'static str {
        match self {
            TriageBucket::Ready => "ready",
            TriageBucket::NeedsDetail => "needs_detail",
            TriageBucket::LooksStale => "looks_stale",
            TriageBucket::NotSure => "not_sure",
        }
    }

    /// Parse the storage string back into a bucket.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ready" => Some(TriageBucket::Ready),
            "needs_detail" => Some(TriageBucket::NeedsDetail),
            "looks_stale" => Some(TriageBucket::LooksStale),
            "not_sure" => Some(TriageBucket::NotSure),
            _ => None,
        }
    }
}

/// A machine-readable reason a ticket landed where it did. Drives the human-facing
/// hint/suggestion shown next to each ticket in onboarding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "code", content = "detail")]
pub enum TriageReason {
    // — positive / active —
    InProgress,
    DueSoon { in_days: i64 },
    InSprint,
    StartDateReached,
    // — quality —
    MissingDescription,
    ThinDescription { chars: usize },
    VagueTitle,
    NoContextAnchor,
    MissingDueDate,
    MissingAssignee,
    MissingLabels,
    MissingPriority,
    MissingEstimate,
    MissingAcceptanceCriteria,
    // — staleness —
    NoActivitySince { days: i64 },
    NotStarted,
    NoDueDate,
    OverdueLong { by_days: i64 },
    FarFutureDue { in_days: i64 },
    NotInSprint,
    AlreadyDone,
    // — ambiguity / meta —
    NoActivitySignal,
    UnreadableUpdatedAt,
}

impl TriageReason {
    /// A short, friendly suggestion the UI can show verbatim.
    pub fn hint(&self) -> String {
        match self {
            TriageReason::InProgress => "Marked in progress on the board.".into(),
            TriageReason::DueSoon { in_days } if *in_days <= 0 => "Due today.".into(),
            TriageReason::DueSoon { in_days } => format!("Due in {in_days} day(s)."),
            TriageReason::InSprint => "In the active sprint.".into(),
            TriageReason::StartDateReached => "Its start date has passed.".into(),
            TriageReason::MissingDescription => {
                "No description — I'll have nothing to match your work against.".into()
            }
            TriageReason::ThinDescription { chars } => {
                format!("Description is only {chars} characters — add a bit of detail.")
            }
            TriageReason::VagueTitle => "Title is generic — make it specific.".into(),
            TriageReason::NoContextAnchor => "No epic or parent to anchor it.".into(),
            TriageReason::MissingDueDate => {
                "No due date — add one so Meridian knows when it's live.".into()
            }
            TriageReason::MissingAssignee => "No assignee — who owns this?".into(),
            TriageReason::MissingLabels => "No labels — add one to categorise it.".into(),
            TriageReason::MissingPriority => "No priority set.".into(),
            TriageReason::MissingEstimate => "No estimate — add story points.".into(),
            TriageReason::MissingAcceptanceCriteria => {
                "No acceptance criteria — define what 'done' means.".into()
            }
            TriageReason::NoActivitySince { days } => {
                format!("No board activity in {days} days.")
            }
            TriageReason::NotStarted => "Still sitting in a not-started column.".into(),
            TriageReason::NoDueDate => "No due date set.".into(),
            TriageReason::OverdueLong { by_days } => {
                format!("Overdue by {by_days} days with no movement.")
            }
            TriageReason::FarFutureDue { in_days } => {
                format!("Not due for {in_days} days — planned, not current work.")
            }
            TriageReason::NotInSprint => "Not in any sprint.".into(),
            TriageReason::AlreadyDone => "Already marked done.".into(),
            TriageReason::NoActivitySignal => "Open, but nothing says it's active yet.".into(),
            TriageReason::UnreadableUpdatedAt => "Couldn't read its last-updated time.".into(),
        }
    }

    /// The in-app fix for this defect, if it is directly fixable on the ticket.
    /// `None` for purely descriptive/active reasons. Drives both the Tasks-view fix
    /// control and the tracker write-back.
    pub fn fix(&self) -> Option<FixAction> {
        use FixControl::*;
        let f = |control, field, label, ai| {
            Some(FixAction {
                control,
                field,
                label,
                ai_suggested: ai,
            })
        };
        match self {
            TriageReason::MissingDescription => {
                f(EditText, "description", "Add a description", true)
            }
            TriageReason::ThinDescription { .. } => {
                f(EditText, "description", "Expand the description", true)
            }
            TriageReason::VagueTitle => f(EditText, "summary", "Make the title specific", true),
            TriageReason::NoContextAnchor => {
                f(PickParent, "parent", "Link to an epic or parent", false)
            }
            TriageReason::MissingDueDate => f(DatePicker, "duedate", "Add a due date", false),
            TriageReason::MissingAssignee => f(AssignSelf, "assignee", "Assign to me", false),
            TriageReason::MissingLabels => f(EditLabels, "labels", "Add a label", false),
            TriageReason::MissingPriority => f(PickPriority, "priority", "Set priority", false),
            TriageReason::MissingEstimate => {
                f(NumberInput, "story_points", "Add an estimate", false)
            }
            TriageReason::MissingAcceptanceCriteria => f(
                EditChecklist,
                "acceptance_criteria",
                "Add acceptance criteria",
                true,
            ),
            // Staleness is fixed at the ticket level (close / snooze), handled by the
            // bucket actions, not a per-field edit.
            _ => None,
        }
    }
}

/// The kind of input control the Tasks view shows to fix a hygiene defect, and how
/// the write-back maps it onto the tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FixControl {
    DatePicker,
    AssignSelf,
    EditText,
    EditChecklist,
    PickParent,
    EditLabels,
    PickPriority,
    NumberInput,
}

/// A concrete fix a dev can apply in-app, which writes back to the tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixAction {
    pub control: FixControl,
    /// Tracker field key the write-back targets.
    pub field: &'static str,
    /// Button/label text shown to the dev.
    pub label: &'static str,
    /// Whether the suggested value comes from AI (description / title / AC) — the
    /// edit box ships now; the AI suggestion is wired later.
    pub ai_suggested: bool,
}

/// The verdict for one ticket: where it landed and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriageVerdict {
    pub task_key: String,
    pub bucket: TriageBucket,
    pub reasons: Vec<TriageReason>,
}

/// Raw ticket fields the triage reads. Mirrors the relevant `pm_tasks` columns;
/// also deserialises directly from the test fixtures.
#[derive(Debug, Clone, Deserialize)]
pub struct TicketSignals {
    pub task_key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description_text: String,
    #[serde(default)]
    pub status_raw: String,
    #[serde(default)]
    pub is_terminal: bool,
    #[serde(default)]
    pub sprint_name: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub epic_title: Option<String>,
    #[serde(default)]
    pub parent_key: Option<String>,
    #[serde(default)]
    pub assignee_name: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub story_points: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Option<String>,
}

/// Which hygiene fields a board actually uses, detected once per board. A missing
/// field is only ever flagged when its `*` here is true — so a team that never
/// sets, say, story points is never nagged about them. A field counts as "used"
/// when at least one ticket on the board carries it.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoardUsage {
    pub sprints: bool,
    pub due_dates: bool,
    pub assignees: bool,
    pub parents: bool,
    pub labels: bool,
    pub priority: bool,
    pub estimates: bool,
    pub acceptance_criteria: bool,
}

impl BoardUsage {
    /// Detect which fields the board uses from the full fetched set.
    pub fn detect(tickets: &[TicketSignals]) -> Self {
        let any = |f: &dyn Fn(&TicketSignals) -> bool| tickets.iter().any(f);
        Self {
            sprints: any(&|t| opt_nonempty(&t.sprint_name).is_some()),
            due_dates: any(&|t| opt_nonempty(&t.due_date).is_some()),
            assignees: any(&|t| opt_nonempty(&t.assignee_name).is_some()),
            parents: any(&|t| {
                opt_nonempty(&t.parent_key).is_some() || opt_nonempty(&t.epic_title).is_some()
            }),
            labels: any(&|t| opt_nonempty(&t.tags).is_some()),
            priority: any(&|t| opt_nonempty(&t.priority).is_some()),
            estimates: any(&|t| opt_nonempty(&t.story_points).is_some()),
            acceptance_criteria: any(&|t| opt_nonempty(&t.acceptance_criteria).is_some()),
        }
    }
}

/// Tunable thresholds + the board's field usage. Defaults bias to KEEP.
#[derive(Debug, Clone, Copy)]
pub struct TriageConfig {
    pub now: DateTime<Utc>,
    /// Which hygiene fields the board uses (gates every "missing field" flag).
    pub usage: BoardUsage,
    /// Updated longer ago than this confirms staleness. (days)
    pub stale_age_days: i64,
    /// A due date within this horizon counts as an "active" signal. (days)
    pub due_soon_days: i64,
    /// Overdue by more than this (and otherwise quiet) reads as abandoned. (days)
    pub overdue_grace_days: i64,
    /// Descriptions shorter than this are too thin to match against. (chars)
    pub thin_desc_chars: usize,
}

impl TriageConfig {
    pub fn new(now: DateTime<Utc>, usage: BoardUsage) -> Self {
        Self {
            now,
            usage,
            stale_age_days: 60,
            due_soon_days: 30,
            overdue_grace_days: 14,
            thin_desc_chars: 40,
        }
    }
}

/// Triage a whole fetched board. Detects field usage once, then verdicts each.
pub fn triage_board(tickets: &[TicketSignals], now: DateTime<Utc>) -> Vec<TriageVerdict> {
    let cfg = TriageConfig::new(now, BoardUsage::detect(tickets));
    tickets.iter().map(|t| triage_ticket(t, &cfg)).collect()
}

/// Triage one ticket against a prepared config. Pure — no I/O.
pub fn triage_ticket(t: &TicketSignals, cfg: &TriageConfig) -> TriageVerdict {
    let verdict = |bucket, reasons| TriageVerdict {
        task_key: t.task_key.clone(),
        bucket,
        reasons,
    };

    // 1. Done tickets never belong in the active candidate pool.
    if t.is_terminal {
        return verdict(TriageBucket::LooksStale, vec![TriageReason::AlreadyDone]);
    }

    // — gather signals —
    let age = rules::age_days(&t.updated_at, cfg.now); // None ⇒ unknown (never "old")
    let started = rules::startedness(&t.status_raw);
    let in_sprint = opt_nonempty(&t.sprint_name).is_some();
    let due_in = opt_nonempty(&t.due_date).and_then(|d| rules::days_until_due(d, cfg.now));
    let start_reached = opt_nonempty(&t.start_date)
        .and_then(|d| rules::days_until_due(d, cfg.now))
        .map(|d| (-90..=0).contains(&d))
        .unwrap_or(false);

    let due_soon =
        due_in.is_some_and(|d| (-cfg.overdue_grace_days..=cfg.due_soon_days).contains(&d));
    let overdue_long = due_in.is_some_and(|d| d < -cfg.overdue_grace_days);
    // A due date beyond the horizon is planned-but-not-current work, not an active
    // signal. When the ticket also isn't started or sprinted, it should be excluded
    // from the current candidate pool (runtime session-evidence rescues it if the
    // user starts early).
    let far_future = due_in.is_some_and(|d| d > cfg.due_soon_days);
    let has_live_date = due_soon || start_reached;
    let active = started == Startedness::Started || in_sprint || has_live_date;

    // 2. Stale signature. The base requirement (not started, no live date window,
    //    not in an active sprint) demotes only when paired with either evidence of
    //    abandonment (old/overdue) OR a far-future due date (not current work).
    let sprint_ok_for_stale = !cfg.usage.sprints || !in_sprint;
    let is_old = age.is_some_and(|a| a > cfg.stale_age_days);
    let base_stale = started != Startedness::Started && !has_live_date && sprint_ok_for_stale;
    if base_stale && (is_old || far_future) {
        let mut reasons = vec![TriageReason::NotStarted];
        if is_old {
            if let Some(a) = age {
                reasons.push(TriageReason::NoActivitySince { days: a });
            }
        }
        if far_future {
            reasons.push(TriageReason::FarFutureDue {
                in_days: due_in.unwrap(),
            });
        } else if overdue_long {
            reasons.push(TriageReason::OverdueLong {
                by_days: -due_in.unwrap(),
            });
        } else if due_in.is_none() {
            reasons.push(TriageReason::NoDueDate);
        }
        if cfg.usage.sprints && !in_sprint {
            reasons.push(TriageReason::NotInSprint);
        }
        return verdict(TriageBucket::LooksStale, reasons);
    }

    // 3. Quality / hygiene — the Definition-of-Ready fields a good ticket carries.
    //    Each "missing field" check is gated on the board actually using that field
    //    (cfg.usage.*), so a team that doesn't track e.g. story points is never
    //    nagged about them. Every reason here maps to an in-app fix (TriageReason::fix).
    let reasons = hygiene_issues(t, cfg);
    if !reasons.is_empty() {
        return verdict(TriageBucket::NeedsDetail, reasons);
    }

    // 4. Active + detailed ⇒ Ready. Otherwise we genuinely can't tell ⇒ NotSure.
    if active {
        let mut reasons = Vec::new();
        if started == Startedness::Started {
            reasons.push(TriageReason::InProgress);
        }
        if due_soon {
            reasons.push(TriageReason::DueSoon {
                in_days: due_in.unwrap(),
            });
        }
        if in_sprint {
            reasons.push(TriageReason::InSprint);
        }
        if start_reached && !due_soon {
            reasons.push(TriageReason::StartDateReached);
        }
        verdict(TriageBucket::Ready, reasons)
    } else {
        let mut reasons = vec![TriageReason::NoActivitySignal];
        if age.is_none() {
            reasons.push(TriageReason::UnreadableUpdatedAt);
        }
        verdict(TriageBucket::NotSure, reasons)
    }
}

/// All Definition-of-Ready hygiene defects on a ticket, each mapping to an in-app
/// fix (`TriageReason::fix`). Board-guarded: a missing field is flagged only when
/// the board actually uses that field. A non-empty result buckets the ticket as
/// `needs_detail`.
pub fn hygiene_issues(t: &TicketSignals, cfg: &TriageConfig) -> Vec<TriageReason> {
    let mut reasons = Vec::new();
    let desc = t.description_text.trim();
    let desc_chars = desc.chars().count();
    if desc.is_empty() {
        reasons.push(TriageReason::MissingDescription);
    } else if desc_chars < cfg.thin_desc_chars {
        reasons.push(TriageReason::ThinDescription { chars: desc_chars });
    }
    if rules::is_vague_title(&t.title) {
        reasons.push(TriageReason::VagueTitle);
    }
    if cfg.usage.due_dates && opt_nonempty(&t.due_date).is_none() {
        reasons.push(TriageReason::MissingDueDate);
    }
    if cfg.usage.assignees && opt_nonempty(&t.assignee_name).is_none() {
        reasons.push(TriageReason::MissingAssignee);
    }
    if cfg.usage.parents
        && opt_nonempty(&t.parent_key).is_none()
        && opt_nonempty(&t.epic_title).is_none()
    {
        reasons.push(TriageReason::NoContextAnchor);
    }
    if cfg.usage.labels && opt_nonempty(&t.tags).is_none() {
        reasons.push(TriageReason::MissingLabels);
    }
    if cfg.usage.priority && opt_nonempty(&t.priority).is_none() {
        reasons.push(TriageReason::MissingPriority);
    }
    if cfg.usage.estimates && opt_nonempty(&t.story_points).is_none() {
        reasons.push(TriageReason::MissingEstimate);
    }
    if cfg.usage.acceptance_criteria && opt_nonempty(&t.acceptance_criteria).is_none() {
        reasons.push(TriageReason::MissingAcceptanceCriteria);
    }
    reasons
}

/// Borrow the inner string only when it is present and not blank — providers store
/// "missing" as either NULL or an empty string, and both must read as absent.
fn opt_nonempty(o: &Option<String>) -> Option<&str> {
    o.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Per-bucket tallies from one triage pass, for logging and the onboarding summary.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct TriageSummary {
    pub ready: u32,
    pub needs_detail: u32,
    pub looks_stale: u32,
    pub not_sure: u32,
    /// Curation rows removed because their ticket left the board.
    pub pruned: u64,
}

impl TriageSummary {
    fn record(&mut self, bucket: TriageBucket) {
        match bucket {
            TriageBucket::Ready => self.ready += 1,
            TriageBucket::NeedsDetail => self.needs_detail += 1,
            TriageBucket::LooksStale => self.looks_stale += 1,
            TriageBucket::NotSure => self.not_sure += 1,
        }
    }

    /// Tickets that want the user's attention (everything but `ready`).
    pub fn needs_attention(&self) -> u32 {
        self.needs_detail + self.looks_stale + self.not_sure
    }
}

/// Triage the whole cached board and persist every verdict into `pm_task_curation`,
/// then drop curation rows for tickets that left the board. Idempotent: re-running
/// refreshes machine verdicts but never overwrites a human decision. Runs right
/// after a PM sync, so the working set is always current.
pub async fn run_triage(pool: &SqlitePool, now: DateTime<Utc>) -> Result<TriageSummary> {
    let inputs = store::load_board(pool).await?;
    let tickets: Vec<TicketSignals> = inputs.iter().map(|i| i.signals.clone()).collect();
    let cfg = TriageConfig::new(now, BoardUsage::detect(&tickets));
    let now_str = now.to_rfc3339();

    let mut summary = TriageSummary::default();
    for input in &inputs {
        let verdict = triage_ticket(&input.signals, &cfg);
        summary.record(verdict.bucket);
        store::save_verdict(pool, &input.provider, &verdict, &now_str).await?;
    }
    summary.pruned = store::prune_orphans(pool).await?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Fixed "now" the fixtures are authored against: 2026-06-12T12:00:00Z.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap()
    }

    #[derive(Deserialize)]
    struct Fixture {
        #[serde(flatten)]
        ticket: TicketSignals,
        expected: TriageBucket,
        board_uses_sprints: bool,
        #[serde(default)]
        board_uses_due_dates: bool,
        #[serde(default)]
        board_uses_assignees: bool,
        #[serde(default)]
        board_uses_parents: bool,
        #[serde(default)]
        board_uses_labels: bool,
        #[serde(default)]
        board_uses_priority: bool,
        #[serde(default)]
        board_uses_estimates: bool,
        #[serde(default)]
        board_uses_acceptance_criteria: bool,
        note: String,
    }

    #[test]
    fn fixture_dataset_matches_expected_buckets() {
        let raw = include_str!("triage_fixtures.json");
        let fixtures: Vec<Fixture> = serde_json::from_str(raw).expect("valid fixture json");
        assert!(fixtures.len() >= 18, "dataset should be substantial");

        let mut failures = Vec::new();
        for fx in &fixtures {
            let usage = BoardUsage {
                sprints: fx.board_uses_sprints,
                due_dates: fx.board_uses_due_dates,
                assignees: fx.board_uses_assignees,
                parents: fx.board_uses_parents,
                labels: fx.board_uses_labels,
                priority: fx.board_uses_priority,
                estimates: fx.board_uses_estimates,
                acceptance_criteria: fx.board_uses_acceptance_criteria,
            };
            let cfg = TriageConfig::new(now(), usage);
            let got = triage_ticket(&fx.ticket, &cfg);
            if got.bucket != fx.expected {
                failures.push(format!(
                    "{}: expected {:?}, got {:?} ({}) — reasons {:?}",
                    fx.ticket.task_key, fx.expected, got.bucket, fx.note, got.reasons
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "triage mismatches:\n{}",
            failures.join("\n")
        );
    }

    fn base() -> TicketSignals {
        TicketSignals {
            task_key: "T-1".into(),
            title: "Integrate Stripe Checkout for subscriptions".into(),
            description_text: "A".repeat(120),
            status_raw: "To Do".into(),
            is_terminal: false,
            sprint_name: None,
            due_date: None,
            start_date: None,
            updated_at: "2026-06-12T00:00:00Z".into(),
            epic_title: None,
            parent_key: None,
            assignee_name: None,
            tags: None,
            priority: None,
            story_points: None,
            acceptance_criteria: None,
        }
    }

    #[test]
    fn terminal_ticket_is_stale_regardless() {
        let mut t = base();
        t.is_terminal = true;
        t.status_raw = "In Progress".into();
        let v = triage_ticket(&t, &TriageConfig::new(now(), BoardUsage::default()));
        assert_eq!(v.bucket, TriageBucket::LooksStale);
        assert_eq!(v.reasons, vec![TriageReason::AlreadyDone]);
    }

    #[test]
    fn one_stale_signal_alone_never_demotes() {
        // No due date (a stale signal) but recently updated ⇒ NOT stale.
        let t = base();
        let v = triage_ticket(&t, &TriageConfig::new(now(), BoardUsage::default()));
        assert_ne!(v.bucket, TriageBucket::LooksStale);
    }

    #[test]
    fn unparseable_timestamp_never_marks_stale() {
        let mut t = base();
        t.updated_at = "garbage".into();
        t.status_raw = "Backlog".into();
        // not started + no due + unknown age ⇒ age gate fails ⇒ not stale.
        let v = triage_ticket(&t, &TriageConfig::new(now(), BoardUsage::default()));
        assert_eq!(v.bucket, TriageBucket::NotSure);
        assert!(v.reasons.contains(&TriageReason::UnreadableUpdatedAt));
    }

    #[test]
    fn active_but_thin_is_needs_detail_not_ready() {
        let mut t = base();
        t.status_raw = "In Progress".into();
        t.description_text = "fix it".into();
        let v = triage_ticket(&t, &TriageConfig::new(now(), BoardUsage::default()));
        assert_eq!(v.bucket, TriageBucket::NeedsDetail);
    }

    #[test]
    fn sprint_rules_disabled_when_board_has_no_sprints() {
        // Old, not started, no due, no sprint — but board doesn't use sprints, so
        // the missing sprint must NOT itself be required; staleness still fires on
        // the other three conditions.
        let mut t = base();
        t.updated_at = "2026-01-01T00:00:00Z".into();
        t.status_raw = "Backlog".into();
        let v = triage_ticket(&t, &TriageConfig::new(now(), BoardUsage::default()));
        assert_eq!(v.bucket, TriageBucket::LooksStale);
        assert!(!v.reasons.contains(&TriageReason::NotInSprint));
    }
}
