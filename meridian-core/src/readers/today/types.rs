//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Response types and DB row shapes for the `/api/today` port.
//!
//! A private sibling of `today/mod.rs`, split out because the query module hit
//! the 500-line cap. Public response types are re-exported from the parent
//! [`super`] (i.e. `meridian_core::today::TodayResponse`).

use crate::intervals::Interval;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::BTreeMap;

// ── Response types (field names match the TS JSON exactly for golden-compare) ──

#[derive(Debug, Clone, Serialize)]
pub struct TodaySession {
    pub id: i64,
    pub app: String,
    pub started_at: String,
    pub dur: i64,
    pub cat: String,
    pub titles: Vec<String>,
    pub explain: Option<String>,
    pub routing: Option<String>,
    pub session_type: Option<String>,
    pub task_key: Option<String>,
    pub candidates: Vec<String>,
    pub confidence: f64,
    pub method: String,
    pub link_method: Option<String>,
    pub link_confidence: Option<f64>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayActive {
    pub app: String,
    pub started_at: String,
    pub elapsed_s: i64,
    pub cat: String,
    pub titles: Vec<String>,
    pub confidence: f64,
    pub explain: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayGap {
    pub id: i64,
    pub kind: String,
    pub started_at: String,
    pub ended_at: String,
    pub dur: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskMeta {
    pub title: String,
    pub provider: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub started_at: String,
    pub dur: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayResponse {
    pub date: String,
    pub sessions: Vec<TodaySession>,
    pub active: Option<TodayActive>,
    pub gaps: Vec<TodayGap>,
    pub focus_s: i64,
    pub idle_s: i64,
    pub agent_s: i64,
    pub supervised_s: i64,
    pub autonomous_s: i64,
    pub presence_segments: Vec<Interval>,
    pub agent_segments: Vec<Interval>,
    pub session_count: i64,
    pub switch_count: i64,
    pub task_totals: BTreeMap<String, i64>,
    pub task_autonomous_s: BTreeMap<String, i64>,
    pub engaged_s: i64,
    pub task_meta: BTreeMap<String, TaskMeta>,
    pub task_agent_summaries: BTreeMap<String, Vec<AgentSummary>>,
}

// ── DB row shapes (private to the today query) ─────────────────────────────────

#[derive(FromRow)]
pub(crate) struct TodayRow {
    pub(crate) id: i64,
    pub(crate) app_name: String,
    pub(crate) started_at: String,
    pub(crate) ended_at: String,
    pub(crate) duration_s: i64,
    pub(crate) coding_agent_session_uuid: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) confidence: Option<f64>,
    pub(crate) category_method: Option<String>,
    pub(crate) category_explanation: Option<String>,
    pub(crate) session_summary: Option<String>,
    pub(crate) window_titles: Option<String>,
    pub(crate) task_key: Option<String>,
    pub(crate) routing: Option<String>,
    pub(crate) session_type: Option<String>,
    pub(crate) link_method: Option<String>,
    pub(crate) link_confidence: Option<f64>,
}

#[derive(FromRow)]
pub(crate) struct ActiveRow {
    pub(crate) app_name: String,
    pub(crate) started_at: String,
    // The daemon's last observation of this block. Used to cap the block's
    // presence extent: a stopped daemon leaves `last_seen_at` stale, so counting
    // presence to "now" would inflate today's focus by the whole dead interval.
    pub(crate) last_seen_at: String,
    pub(crate) window_titles: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) confidence: Option<f64>,
    // active_session has no category_explanation column (only app_sessions does,
    // added in migration 013). The classifier writes it AFTER sealing — a live
    // block genuinely has no explanation yet.
}

#[derive(FromRow)]
pub(crate) struct GapRow {
    pub(crate) id: i64,
    pub(crate) kind: String,
    pub(crate) started_at: String,
    pub(crate) ended_at: String,
    pub(crate) duration_s: i64,
}

#[derive(FromRow)]
pub(crate) struct TaskMetaRow {
    pub(crate) task_key: String,
    pub(crate) title: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) url: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct TitleEntry {
    pub(crate) window_name: Option<String>,
    pub(crate) title: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parsed window_titles → (top title falling back to `app`, list of non-empty names).
pub(crate) fn parse_titles(window_titles: &Option<String>, app: &str) -> (String, Vec<String>) {
    let entries: Vec<TitleEntry> = window_titles
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let name = |t: &TitleEntry| t.window_name.clone().or_else(|| t.title.clone());
    let top = entries
        .first()
        .and_then(name)
        .unwrap_or_else(|| app.to_string());
    let names: Vec<String> = if entries.is_empty() {
        vec![top.clone()]
    } else {
        entries
            .iter()
            .filter_map(|t| name(t).filter(|s| !s.is_empty()))
            .collect()
    };
    (top, names)
}

/// `cat`, with fm_parse_error/fm_skip and empty/NULL normalised to idle_personal.
pub(crate) fn normalize_cat(category: &Option<String>) -> String {
    match category.as_deref() {
        Some("fm_parse_error") | Some("fm_skip") => "idle_personal".to_string(),
        Some(c) if !c.is_empty() => c.to_string(),
        _ => "idle_personal".to_string(),
    }
}
