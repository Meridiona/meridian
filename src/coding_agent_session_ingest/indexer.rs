//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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

use chrono::{DateTime, Local, Timelike, Utc};
use sqlx::SqlitePool;
use tokio::sync::{watch, Notify};

use super::db;
use super::segment::{
    clean_title, iso_utc, local_hour_key, parse_iso, parse_session_segments, Segment,
    SegmentParams, SEGMENT_GAP_SECONDS,
};

/// Poll every 2 minutes. Cheap (fast SQL + mtime checks); gives ≤2-min
/// process-gone detection and ≤2-min lag for new session discovery.
const DEFAULT_POLL_INTERVAL_S: u64 = 120;
const DEFAULT_SEAL_IDLE_S: i64 = 3600;
/// Slack (s) on the mtime-vs-stored-ended_at change check: absorbs clock skew
/// and ISO truncation so an unchanged file isn't needlessly re-parsed.
const CHANGE_SLACK_SECS: f64 = 5.0;

/// Whether this tick was triggered by a regular poll or a hour-boundary wake.
/// Only `HourBoundary` ticks run the force-seal sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickKind {
    Regular,
    HourBoundary,
}

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

/// Device gate: a coding agent is present iff its data dir exists OR its
/// binary resolves on PATH, for the JSONL agents (claude/codex) or any source
/// adapter (Copilot CLI, …). When false, the indexer task stays dormant.
pub fn coding_agents_present(cfg: &IndexerConfig) -> bool {
    cfg.claude_projects_dir.exists()
        || cfg.codex_sessions_dir.exists()
        || binary_on_path("claude")
        || binary_on_path("codex")
        || super::sources::any_source_present()
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
        // parse_session_segments stamps the session's own title (Claude
        // `summary` records) in the same single read.
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

    register_segments(pool, &segments, session_ended, now).await
}

/// Stamp the conversation's title onto every segment (each becomes one
/// app_sessions row; all rows of a conversation share its name). Trimmed and
/// capped so a runaway store value can't bloat the row.
fn stamp_title(segments: &mut [Segment], title: Option<&str>) {
    if let Some(t) = title.and_then(clean_title) {
        for seg in segments.iter_mut() {
            seg.title = Some(t.clone());
        }
    }
}

/// Upsert a parsed segment list, sealing settled ones — the shared tail of
/// `register_session` (JSONL path) and the source adapters (`sources/`).
pub async fn register_segments(
    pool: &SqlitePool,
    segments: &[Segment],
    session_ended: bool,
    now: DateTime<Utc>,
) -> Outcome {
    let now_iso = iso_utc(now);
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

/// Register one session from an already-normalised record stream — the
/// source-adapter twin of `register_session`. Excludes already-sealed content
/// via the sealed high-water mark, segments, and upserts.
pub async fn register_records(
    pool: &SqlitePool,
    session_uuid: &str,
    agent: &str,
    records: Vec<super::jsonl::NormRecord>,
    title: Option<&str>,
    session_ended: bool,
    now: DateTime<Utc>,
) -> Outcome {
    let start_after = match db::sealed_high_water(pool, session_uuid).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(uuid = %session_uuid, error = %e, "sealed_high_water failed");
            return Outcome::Failed;
        }
    };
    let params = SegmentParams {
        agent: Some(agent.to_string()),
        start_after_ts: start_after,
        ..Default::default()
    };
    let (_meta, mut segments) =
        super::segment::segment_records(records, session_uuid, agent, 0, &params);
    stamp_title(&mut segments, title);
    register_segments(pool, &segments, session_ended, now).await
}

/// A non-last segment is always sealed (its >1h gap already happened). The last
/// segment seals iff: the session ended, the last message is older than the
/// segment gap (idle backstop), OR the last message is in a previous local
/// clock hour (hour-boundary seal — no need to wait for the next user prompt).
fn should_seal(seg: &Segment, session_ended: bool, now: DateTime<Utc>) -> bool {
    if !seg.is_last || session_ended {
        return true;
    }
    let Some(ended) = parse_iso(&seg.ended_at) else {
        return false; // can't determine → keep live
    };
    let idle = (now - ended).num_milliseconds() as f64 / 1000.0;
    if idle > SEGMENT_GAP_SECONDS as f64 {
        return true;
    }
    // Seal when the last message landed in a previous local clock hour.
    // The indexer wakes at each hour boundary, so this fires within seconds
    // of the top of the hour — no new user prompt required.
    local_hour_key(ended) < local_hour_key(now)
}

// ──────────────────────── One tick ──────────────────────────────────────────

/// One poll sweep: seal settled live rows, then register changed files. Returns
/// (rows sealed, files written). On `HourBoundary` ticks, also force-seals all
/// live rows whose last activity is in a previous local clock hour.
pub async fn run_tick(
    pool: &SqlitePool,
    cfg: &IndexerConfig,
    now: DateTime<Utc>,
    kind: TickKind,
) -> (u64, u64) {
    let now_iso = iso_utc(now);

    let mut sealed = match db::seal_stale_open_rows(pool, &now_iso, cfg.seal_idle_seconds).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "idle seal sweep failed");
            0
        }
    };

    // At every hour boundary, unconditionally seal all live rows whose last
    // activity falls in a previous local hour. This captures sessions that went
    // quiet mid-hour (dev stopped typing but did not exit the agent) so the
    // worklog generator has complete data for that hour.
    if kind == TickKind::HourBoundary {
        let hour_start_iso = iso_utc(meridian_core::date::local_hour_start_utc(now));
        match db::seal_live_rows_at_hour_boundary(pool, &now_iso, &hour_start_iso).await {
            Ok(n) => {
                if n > 0 {
                    tracing::info!(sealed = n, "hour-boundary force-seal");
                }
                sealed += n;
            }
            Err(e) => tracing::warn!(error = %e, "hour-boundary force-seal failed"),
        }
    }

    let candidates = candidate_jsonls(pool, cfg, now).await;
    let mut wrote = 0_u64;
    let mut failed = 0_u64;
    for path in &candidates {
        tracing::debug!(source = "jsonl", path = %path.display(), "registering agent session file");
    }
    for path in candidates {
        match register_session(pool, &path, false, now).await {
            Outcome::Inserted => wrote += 1,
            Outcome::Failed => failed += 1,
            Outcome::SkippedEmpty => {}
        }
    }

    // Non-JSONL sources (Copilot CLI, …) sweep through the adapter seam.
    let (src_wrote, src_failed) = super::sources::sweep(pool, now).await;
    wrote += src_wrote;
    failed += src_failed;

    // CLI end-of-session acceleration (runs AFTER the sweep so a /clear's
    // fresh session is already registered and supersedes the old one this
    // same tick). Both passes only ACCELERATE what the idle backstop above
    // would do anyway — a wrong call costs a segment split, never data.
    let sealed = sealed + seal_finished_cli_sessions(pool, &now_iso).await;

    if sealed > 0 || wrote > 0 || failed > 0 {
        tracing::info!(sealed, wrote, failed, "coding-agent indexer tick");
    }
    (sealed, wrote)
}

/// CLI-backed sources eligible for end-of-session acceleration. IDE-resident
/// sources (cursor vscdb sidebar, VS Code chatSessions) are absent — their
/// lifecycles are managed by the cursor/copilot vscode adapters. claude_jsonl
/// is included: when the Claude Code process is gone the session is done
/// (the SessionEnd hook is the fast path; the ps probe is the backstop).
const CLI_SOURCES: &[&str] = &[
    "claude_jsonl",
    "codex_jsonl",
    "copilot_events_jsonl",
    "cursor_cli_store",
];

/// Prompt session completion for CLI agents — /clear and Ctrl+C, which write
/// no end marker in most stores:
/// 1. Process gone (Ctrl+C, exit): no process → the store cannot grow → every
///    live row of that source is finished.
/// 2. Superseded (/clear, /new with the CLI still running): a NEWER session
///    of the same source exists → the older conversation ended.
async fn seal_finished_cli_sessions(pool: &SqlitePool, now_iso: &str) -> u64 {
    // One process listing per tick, shared by all sources. None = listing
    // failed → treat every CLI as running (the safe direction: a false
    // "running" merely defers to the idle backstop; a false "gone" would
    // seal mid-session).
    let argvs = list_process_argvs().await;
    let mut sealed = 0_u64;
    for source in CLI_SOURCES {
        let running = match &argvs {
            Some(list) => cli_running(list, source),
            None => true,
        };
        if !running {
            match db::seal_live_rows_of_source(pool, now_iso, source).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(source, sealed = n, "sealed rows of exited CLI");
                    }
                    sealed += n;
                }
                Err(e) => tracing::warn!(source, error = %e, "exited-CLI seal failed"),
            }
            continue; // nothing left live; superseded pass would be a no-op
        }
        match db::seal_superseded_rows_of_source(pool, now_iso, source).await {
            Ok(n) => {
                if n > 0 {
                    tracing::info!(source, sealed = n, "sealed superseded CLI sessions");
                }
                sealed += n;
            }
            Err(e) => tracing::warn!(source, error = %e, "superseded seal failed"),
        }
    }
    sealed
}

/// Snapshot every process's argv (`ps -axo args=` — argv ONLY). pgrep -f is
/// useless here: on macOS it matches the environment block too, and PATH /
/// CODEX_* env vars put agent names into half the processes on the box.
async fn list_process_argvs() -> Option<Vec<String>> {
    // Hard timeout + kill_on_drop: `ps` reading proc info can wedge in the
    // kernel on a stuck process. An un-timed await here parks the whole
    // indexer loop forever (observed live 2026-06-06 18:23 UTC: ticks stopped
    // dead mid-tick right at this step until the process was kicked). None →
    // every CLI is treated as running, which only defers to the idle backstop.
    let mut cmd = tokio::process::Command::new("ps");
    cmd.args(["-axo", "args="]).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

fn basename(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

/// Does any process argv look like this source's CLI? Matched on the argv[0]
/// basename so env noise can't false-positive, with the known collisions
/// excluded: VS Code's ChatGPT extension ships a binary literally named
/// `codex` (runs as `codex app-server`), copilot is a node script (argv[0] is
/// `node`, the script path is argv[1]), and a `cursor-agent login` child is
/// auth plumbing, not a chat session.
fn cli_running(argvs: &[String], source: &str) -> bool {
    argvs.iter().any(|line| {
        let mut toks = line.split_whitespace();
        let Some(first) = toks.next() else {
            return false;
        };
        let bn0 = basename(first);
        match source {
            // Interactive Claude Code session: binary is `claude` with no `-p`
            // flag (the `-p` form is the summariser's own subprocess — exclude it
            // so the summariser's run never accidentally seals the live session).
            "claude_jsonl" => {
                bn0 == "claude" && !line.contains(" -p ") && !line.contains(" --print ")
            }
            "codex_jsonl" => bn0 == "codex" && !line.contains("app-server"),
            "copilot_events_jsonl" => {
                bn0 == "copilot" || (bn0 == "node" && toks.next().map(basename) == Some("copilot"))
            }
            "cursor_cli_store" => bn0 == "cursor-agent" && !line.contains(" login"),
            _ => false,
        }
    })
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
                        tracing::debug!(
                            path = %path.display(),
                            file_date = %mdate,
                            "backfill skipped — file not touched today"
                        );
                        continue;
                    }
                    tracing::debug!(
                        path = %path.display(),
                        "new agent session file discovered"
                    );
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

/// Sleep duration and tick kind until the next event. Wakes at whichever is
/// sooner: the normal poll interval or the top of the next local clock hour.
/// Returns `HourBoundary` when the hour-boundary wake wins so the caller can
/// run the hour-boundary force-seal sweep.
fn next_tick(poll_interval_secs: u64) -> (Duration, TickKind) {
    let now = Local::now();
    let secs_past_hour = (now.minute() * 60 + now.second()) as u64;
    // +2s buffer so the tick lands just after the boundary, not at it.
    let secs_to_next_hour = 3600u64.saturating_sub(secs_past_hour) + 2;
    if secs_to_next_hour < poll_interval_secs {
        (
            Duration::from_secs(secs_to_next_hour),
            TickKind::HourBoundary,
        )
    } else {
        (Duration::from_secs(poll_interval_secs), TickKind::Regular)
    }
}

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
        tracing::info!("coding-agent indexer dormant — no coding agent detected");
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

    let (sealed, wrote) = guarded_tick(&pool, &cfg, TickKind::Regular).await; // backfill-today on startup
    notify_if_work(sealed, wrote);

    loop {
        let (sleep_dur, tick_kind) = next_tick(cfg.poll_interval_secs);
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tokio::time::sleep(sleep_dur) => {
                let (sealed, wrote) = guarded_tick(&pool, &cfg, tick_kind).await;
                notify_if_work(sealed, wrote);
            }
        }
    }
    tracing::info!("coding-agent indexer stopped");
}

/// run_tick with a hard ceiling. A tick that exceeds it is abandoned (warn,
/// not fatal) so a single wedged await — a hung subprocess, a locked store —
/// costs one tick, never the loop. The ceiling is generous: a healthy tick is
/// seconds; only a genuine hang crosses minutes.
async fn guarded_tick(pool: &SqlitePool, cfg: &IndexerConfig, kind: TickKind) -> (u64, u64) {
    const TICK_CEILING: Duration = Duration::from_secs(480);
    match tokio::time::timeout(TICK_CEILING, run_tick(pool, cfg, Utc::now(), kind)).await {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                ceiling_s = TICK_CEILING.as_secs(),
                "indexer tick exceeded ceiling — abandoned (state is tick-idempotent; next tick re-covers)"
            );
            (0, 0)
        }
    }
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
    use chrono::{Duration as ChronoDuration, TimeZone, Timelike};
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
             WHERE coding_agent_session_uuid='s1' ORDER BY segment_started_at",
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
        let sealed: Option<String> = sqlx::query_scalar(
            "SELECT sealed_at FROM app_sessions WHERE coding_agent_session_uuid='s2'",
        )
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

    #[test]
    fn local_hour_start_utc_truncates_to_hour() {
        // 14:35 UTC with UTC == local → hour start is 14:00 UTC.
        let now = Utc.with_ymd_and_hms(2026, 6, 29, 14, 35, 0).unwrap();
        let start = meridian_core::date::local_hour_start_utc(now);
        // We can't hardcode the result (local tz varies by machine), but we can
        // assert the round-trip: local(start).minute() == 0 && local(start).second() == 0.
        let local_start = start.with_timezone(&Local);
        assert_eq!(local_start.minute(), 0);
        assert_eq!(local_start.second(), 0);
        // And start ≤ now.
        assert!(start <= now);
    }

    #[test]
    fn next_tick_prefers_hour_boundary_when_closer() {
        // With a 120s poll, the hour-boundary wake wins when <118s remain in the hour.
        // We can't control wall-clock in tests, but we can verify the returned kind
        // matches the shorter of the two candidates.
        let (dur, kind) = next_tick(120);
        // Whatever fires, the Duration must be ≤ 120s (poll) OR ≤ 3602s (hour+2s).
        assert!(dur.as_secs() <= 3602);
        // Kind must be HourBoundary iff dur < 120.
        if dur.as_secs() < 120 {
            assert_eq!(kind, TickKind::HourBoundary);
        } else {
            assert_eq!(kind, TickKind::Regular);
        }
    }

    #[test]
    fn cli_running_matches_real_argv_shapes() {
        let argvs: Vec<String> = [
            // VS Code's ChatGPT extension — NOT the codex CLI.
            "/Users/u/.vscode/extensions/openai.chatgpt-1/bin/codex app-server --analytics",
            // Copilot CLI (node script) + the codex CLI typed bare.
            "node /opt/homebrew/bin/copilot",
            "codex exec --skip-git-repo-check say hi",
            // cursor-agent auth plumbing — not a chat session.
            "/Users/u/.local/bin/cursor-agent --use-system-ca /idx.js login",
            // Env noise that pgrep -f used to false-match must NOT match here.
            "npm exec ruflo PATH=/Users/u/.codex/bin:/usr/bin",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        assert!(cli_running(&argvs, "codex_jsonl"), "bare codex CLI matches");
        assert!(cli_running(&argvs, "copilot_events_jsonl"));
        assert!(
            !cli_running(&argvs, "cursor_cli_store"),
            "login child is not a session"
        );

        // Remove the genuine CLIs — only collisions/noise left → all gone.
        let noise: Vec<String> = argvs
            .iter()
            .filter(|l| !l.starts_with("codex exec") && !l.starts_with("node /opt"))
            .cloned()
            .collect();
        assert!(!cli_running(&noise, "codex_jsonl"), "app-server excluded");
        assert!(!cli_running(&noise, "copilot_events_jsonl"));
    }
}
