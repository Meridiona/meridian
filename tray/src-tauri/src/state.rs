//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
use crate::format;
use serde::Serialize;
use std::time::Instant;
use tauri::tray::TrayIconId;

#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Unknown,
    Healthy,
    Unhealthy,
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
    pub healthy: bool,
    pub active_app: Option<String>,
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
            active_session: None,
            active_set_at: None,
            current_task_key: None,
            task_percent: None,
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
            healthy: self.health == HealthStatus::Healthy,
            active_app: active.map(|a| a.app_name.clone()),
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
