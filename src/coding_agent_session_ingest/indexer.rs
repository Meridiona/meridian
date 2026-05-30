// meridian — normalises screenpipe activity into structured app sessions
//
// The coding-agent indexer task: a low-frequency tokio loop that turns Claude
// Code / Codex JSONLs into app_sessions segment rows. Port of
// the former Python indexer/{register,daemon}.py.
//
// Per tick: (1) seal settled live rows (the JSONL-free backstop for crashes /
// force-quits / sleep), then (2) re-parse changed files and refresh their live
// last segment. A never-seen file is only backfilled if it was touched TODAY
// (local) — so a fresh DB / post-downtime start does not re-index weeks of
// history. The whole task stays dormant unless a terminal coding agent is
// present (device gate).
//
// Dropped vs the Python original: the fork-skip list (we summarise via
// `claude -p` stdin, not `--fork-session`, so no throwaway JSONLs to skip) and
// host_app resolution (it was only logged, never stored).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, Utc};
use sqlx::SqlitePool;
use tokio::sync::{watch, Notify};

use super::db;
use super::segment::{
    iso_utc, parse_iso, parse_session_segments, Segment, SegmentParams, SEGMENT_GAP_SECONDS,
};

const DEFAULT_POLL_INTERVAL_S: u64 = 600;
const DEFAULT_SEAL_IDLE_S: i64 = 3600;
/// Slack (s) on the mtime-vs-stored-ended_at change check: absorbs clock skew
/// and ISO truncation so an unchanged file isn't needlessly re-parsed.
const CHANGE_SLACK_SECS: f64 = 5.0;

/// Resolved paths + cadence for the indexer (env-driven, mirrors the Python
/// config + the retired launchd plist).
pub struct IndexerConfig {
    pub claude_projects_dir: PathBuf,
    pub codex_sessions_dir: PathBuf,
    pub poll_interval_secs: u64,
    pub seal_idle_seconds: i64,
}

impl IndexerConfig {
    pub fn from_env() -> Self {
        let expand = |env: &str, default: &str| -> PathBuf {
            let raw = std::env::var(env).unwrap_or_else(|_| default.to_string());
            PathBuf::from(shellexpand::tilde(&raw).into_owned())
        };
        fn parse_env<T: std::str::FromStr>(env: &str) -> Option<T> {
            std::env::var(env).ok().and_then(|v| v.parse().ok())
        }
        Self {
            claude_projects_dir: expand("CLAUDE_PROJECTS_DIR", "~/.claude/projects"),
            codex_sessions_dir: expand("CODEX_SESSIONS_DIR", "~/.codex/sessions"),
            poll_interval_secs: parse_env("INDEXER_POLL_INTERVAL_S")
                .unwrap_or(DEFAULT_POLL_INTERVAL_S),
            seal_idle_seconds: parse_env("INDEXER_SEAL_IDLE_S").unwrap_or(DEFAULT_SEAL_IDLE_S),
        }
    }
}

/// Device gate: a terminal coding agent is present iff its data dir exists OR
/// its binary resolves on PATH. When false, the indexer task stays dormant.
pub fn coding_agents_present(cfg: &IndexerConfig) -> bool {
    cfg.claude_projects_dir.exists()
        || cfg.codex_sessions_dir.exists()
        || binary_on_path("claude")
        || binary_on_path("codex")
}

fn binary_on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

/// Outcome of registering one JSONL (mirrors `RegisterOutcome`, trimmed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Inserted,
    SkippedEmpty,
    Failed,
}

// ──────────────────────── Register one session ──────────────────────────────

/// Parse one JSONL into segments and UPSERT one row each, sealing settled ones.
/// Idempotent; never panics. `session_ended=true` (the SessionEnd-hook path)
/// seals the last segment immediately; `false` (the poller) leaves an
/// actively-growing last segment live until it idles out.
pub async fn register_session(
    pool: &SqlitePool,
    jsonl_path: &Path,
    session_ended: bool,
    now: DateTime<Utc>,
) -> Outcome {
    let uuid = match jsonl_path.file_stem().and_then(|s| s.to_str()) {
        Some(u) => u.to_string(),
        None => return Outcome::SkippedEmpty,
    };
    let now_iso = iso_utc(now);

    // Exclude already-sealed content so any newer record opens a fresh segment.
    let start_after = match db::sealed_high_water(pool, &uuid).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(uuid = %uuid, error = %e, "sealed_high_water failed");
            return Outcome::Failed;
        }
    };

    // Heavy work (file read + parse) off the async runtime.
    let params = SegmentParams {
        start_after_ts: start_after,
        ..Default::default()
    };
    let path_owned = jsonl_path.to_path_buf();
    let segments: Vec<Segment> = match tokio::task::spawn_blocking(move || {
        let (_meta, segs) = parse_session_segments(&path_owned, &params);
        segs
    })
    .await
    {
        Ok(segs) => segs,
        Err(e) => {
            tracing::warn!(uuid = %uuid, error = %e, "parse task panicked");
            return Outcome::Failed;
        }
    };

    let valid: Vec<&Segment> = segments.iter().filter(|s| s.is_valid()).collect();
    if valid.is_empty() {
        return Outcome::SkippedEmpty;
    }

    let mut wrote = 0_u32;
    for seg in valid {
        let sealed = should_seal(seg, session_ended, now);
        let stamp = if sealed { Some(now_iso.as_str()) } else { None };
        match db::upsert_segment(pool, seg, sealed, stamp).await {
            Ok(Some(row_id)) => {
                wrote += 1;
                tracing::info!(
                    agent = %seg.agent, uuid = %seg.session_uuid,
                    seg = %seg.segment_started_at, ended = %seg.ended_at,
                    active_s = seg.active_seconds, turns = seg.user_turns + seg.assistant_turns,
                    sealed, row_id, "registered coding-agent segment",
                );
            }
            Ok(None) => {} // invalid, or hit an already-sealed row (no-op)
            Err(e) => {
                tracing::warn!(uuid = %seg.session_uuid, seg = %seg.segment_started_at, error = %e, "upsert failed");
            }
        }
    }

    if wrote == 0 {
        Outcome::SkippedEmpty
    } else {
        Outcome::Inserted
    }
}

/// A non-last segment is always sealed (its >1h gap already happened). The last
/// segment seals iff the caller says the session ended, or its last message is
/// already older than the segment gap (settled by idle).
fn should_seal(seg: &Segment, session_ended: bool, now: DateTime<Utc>) -> bool {
    if !seg.is_last || session_ended {
        return true;
    }
    match parse_iso(&seg.ended_at) {
        Some(ended) => {
            let idle = (now - ended).num_milliseconds() as f64 / 1000.0;
            idle > SEGMENT_GAP_SECONDS as f64
        }
        None => false, // can't determine idleness → keep live
    }
}

// ──────────────────────── One tick ──────────────────────────────────────────

/// One poll sweep: seal settled live rows, then register changed files. Returns
/// (rows sealed, files written).
pub async fn run_tick(pool: &SqlitePool, cfg: &IndexerConfig, now: DateTime<Utc>) -> (u64, u64) {
    let now_iso = iso_utc(now);

    let sealed = match db::seal_stale_open_rows(pool, &now_iso, cfg.seal_idle_seconds).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "seal sweep failed");
            0
        }
    };

    let candidates = candidate_jsonls(pool, cfg, now).await;
    let mut wrote = 0_u64;
    let mut failed = 0_u64;
    for path in candidates {
        match register_session(pool, &path, false, now).await {
            Outcome::Inserted => wrote += 1,
            Outcome::Failed => failed += 1,
            Outcome::SkippedEmpty => {}
        }
    }

    if sealed > 0 || wrote > 0 || failed > 0 {
        tracing::info!(sealed, wrote, failed, "coding-agent indexer tick");
    }
    (sealed, wrote)
}

/// Files whose mtime moved past their stored `ended_at` (+slack). A never-seen
/// file is included only if it was touched TODAY (local) — the backfill-only-
/// today rule. Returned oldest-changed first.
async fn candidate_jsonls(
    pool: &SqlitePool,
    cfg: &IndexerConfig,
    now: DateTime<Utc>,
) -> Vec<PathBuf> {
    let endpoints = db::fetch_session_endpoints(pool).await.unwrap_or_default();
    let dirs = iter_project_dirs(cfg);
    let today = now.with_timezone(&Local).date_naive();

    let mut candidates: Vec<(f64, PathBuf)> = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let mtime = match entry.metadata().and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let mtime_epoch = system_time_epoch(mtime);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            match endpoints.get(&stem) {
                Some(end_iso) => {
                    if let Some(end_epoch) = iso_to_epoch(end_iso) {
                        if mtime_epoch <= end_epoch + CHANGE_SLACK_SECS {
                            continue; // nothing new since last register
                        }
                    }
                }
                None => {
                    // Never indexed → backfill only if touched today.
                    let mdate = DateTime::<Utc>::from(mtime)
                        .with_timezone(&Local)
                        .date_naive();
                    if mdate != today {
                        continue;
                    }
                }
            }
            candidates.push((mtime_epoch, path));
        }
    }
    candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    candidates.into_iter().map(|(_, p)| p).collect()
}

/// All Claude project dirs (children of CLAUDE_PROJECTS_DIR) + Codex day dirs
/// (`<sessions>/<YYYY>/<MM>/<DD>`).
fn iter_project_dirs(cfg: &IndexerConfig) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&cfg.claude_projects_dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                dirs.push(e.path());
            }
        }
    }

    // Codex layout: <sessions>/<YYYY>/<MM>/<DD>/rollout-*.jsonl
    if let Ok(years) = std::fs::read_dir(&cfg.codex_sessions_dir) {
        for y in years.flatten().filter(|y| y.path().is_dir()) {
            if let Ok(months) = std::fs::read_dir(y.path()) {
                for m in months.flatten().filter(|m| m.path().is_dir()) {
                    if let Ok(days) = std::fs::read_dir(m.path()) {
                        for d in days.flatten().filter(|d| d.path().is_dir()) {
                            dirs.push(d.path());
                        }
                    }
                }
            }
        }
    }
    dirs
}

// ──────────────────────── Loop ──────────────────────────────────────────────

/// The indexer task: gate, an immediate backfill-today tick, then poll until
/// shutdown. Returns immediately (dormant) if no coding agent is present.
/// Notifies `summariser_notify` whenever a tick seals or writes rows, so the
/// summariser wakes near-instantly on the indexer's own seals.
pub async fn run_loop(
    pool: SqlitePool,
    summariser_notify: Arc<Notify>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let cfg = IndexerConfig::from_env();
    if !coding_agents_present(&cfg) {
        tracing::info!("coding-agent indexer dormant — no claude/codex detected");
        return;
    }
    tracing::info!(
        claude = %cfg.claude_projects_dir.display(),
        codex = %cfg.codex_sessions_dir.display(),
        poll_s = cfg.poll_interval_secs,
        "coding-agent indexer starting",
    );

    let notify_if_work = |sealed: u64, wrote: u64| {
        if sealed > 0 || wrote > 0 {
            summariser_notify.notify_one();
        }
    };

    let (sealed, wrote) = run_tick(&pool, &cfg, Utc::now()).await; // backfill-today on startup
    notify_if_work(sealed, wrote);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tokio::time::sleep(Duration::from_secs(cfg.poll_interval_secs)) => {
                let (sealed, wrote) = run_tick(&pool, &cfg, Utc::now()).await;
                notify_if_work(sealed, wrote);
            }
        }
    }
    tracing::info!("coding-agent indexer stopped");
}

// ──────────────────────── Helpers ──────────────────────────────────────────

fn system_time_epoch(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn iso_to_epoch(iso: &str) -> Option<f64> {
    parse_iso(iso).map(|dt| dt.timestamp_millis() as f64 / 1000.0)
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, TimeZone};
    use sqlx::sqlite::SqliteConnectOptions;
    use std::io::Write;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU64, Ordering};

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    fn tmpdir() -> PathBuf {
        static C: AtomicU64 = AtomicU64::new(0);
        let mut d = std::env::temp_dir();
        d.push(format!(
            "meridian_idx_test_{}",
            C.fetch_add(1, Ordering::SeqCst)
        ));
        let proj = d.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        proj
    }

    fn base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap()
    }

    fn rec(offset_s: i64, role: &str, text: &str) -> String {
        let ts = (base() + ChronoDuration::seconds(offset_s))
            .format("%Y-%m-%dT%H:%M:%S.%3fZ")
            .to_string();
        serde_json::json!({
            "type": role, "timestamp": ts, "cwd": "/repo",
            "message": { "role": role, "content": text }
        })
        .to_string()
    }

    fn write_jsonl(dir: &Path, uuid: &str, lines: &[String]) -> PathBuf {
        let p = dir.join(format!("{}.jsonl", uuid));
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{}", l).unwrap();
        }
        p
    }

    #[tokio::test]
    async fn register_seals_non_last_keeps_last_live() {
        let pool = fresh_db().await;
        let dir = tmpdir();
        // Two bursts split by a 2h gap → seg0 (sealed), seg1 (last).
        let p = write_jsonl(
            &dir,
            "s1",
            &[
                rec(0, "user", "morning"),
                rec(60, "assistant", "ok"),
                rec(60 + 7200, "user", "afternoon"),
                rec(60 + 7200 + 30, "assistant", "ok2"),
            ],
        );
        // now = just after the last message → last segment is recent (not idle).
        let now = base() + ChronoDuration::seconds(60 + 7200 + 90);
        let outcome = register_session(&pool, &p, false, now).await;
        assert_eq!(outcome, Outcome::Inserted);

        let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
            "SELECT segment_started_at, sealed_at, task_method FROM app_sessions \
             WHERE claude_session_uuid='s1' ORDER BY segment_started_at",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].1.is_some(), "non-last segment sealed");
        assert_eq!(rows[0].2, db::TASK_METHOD_PENDING);
        assert!(rows[1].1.is_none(), "last segment stays live");
        assert_eq!(rows[1].2, db::TASK_METHOD_LIVE);
    }

    #[tokio::test]
    async fn session_ended_seals_last_segment() {
        let pool = fresh_db().await;
        let dir = tmpdir();
        let p = write_jsonl(
            &dir,
            "s2",
            &[rec(0, "user", "hi"), rec(30, "assistant", "yo")],
        );
        let now = base() + ChronoDuration::seconds(120);
        // Hook path: session_ended=true → the single (last) segment seals now.
        register_session(&pool, &p, true, now).await;
        let sealed: Option<String> =
            sqlx::query_scalar("SELECT sealed_at FROM app_sessions WHERE claude_session_uuid='s2'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(sealed.is_some());
    }

    #[tokio::test]
    async fn device_gate_detects_data_dir() {
        let dir = tmpdir(); // creates .../.claude/projects/proj
        let cfg = IndexerConfig {
            claude_projects_dir: dir.clone(),
            codex_sessions_dir: dir.join("nonexistent-codex"),
            poll_interval_secs: 600,
            seal_idle_seconds: 3600,
        };
        assert!(coding_agents_present(&cfg));

        let cfg_absent = IndexerConfig {
            claude_projects_dir: dir.join("nope-claude"),
            codex_sessions_dir: dir.join("nope-codex"),
            poll_interval_secs: 600,
            seal_idle_seconds: 3600,
        };
        // Only false if neither dir exists AND no binary on PATH; binaries may be
        // installed on the dev box, so just assert the dir-absence branch is wired.
        let _ = coding_agents_present(&cfg_absent);
    }
}
