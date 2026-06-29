//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
use crate::format;
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;
use tauri::tray::TrayIconId;

#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Unknown,
    Healthy,
    Unhealthy,
}

/// Source of an active capture pause — drives which panel the popover shows.
#[derive(Debug, Clone, PartialEq)]
pub enum PauseSource {
    /// User paused for a fixed duration via the popover duration picker.
    Timed,
    /// Auto-paused because current time is outside the configured work hours.
    Schedule,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveSession {
    pub app_name: String,
    pub elapsed_s: u64,
    /// Classifier category for the live block (e.g. "coding"); "idle_personal" default.
    pub category: String,
    /// Classifier confidence 0.0–1.0 — rendered as the "% match" readout.
    pub confidence: f64,
    /// Top foreground window title (the file/context line), if any.
    pub title: Option<String>,
}

/// Today's active time split by category, for the popover's Time Tracker tiles.
/// All seconds; aggregated in [`crate::poll::refresh::refresh_today`] from the
/// same `get_today` response that feeds `focus_s`.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct TodayBreakdown {
    pub coding_s: u64,
    pub review_s: u64,
    pub comms_s: u64,
    /// Time an AI agent ran autonomously (drives the "incl. Xm autonomous AI agent" note).
    pub autonomous_s: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusPayload {
    /// `false` on the very first `get_status` call before the poll loop has
    /// completed its first tick — lets the frontend show "Connecting…" instead
    /// of a misleading "PAUSED / Offline" during the 1–3 s startup window.
    pub has_polled: bool,
    pub healthy: bool,
    /// Unix timestamp (ms) when the current timed pause expires. `None` when
    /// not paused or when paused by a schedule (no fixed end time).
    pub pause_until_ms: Option<u64>,
    /// `"timed"` | `"schedule"` when capture is actively paused; `None` otherwise.
    pub pause_source: Option<String>,
    /// Display hint for the schedule-paused panel: the work-hours start time
    /// ("09:00"). `None` when not schedule-paused.
    pub schedule_resume_at: Option<String>,
    pub active_app: Option<String>,
    // ── Current task (tooltip card data) ────────────────────────────────────
    pub task_key: Option<String>,
    pub task_title: Option<String>,
    pub task_status_category: Option<String>,
    pub task_priority: Option<String>,
    pub task_spent_today_s: u64,
    pub task_estimate_s: Option<u64>,
    pub task_percent: Option<f64>,
    pub active_elapsed_s: u64,
    /// Pre-formatted: "Deep in VS Code for 1 hour 24 minutes"
    pub active_desc: Option<String>,
    /// Classifier category of the live session ("coding", "meeting", …).
    pub active_category: Option<String>,
    /// Classifier confidence 0.0–1.0 for the live session.
    pub active_confidence: f64,
    /// Top foreground window title for the live session.
    pub active_title: Option<String>,
    pub focus_s: u64,
    /// Pre-formatted: "6 hours 12 minutes"
    pub focus_desc: String,
    /// Today's per-category split for the Time Tracker tiles.
    pub coding_s: u64,
    pub review_s: u64,
    pub comms_s: u64,
    pub autonomous_s: u64,
    pub switch_count: u32,
    pub drafts_count: u32,
    pub ui_reachable: bool,
}

pub struct AppState {
    pub tray_id: Option<TrayIconId>,
    pub health: HealthStatus,
    /// Shared flag checked by the capture frame + UI-event consumers: when
    /// `true`, each incoming frame is silently dropped rather than written to
    /// `capture_frames`. The flag is unconditional (not feature-gated) so the
    /// `pause_for_duration` command can always reference `AppState` cleanly.
    pub capture_paused: Arc<AtomicBool>,
    /// Unix timestamp (secs) when the current timed pause expires.
    /// `None` when not timed-paused. Set by `pause_for_duration`, cleared on resume.
    pub pause_until: Option<u64>,
    /// Whether the current pause was triggered by the user (Timed) or the
    /// work-hours scheduler (Schedule).
    pub pause_source: Option<PauseSource>,
    /// Unix timestamp (secs) when the current pause started — used to compute
    /// `duration_s` when writing the gap row on resume.
    pub pause_started_at: Option<u64>,
    /// Work-hours resume time shown in the schedule-paused panel (e.g. "09:00").
    /// Set when the poll loop starts a schedule pause, cleared on resume.
    pub schedule_resume_at: Option<String>,
    pub active_session: Option<ActiveSession>,
    /// When `active_session` was last refreshed — lets the 1 s tray-title
    /// ticker advance the timer smoothly between the 30 s poll refreshes.
    pub active_set_at: Option<Instant>,
    /// Current task key for the menu-bar pill (e.g. `MER-142`) — the most
    /// recently classified task today; `None` when nothing is classified yet.
    pub current_task_key: Option<String>,
    /// Progress-ring fill `[0.0, 1.0]` for that task, or `None` when the ticket
    /// has no usable story-point budget (draw an un-filled ring then).
    pub task_percent: Option<f64>,
    // ── Tooltip card data (populated alongside current_task_key) ────────────
    pub task_title: Option<String>,
    pub task_status_category: Option<String>,
    pub task_priority: Option<String>,
    pub task_spent_today_s: u64,
    pub task_estimate_s: Option<u64>,
    pub focus_s: u64,
    pub today: TodayBreakdown,
    pub switch_count: u32,
    pub drafts_count: u32,
    pub ui_reachable: bool,
    pub last_poll: Option<Instant>,
    pub daemon_was_healthy: bool,
    pub consecutive_health_failures: u32,
    pub last_menu_state: HealthStatus,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tray_id: None,
            health: HealthStatus::Unknown,
            capture_paused: Arc::new(AtomicBool::new(false)),
            pause_until: None,
            pause_source: None,
            pause_started_at: None,
            schedule_resume_at: None,
            active_session: None,
            active_set_at: None,
            current_task_key: None,
            task_percent: None,
            task_title: None,
            task_status_category: None,
            task_priority: None,
            task_spent_today_s: 0,
            task_estimate_s: None,
            focus_s: 0,
            today: TodayBreakdown::default(),
            switch_count: 0,
            drafts_count: 0,
            ui_reachable: false,
            last_poll: None,
            daemon_was_healthy: false,
            consecutive_health_failures: 0,
            last_menu_state: HealthStatus::Unknown,
        }
    }
}

impl AppState {
    pub fn to_payload(&self) -> StatusPayload {
        let elapsed_s = self
            .active_session
            .as_ref()
            .map(|a| a.elapsed_s)
            .unwrap_or(0);
        let active_desc = self
            .active_session
            .as_ref()
            .map(|a| format::describe_active(&a.app_name, a.elapsed_s));
        let active = self.active_session.as_ref();
        StatusPayload {
            has_polled: self.last_poll.is_some(),
            healthy: self.health == HealthStatus::Healthy,
            pause_until_ms: self.pause_until.map(|t| t * 1000),
            pause_source: self.pause_source.as_ref().map(|s| match s {
                PauseSource::Timed => "timed".to_string(),
                PauseSource::Schedule => "schedule".to_string(),
            }),
            schedule_resume_at: self.schedule_resume_at.clone(),
            active_app: active.map(|a| a.app_name.clone()),
            task_key: self.current_task_key.clone(),
            task_title: self.task_title.clone(),
            task_status_category: self.task_status_category.clone(),
            task_priority: self.task_priority.clone(),
            task_spent_today_s: self.task_spent_today_s,
            task_estimate_s: self.task_estimate_s,
            task_percent: self.task_percent,
            active_elapsed_s: elapsed_s,
            active_desc,
            active_category: active.map(|a| a.category.clone()),
            active_confidence: active.map(|a| a.confidence).unwrap_or(0.0),
            active_title: active.and_then(|a| a.title.clone()),
            focus_s: self.focus_s,
            focus_desc: format::format_duration(self.focus_s),
            coding_s: self.today.coding_s,
            review_s: self.today.review_s,
            comms_s: self.today.comms_s,
            autonomous_s: self.today.autonomous_s,
            switch_count: self.switch_count,
            drafts_count: self.drafts_count,
            ui_reachable: self.ui_reachable,
        }
    }
}
