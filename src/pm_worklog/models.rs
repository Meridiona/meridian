//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Data models for the pm-worklog stage — the `JiraUpdate` shape (a summary +
// evidence-bearing bullets) that the worklog row stores and the status CLI reads.
// (The old `SessionBundle` synth-request contract went with the removed Stage-4
// chain.)

use serde::{Deserialize, Serialize};

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
