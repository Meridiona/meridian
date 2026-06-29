//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Stage 4 — pm-worklog. Turns classified `task` sessions into Jira worklogs,
// one per (task, hour), entirely in Rust except the single LLM hop (the agno
// synth, hosted on the MLX server's `/synthesise_worklog` endpoint and reached
// through the global LLM gate). The hour-driven driver walks each day's hours,
// processing every hour whose upstream stages have settled.
//
//   collect → synthesise (gated LLM) → ground → DRAFT
//
// The driver NEVER posts. Every worklog lands as a `drafted` row for a human to
// review, edit, and approve in the dashboard; approval flips it to `approved`,
// and the `post` sweep (the only path to real Jira) posts it. This is the sole
// post gate — there is no unattended auto-post.
//
// Spawned as two gated tokio tasks from `main.rs` (the hourly driver + the
// ~60s approved-poster); also runnable one-shot via
// `meridian pm-worklog [--day YYYY-MM-DD]` and `meridian worklog-post-approved`.

pub mod azure_devops;
pub mod collect;
pub mod comment;
pub mod config;
pub mod create;
pub mod db;
pub mod github;
pub mod ground;
pub mod jira;
pub mod ledger;
pub mod linear;
pub mod models;
pub mod post;
pub mod route;
pub mod scheduler;
pub mod status;
pub mod synth;
pub mod trello;

pub use config::PmWorklogConfig;
pub use post::{cli_post_approved, post_approved, run_post_loop};
pub use scheduler::{cli_run, run_driver, run_loop, DriverSummary};
pub use status::cli_status;
