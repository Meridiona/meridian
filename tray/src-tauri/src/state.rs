// meridian — normalises screenpipe activity into structured app sessions
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
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusPayload {
    pub healthy: bool,
    pub active_app: Option<String>,
    pub active_elapsed_s: u64,
    /// Pre-formatted: "Deep in VS Code for 1 hour 24 minutes"
    pub active_desc: Option<String>,
    pub focus_s: u64,
    /// Pre-formatted: "6 hours 12 minutes"
    pub focus_desc: String,
    pub switch_count: u32,
    pub drafts_count: u32,
    pub ui_reachable: bool,
}

pub struct AppState {
    pub tray_id: Option<TrayIconId>,
    pub health: HealthStatus,
    pub active_session: Option<ActiveSession>,
    pub focus_s: u64,
    pub switch_count: u32,
    pub drafts_count: u32,
    pub ui_reachable: bool,
    pub last_poll: Option<Instant>,
    /// notification dedup
    pub last_notified_drafts: u32,
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
            focus_s: 0,
            switch_count: 0,
            drafts_count: 0,
            ui_reachable: false,
            last_poll: None,
            last_notified_drafts: 0,
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
        StatusPayload {
            healthy: self.health == HealthStatus::Healthy,
            active_app: self.active_session.as_ref().map(|a| a.app_name.clone()),
            active_elapsed_s: elapsed_s,
            active_desc,
            focus_s: self.focus_s,
            focus_desc: format::format_duration(self.focus_s),
            switch_count: self.switch_count,
            drafts_count: self.drafts_count,
            ui_reachable: self.ui_reachable,
        }
    }
}
