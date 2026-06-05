// meridian — normalises screenpipe activity into structured app sessions
//
// Source adapters: each coding agent stores conversations in its own on-disk
// format (Claude/Codex append JSONLs, Copilot CLI writes an event log, Cursor
// keeps bubbles in a SQLite KV store). A `SessionSource` normalises one such
// store into `NormRecord` streams keyed by session uuid; everything downstream
// (segmentation, sealing, summarising, classifying) is shared and agent-blind.
//
// The legacy Claude/Codex JSONL path predates this trait and still runs
// through `indexer::candidate_jsonls` + `register_session`; new sources plug
// in here and are swept by `sweep()` from the same indexer tick.

pub mod copilot_cli;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, Utc};
use sqlx::SqlitePool;

use super::indexer::{register_records, Outcome};
use super::jsonl::NormRecord;
use super::segment::parse_iso;

/// Slack (s) on the mtime-vs-stored-ended_at change check — mirrors the
/// indexer's `CHANGE_SLACK_SECS` (clock skew + ISO truncation absorption).
const CHANGE_SLACK_SECS: f64 = 5.0;

/// One discovered session within a source: a stable uuid plus a
/// source-specific locator (a file path for event logs; for DB-backed sources
/// the store path, with `session_uuid` as the in-store key).
#[derive(Debug, Clone)]
pub struct SourceSessionRef {
    pub session_uuid: String,
    pub locator: PathBuf,
}

/// A coding-agent conversation store. All methods may do blocking IO — the
/// sweep calls them inside `spawn_blocking`.
pub trait SessionSource: Send + Sync {
    /// Agent tag stored on segments (drives app_name / session_text_source).
    fn agent(&self) -> &'static str;

    /// Device gate: the store exists on this machine.
    fn present(&self) -> bool;

    /// Sessions whose content moved past the stored endpoint. `endpoints` is
    /// {session_uuid: latest stored ended_at}; a never-seen session follows
    /// the backfill-only-today rule (same as the JSONL indexer).
    fn changed_sessions(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<SourceSessionRef>;

    /// Load + normalise every record of one session, oldest-first.
    fn load(&self, sref: &SourceSessionRef) -> Vec<NormRecord>;
}

/// All registered source adapters (present or not — the sweep gates).
pub fn all_sources() -> Vec<Box<dyn SessionSource>> {
    vec![Box::new(copilot_cli::CopilotCliSource::from_env())]
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

        // Discovery + load are blocking IO → off the async runtime. The
        // source is moved into the task and yields (uuid, records) pairs.
        let eps = endpoints.clone();
        let loaded: Vec<(String, Vec<NormRecord>)> = match tokio::task::spawn_blocking(move || {
            source
                .changed_sessions(&eps, now)
                .into_iter()
                .map(|sref| (sref.session_uuid.clone(), source.load(&sref)))
                .collect()
        })
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(agent, error = %e, "source sweep task panicked");
                failed += 1;
                continue;
            }
        };

        for (uuid, records) in loaded {
            if records.is_empty() {
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

// ──────────────────────── Shared helpers ───────────────────────────────────

/// The JSONL indexer's change-detection rule, reusable by file-backed sources:
/// changed iff mtime moved past the stored `ended_at` (+slack); a never-seen
/// session is a candidate only if touched TODAY (local) — so a fresh DB does
/// not re-index weeks of history.
pub(crate) fn file_is_candidate(
    mtime: SystemTime,
    stored_end: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    let mtime_epoch = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    match stored_end {
        Some(end_iso) => match parse_iso(end_iso) {
            Some(end) => {
                let end_epoch = end.timestamp_millis() as f64 / 1000.0;
                mtime_epoch > end_epoch + CHANGE_SLACK_SECS
            }
            None => true, // unparseable endpoint → re-register to repair
        },
        None => {
            let mdate = DateTime::<Utc>::from(mtime)
                .with_timezone(&Local)
                .date_naive();
            mdate == now.with_timezone(&Local).date_naive()
        }
    }
}
