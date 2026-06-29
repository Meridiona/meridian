//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Summariser: turn each sealed coding-agent segment into a factual prose summary
// for the PM work-log (task_method → 'summarised'). The agno worklog workflow
// picks up summarised rows by querying session_summary IS NOT NULL directly.
//
// Engine routing per segment: Codex sessions → `codex exec`, else → `claude -p`
// (both Rust subprocesses on the user's subscription). Each primary engine is
// tried up to `primary_attempts` times; a rate-limit short-circuits straight to
// the local MLX server (`/summarise`, the only remaining Python hop). Sequential
// (one transcript in flight) keeps memory flat and avoids bursting rate limits.
//
// Cadence: woken in-process by the indexer's own seals (near-instant) plus a
// short catch-up sweep for hook-sealed rows — no listener (local-only rule).

pub mod claude;
pub mod codex;
pub mod config;
pub mod copilot;
pub mod cursor_agent;
pub mod db;
pub mod mlx;
pub mod prompts;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{watch, Notify};
use tracing::Instrument;

use config::SummariserConfig;
use db::PendingRow;

// ──────────────────────── Errors / engine output ────────────────────────────

/// A summariser engine failure. `RateLimited` means switch to MLX now;
/// `Failed` is anything else (retry the primary, then MLX).
#[derive(Debug, Clone)]
pub enum SummariserError {
    RateLimited(String),
    Failed(String),
}

impl fmt::Display for SummariserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SummariserError::RateLimited(m) => write!(f, "rate-limited: {m}"),
            SummariserError::Failed(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for SummariserError {}

/// The validated output of a primary (claude/codex) engine.
pub struct EngineOutput {
    pub summary: String,
}

/// Captured result of a subprocess run.
pub(super) struct Capture {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Spawn `program args`, feed `stdin_text`, capture stdout/stderr with a hard
/// timeout. `kill_on_drop` guarantees a timed-out child is reaped (no leak);
/// stdin is written from a concurrent task so a large prompt can't deadlock the
/// pipe. Summariser stdout is small (a JSON envelope), so no read-side deadlock.
pub(super) async fn run_capture(
    program: &str,
    args: &[String],
    stdin_text: &str,
    cwd: &Path,
    timeout_s: u64,
    extra_env: &[(&str, &str)],
    remove_env: &[&str],
) -> Result<Capture, SummariserError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(cwd)
        .kill_on_drop(true);
    for k in remove_env {
        cmd.env_remove(k);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SummariserError::Failed(format!("{program} CLI not found on PATH"))
        } else {
            SummariserError::Failed(format!("{program} spawn failed: {e}"))
        }
    })?;

    if let Some(mut sin) = child.stdin.take() {
        let input = stdin_text.to_string();
        tokio::spawn(async move {
            let _ = sin.write_all(input.as_bytes()).await;
            let _ = sin.shutdown().await;
        });
    }

    let output = match tokio::time::timeout(
        Duration::from_secs(timeout_s),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(SummariserError::Failed(format!("{program}: {e}"))),
        Err(_) => {
            return Err(SummariserError::Failed(format!(
                "{program} timed out after {timeout_s}s"
            )))
        }
    };

    Ok(Capture {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// ──────────────────────── One unit of work ──────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
    Copilot,
    CursorAgent,
    Mlx,
    None,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::Copilot => "copilot",
            Source::CursorAgent => "cursor",
            Source::Mlx => "mlx",
            Source::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub row_id: i64,
    pub written: bool,
    pub source: Source,
    pub rate_limited: bool,
    pub error: Option<String>,
    pub summary: Option<String>,
}

/// Produce (and by default persist) a summary for one segment. Never panics —
/// returns an Outcome so a bad row can't kill the drain loop.
pub async fn summarise_one(
    pool: &SqlitePool,
    row: &PendingRow,
    cfg: &SummariserConfig,
    write: bool,
) -> Outcome {
    // One span per sealed segment — exported to OpenObserve via the tracing→OTel
    // bridge. The `prior_*` attributes make prior-burst continuity (a resumed
    // session reading as one story) a first-class, queryable signal: whether the
    // model received the earlier burst's summary as context, and how much.
    let span = tracing::info_span!(
        "summarise_segment",
        row_id = row.id,
        session_uuid = %row.session_uuid,
        agent = %row.agent,
        prior_present = tracing::field::Empty,
        prior_chars = tracing::field::Empty,
        transcript_chars = tracing::field::Empty,
        prompt_chars = tracing::field::Empty,
        summary_source = tracing::field::Empty,
        summary_chars = tracing::field::Empty,
        written = tracing::field::Empty,
        is_error = tracing::field::Empty,
    );
    async move { summarise_one_inner(pool, row, cfg, write).await }
        .instrument(span)
        .await
}

async fn summarise_one_inner(
    pool: &SqlitePool,
    row: &PendingRow,
    cfg: &SummariserConfig,
    write: bool,
) -> Outcome {
    let span = tracing::Span::current();
    let err = |row_id, e: String| Outcome {
        row_id,
        written: false,
        source: Source::None,
        rate_limited: false,
        error: Some(e),
        summary: None,
    };

    let transcript = match db::fetch_transcript(pool, row.id).await {
        Ok(t) => t,
        Err(e) => {
            span.record("is_error", true);
            return err(row.id, format!("fetch transcript: {e}"));
        }
    };
    if transcript.trim().is_empty() {
        span.record("is_error", true);
        return err(row.id, "empty transcript".into());
    }
    let prior = db::fetch_prior_summary(pool, &row.session_uuid, &row.segment_started_at)
        .await
        .unwrap_or(None);
    let stdin_text = build_prompt(&transcript, prior.as_deref(), cfg.transcript_cap_chars);

    // Continuity telemetry: record the prior-burst context on the span and emit a
    // dedicated log line when it was applied, so a resumed-session summary is
    // distinguishable from a fresh-burst one in both Traces and Logs.
    let prior_chars = prior.as_deref().map(str::len).unwrap_or(0) as i64;
    span.record("prior_present", prior.is_some());
    span.record("prior_chars", prior_chars);
    span.record("transcript_chars", transcript.len() as i64);
    span.record("prompt_chars", stdin_text.len() as i64);
    if prior.is_some() {
        tracing::info!(
            row_id = row.id,
            prior_chars,
            "summarising coding-agent segment with prior-burst continuity context"
        );
    }

    // Debug child span: the EXACT prompt sent to the engine (post-cap, with the
    // prior-burst context already inlined). `llm_input` is an OpenObserve FTS key,
    // so a questionable summary can be traced straight back to what the model
    // actually saw. Mirrors the classifier's `classifier_input` span.
    tracing::info_span!(
        "summariser_prompt",
        llm_input = %stdin_text,
        prior_present = prior.is_some(),
        prior_chars = prior_chars,
        transcript_chars = transcript.len() as i64,
        prompt_chars = stdin_text.len() as i64,
    )
    .in_scope(|| {});

    let mut errors: Vec<String> = Vec::new();

    // 1. Primary: the session's own agent, up to `primary_attempts` tries.
    // Each agent's transcripts go to its own CLI (codex→codex, copilot→copilot,
    // cursor→cursor-agent, claude/unknown→claude); MLX is the shared fallback
    // for all of them.
    let agent = row.agent.trim();
    let primary_source = if agent.eq_ignore_ascii_case("codex") {
        Source::Codex
    } else if agent.eq_ignore_ascii_case("github copilot") {
        Source::Copilot
    } else if agent.eq_ignore_ascii_case("cursor agent") {
        Source::CursorAgent
    } else {
        Source::Claude
    };

    // Debug child span: the operational story of one summarisation — which engine
    // ran, how many attempts, whether it fell back to MLX, and wall-clock. Mirrors
    // the classifier's `llm_inference` span; the per-attempt warn! logs below
    // attach to it, so a degraded-to-MLX state is never silent.
    let infer_span = tracing::info_span!(
        "summariser_inference",
        primary_engine = primary_source.as_str(),
        engine_used = tracing::field::Empty,
        model = tracing::field::Empty,
        attempts_made = tracing::field::Empty,
        fell_back_to_mlx = tracing::field::Empty,
        rate_limited = tracing::field::Empty,
        elapsed_s = tracing::field::Empty,
        is_error = tracing::field::Empty,
    );
    let t_infer = std::time::Instant::now();
    let (summary, source, rate_limited, attempts_made) = async {
        let mut summary: Option<String> = None;
        let mut source = Source::None;
        let mut rate_limited = false;
        let mut attempts_made: u32 = 0;
        for attempt in 1..=cfg.primary_attempts.max(1) {
            attempts_made = attempt;
            let res = match primary_source {
                Source::Codex => codex::run_codex(&stdin_text, cfg).await,
                Source::Copilot => copilot::run_copilot(&stdin_text, cfg).await,
                Source::CursorAgent => cursor_agent::run_cursor_agent(&stdin_text, cfg).await,
                _ => claude::run_claude(&stdin_text, cfg).await,
            };
            match res {
                Ok(out) => {
                    summary = Some(out.summary);
                    source = primary_source;
                    break;
                }
                Err(SummariserError::RateLimited(m)) => {
                    rate_limited = true;
                    // Log the primary failure even though MLX will likely save the
                    // row — otherwise a degraded-to-MLX state is invisible (exactly
                    // how the missing-PATH outage hid: every row silently → mlx).
                    tracing::warn!(
                        row_id = row.id,
                        engine = primary_source.as_str(),
                        error = %m,
                        "primary summariser rate-limited — falling back to MLX"
                    );
                    errors.push(format!("{} rate-limited: {m}", primary_source.as_str()));
                    break; // retrying a limit is pointless → fall through to MLX
                }
                Err(SummariserError::Failed(m)) => {
                    tracing::warn!(
                        row_id = row.id,
                        engine = primary_source.as_str(),
                        attempt,
                        error = %m,
                        "primary summariser attempt failed"
                    );
                    errors.push(format!(
                        "{} attempt {attempt} failed: {m}",
                        primary_source.as_str()
                    ));
                }
            }
        }

        // 2. Fallback: local MLX (on any primary failure).
        if summary.is_none() {
            match mlx::run_mlx(&stdin_text, cfg).await {
                Ok(s) => {
                    tracing::warn!(
                        row_id = row.id,
                        primary = primary_source.as_str(),
                        "summarised via MLX fallback — primary engine unavailable"
                    );
                    summary = Some(s);
                    source = Source::Mlx;
                }
                Err(e) => errors.push(format!("mlx failed: {e}")),
            }
        }
        (summary, source, rate_limited, attempts_made)
    }
    .instrument(infer_span.clone())
    .await;
    // Which concrete model produced this summary — the configured model for the
    // engine that actually ran (empty config → that CLI's own default).
    let model_used = match source {
        Source::Claude => cfg.claude_model.clone(),
        Source::Codex if !cfg.codex_model.is_empty() => cfg.codex_model.clone(),
        Source::Codex => "codex-default".into(),
        Source::CursorAgent if !cfg.cursor_model.is_empty() => cfg.cursor_model.clone(),
        Source::CursorAgent => "cursor-agent-default".into(),
        Source::Copilot => "copilot-default".into(),
        Source::Mlx => "mlx-server".into(),
        Source::None => String::new(),
    };
    infer_span.record("engine_used", source.as_str());
    infer_span.record("model", model_used.as_str());
    infer_span.record("attempts_made", attempts_made as i64);
    infer_span.record("fell_back_to_mlx", matches!(source, Source::Mlx));
    infer_span.record("rate_limited", rate_limited);
    infer_span.record("elapsed_s", t_infer.elapsed().as_secs_f64());
    infer_span.record("is_error", summary.is_none());

    let summary = match summary {
        Some(s) => s,
        None => {
            span.record("is_error", true);
            span.record("summary_source", Source::None.as_str());
            return Outcome {
                row_id: row.id,
                written: false,
                source: Source::None,
                rate_limited,
                error: Some(errors.join("; ")),
                summary: None,
            };
        }
    };

    // Debug child span: the EXACT summary produced (`llm_output`, FTS-indexed).
    // Pairs with `summariser_prompt` so the full input→output of one summarisation
    // is reconstructable from the trace. Mirrors the classifier's
    // `classifier_output` span.
    tracing::info_span!(
        "summariser_output",
        llm_output = %summary,
        summary_source = source.as_str(),
        summary_chars = summary.len() as i64,
    )
    .in_scope(|| {});

    let written = if write {
        db::write_summary(pool, row.id, &summary, source.as_str())
            .await
            .unwrap_or(false)
    } else {
        false
    };
    span.record("summary_source", source.as_str());
    span.record("summary_chars", summary.len() as i64);
    span.record("written", written);
    span.record("is_error", false);
    let uuid_short: String = row.session_uuid.chars().take(8).collect();
    tracing::info!(
        row_id = row.id, uuid = %uuid_short, source = source.as_str(),
        written, chars = summary.len(), "summarised coding-agent segment",
    );
    Outcome {
        row_id: row.id,
        written,
        source,
        rate_limited,
        error: None,
        summary: Some(summary),
    }
}

/// stdin for the model: (capped) transcript + optional prior-burst context below.
/// Prior context is placed AFTER the transcript so the model reads the current
/// burst first; the earlier summary is there purely to give continuity context
/// (same session, earlier hour) and should not be repeated in the output.
fn build_prompt(transcript: &str, prior: Option<&str>, cap: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "## TRANSCRIPT\n{}",
        cap_transcript(transcript, cap)
    ));
    if let Some(p) = prior {
        parts.push(format!(
            "## EARLIER IN THIS SESSION (provided for context — this is the same session continued from a previous hour; do not repeat or summarise this section)\n{p}"
        ));
    }
    parts.join("\n\n")
}

/// Bound transcript size: keep the head (task setup) and tail (outcome). Most
/// bursts pass through untouched. Char-counted to match the Python original.
/// Also used by copilot.rs to re-cap for argv embedding (no stdin support).
fn cap_transcript(transcript: &str, cap: usize) -> String {
    let chars: Vec<char> = transcript.chars().collect();
    if chars.len() <= cap {
        return transcript.to_string();
    }
    let head_len = cap * 7 / 10;
    let tail_len = cap - head_len;
    let elided = chars.len() - cap;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    format!("{head}\n\n…[{elided} chars elided — long autonomous stretch omitted]…\n\n{tail}")
}

// ──────────────────────── Loop ──────────────────────────────────────────────

/// The summariser task: drain the queue, then wait for an indexer notify or the
/// catch-up sweep. Dormant if no coding agent is present. Backs off when both
/// the primary engine and MLX are unavailable.
pub async fn run_loop(
    pool: SqlitePool,
    notify: Arc<Notify>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    use super::indexer::{coding_agents_present, IndexerConfig};
    if !coding_agents_present(&IndexerConfig::from_env()) {
        tracing::info!("coding-agent summariser dormant — no coding agent detected");
        return;
    }
    let cfg = SummariserConfig::from_env();
    if let Err(e) = db::ensure_summary_source_column(&pool).await {
        tracing::warn!(error = %e, "summariser: could not ensure summary_source column");
    }
    tracing::info!(
        sweep_s = cfg.sweep_interval_secs,
        batch = cfg.batch_per_tick,
        "coding-agent summariser starting"
    );

    // Per-row failure ledger for dead-letter cap (see MAX_ROW_ATTEMPTS).
    let mut attempts: HashMap<i64, u32> = HashMap::new();
    // Per-source rate-limit backoff: tracks when each agent source's primary
    // engine is available again. Keyed on app_name ("Claude Code", "Codex", …).
    // Only rows whose source is in backoff are skipped; other sources continue.
    let mut source_backoff: HashMap<String, std::time::Instant> = HashMap::new();
    loop {
        let all_backed_off = drain(&pool, &cfg, &mut attempts, &mut source_backoff).await;
        let wait = if all_backed_off {
            // Nothing could be processed (all pending rows are from backed-off
            // sources). Wait out the backoff period rather than spinning.
            cfg.rate_limit_backoff_secs
        } else {
            cfg.sweep_interval_secs
        };
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
    tracing::info!("coding-agent summariser stopped");
}

/// Per-daemon-lifetime failure ledger: a row that fails this many drain
/// passes is dead-lettered (skipped with a warn) instead of retried forever.
/// The churn this prevents was observed live 2026-06-07: rows whose capped
/// prompt exceeds claude's 200k context AND whose MLX answer is empty cycled
/// every drain, each burning 2 claude calls + 1 MLX call, indefinitely. The
/// ledger is in-memory by design: a daemon restart (or `--day` backfill after
/// fixing the engine) retries cleanly.
const MAX_ROW_ATTEMPTS: u32 = 3;

/// One drain pass: summarise pending rows from a bounded recent window
/// (yesterday + today), oldest-first. Returns true only when all available
/// rows belong to rate-limited sources (no progress was possible this pass),
/// signalling the caller to wait longer rather than spinning.
///
/// Rate-limit backoff is per-source (keyed on `app_name`): if Claude Code is
/// rate-limited, Codex / Cursor / Copilot rows continue draining via MLX or
/// their own primary. The old global 30-minute freeze is gone.
///
/// Why the yesterday+today window: today-only strands rows sealed just before
/// midnight; all-days walks the full historical backlog. Yesterday+today
/// catches the rollover without the churn. Older days remain an explicit
/// operator action: `meridian coding-agent-summarise --day <YYYY-MM-DD>`.
async fn drain(
    pool: &SqlitePool,
    cfg: &SummariserConfig,
    attempts: &mut HashMap<i64, u32>,
    source_backoff: &mut HashMap<String, std::time::Instant>,
) -> bool {
    // Expire stale backoffs before deciding what to skip.
    let now_instant = std::time::Instant::now();
    source_backoff.retain(|_, until| *until > now_instant);

    let now = Utc::now();
    let days = [
        (now - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string(),
        now.format("%Y-%m-%d").to_string(),
    ];
    let rows = match db::fetch_pending(pool, cfg, cfg.batch_per_tick, &days).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "summariser: fetch_pending failed");
            return false;
        }
    };

    if rows.is_empty() {
        return false;
    }

    let mut summarised = 0u32;
    let mut skipped_backoff = 0u32;

    for row in &rows {
        // Skip rows whose agent source is still in rate-limit backoff.
        if source_backoff.contains_key(&row.agent) {
            skipped_backoff += 1;
            continue;
        }

        let tries = attempts.get(&row.id).copied().unwrap_or(0);
        if tries >= MAX_ROW_ATTEMPTS {
            continue; // dead-lettered this daemon lifetime
        }

        let outcome = summarise_one(pool, row, cfg, true).await;
        if outcome.written {
            summarised += 1;
        } else if outcome.rate_limited {
            // Apply per-source backoff so other sources can still drain.
            let until =
                std::time::Instant::now() + Duration::from_secs(cfg.rate_limit_backoff_secs);
            source_backoff.insert(row.agent.clone(), until);
            skipped_backoff += 1;
            tracing::warn!(
                row_id = outcome.row_id,
                source = %row.agent,
                backoff_s = cfg.rate_limit_backoff_secs,
                "primary summariser rate-limited — backing off this source, other sources continue"
            );
        } else {
            // Transient failure. Leave pending for retry; log so it isn't silent.
            let tries = tries + 1;
            attempts.insert(row.id, tries);
            if tries >= MAX_ROW_ATTEMPTS {
                if let Err(e) = db::write_dead_letter(pool, row.id).await {
                    tracing::error!(row_id = row.id, error = %e, "failed to dead-letter row");
                }
                tracing::warn!(
                    row_id = outcome.row_id,
                    error = outcome.error.as_deref().unwrap_or("unknown"),
                    attempts = tries,
                    "summarise failed repeatedly — dead-lettering (restart or `coding-agent-summarise --day` retries)"
                );
            } else {
                tracing::warn!(
                    row_id = outcome.row_id,
                    error = outcome.error.as_deref().unwrap_or("unknown"),
                    attempts = tries,
                    "summarise failed — leaving pending for retry"
                );
            }
        }
    }

    if summarised > 0 {
        tracing::info!(summarised, "summariser drain");
    }

    // Signal global backoff only when ALL rows were skipped due to source
    // backoffs — nothing useful can be done until at least one source recovers.
    skipped_backoff > 0 && skipped_backoff == rows.len() as u32 && summarised == 0
}

/// One-shot CLI: `meridian coding-agent-summarise [--dry-run] [--day D] [--limit N]`.
/// Summarise (or dry-run) the pending queue for a day — manual backfill / eval.
pub async fn cli_summarise(pool: &SqlitePool, dry_run: bool, day: Option<&str>, limit: i64) {
    let cfg = SummariserConfig::from_env();
    if let Err(e) = db::ensure_summary_source_column(pool).await {
        eprintln!("summarise: ensure column: {e}");
        return;
    }
    let day = day
        .map(str::to_string)
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    let rows = match db::fetch_pending(pool, &cfg, limit, std::slice::from_ref(&day)).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("summarise: fetch_pending: {e}");
            return;
        }
    };
    println!(
        "summarise: {} pending row(s) for {day} (dry_run={dry_run})",
        rows.len()
    );
    for row in rows {
        let o = summarise_one(pool, &row, &cfg, !dry_run).await;
        match (&o.summary, &o.error) {
            (Some(s), _) => {
                let preview: String = s.chars().take(160).collect();
                println!(
                    "  row {} [{}] written={} chars={}: {preview}",
                    o.row_id,
                    o.source.as_str(),
                    o.written,
                    s.len(),
                );
            }
            (None, Some(e)) => println!("  row {} FAILED: {e}", o.row_id),
            (None, None) => {}
        }
    }
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_transcript_passes_short_through() {
        let t = "short transcript";
        assert_eq!(cap_transcript(t, 1000), t);
    }

    #[test]
    fn cap_transcript_keeps_head_and_tail() {
        let t: String = "A".repeat(500) + &"B".repeat(500); // 1000 chars
        let capped = cap_transcript(&t, 100);
        assert!(capped.starts_with(&"A".repeat(70)), "70% head kept");
        assert!(capped.ends_with(&"B".repeat(30)), "30% tail kept");
        assert!(capped.contains("chars elided"));
    }

    #[test]
    fn build_prompt_includes_prior_context() {
        let p = build_prompt("the work", Some("earlier summary"), 1000);
        assert!(p.contains("EARLIER IN THIS SESSION"));
        assert!(p.contains("earlier summary"));
        assert!(p.contains("## TRANSCRIPT"));
        assert!(p.contains("the work"));

        let p2 = build_prompt("just work", None, 1000);
        assert!(!p2.contains("EARLIER IN THIS SESSION"));
    }
}
