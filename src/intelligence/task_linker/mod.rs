//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

mod db;
mod db_write;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::config::Config;
use tracing::field;

use db::{
    count_pending_sessions, fetch_pending_classifier_sessions, fetch_sessions_in_range,
    fetch_unclassified_sessions, get_agent_cursor, get_max_session_id,
};
use db_write::{
    advance_agent_cursor, complete_agent_run, start_agent_run, update_coding_agent_task,
    update_session_overhead, update_session_task, write_dimensions, write_error_sentinel,
};

// ---------------------------------------------------------------------------
// Outcome type returned by run_task_linking
// ---------------------------------------------------------------------------

/// What happened during a single classification cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskLinkOutcome {
    /// No sessions pending — cursor is fully caught up.
    NoPendingWork,
    /// At least one session was processed (classified or marked overhead).
    Classified,
    /// MLX server call failed for this session. Cursor was NOT advanced —
    /// the caller tracks consecutive failures and writes a sentinel after
    /// MAX_CONSECUTIVE_FAILURES to unblock the cursor.
    SubprocessFailed {
        session_id: i64,
        /// How many sessions are still waiting behind the cursor.
        pending: i64,
    },
}

// One session per daemon tick — at 30-60s cadence there is typically one new
// session. The backfill binary handles bulk catch-up after downtime.
pub(super) const BATCH_LIMIT: i64 = 1;

// Coding-agent rows are classified in small batches from their (non-cursor)
// pending_classifier queue. The MLX server classifies the whole batch in one
// call; the drain loop repeats until the queue is empty.
const CODING_CLASSIFY_BATCH: i64 = 8;

// ---------------------------------------------------------------------------
// Sentinel helper
// ---------------------------------------------------------------------------

/// Write `task_method = 'subprocess_error'` and advance the cursor past
/// `session_id` so the drain loop is not permanently stuck.
pub async fn mark_session_subprocess_error(pool: &SqlitePool, session_id: i64) -> Result<()> {
    write_error_sentinel(pool, session_id).await?;
    advance_agent_cursor(pool, session_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Startup preflight check
// ---------------------------------------------------------------------------

/// Validates that the persistent MLX server is reachable.
/// Called once at daemon startup — returns Err with a human-readable fix
/// if the server is not listening.
///
/// No-op when `classification_enabled` is false.
pub fn check_classification_ready(cfg: &Config) -> Result<()> {
    if !cfg.classification_enabled {
        return Ok(());
    }

    let port = cfg.mlx_server_port;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .context("invalid MLX server address")?;

    // The MLX server loads a large model in its lifespan startup before it
    // accepts connections. Retry for up to 120 s to cover first-load time.
    let timeout = std::time::Duration::from_secs(2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => return Ok(()),
            Err(_) if std::time::Instant::now() < deadline => {
                warn!(
                    port,
                    "MLX server not yet ready — waiting for model to load (up to 120 s total)"
                );
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
            Err(_) => {
                anyhow::bail!(
                    "MLX server not running on port {port}\n\
                     Fix: cd services && .venv313/bin/meridian-server --backend mlx --port {port}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization structs — sent to and received from the MLX server
// ---------------------------------------------------------------------------

/// Payload sent to `POST /classify_sessions`.
#[derive(Serialize)]
struct ClassifyInput {
    session_ids: Vec<i64>,
    meridian_db: String,
    traceparent: Option<String>,
}

/// Top-level response from the MLX server.
#[derive(Deserialize)]
struct ClassifyOutput {
    results: Vec<SessionClassification>,
}

fn default_session_type() -> String {
    "overhead".to_owned()
}

fn default_routing() -> String {
    "pending".to_owned()
}

fn default_category() -> String {
    "idle_personal".to_owned()
}

/// Per-session classification result returned by the MLX server.
#[derive(Deserialize)]
pub(super) struct SessionClassification {
    pub(super) session_id: i64,
    pub(super) task_key: Option<String>,
    pub(super) confidence: f64,
    /// Activity category emitted by the classifier (replaces the former
    /// Foundation Models settler). Defaults keep deserialization backward
    /// compatible with an older server that omits the field.
    #[serde(default = "default_category")]
    pub(super) category: String,
    #[serde(default)]
    pub(super) category_confidence: f64,
    /// One-sentence justification for `category`, surfaced in the dashboard
    /// (Today view + queue-review) as `explain`. Empty string → NULL on write.
    #[serde(default)]
    pub(super) category_explanation: String,
    /// Routing is computed by Rust, not the server. The default "pending" is a
    /// placeholder until routing logic applies.
    #[serde(default = "default_routing")]
    pub(super) routing: String,
    /// LLM-determined session type: "task" | "overhead" | "untracked".
    #[serde(default = "default_session_type")]
    pub(super) session_type: String,
    pub(super) reasoning: String,
    pub(super) method: String,
    #[serde(default)]
    pub(super) dimensions: HashMap<String, Vec<String>>,
    /// Factual prose summary of the session (10-40 sentences, adaptive to
    /// content). Persisted to `app_sessions.session_summary` and consumed
    /// by the PM-update workflow as its primary signal.
    #[serde(default)]
    pub(super) session_summary: String,
    /// W3C traceparent of this session's `classify_session` trace (set by the
    /// MLX server). Persisted to `app_sessions.classify_traceparent` so a
    /// worklog_draft span can link back to exactly how this session was
    /// classified. `None` on the Apple-FM path or an older server.
    #[serde(default)]
    pub(super) classify_traceparent: Option<String>,
    #[allow(dead_code)]
    pub(super) elapsed_s: f64,
}

// ---------------------------------------------------------------------------
// MLX HTTP server helper
// ---------------------------------------------------------------------------

/// POST `input` to the persistent MLX FastAPI server and return the parsed response.
///
/// The server holds the model in memory — no cold load per call.
/// On failure returns `Err` with a human-readable hint to start the server.
async fn call_mlx_server(
    input: &ClassifyInput,
    port: u16,
    timeout_s: u64,
) -> Result<ClassifyOutput> {
    // Single global LLM gate: hold one permit for the whole request so no other
    // stage (summarise fallback, pm-worklog synth) hits the model concurrently.
    let _llm_permit = crate::llm_gate::acquire().await;

    let url = format!("http://127.0.0.1:{port}/classify_sessions");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(timeout_s))
        .build()
        .context("building http client")?;

    let resp = client
        .post(&url)
        .json(input)
        .send()
        .await
        .with_context(|| {
            format!(
                "MLX server unreachable at {url} — start it with: \
                 cd services && .venv313/bin/meridian-server --backend mlx --port {port}"
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("MLX server returned {status}: {body}");
    }

    resp.json::<ClassifyOutput>()
        .await
        .context("parsing MLX server response")
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run one classification cycle:
///   - trivial sessions (empty session_text) → `overhead/skip` without LLM
///   - non-trivial → POST to the persistent MLX server
///
/// Returns a `TaskLinkOutcome` that tells the caller whether to loop immediately
/// (more work), wait for the next ETL notification (caught up), or track a
/// server failure for sentinel logic.
#[tracing::instrument(
    skip_all,
    fields(
        session_id = field::Empty,
        run_id = field::Empty,
        cursor = field::Empty,
        trivial = field::Empty,
        classifiable = field::Empty,
        sessions = field::Empty,
        links = field::Empty,
        outcome = field::Empty,
    )
)]
pub async fn run_task_linking(
    pool: &SqlitePool,
    cfg: &Config,
    tick_link: Option<opentelemetry::trace::SpanContext>,
) -> Result<TaskLinkOutcome> {
    // This call is its OWN root trace — the caller deliberately does not parent it
    // under the poll/startup tick, so each drained session is a self-contained
    // trace (one `classify_session` subtree, not N siblings sharing one tick
    // trace). Link back to the triggering tick so the daemon→session relationship
    // stays navigable in OpenObserve without collapsing the drain into one trace.
    if let Some(sc) = tick_link {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        tracing::Span::current().add_link_with_attributes(
            sc,
            vec![opentelemetry::KeyValue::new("link.kind", "poll_tick")],
        );
    }

    if !cfg.classification_enabled {
        tracing::Span::current().record("outcome", "disabled");
        debug!("classification disabled — skipping");
        return Ok(TaskLinkOutcome::NoPendingWork);
    }

    let wall = Instant::now();
    let run_id = start_agent_run(pool).await?;
    let span = tracing::Span::current();
    span.record("run_id", run_id);

    let cursor = get_agent_cursor(pool).await?;
    span.record("cursor", cursor);

    if cursor == 0 && !cfg.classification_backfill {
        if let Some(max_id) = get_max_session_id(pool).await? {
            span.record("outcome", "first_run");
            info!(
                max_session_id = max_id,
                "first classification run — advancing cursor to skip historical sessions"
            );
            advance_agent_cursor(pool, max_id).await?;
            complete_agent_run(pool, run_id, "success", 0, 0).await?;
            return Ok(TaskLinkOutcome::NoPendingWork);
        }
    }

    debug!(cursor, "fetching unclassified sessions (cursor-based)");
    let raw_sessions =
        fetch_unclassified_sessions(pool, cursor, cfg.min_classification_duration_s).await?;

    if raw_sessions.is_empty() {
        span.record("outcome", "idle");
        debug!("no sessions pending classification — idle");
        complete_agent_run(pool, run_id, "success", 0, 0).await?;
        return Ok(TaskLinkOutcome::NoPendingWork);
    }

    info!(
        sessions = raw_sessions.len(),
        cursor,
        min_duration_s = cfg.min_classification_duration_s,
        "classification cycle started"
    );

    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable_ids: Vec<i64> = Vec::new();

    for (
        id,
        _app_name,
        _duration_s,
        _wt_json,
        session_text_opt,
        _started_at,
        _ended_at,
        _category,
        _confidence,
        _text_source,
    ) in raw_sessions
    {
        if session_text_opt.unwrap_or_default().trim().is_empty() {
            trivial_ids.push(id);
        } else {
            classifiable_ids.push(id);
        }
    }

    let trivial_count = trivial_ids.len() as i64;
    span.record("trivial", trivial_count);
    span.record("classifiable", classifiable_ids.len() as i64);

    for id in trivial_ids {
        update_session_overhead(pool, id).await?;
        advance_agent_cursor(pool, id).await?;
        info!(
            session_id = id,
            session_type = "overhead",
            method = "trivial",
            "session classified"
        );
    }

    if classifiable_ids.is_empty() {
        let elapsed = wall.elapsed().as_secs_f64();
        span.record("sessions", trivial_count);
        span.record("links", trivial_count);
        span.record("outcome", "trivial_only");
        info!(
            sessions = trivial_count,
            links = trivial_count,
            elapsed = format!("{:.2}s", elapsed),
            "classification run complete (trivial only)"
        );
        complete_agent_run(pool, run_id, "success", trivial_count, trivial_count).await?;
        return Ok(TaskLinkOutcome::Classified);
    }

    // Gate: a classifiable session reaches the LLM only when the whole pipeline
    // is WORKING — the MLX classifier loaded AND a PM tracker that has synced
    // tasks. If either is down we pause WITHOUT advancing the cursor, so this
    // session (and any that accumulate behind it) is classified retroactively the
    // moment both recover, rather than being skipped. Placed after the trivial/
    // short prefilter so those local no-LLM paths still run while paused. Trivial
    // sessions never reach here (BATCH_LIMIT = 1 → this batch is one classifiable
    // session), so the held cursor cannot strand a sibling.
    if !super::pipeline_ready(pool, cfg).await {
        span.record("outcome", "paused_not_ready");
        info!(
            "task linking paused — the MLX classifier and a synced PM tracker must \
             both be working; the backlog will drain automatically once they are"
        );
        complete_agent_run(pool, run_id, "success", trivial_count, trivial_count).await?;
        return Ok(TaskLinkOutcome::NoPendingWork);
    }

    // Refresh the PM task cache before classifying so the candidate ticket list
    // the classifier matches against is current. Gated by SYNC_INTERVAL_MINS, so
    // the one-session-per-tick drain loop does not fetch Jira per session. A
    // refresh failure is non-fatal — fall through and classify against the cache.
    if let Err(e) = super::run_pm_sync(pool, cfg).await {
        warn!(error = %e, "pm_tasks refresh before classification failed — using cached tasks");
    }

    // BATCH_LIMIT is 1, so there is exactly one session in classifiable_ids.
    let failing_session_id = classifiable_ids[0];
    // Stamp the session id on the root span so the trace HEADER identifies which
    // session this one-session-per-trace run is about (app_name + the rest of the
    // app_sessions row land on the MLX `db_fetch` span).
    span.record("session_id", failing_session_id);

    let input = ClassifyInput {
        session_ids: classifiable_ids,
        meridian_db: cfg.meridian_db.clone(),
        traceparent: crate::observability::current_traceparent(),
    };

    info!(
        port = cfg.mlx_server_port,
        session_ids = ?input.session_ids,
        timeout_s = cfg.classification_timeout_s,
        "calling mlx server"
    );

    let classify_output =
        match call_mlx_server(&input, cfg.mlx_server_port, cfg.classification_timeout_s).await {
            Ok(out) => out,
            Err(e) => {
                warn!(error = %e, "mlx server call failed");
                span.record("outcome", "server_failed");
                complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
                let pending =
                    count_pending_sessions(pool, cursor, cfg.min_classification_duration_s)
                        .await
                        .unwrap_or(-1);
                return Ok(TaskLinkOutcome::SubprocessFailed {
                    session_id: failing_session_id,
                    pending,
                });
            }
        };

    info!(
        results_count = classify_output.results.len(),
        results = ?classify_output.results.iter().map(|r| (r.session_id, r.task_key.as_deref().unwrap_or("-"), r.session_type.as_str())).collect::<Vec<_>>(),
        "subprocess returned"
    );

    let mut links_written: i64 = trivial_count;
    let total_sessions = trivial_count + classify_output.results.len() as i64;

    let write_result: Result<()> = 'write_loop: {
        for r in &classify_output.results {
            if let Err(e) = update_session_task(pool, r).await {
                break 'write_loop Err(e);
            }
            if let Err(e) = write_dimensions(pool, r.session_id, &r.dimensions).await {
                break 'write_loop Err(e);
            }
            if let Err(e) = advance_agent_cursor(pool, r.session_id).await {
                break 'write_loop Err(e);
            }
            info!(
                session_id   = r.session_id,
                task_key     = ?r.task_key,
                session_type = %r.session_type,
                routing      = %r.routing,
                confidence   = r.confidence,
                method       = %r.method,
                elapsed_s    = r.elapsed_s,
                reasoning    = %r.reasoning,
                "session classified"
            );
            links_written += 1;
        }
        Ok(())
    };
    if let Err(e) = write_result {
        span.record("outcome", "write_failed");
        complete_agent_run(pool, run_id, "failed", total_sessions, links_written).await?;
        return Err(e);
    }

    let elapsed = wall.elapsed().as_secs_f64();
    span.record("sessions", total_sessions);
    span.record("links", links_written);
    span.record("outcome", "success");
    info!(
        sessions = total_sessions,
        links = links_written,
        elapsed = format!("{:.2}s", elapsed),
        "classification run complete"
    );

    complete_agent_run(pool, run_id, "success", total_sessions, links_written).await?;
    Ok(TaskLinkOutcome::Classified)
}

// ---------------------------------------------------------------------------
// Coding-agent classify trigger — the seal→summarise→classify chain's last link
// ---------------------------------------------------------------------------

/// Classify up to `CODING_CLASSIFY_BATCH` summarised coding-agent rows (the
/// `pending_classifier` queue). NON-cursor: selected by summarised-state, not id
/// order, so it never collides with the screen-capture cursor. The MLX server
/// reasons over each row's `session_summary` (not the transcript); we persist
/// the task fields WITHOUT touching `session_summary`. Returns rows classified.
pub async fn run_coding_agent_classification(
    pool: &SqlitePool,
    cfg: &Config,
    tick_link: Option<opentelemetry::trace::SpanContext>,
) -> Result<usize> {
    if !cfg.classification_enabled {
        return Ok(0);
    }

    let ids = fetch_pending_classifier_sessions(pool, CODING_CLASSIFY_BATCH).await?;
    if ids.is_empty() {
        return Ok(0);
    }

    info!(session_ids = ?ids, "classifying summarised coding-agent rows");

    // One session per MLX request. The server classifies sequentially (~1 min
    // per session), so batching N rows into a single call needs N × that
    // wall-time — far beyond `classification_timeout_s`. The batched call fired
    // its timeout every time (~130-157 s for 8 rows vs a 120 s ceiling) and the
    // reqwest timeout surfaced as a misleading "MLX server unreachable", leaving
    // the rows unwritten so the same oldest batch was retried forever. Sending
    // one row per call keeps each request well inside the timeout, advances each
    // row independently, and (via the per-call llm_gate) interleaves fairly with
    // the live classifier and the summariser instead of holding the model for a
    // whole batch.
    let mut n = 0usize;
    for id in ids {
        use tracing::Instrument;

        // One root trace per coding-agent session (same rationale as the ETL
        // classify path): the `current_traceparent()` sent to the MLX server is
        // captured INSIDE this span, so the server's `classify_session` subtree
        // nests under this session alone. Link back to the triggering tick.
        let sess_span = tracing::info_span!("coding_agent_classify_session", session_id = id);
        if let Some(sc) = tick_link.clone() {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            sess_span.add_link_with_attributes(
                sc,
                vec![opentelemetry::KeyValue::new("link.kind", "poll_tick")],
            );
        }

        let out = async {
            let input = ClassifyInput {
                session_ids: vec![id],
                meridian_db: cfg.meridian_db.clone(),
                traceparent: crate::observability::current_traceparent(),
            };
            call_mlx_server(&input, cfg.mlx_server_port, cfg.classification_timeout_s).await
        }
        .instrument(sess_span)
        .await;

        let out = match out {
            Ok(out) => out,
            Err(e) => {
                // Don't abort the whole batch on one row — log and move on so a
                // single slow or malformed row can't block the others.
                warn!(session_id = id, error = %e, "coding-agent classification failed for session");
                continue;
            }
        };

        for r in &out.results {
            update_coding_agent_task(pool, r).await?;
            write_dimensions(pool, r.session_id, &r.dimensions).await?;
            info!(
                session_id = r.session_id,
                task_key = ?r.task_key,
                session_type = %r.session_type,
                method = %r.method,
                "coding-agent session classified",
            );
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Backfill entry point — does NOT touch agent_cursor
// ---------------------------------------------------------------------------

/// Classify sessions in an explicit id range without advancing `agent_cursor`.
/// Safe to run while the daemon is active. Returns `(processed, linked)`.
pub async fn link_range(
    pool: &SqlitePool,
    cfg: &Config,
    from_id: i64,
    to_id: Option<i64>,
    dry_run: bool,
) -> Result<(usize, usize)> {
    let raw_sessions =
        fetch_sessions_in_range(pool, from_id, to_id, cfg.min_classification_duration_s).await?;

    if raw_sessions.is_empty() {
        return Ok((0, 0));
    }

    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable_ids: Vec<i64> = Vec::new();

    for (
        id,
        _app_name,
        _duration_s,
        _wt_json,
        session_text_opt,
        _started_at,
        _ended_at,
        _category,
        _confidence,
        _text_source,
    ) in raw_sessions
    {
        if session_text_opt.unwrap_or_default().trim().is_empty() {
            trivial_ids.push(id);
        } else {
            classifiable_ids.push(id);
        }
    }

    let total = trivial_ids.len() + classifiable_ids.len();
    let mut linked: usize = 0;

    if dry_run {
        for id in &trivial_ids {
            println!("  session {id}: overhead/skip (empty text — would write prefilter_trivial)");
        }
        for id in &classifiable_ids {
            println!("  session {id}: would classify via mlx server");
        }
        return Ok((total, 0));
    }

    for id in trivial_ids {
        update_session_overhead(pool, id).await?;
        linked += 1;
    }

    if classifiable_ids.is_empty() {
        return Ok((total, linked));
    }

    let input = ClassifyInput {
        session_ids: classifiable_ids,
        meridian_db: cfg.meridian_db.clone(),
        traceparent: crate::observability::current_traceparent(),
    };

    match call_mlx_server(&input, cfg.mlx_server_port, cfg.classification_timeout_s).await {
        Ok(classify_output) => {
            for r in &classify_output.results {
                update_session_task(pool, r).await?;
                write_dimensions(pool, r.session_id, &r.dimensions).await?;
                linked += 1;
            }
        }
        Err(e) => {
            warn!(error = %e, "mlx server call failed — skipping classification");
        }
    }

    Ok((total, linked))
}

#[cfg(test)]
mod tests {
    use super::ClassifyOutput;

    // Verbatim envelope captured from the live MLX classifier on session 19958.
    // This is the actual wire shape Rust receives over HTTP — pinning it here
    // closes the Python→Rust transport seam without needing a running server.
    const REAL_SERVER_JSON: &str = r#"{"results": [{"session_id": 19958,
        "task_key": "KAN-64", "confidence": 0.85, "category": "coding",
        "category_confidence": 0.9,
        "category_explanation": "Editing meridian-cli.sh in VS Code with terminal showing session classifier logic.",
        "session_type": "task", "reasoning": "title mentions classifier",
        "method": "mlx_direct", "dimensions": {"activity": ["coding"]},
        "session_summary": "Reviewed meridian-cli.sh.", "elapsed_s": 64.4}]}"#;

    #[test]
    fn deserializes_real_mlx_server_response_with_category_fields() {
        let out: ClassifyOutput = serde_json::from_str(REAL_SERVER_JSON).unwrap();
        let r = &out.results[0];
        assert_eq!(r.session_id, 19958);
        assert_eq!(r.category, "coding");
        assert!((r.category_confidence - 0.9).abs() < 1e-9);
        assert_eq!(
            r.category_explanation,
            "Editing meridian-cli.sh in VS Code with terminal showing session classifier logic."
        );
        assert_eq!(r.task_key.as_deref(), Some("KAN-64"));
    }

    #[test]
    fn old_response_without_category_fields_falls_back_to_defaults() {
        // A server predating this change omits category/category_confidence/
        // category_explanation — serde defaults must keep deserialization working.
        let legacy = r#"{"results": [{"session_id": 7, "task_key": null,
            "confidence": 0.1, "reasoning": "", "method": "mlx_direct",
            "session_summary": "", "elapsed_s": 0.0}]}"#;
        let out: ClassifyOutput = serde_json::from_str(legacy).unwrap();
        let r = &out.results[0];
        assert_eq!(r.category, "idle_personal"); // default_category()
        assert_eq!(r.category_confidence, 0.0);
        assert_eq!(r.category_explanation, "");
    }
}
