//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Summariser: turn each sealed coding-agent segment into a factual prose summary
// for the PM work-log (task_method → 'summarised'). The agno worklog workflow
// picks up summarised rows by querying session_summary IS NOT NULL directly.
//
// Engine routing per segment: Codex sessions → `codex exec`, else → `claude -p`
// (both Rust subprocesses on the user's subscription). Each engine is tried up to
// `primary_attempts` times.
//
// Escalation, in order:
//   1. the session's OWN agent CLI, `primary_attempts` times
//   2. a rate limit → stop immediately, back the source off, retry on a later
//      tick. A quota refills; it is waited out, never routed around — but only
//      up to MAX_RATE_LIMITED_ROW_ATTEMPTS (~a day of backoff), because this
//      branch used to be the one outcome that never recorded an attempt, so a
//      row misclassified as rate-limited retried forever and the queue never
//      drained. See that constant.
//   3. otherwise (crashed / not installed / signed out / unusable output) → the
//      user's globally chosen AI provider, once (`fallback`). Usually the same
//      CLI, in which case it is skipped without a call.
//   4. still nothing → the row stays `pending_summariser` and the drain loop's
//      attempt ledger dead-letters it to 'subprocess_error' after
//      MAX_ROW_ATTEMPTS so it cannot churn forever.
//
// Sequential (one transcript in flight) keeps memory flat and avoids bursting
// rate limits.
//
// Cadence: woken in-process by the indexer's own seals (near-instant) plus a
// short catch-up sweep for hook-sealed rows — no listener (local-only rule).

pub mod claude;
pub mod codex;
pub mod config;
pub mod copilot;
pub mod cursor_agent;
pub mod db;
pub mod fallback;
pub mod prompts;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use meridian_core::proc_ext::NoWindow;
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{watch, Notify};
use tracing::Instrument;

use config::SummariserConfig;
use db::PendingRow;

// ──────────────────────── Errors / engine output ────────────────────────────

/// A summariser engine failure. `RateLimited` means stop now — the quota will not
/// refill between attempts; `Failed` is anything else and earns a retry.
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
///
/// `pub(crate)` (not `pub(super)`): `crate::llm`'s CLI backends run the very same
/// subprocess dance for the prose calls, so they share this runner rather than
/// growing a second copy of the spawn/timeout/kill-on-drop logic.
pub(crate) struct Capture {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Spawn `program args`, feed `stdin_text`, capture stdout/stderr with a hard
/// timeout. `kill_on_drop` guarantees a timed-out child is reaped (no leak);
/// stdin is written from a concurrent task so a large prompt can't deadlock the
/// pipe. Summariser stdout is small (a JSON envelope), so no read-side deadlock.
pub(crate) async fn run_capture(
    program: &str,
    args: &[String],
    stdin_text: &str,
    cwd: &Path,
    timeout_s: u64,
    extra_env: &[(&str, &str)],
    remove_env: &[&str],
) -> Result<Capture, SummariserError> {
    // Spawn the RESOLVED absolute path, not the bare name. `Command::new("claude")`
    // searches only the calling process's `PATH`, and the tray is a Finder-launched
    // `.app` whose `PATH` is the stripped launchd default — so every CLI provider
    // reported "not found on PATH" in Test Connection on machines where the CLI works
    // fine. The daemon was unaffected (its plist sets a rich `PATH`), which is why this
    // only ever surfaced in the tray. Falls back to the bare name so a process that
    // does have a working `PATH` behaves exactly as before.
    let resolved = crate::llm::detect::resolve_cli(program).await;
    // `claude`, `codex`, etc. install as npm-shimmed `.cmd`/`.bat` files on
    // Windows, not PE executables. Do NOT hand-wrap these in `cmd.exe /C` —
    // `Command::new(target)` already detects a `.bat`/`.cmd` target and routes
    // it through `cmd.exe` internally, with cmd.exe-safe argument escaping
    // (the fix for the "BatBadBut" class of bugs, GHSA-q455-m56c-85mh). A
    // manual `cmd.arg("/C").arg(target)` wrapper bypasses that safe escaping —
    // `args` below can carry session-derived prompt text, so a hand-rolled
    // wrapper reopens exactly the injection std's built-in handling closes.
    //
    // Resolving `program`'s own absolute path is not enough on macOS: `codex`/`claude`
    // are `#!/usr/bin/env node` shims, and `env` does its OWN independent `PATH` search
    // for `node` at exec time, using whatever the CHILD inherits — which under the
    // packaged tray is launchd's stripped default with no `/opt/homebrew/bin`. Use
    // `command_for_resolved_cli`, the same fix already applied to the sign-in flows
    // (`cursor_sign_in`/`codex_sign_in`/`claude_sign_in`), so a resolved CLI's own
    // directory is prepended onto the child's `PATH` here too — see its doc comment for
    // the live incident this closes (a clean `codex login` followed by a `codex exec`
    // that failed with `env: node: No such file or directory`).
    let mut cmd = match resolved.as_deref() {
        Some(path) => crate::llm::detect::command_for_resolved_cli(path),
        None => Command::new(program),
    };
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(cwd)
        .kill_on_drop(true)
        .no_window();
    for k in remove_env {
        cmd.env_remove(k);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // User-facing (it surfaces on the provider card), so: plain hyphen, and
            // honest about where we looked - "not on PATH" was actively misleading
            // once resolution also searches the login shell and the install dirs.
            SummariserError::Failed(format!(
                "{program} CLI not found - looked on PATH, in your login shell, \
                 and in the usual install locations"
            ))
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
    /// The user's globally chosen AI provider, used only after the session's own CLI
    /// failed every attempt (see [`fallback`]). Carries WHICH provider answered so the
    /// persisted `summary_source` records that this summary did not come from the agent
    /// that produced the transcript.
    Fallback(crate::llm::LlmProvider),
    /// Historical only — nothing produces this any more.
    ///
    /// MLX used to be the shared fallback for every agent; that was removed with the
    /// on-device deprecation. The variant stays because `summary_source` is a PERSISTED
    /// vocabulary: rows summarised before the removal still read `"mlx"` on disk, and
    /// dropping it would make those rows undescribable.
    Mlx,
    None,
}

impl Source {
    /// The persisted `summary_source` value. Fallback summaries are prefixed so a
    /// consumer can tell "summarised by the agent that did the work" from "summarised by
    /// a substitute" without a second column.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::Copilot => "copilot",
            Source::CursorAgent => "cursor",
            Source::Mlx => "mlx",
            Source::None => "none",
            Source::Fallback(p) => match p {
                crate::llm::LlmProvider::Claude => "fallback:claude",
                crate::llm::LlmProvider::Codex => "fallback:codex",
                crate::llm::LlmProvider::Cursor => "fallback:cursor",
                crate::llm::LlmProvider::Copilot => "fallback:copilot",
                crate::llm::LlmProvider::Custom => "fallback:custom",
            },
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

    // The session's own agent summarises it, up to `primary_attempts` tries
    // (codex→codex, copilot→copilot, cursor→cursor-agent, claude/unknown→claude).
    // Only once those attempts are spent on a NON-quota failure does the user's
    // global provider get a turn (`fallback`); a row neither can summarise is left
    // `pending_summariser` for a later tick, and `drain`'s attempt ledger
    // dead-letters it after MAX_ROW_ATTEMPTS so a broken row cannot churn.
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
    // ran, how many attempts, and wall-clock. Mirrors the classifier's
    // `llm_inference` span; the per-attempt warn! logs below attach to it, so a row
    // that failed to summarise is never silent.
    let infer_span = tracing::info_span!(
        "summariser_inference",
        primary_engine = primary_source.as_str(),
        engine_used = tracing::field::Empty,
        model = tracing::field::Empty,
        attempts_made = tracing::field::Empty,
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
                    // Log the primary failure explicitly — with no fallback, a
                    // rate-limited engine means this row simply waits for a later
                    // drain, and that state must be visible, not silent.
                    tracing::warn!(
                        row_id = row.id,
                        engine = primary_source.as_str(),
                        error = %m,
                        "summariser rate-limited — leaving the row pending for a later tick"
                    );
                    errors.push(format!("{} rate-limited: {m}", primary_source.as_str()));
                    break; // retrying a limit is pointless
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

        // Last resort: the user's globally chosen provider. Reached ONLY when the
        // session's own CLI failed every attempt for a non-quota reason — a rate limit
        // breaks out above, because a quota refills and must be waited out rather than
        // routed around onto a second subscription. See `fallback` for the full rule.
        if summary.is_none() && !rate_limited {
            match fallback::try_summarise(&stdin_text, &row.session_uuid, primary_source).await {
                Ok(Some((s, provider))) => {
                    summary = Some(s);
                    source = Source::Fallback(provider);
                }
                // The global provider IS the engine that just failed — nothing to add.
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        row_id = row.id,
                        error = %e,  // not-anyhow: this error has no source chain to walk
                        "summariser fallback provider failed — leaving the row pending"
                    );
                    errors.push(format!("fallback: {e}"));
                }
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
        // The concrete model is the global provider's own configured one, already recorded
        // on the `llm.call` span that `resolver::complete` opened; naming the provider here
        // keeps this field meaningful without guessing at a value we do not own.
        Source::Fallback(p) => format!("provider:{}", p.as_str()),
        // Unreachable now that nothing sets `Source::Mlx`; kept so the match stays total
        // and reads correctly against rows written before the fallback was removed.
        Source::Mlx => "mlx-server".into(),
        Source::None => String::new(),
    };
    infer_span.record("engine_used", source.as_str());
    infer_span.record("model", model_used.as_str());
    infer_span.record("attempts_made", attempts_made as i64);
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
pub(crate) fn cap_transcript(transcript: &str, cap: usize) -> String {
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

/// What one [`drain`] pass actually did, so the loop can pace itself.
///
/// Replaces a bare `bool` ("were all rows backed off?") that could not
/// distinguish the three cases the wait needs to tell apart: real progress, a
/// quota wall, and *work attempted that failed*. The old signal also
/// miscounted - it compared `skipped_backoff` against the whole batch, so a
/// single failing row alongside backed-off ones collapsed the 30-minute
/// rate-limit wait back to the 5-second sweep.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
struct DrainOutcome {
    /// Rows fetched for consideration this pass.
    seen: u32,
    /// Rows summarised and persisted.
    summarised: u32,
    /// Rows not attempted because their source is in rate-limit backoff, plus
    /// rows whose attempt reported a rate limit.
    skipped_backoff: u32,
    /// Rows attempted that failed for a non-rate-limit reason.
    failed: u32,
    /// Rows not attempted because they have exhausted an attempt cap.
    skipped_exhausted: u32,
}

/// Longest the loop will wait when nothing is succeeding.
///
/// Bounds the retry storm without letting a transient outage park the queue
/// for the full rate-limit backoff: a pass that fails is retried within five
/// minutes, not five seconds.
const MAX_IDLE_BACKOFF_SECS: u64 = 300;

/// Seconds to wait before the next pass, given what the last one did.
///
/// # The fork storm this bounds
///
/// Measured in production over ~46 hours: 18,674 forks, one every ~9 seconds,
/// continuously, while the queue sat flat at 59-61 rows. The shape is a batch
/// of `batch_per_tick` (32) rows that each fail FAST - two primary attempts,
/// so 64 spawns - re-attempted every `sweep_interval_secs` (5 s). Nothing in
/// the loop distinguished "nothing to do" from "everything I tried failed", so
/// a failing queue was retried at the same cadence as an idle one forever.
///
/// Note this is the opposite of the reported diagnosis. A row that *hangs*
/// produces FEW forks - it sits in `wait_with_output`, not in `spawn` - so a
/// high fork rate is positive evidence of fast failures, and pacing them is
/// the fix rather than a timeout change.
fn next_wait_secs(out: DrainOutcome, cfg: &SummariserConfig, consecutive_idle: u32) -> u64 {
    // Progress, or nothing pending: keep the responsive cadence. An empty
    // queue costs nothing to re-check, and the indexer's notify wakes it
    // sooner anyway.
    if out.summarised > 0 || out.seen == 0 {
        return cfg.sweep_interval_secs;
    }
    // Everything that could have been worked on is waiting out a quota, and
    // nothing failed for another reason. Wait out the backoff properly - this
    // is the case the old `all_backed_off` flag was trying to express.
    if out.failed == 0 && out.skipped_backoff > 0 {
        return cfg.rate_limit_backoff_secs;
    }
    // Rows were attempted and none succeeded. Back off exponentially from the
    // normal sweep so a wedged queue costs a fork every few minutes instead of
    // 64 every few seconds. Capped, so recovery is still prompt.
    if out.failed > 0 {
        let factor = 1u64 << consecutive_idle.min(6);
        return cfg
            .sweep_interval_secs
            .saturating_mul(factor)
            .min(MAX_IDLE_BACKOFF_SECS);
    }
    // Everything left is exhausted (dead-lettered or capped) - there is
    // nothing to retry until new rows seal, and the notify covers that.
    cfg.sweep_interval_secs
}

/// The summariser task: drain the queue, then wait for an indexer notify or the
/// catch-up sweep. Dormant if no coding agent is present. Backs off when the
/// primary engine is unavailable (a failed row stays pending for a later drain).
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
        tracing::warn!(error = %crate::errors::chain(&e), "summariser: could not ensure summary_source column");
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
    // Separate ledger for consecutive rate-limited reports — see
    // `MAX_RATE_LIMITED_ROW_ATTEMPTS`.
    let mut rate_limited_attempts: HashMap<i64, u32> = HashMap::new();
    // Consecutive passes that attempted work and summarised nothing. Drives
    // `next_wait_secs`'s exponential backoff so a wedged queue stops re-forking
    // its whole batch every few seconds.
    let mut consecutive_idle: u32 = 0;
    loop {
        let out = drain(
            &pool,
            &cfg,
            &mut attempts,
            &mut rate_limited_attempts,
            &mut source_backoff,
        )
        .await;
        if out.summarised > 0 || out.failed == 0 {
            consecutive_idle = 0;
        } else {
            consecutive_idle = consecutive_idle.saturating_add(1);
        }
        let wait = next_wait_secs(out, &cfg, consecutive_idle);
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
/// prompt exceeds claude's 200k context cycled every drain, each burning two
/// claude calls (plus, at the time, an MLX fallback call), indefinitely. The
/// ledger is in-memory by design: a daemon restart (or `--day` backfill after
/// fixing the engine) retries cleanly.
///
/// This is what makes removing the MLX fallback safe rather than a hot loop:
/// MLX used to terminate a row the primary engine could never summarise, and
/// this ledger is now the only thing that does.
const MAX_ROW_ATTEMPTS: u32 = 3;

/// One drain pass: summarise pending rows from a bounded recent window
/// (yesterday + today), oldest-first. Returns true only when all available
/// rows belong to rate-limited sources (no progress was possible this pass),
/// signalling the caller to wait longer rather than spinning.
///
/// Rate-limit backoff is per-source (keyed on `app_name`): if Claude Code is
/// rate-limited, Codex / Cursor / Copilot rows continue draining via their own
/// primary. The old global 30-minute freeze is gone.
///
/// Why the yesterday+today window: today-only strands rows sealed just before
/// midnight; all-days walks the full historical backlog. Yesterday+today
/// catches the rollover without the churn. Older days remain an explicit
/// operator action: `meridian coding-agent-summarise --day <YYYY-MM-DD>`.
async fn drain(
    pool: &SqlitePool,
    cfg: &SummariserConfig,
    attempts: &mut HashMap<i64, u32>,
    rate_limited_attempts: &mut HashMap<i64, u32>,
    source_backoff: &mut HashMap<String, std::time::Instant>,
) -> DrainOutcome {
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
            tracing::warn!(error = %crate::errors::chain(&e), "summariser: fetch_pending failed");
            return DrainOutcome::default();
        }
    };

    if rows.is_empty() {
        return DrainOutcome::default();
    }

    let mut out = DrainOutcome {
        seen: rows.len() as u32,
        ..DrainOutcome::default()
    };

    for row in &rows {
        // Skip rows whose agent source is still in rate-limit backoff.
        if source_backoff.contains_key(&row.agent) {
            out.skipped_backoff += 1;
            continue;
        }

        let tries = attempts.get(&row.id).copied().unwrap_or(0);
        if tries >= MAX_ROW_ATTEMPTS {
            out.skipped_exhausted += 1;
            continue; // dead-lettered this daemon lifetime
        }
        if rate_limited_attempts.get(&row.id).copied().unwrap_or(0) >= MAX_RATE_LIMITED_ROW_ATTEMPTS
        {
            out.skipped_exhausted += 1;
            continue; // gave up on this row's endless "rate limited" reports
        }

        let outcome = summarise_one(pool, row, cfg, true).await;
        if outcome.written {
            out.summarised += 1;
            // Row succeeded and moved past pending_summariser — drop its
            // ledger entry via record_attempt so `attempts` only ever holds
            // currently-pending rows that have failed at least once, not
            // every row that ever failed once over the daemon's lifetime.
            record_attempt(attempts, row.id, true);
            // Same for the rate-limit ledger: a success means whatever quota
            // was in the way has refilled, so the row starts clean if it is
            // ever retried.
            rate_limited_attempts.remove(&row.id);
        } else if outcome.rate_limited {
            // Apply per-source backoff so other sources can still drain.
            let until =
                std::time::Instant::now() + Duration::from_secs(cfg.rate_limit_backoff_secs);
            source_backoff.insert(row.agent.clone(), until);
            out.skipped_backoff += 1;
            // Count it. This branch used to be the ONE outcome that never
            // touched the ledger, which made `MAX_ROW_ATTEMPTS` inapplicable to
            // it: a row that classified `RateLimited` on every attempt was
            // retried forever and could never dead-letter. That is not a
            // theoretical hole - `RATE_LIMIT_MARKERS` matches bare substrings
            // like "429" and "quota" anywhere in an engine's output, and this
            // is a developer tool whose transcripts are full of both, so a row
            // failing for an unrelated reason can be misfiled as rate-limited
            // and then never terminate.
            //
            // Counted against its OWN, much more generous cap rather than
            // `MAX_ROW_ATTEMPTS`, because the intent behind this branch is
            // still right: a quota refills, and it should be waited out rather
            // than routed around. See `MAX_RATE_LIMITED_ROW_ATTEMPTS`.
            let tries = record_rate_limited_attempt(rate_limited_attempts, row.id);
            if tries >= MAX_RATE_LIMITED_ROW_ATTEMPTS {
                if let Err(e) = db::write_dead_letter(pool, row.id).await {
                    tracing::error!(row_id = row.id, error = %crate::errors::chain(&e), "failed to dead-letter a persistently rate-limited row");
                }
                tracing::warn!(
                    row_id = row.id,
                    attempts = tries,
                    "row has reported rate-limited on every attempt for far longer than a \
                     quota takes to refill - dead-lettering it rather than retrying forever \
                     (it may be a misclassified failure)"
                );
            }
            tracing::warn!(
                row_id = outcome.row_id,
                source = %row.agent,
                backoff_s = cfg.rate_limit_backoff_secs,
                "primary summariser rate-limited — backing off this source, other sources continue"
            );
        } else {
            // Transient failure. Leave pending for retry; log so it isn't silent.
            out.failed += 1;
            let tries = record_attempt(attempts, row.id, false)
                .expect("record_attempt(.., written=false) always returns Some");
            if tries >= MAX_ROW_ATTEMPTS {
                if let Err(e) = db::write_dead_letter(pool, row.id).await {
                    tracing::error!(row_id = row.id, error = %crate::errors::chain(&e), "failed to dead-letter row");
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

    if out.summarised > 0 {
        tracing::info!(summarised = out.summarised, "summariser drain");
    }

    out
}

/// Updates the per-row failure ledger (`attempts`, see [`MAX_ROW_ATTEMPTS`])
/// after one `summarise_one` outcome.
///
/// On success (`written = true`) the row's entry is removed entirely —
/// `attempts` must only ever hold currently-pending rows that have failed at
/// least once this daemon lifetime, not every row that has ever failed once,
/// which would otherwise grow the map forever as rows are summarised and new
/// ones come in. Returns `None` in that case.
///
/// On failure (`written = false`) the row's attempt count is incremented and
/// the new count returned as `Some(tries)`, for the caller to compare against
/// [`MAX_ROW_ATTEMPTS`].
/// How many consecutive `RateLimited` outcomes a single row may report before
/// it is dead-lettered anyway.
///
/// Deliberately far larger than [`MAX_ROW_ATTEMPTS`], and paced by
/// `rate_limit_backoff_secs` (30 minutes by default) rather than the 5-second
/// sweep - so this is roughly a day of genuinely waiting out a quota before
/// giving up. A real usage limit refills long inside that; a row still
/// reporting "rate limited" a day later is almost certainly a misclassified
/// failure, and the alternative to giving up is retrying it forever.
const MAX_RATE_LIMITED_ROW_ATTEMPTS: u32 = 48;

/// Increment and return a row's consecutive rate-limited count.
///
/// Kept in a ledger separate from [`record_attempt`]'s so the two caps cannot
/// interfere: a row that fails twice, then hits a genuine quota, must not
/// arrive at the rate-limit cap carrying unrelated strikes.
fn record_rate_limited_attempt(attempts: &mut HashMap<i64, u32>, row_id: i64) -> u32 {
    let tries = attempts.get(&row_id).copied().unwrap_or(0) + 1;
    attempts.insert(row_id, tries);
    tries
}

fn record_attempt(attempts: &mut HashMap<i64, u32>, row_id: i64, written: bool) -> Option<u32> {
    if written {
        attempts.remove(&row_id);
        None
    } else {
        let tries = attempts.get(&row_id).copied().unwrap_or(0) + 1;
        attempts.insert(row_id, tries);
        Some(tries)
    }
}

/// One-shot CLI: `meridian coding-agent-summarise [--dry-run] [--day D] [--limit N]`.
/// Summarise (or dry-run) the pending queue for a day — manual backfill / eval.
pub async fn cli_summarise(pool: &SqlitePool, dry_run: bool, day: Option<&str>, limit: i64) {
    let cfg = SummariserConfig::from_env();
    // `{e:#}` — anyhow's alternate form, which prints the whole context chain.
    // Plain `{e}` renders only the outermost `.context(...)`, which is how this
    // reported an unkeyed-pool failure as the bare, unactionable
    // "ensure column: check summary_source column" while the real cause
    // (SQLite code 26, "file is not a database") was thrown away.
    if let Err(e) = db::ensure_summary_source_column(pool).await {
        eprintln!("summarise: ensure column: {e:#}");
        return;
    }
    let day = day
        .map(str::to_string)
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    let rows = match db::fetch_pending(pool, &cfg, limit, std::slice::from_ref(&day)).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("summarise: fetch_pending: {e:#}");
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

/// Daily digest: if any coding-agent sessions were permanently dead-lettered
/// today (see [`db::write_dead_letter`]), enqueue one batched notification
/// rather than a toast per row — matches [`crate::daily_plan::maybe_nudge`]'s
/// once-per-day dedup shape (`dedup_key` scoped to today), so calling this
/// unconditionally every ETL tick is safe: the outbox's UNIQUE constraint on
/// `dedup_key` makes a repeat call within the same day a no-op.
///
/// # Who calls this
/// The daemon's poll-tick loop (`src/main.rs`), alongside `daily_plan::maybe_nudge`.
pub async fn maybe_notify_dead_letters(pool: &SqlitePool) -> anyhow::Result<()> {
    let today = meridian_core::date::today_string();
    let (since_utc, _) = meridian_core::date::local_day_bounds(&today);
    let count = db::count_dead_lettered_since(pool, &since_utc).await?;
    if count == 0 {
        return Ok(());
    }
    let dedup = format!("summariser.dead_letter:{today}");
    let body = format!(
        "{count} coding session{} couldn't be summarised. Check that your coding-agent CLIs are signed in.",
        if count == 1 { "" } else { "s" }
    );
    crate::notifications::enqueue(
        pool,
        crate::notifications::NewNotification::event(
            &dedup,
            "summariser.dead_letter",
            "Some coding sessions weren't summarised",
            &body,
        )
        .link(meridian_core::notifications::deep_links::SETTINGS),
    )
    .await?;
    Ok(())
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

    /// `summary_source` is a PERSISTED vocabulary read back by the worklog pipeline, so
    /// these strings are a contract. A fallback summary must be distinguishable from one
    /// the session's own agent produced - same prefix, never the bare provider name.
    #[test]
    fn fallback_source_is_distinguishable_from_the_primary_engine() {
        use crate::llm::LlmProvider;
        assert_eq!(Source::Claude.as_str(), "claude");
        assert_eq!(
            Source::Fallback(LlmProvider::Claude).as_str(),
            "fallback:claude"
        );
        assert_ne!(
            Source::CursorAgent.as_str(),
            Source::Fallback(LlmProvider::Cursor).as_str()
        );
        for p in LlmProvider::all() {
            assert!(Source::Fallback(p).as_str().starts_with("fallback:"));
        }
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

    #[test]
    fn record_attempt_removes_entry_on_success() {
        let mut attempts: HashMap<i64, u32> = HashMap::new();
        attempts.insert(42, 2); // simulate two prior failures

        let result = record_attempt(&mut attempts, 42, true);

        assert_eq!(result, None);
        assert!(
            !attempts.contains_key(&42),
            "a row that succeeds must have its ledger entry removed, not just left at its prior count"
        );
    }

    #[test]
    fn record_attempt_increments_on_failure() {
        let mut attempts: HashMap<i64, u32> = HashMap::new();

        assert_eq!(record_attempt(&mut attempts, 7, false), Some(1));
        assert_eq!(record_attempt(&mut attempts, 7, false), Some(2));
        assert_eq!(record_attempt(&mut attempts, 7, false), Some(3));
        assert_eq!(attempts.get(&7), Some(&3));
    }

    #[test]
    fn record_attempt_success_after_failures_leaves_no_trace() {
        let mut attempts: HashMap<i64, u32> = HashMap::new();

        record_attempt(&mut attempts, 99, false);
        record_attempt(&mut attempts, 99, false);
        record_attempt(&mut attempts, 99, true); // succeeds on the third try

        assert!(
            attempts.is_empty(),
            "map must not retain any entry for a row once it has succeeded"
        );
    }

    #[test]
    fn record_attempt_only_touches_the_given_row() {
        let mut attempts: HashMap<i64, u32> = HashMap::new();
        record_attempt(&mut attempts, 1, false);
        record_attempt(&mut attempts, 2, false);

        record_attempt(&mut attempts, 1, true);

        assert!(!attempts.contains_key(&1), "row 1 succeeded — removed");
        assert_eq!(
            attempts.get(&2),
            Some(&1),
            "row 2's independent failure count must be untouched"
        );
    }

    // ── pacing + the rate-limit cap ─────────────────────────────────────────

    fn cfg_for_pacing() -> SummariserConfig {
        let mut c = SummariserConfig::from_env();
        c.sweep_interval_secs = 5;
        c.rate_limit_backoff_secs = 1800;
        c
    }

    /// THE fork storm. Measured in production: 18,674 forks over ~46 hours,
    /// one every ~9 s, while the queue sat flat. A batch of 32 rows that each
    /// fail fast costs 64 spawns, and it used to be re-attempted every 5 s
    /// forever because the loop could not tell "nothing to do" from
    /// "everything I tried failed".
    #[test]
    fn a_failing_pass_backs_off_instead_of_re_forking_every_sweep() {
        let cfg = cfg_for_pacing();
        let failing = DrainOutcome {
            seen: 32,
            failed: 32,
            ..DrainOutcome::default()
        };
        let mut prev = 0;
        for idle in 1..=6 {
            let w = next_wait_secs(failing, &cfg, idle);
            assert!(
                w > prev,
                "wait must grow while nothing succeeds (idle={idle} gave {w}, previous {prev})"
            );
            prev = w;
        }
        assert!(
            next_wait_secs(failing, &cfg, 1) > cfg.sweep_interval_secs,
            "the first failing pass must already back off past the normal sweep"
        );
        assert_eq!(
            next_wait_secs(failing, &cfg, 99),
            MAX_IDLE_BACKOFF_SECS,
            "backoff must cap so recovery stays prompt"
        );
    }

    /// Progress and an empty queue both keep the responsive cadence - backing
    /// off a working summariser would delay every summary.
    #[test]
    fn progress_and_idleness_keep_the_normal_sweep() {
        let cfg = cfg_for_pacing();
        let progressing = DrainOutcome {
            seen: 4,
            summarised: 4,
            ..DrainOutcome::default()
        };
        assert_eq!(
            next_wait_secs(progressing, &cfg, 3),
            cfg.sweep_interval_secs,
            "a pass that summarised must not inherit an earlier backoff"
        );
        assert_eq!(
            next_wait_secs(DrainOutcome::default(), &cfg, 3),
            cfg.sweep_interval_secs,
            "an empty queue is cheap to re-check"
        );
    }

    /// A genuine quota wall waits the full backoff rather than spinning.
    #[test]
    fn an_entirely_rate_limited_pass_waits_out_the_quota() {
        let cfg = cfg_for_pacing();
        let walled = DrainOutcome {
            seen: 6,
            skipped_backoff: 6,
            ..DrainOutcome::default()
        };
        assert_eq!(
            next_wait_secs(walled, &cfg, 0),
            cfg.rate_limit_backoff_secs,
            "nothing can proceed until a quota refills - do not spin on it"
        );
    }

    /// The old `all_backed_off` flag required `skipped_backoff == rows.len()`,
    /// so ONE failing row alongside backed-off ones collapsed the 30-minute
    /// wait back to the 5-second sweep. It must still not spin - but it also
    /// must not wait out the full quota, because the failing row is real work
    /// that deserves a retry sooner than that.
    #[test]
    fn one_failure_beside_backed_off_rows_neither_spins_nor_stalls() {
        let cfg = cfg_for_pacing();
        let mixed = DrainOutcome {
            seen: 10,
            skipped_backoff: 9,
            failed: 1,
            ..DrainOutcome::default()
        };
        let w = next_wait_secs(mixed, &cfg, 1);
        assert!(
            w > cfg.sweep_interval_secs,
            "a mixed pass collapsed back to the bare sweep - this is the spin"
        );
        assert!(
            w < cfg.rate_limit_backoff_secs,
            "the failing row must be retried well before the quota window elapses"
        );
    }

    /// THE immortality bug. The `rate_limited` branch was the one outcome that
    /// never touched a ledger, so `MAX_ROW_ATTEMPTS` did not apply to it and a
    /// row misclassified as rate-limited retried forever. `RATE_LIMIT_MARKERS`
    /// matches bare "429"/"quota" anywhere in an engine's output, and this is a
    /// developer tool whose transcripts are full of both.
    #[test]
    fn a_persistently_rate_limited_row_eventually_terminates() {
        let mut ledger: HashMap<i64, u32> = HashMap::new();
        let mut last = 0;
        for _ in 0..MAX_RATE_LIMITED_ROW_ATTEMPTS {
            last = record_rate_limited_attempt(&mut ledger, 7);
        }
        assert_eq!(
            last, MAX_RATE_LIMITED_ROW_ATTEMPTS,
            "consecutive rate-limited reports must be counted, not ignored"
        );
    }

    /// The two caps are independent: a row that failed twice and then hits a
    /// genuine quota must not arrive at the rate-limit cap carrying unrelated
    /// strikes, and vice versa.
    #[test]
    fn the_two_attempt_ledgers_do_not_bleed_into_each_other() {
        let mut failures: HashMap<i64, u32> = HashMap::new();
        let mut rate_limits: HashMap<i64, u32> = HashMap::new();
        record_attempt(&mut failures, 42, false);
        record_attempt(&mut failures, 42, false);
        assert_eq!(record_rate_limited_attempt(&mut rate_limits, 42), 1);
        assert_eq!(failures.get(&42), Some(&2));
    }

    /// Waiting out a quota must be generous - far longer than the fast-failure
    /// cap - or a real usage limit would dead-letter a legitimate row.
    #[test]
    fn the_rate_limit_cap_is_far_more_patient_than_the_failure_cap() {
        // Read through locals so this is a value comparison rather than an
        // assertion on a constant expression (which clippy rejects).
        let patient: u32 = MAX_RATE_LIMITED_ROW_ATTEMPTS;
        let strict: u32 = MAX_ROW_ATTEMPTS;
        assert!(
            patient > strict.saturating_mul(10),
            "a refilling quota must be waited out, not treated like a broken row \
             (rate-limit cap {patient}, failure cap {strict})"
        );
    }
}
