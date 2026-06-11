//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Coding-agent indexer + summariser, ported from the Python services
// (the former Python indexer, the former Python summariser) into the
// daemon. Spawned as gated tokio tasks from main.rs: the indexer turns
// Claude/Codex JSONLs into app_sessions segment rows; the summariser turns
// sealed segments into prose summaries; both stay dormant without a coding
// agent present. CLI subcommands (`coding-agent-hook`, `coding-agent-summarise`)
// run one-shot against the same DB.

pub mod cursor_agent_init;
pub mod db;
pub mod hook;
pub mod indexer;
pub mod jsonl;
pub mod segment;
pub mod sources;
pub mod summariser;

use std::path::PathBuf;
use std::str::FromStr;

pub use segment::{
    iso_utc, norm_iso, parse_iso, parse_session_segments, Segment, SegmentParams, SessionMeta,
};

/// Path to the meridian DB (MERIDIAN_DB env, default `~/.meridian/meridian.db`).
pub fn meridian_db_path() -> PathBuf {
    let raw =
        std::env::var("MERIDIAN_DB").unwrap_or_else(|_| "~/.meridian/meridian.db".to_string());
    PathBuf::from(shellexpand::tilde(&raw).into_owned())
}

/// Open a short-lived pool against the meridian DB (the daemon already created +
/// migrated it; we never migrate here). Used by the one-shot CLI subcommands.
pub async fn open_meridian_pool() -> anyhow::Result<sqlx::SqlitePool> {
    let path = meridian_db_path();
    let uri = format!("sqlite://{}", path.display());
    let opts = sqlx::sqlite::SqliteConnectOptions::from_str(&uri)?.create_if_missing(false);
    Ok(sqlx::SqlitePool::connect_with(opts).await?)
}
