//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/coding-agents` ported to Rust — a faithful port of
//! `ui/app/api/coding-agents/route.ts`.
//!
//! # What this is
//! Today's coding-agent activity: the union of all coding-agent sessions
//! (overlap deduped) plus a per-agent union, descending. NOTE: the route unions
//! the raw `started_at`/`ended_at` spans (it does NOT cap to `duration_s` the
//! way `today`/the session-interval math does), so neither do we — byte-identical.
//!
//! # Who calls this
//! The tray `get_coding_agents` command → (currently no dashboard consumer; Today
//! already surfaces agent time via `agent_s`/`agent_segments`). Ported for parity.
//!
//! # Related
//! - [`crate::intervals::union_seconds`] does the overlap-dedup math.
//! - [`crate::today`] is the live surface for agent time; [`crate::tasks`] splits
//!   agent time into supervised vs autonomous (this module does not).

use crate::intervals::{union_seconds, Interval};
use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use tracing::Instrument;

/// The agents we attribute (matches the route's `CODING_AGENTS`).
const CODING_AGENTS: [&str; 2] = ["Claude Code", "Codex"];

/// One agent's deduped total for the day.
#[derive(Debug, Clone, Serialize)]
pub struct AgentTotal {
    /// App name, e.g. "Claude Code" / "Codex".
    pub app: String,
    /// Union seconds across that agent's sessions today (overlap counted once).
    pub total_s: i64,
}

/// The `/api/coding-agents` payload: the day, the cross-agent union total, and
/// the per-agent breakdown (descending, agents with zero time dropped).
#[derive(Debug, Clone, Serialize)]
pub struct CodingAgentsResponse {
    /// Local day, "YYYY-MM-DD".
    pub date: String,
    /// Union across ALL coding-agent sessions (overlap deduped).
    pub total_s: i64,
    pub agents: Vec<AgentTotal>,
}

/// One `app_sessions` row → an [`Interval`]; a missing `ended_at` becomes an
/// empty string that `parse_ms` rejects, so the row drops out of the union
/// (matching the route, where an undefined end parses to NaN and is filtered).
fn iv(started_at: &str, ended_at: &Option<String>) -> Interval {
    Interval {
        started_at: started_at.to_string(),
        ended_at: ended_at.clone().unwrap_or_default(),
    }
}

#[tracing::instrument(skip(pool))]
pub async fn get_coding_agents(
    pool: &SqlitePool,
    date: &str,
) -> anyhow::Result<CodingAgentsResponse> {
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as::<_, (String, String, Option<String>)>(
            r#"
            SELECT app_name, started_at, ended_at
            FROM app_sessions
            WHERE claude_session_uuid IS NOT NULL
              AND substr(started_at, 1, 10) = ?
              AND app_name IN ('Claude Code', 'Codex')
            "#,
        )
        .bind(date)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("coding_agents.read.app_sessions"))
        .await
        .context("coding-agents: fetch app_sessions")?;
    tracing::debug!(rows = rows.len(), "coding_agents.read.app_sessions");

    let all: Vec<Interval> = rows.iter().map(|(_, s, e)| iv(s, e)).collect();

    let mut agents: Vec<AgentTotal> = CODING_AGENTS
        .iter()
        .map(|app| {
            let ivs: Vec<Interval> = rows
                .iter()
                .filter(|(a, _, _)| a == app)
                .map(|(_, s, e)| iv(s, e))
                .collect();
            AgentTotal {
                app: (*app).to_string(),
                total_s: union_seconds(&ivs),
            }
        })
        .filter(|a| a.total_s > 0)
        .collect();
    agents.sort_by(|a, b| b.total_s.cmp(&a.total_s));

    let total_s = union_seconds(&all);
    tracing::info!(
        date,
        total_s,
        agents = agents.len(),
        "coding-agents computed"
    );
    Ok(CodingAgentsResponse {
        date: date.to_string(),
        total_s,
        agents,
    })
}
