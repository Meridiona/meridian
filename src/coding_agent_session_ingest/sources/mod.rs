// meridian — normalises screenpipe activity into structured app sessions
//
// Source adapters: each coding agent stores conversations in its own on-disk
// format (Claude/Codex append JSONLs, Copilot CLI writes an event log, Cursor
// keeps bubbles in a SQLite KV store). Each `AgentSource` normalises one such
// store into `NormRecord` streams keyed by session uuid; everything downstream
// (segmentation, sealing, summarising, classifying) is shared and agent-blind.
//
// Dispatch is an enum, not a trait object: file-backed sources do blocking IO
// (wrapped in spawn_blocking), DB-backed sources (Cursor's vscdb) need async
// sqlx — an enum lets each variant own its IO model without `dyn`-compatible
// async gymnastics or a second SQLite dependency.
//
// The legacy Claude/Codex JSONL path predates this seam and still runs
// through `indexer::candidate_jsonls` + `register_session`; new sources plug
// in here and are swept by `sweep()` from the same indexer tick.

pub mod copilot_cli;
pub mod cursor;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, Utc};
use sqlx::SqlitePool;

use super::indexer::{register_records, Outcome};
use super::jsonl::NormRecord;
use super::segment::parse_iso;

/// Slack (s) on the mtime-vs-stored-ended_at change check — mirrors the
/// indexer's `CHANGE_SLACK_SECS` (clock skew + ISO truncation absorption).
const CHANGE_SLACK_SECS: f64 = 5.0;

/// One coding-agent conversation store.
pub enum AgentSource {
    CopilotCli(copilot_cli::CopilotCliSource),
    Cursor(cursor::CursorSource),
}

impl AgentSource {
    /// Agent tag stored on segments (drives app_name / session_text_source).
    pub fn agent(&self) -> &'static str {
        match self {
            AgentSource::CopilotCli(_) => copilot_cli::AGENT,
            AgentSource::Cursor(_) => cursor::AGENT,
        }
    }

    /// Device gate: the store exists on this machine.
    pub fn present(&self) -> bool {
        match self {
            AgentSource::CopilotCli(s) => s.present(),
            AgentSource::Cursor(s) => s.present(),
        }
    }

    /// Discover sessions whose content moved past the stored endpoint and
    /// load their full record streams, oldest-changed first. `endpoints` is
    /// {session_uuid: latest stored ended_at}; a never-seen session follows
    /// the backfill-only-today rule (same as the JSONL indexer).
    pub async fn collect_changed(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<(String, Vec<NormRecord>)> {
        match self {
            // File-backed: blocking IO off the async runtime.
            AgentSource::CopilotCli(s) => {
                let src = s.clone();
                let eps = endpoints.clone();
                tokio::task::spawn_blocking(move || {
                    src.changed_sessions(&eps, now)
                        .into_iter()
                        .map(|(uuid, path)| {
                            let records = copilot_cli::parse_events_jsonl(&path);
                            (uuid, records)
                        })
                        .collect()
                })
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "copilot_cli collect task panicked");
                    Vec::new()
                })
            }
            // DB-backed: async sqlx against the (read-only) vscdb.
            AgentSource::Cursor(s) => s.collect_changed(endpoints, now).await,
        }
    }
}

/// All registered source adapters (present or not — the sweep gates).
pub fn all_sources() -> Vec<AgentSource> {
    vec![
        AgentSource::CopilotCli(copilot_cli::CopilotCliSource::from_env()),
        AgentSource::Cursor(cursor::CursorSource::from_env()),
    ]
}

/// True if any source adapter has data on this device (extends the indexer's
/// claude/codex device gate).
pub fn any_source_present() -> bool {
    all_sources().iter().any(|s| s.present())
}

/// One sweep across every present source: discover changed sessions, load,
/// register. Returns (sessions written, sessions failed).
pub async fn sweep(pool: &SqlitePool, now: DateTime<Utc>) -> (u64, u64) {
    let mut wrote = 0_u64;
    let mut failed = 0_u64;

    let endpoints = match super::db::fetch_session_endpoints(pool).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "source sweep: fetch endpoints failed");
            return (0, 0);
        }
    };

    for source in all_sources() {
        if !source.present() {
            continue;
        }
        let agent = source.agent();
        let loaded = source.collect_changed(&endpoints, now).await;

        for (uuid, records) in loaded {
            if records.is_empty() {
                continue;
            }
            // Trailing non-turn events (Copilot's session.shutdown, late
            // session.info, …) keep the store's change signal ahead of the
            // stored ended_at forever. Without this check every settled
            // session would re-register — and spuriously wake the summariser
            // — on every tick. Only genuinely new TURNS count as a change.
            if !has_new_turns(&records, endpoints.get(&uuid).map(String::as_str)) {
                continue;
            }
            match register_records(pool, &uuid, agent, records, false, now).await {
                Outcome::Inserted => wrote += 1,
                Outcome::Failed => failed += 1,
                Outcome::SkippedEmpty => {}
            }
        }
    }

    if wrote > 0 || failed > 0 {
        tracing::info!(wrote, failed, "coding-agent source sweep");
    }
    (wrote, failed)
}

/// True iff the record stream contains a TURN newer than the stored endpoint
/// (canonical-ISO lexicographic compare). No stored endpoint → any turn is new.
fn has_new_turns(records: &[NormRecord], stored_end: Option<&str>) -> bool {
    let last_turn = records
        .iter()
        .rev()
        .find(|r| r.is_turn && r.timestamp.is_some())
        .and_then(|r| r.timestamp.as_deref());
    match (last_turn, stored_end) {
        (None, _) => false, // no turns at all → nothing to register
        (Some(_), None) => true,
        (Some(lt), Some(end)) => super::segment::norm_iso(lt).as_str() > end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts: Option<&str>, is_turn: bool) -> NormRecord {
        NormRecord {
            timestamp: ts.map(String::from),
            cwd: None,
            is_turn,
            is_user: false,
            is_user_prompt: false,
            role_label: None,
            body: String::new(),
        }
    }

    #[test]
    fn has_new_turns_ignores_trailing_non_turn_events() {
        // Turn at 06:18, then a session.shutdown 3h later. Stored endpoint
        // covers the turn → the shutdown alone must NOT count as a change.
        let records = vec![
            rec(Some("2026-05-29T06:18:55.697Z"), true),
            rec(Some("2026-05-29T09:31:22.703Z"), false),
        ];
        let stored = "2026-05-29T06:18:55.697000+00:00";
        assert!(!has_new_turns(&records, Some(stored)));

        // A turn AFTER the endpoint is a real change.
        let records2 = vec![
            rec(Some("2026-05-29T06:18:55.697Z"), true),
            rec(Some("2026-05-29T09:31:22.703Z"), true),
        ];
        assert!(has_new_turns(&records2, Some(stored)));

        // Never-seen session with any turn → change; turn-less stream → not.
        assert!(has_new_turns(&records, None));
        assert!(!has_new_turns(
            &[rec(Some("2026-05-29T09:31:22.703Z"), false)],
            None
        ));
    }
}

// ──────────────────────── Shared helpers ───────────────────────────────────

/// The JSONL indexer's change-detection rule, reusable by any source with a
/// change timestamp: changed iff it moved past the stored `ended_at` (+slack);
/// a never-seen session is a candidate only if touched TODAY (local) — so a
/// fresh DB does not re-index weeks of history.
pub(crate) fn file_is_candidate(
    mtime: SystemTime,
    stored_end: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    let mtime_epoch = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    epoch_is_candidate(mtime_epoch, stored_end, now)
}

/// Epoch-seconds flavour of `file_is_candidate` (Cursor's lastUpdatedAt is
/// epoch milliseconds, not a file mtime).
pub(crate) fn epoch_is_candidate(
    changed_epoch: f64,
    stored_end: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    match stored_end {
        Some(end_iso) => match parse_iso(end_iso) {
            Some(end) => {
                let end_epoch = end.timestamp_millis() as f64 / 1000.0;
                changed_epoch > end_epoch + CHANGE_SLACK_SECS
            }
            None => true, // unparseable endpoint → re-register to repair
        },
        None => {
            let secs = changed_epoch as i64;
            let nanos = ((changed_epoch - secs as f64) * 1e9) as u32;
            match DateTime::<Utc>::from_timestamp(secs, nanos) {
                Some(changed) => {
                    changed.with_timezone(&Local).date_naive()
                        == now.with_timezone(&Local).date_naive()
                }
                None => false,
            }
        }
    }
}
