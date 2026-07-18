//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// pm-worklog — the PM-provider worklog/ticket layer: turns a day-task workstream into
// a drafted worklog matched to (or proposing) a ticket, and posts approved drafts to the
// real tracker.
//
// The live surface is the per-card engine ([`generate`]/[`approve`] → [`create`] →
// [`post_comment`]) plus the ~60s approved-poster sweep ([`post::run_post_loop`], the ONLY
// path to real Jira/Linear/GitHub — approval-gated, never unattended). The old hourly
// Stage-4 driver (collect → synthesise → ground → draft) was superseded by the
// Rust-owned per-card generate path and has been removed.

pub mod azure_devops;
pub mod comment;
pub mod config;
pub mod create;
pub mod db;
pub mod generate;
pub mod github;
pub mod jira;
pub mod ledger;
pub mod linear;
pub mod models;
pub mod post;
pub mod post_comment;
pub mod status;
pub mod trello;

pub use config::PmWorklogConfig;
pub use generate::{approve, generate, ApproveResult};
pub use post::{cli_post_approved, post_approved, run_post_loop};
pub use post_comment::post_comment;
pub use status::cli_status;
