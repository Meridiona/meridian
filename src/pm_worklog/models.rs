// meridian — normalises screenpipe activity into structured app sessions
//
// Data models for the pm-worklog stage (Stage 4). These mirror the Python
// `pm_worklog_update/models.py` field-for-field so the JSON contract with the
// MLX server's `/synthesise_worklog` endpoint stays exact: Rust SERIALISES a
// `SessionBundle` to send, and DESERIALISES the `JiraUpdate` the agno synth
// returns. Everything else (collect, ground, route, post) is Rust.

use serde::{Deserialize, Serialize};

/// One classified session, condensed for the synth prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDigest {
    pub id: i64,
    pub app_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_s: i64,
    #[serde(default)]
    pub idle_frame_s: i64,
    #[serde(default)]
    pub top_titles: Vec<String>,
    #[serde(default)]
    pub dimensions: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub text_source: Option<String>,
}

/// The collected input for one (task, hour) window — what Rust sends to the
/// synth endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBundle {
    pub task_key: String,
    pub window_start: String,
    pub window_end: String,
    #[serde(default)]
    pub cycle_index: i64,
    pub sessions: Vec<SessionDigest>,
    pub total_seconds: i64,
    pub real_seconds: i64,
    #[serde(default)]
    pub raw_text_bytes: i64,
    #[serde(default)]
    pub is_heavy: bool,
    #[serde(default)]
    pub pm_task_status: Option<String>,
    #[serde(default)]
    pub pm_task_title: Option<String>,
    #[serde(default)]
    pub pm_task_description: Option<String>,
    #[serde(default)]
    pub assignee_name: Option<String>,
    #[serde(default)]
    pub earlier_today_summaries: Vec<String>,
}

impl SessionBundle {
    /// Min/max session id in the bundle — stored on the worklog row for the
    /// evidence panel and for backfill bookkeeping.
    pub fn session_id_bounds(&self) -> (Option<i64>, Option<i64>) {
        let min = self.sessions.iter().map(|s| s.id).min();
        let max = self.sessions.iter().map(|s| s.id).max();
        (min, max)
    }
}

/// One worklog bullet plus the session ids that prove it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulletWithEvidence {
    pub text: String,
    #[serde(default)]
    pub evidence_refs: Vec<i64>,
}

/// The synth's structured output (and our worklog payload). Field names match
/// `JiraUpdate` in the Python package exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraUpdate {
    pub task_key: String,
    pub window_start: String,
    pub window_end: String,
    #[serde(default)]
    pub cycle_index: i64,
    #[serde(default)]
    pub time_spent_seconds: i64,

    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub what_shipped: Vec<BulletWithEvidence>,
    #[serde(default)]
    pub in_progress: Vec<BulletWithEvidence>,
    #[serde(default)]
    pub blockers: Vec<BulletWithEvidence>,
    #[serde(default)]
    pub decisions: Vec<BulletWithEvidence>,
    #[serde(default)]
    pub next_steps: Vec<String>,

    #[serde(default)]
    pub risk_flags: Vec<String>,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub reasoning: String,
}

impl JiraUpdate {
    /// All evidence-bearing bullets in display order (shipped → in-progress →
    /// blockers → decisions), matching the Python `bullets` property.
    pub fn bullets(&self) -> impl Iterator<Item = &BulletWithEvidence> {
        self.what_shipped
            .iter()
            .chain(self.in_progress.iter())
            .chain(self.blockers.iter())
            .chain(self.decisions.iter())
    }

    /// The (kind, bullets) groups in the canonical order used for evidence rows.
    pub fn bullet_groups(&self) -> [(&'static str, &Vec<BulletWithEvidence>); 4] {
        [
            ("shipped", &self.what_shipped),
            ("in_progress", &self.in_progress),
            ("blocker", &self.blockers),
            ("decision", &self.decisions),
        ]
    }
}

/// The grounded narrative — a JiraUpdate after un-evidenced bullets are dropped,
/// with the coverage metric and what was removed.
#[derive(Debug, Clone)]
pub struct GroundedNarrative {
    pub update: JiraUpdate,
    pub coverage: f64,
    pub dropped_bullets: Vec<String>,
}

/// Lifecycle state of a worklog row.
///
///   drafted ──(UI edit)──▶ drafted ──(UI approve)──▶ approved ──(daemon)──▶ posted
///
/// `Approved` is set by the dashboard, never by the daemon: the driver only ever
/// drafts, and the approved-sweep is the sole path that posts to Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    Drafted,
    Approved,
    Posted,
    Skipped,
    Failed,
}

impl UpdateState {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateState::Drafted => "drafted",
            UpdateState::Approved => "approved",
            UpdateState::Posted => "posted",
            UpdateState::Skipped => "skipped",
            UpdateState::Failed => "failed",
        }
    }
}
