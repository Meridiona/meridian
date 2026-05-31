// meridian — normalises screenpipe activity into structured app sessions
//
// Stage 4 — pm-worklog. Turns classified `task` sessions into Jira worklogs,
// one per (task, hour), entirely in Rust except the single LLM hop (the agno
// synth, hosted on the MLX server's `/synthesise_worklog` endpoint and reached
// through the global LLM gate). The hour-driven driver walks each day's hours,
// processing every hour whose upstream stages have settled.
//
//   collect → synthesise (gated LLM) → ground → route (persist + post)
//
// Spawned as a gated tokio task from `main.rs`; also runnable one-shot via
// `meridian pm-worklog [--day YYYY-MM-DD] [--dry-run]`.

pub mod collect;
pub mod config;
pub mod db;
pub mod ground;
pub mod jira;
pub mod ledger;
pub mod models;
pub mod route;
pub mod scheduler;
pub mod synth;

pub use config::PmWorklogConfig;
pub use scheduler::{cli_run, run_driver, run_loop, DriverSummary};
