//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

pub mod coding_agent_session_ingest;
pub mod config;
pub mod daily_plan;
pub mod db;
pub mod etl;
pub mod health;
pub mod intelligence;
pub mod llm_gate;
pub mod notices;
pub mod notifications;
pub mod observability;
pub mod pm_worklog;
pub mod telemetry_spool;
pub mod worklog_pipeline;
