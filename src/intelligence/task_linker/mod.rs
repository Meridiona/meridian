// meridian — normalises screenpipe activity into structured app sessions

mod db;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::config::Config;

use db::{
    advance_agent_cursor, complete_agent_run, fetch_open_pm_tasks, fetch_sessions_in_range,
    fetch_unclassified_sessions, get_agent_cursor, get_max_session_id, start_agent_run,
    update_session_overhead, update_session_task, write_dimensions,
};

// One session per daemon tick — at 30-60s cadence there is typically one new
// session. The backfill binary handles bulk catch-up after downtime.
pub(super) const BATCH_LIMIT: i64 = 1;

// ---------------------------------------------------------------------------
// Serialization structs — sent to and received from the Python subprocess
// ---------------------------------------------------------------------------

/// Full payload sent to `python3 -m agents.run_task_linker` via stdin.
#[derive(Serialize)]
struct ClassifyInput {
    sessions: Vec<SessionPayload>,
    pm_tasks: Vec<TaskPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    traceparent: Option<String>,
}

/// Per-session data sent to Python.
#[derive(Serialize)]
struct SessionPayload {
    id: i64,
    app_name: String,
    duration_s: i64,
    session_text: String,
    window_titles: serde_json::Value,
    category: Option<String>,
    confidence: Option<f64>,
}

/// Per-task data sent to Python.
#[derive(Serialize)]
pub struct TaskPayload {
    pub task_key: String,
    pub title: String,
    pub description_text: String,
    pub status: String,
    pub status_category: String,
}

/// Top-level response read from Python stdout.
#[derive(Deserialize)]
struct ClassifyOutput {
    results: Vec<SessionClassification>,
}

/// Per-session classification result returned by Python.
#[derive(Deserialize)]
pub(super) struct SessionClassification {
    pub(super) session_id: i64,
    pub(super) task_key: Option<String>,
    pub(super) confidence: f64,
    pub(super) routing: String,
    pub(super) reasoning: String,
    pub(super) method: String,
    #[serde(default)]
    pub(super) dimensions: HashMap<String, Vec<String>>,
    #[allow(dead_code)]
    pub(super) elapsed_s: f64,
}

// ---------------------------------------------------------------------------
// Services directory discovery
// ---------------------------------------------------------------------------

/// Locate the `services/` directory that contains `agents/run_task_linker.py`.
/// Resolution order:
///   1. `cfg.classification_services_dir` if set
///   2. Relative to the running executable: `../../services`, `../../../services`
///   3. `services/` in the current working directory
pub(crate) fn find_services_dir(cfg: &Config) -> Option<std::path::PathBuf> {
    if let Some(ref dir) = cfg.classification_services_dir {
        let p = std::path::Path::new(dir);
        if p.join("agents/run_task_linker.py").exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        for ancestor_steps in &[2usize, 3] {
            let mut candidate = exe.clone();
            for _ in 0..*ancestor_steps {
                candidate.pop();
            }
            candidate.push("services");
            if candidate.join("agents/run_task_linker.py").exists() {
                return Some(candidate);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("services");
        if candidate.join("agents/run_task_linker.py").exists() {
            return Some(candidate);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Python executable resolution
// ---------------------------------------------------------------------------

/// Pick the Python binary to use, in priority order:
///   1. `MERIDIAN_PYTHON` env var — explicit override
///   2. `{services_dir}/.venv/bin/python3` — venv created by `uv venv` / `python -m venv`
///   3. `python3` — system fallback
pub(crate) fn resolve_python(services_dir: &std::path::Path) -> String {
    if let Ok(explicit) = std::env::var("MERIDIAN_PYTHON") {
        if !explicit.is_empty() {
            return explicit;
        }
    }
    let venv_python = services_dir.join(".venv/bin/python3");
    if venv_python.exists() {
        return venv_python.to_string_lossy().into_owned();
    }
    "python3".to_string()
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run one hermes-based classification cycle:
///   - trivial sessions (empty session_text) → `overhead/skip` without LLM
///   - non-trivial batch → spawned `python3 -m agents.run_task_linker` subprocess
#[tracing::instrument(skip_all, fields(sessions = 0, pm_tasks = 0, cursor = 0))]
pub async fn run_task_linking(pool: &SqlitePool, cfg: &Config) -> Result<()> {
    if !cfg.classification_enabled {
        debug!("classification disabled — skipping");
        return Ok(());
    }

    let wall = Instant::now();
    let run_id = start_agent_run(pool).await?;

    let cursor = get_agent_cursor(pool).await?;
    tracing::Span::current().record("cursor", cursor);

    if cursor == 0 && !cfg.classification_backfill {
        if let Some(max_id) = get_max_session_id(pool).await? {
            info!(
                max_session_id = max_id,
                "first classification run — advancing cursor to skip historical sessions"
            );
            advance_agent_cursor(pool, max_id).await?;
            complete_agent_run(pool, run_id, "success", 0, 0).await?;
            return Ok(());
        }
    }

    debug!(cursor, "fetching unclassified sessions");

    let raw_sessions =
        fetch_unclassified_sessions(pool, cursor, cfg.min_classification_duration_s).await?;

    if raw_sessions.is_empty() {
        debug!("no sessions pending classification — idle");
        complete_agent_run(pool, run_id, "success", 0, 0).await?;
        return Ok(());
    }

    let pm_tasks = fetch_open_pm_tasks(pool).await?;

    tracing::Span::current().record("sessions", raw_sessions.len());
    tracing::Span::current().record("pm_tasks", pm_tasks.len());
    info!(
        sessions = raw_sessions.len(),
        pm_tasks = pm_tasks.len(),
        cursor,
        min_duration_s = cfg.min_classification_duration_s,
        "classification cycle started"
    );

    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable: Vec<SessionPayload> = Vec::new();

    for (id, app_name, duration_s, wt_json, session_text_opt, category, confidence) in raw_sessions
    {
        let text = session_text_opt.unwrap_or_default();
        if text.trim().is_empty() {
            trivial_ids.push(id);
        } else {
            let window_titles =
                serde_json::from_str(&wt_json).unwrap_or(serde_json::Value::Array(vec![]));
            classifiable.push(SessionPayload {
                id,
                app_name,
                duration_s,
                session_text: text,
                window_titles,
                category,
                confidence,
            });
        }
    }

    let trivial_count = trivial_ids.len() as i64;
    for id in &trivial_ids {
        debug!(
            session_id = id,
            "session skipped (empty session_text → overhead/skip)"
        );
    }
    for id in trivial_ids {
        update_session_overhead(pool, id).await?;
        advance_agent_cursor(pool, id).await?;
    }

    if classifiable.is_empty() {
        let elapsed = wall.elapsed().as_secs_f64();
        info!(
            sessions = trivial_count,
            links = trivial_count,
            elapsed = format!("{:.2}s", elapsed),
            "classification run complete (trivial only)"
        );
        complete_agent_run(pool, run_id, "success", trivial_count, trivial_count).await?;
        return Ok(());
    }

    let services_dir = match find_services_dir(cfg) {
        Some(d) => d,
        None => {
            warn!("could not locate services/agents/run_task_linker.py — skipping classification");
            complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
            return Ok(());
        }
    };

    let batch_size = classifiable.len();
    let input = ClassifyInput {
        sessions: classifiable,
        pm_tasks,
        traceparent: crate::observability::current_traceparent(),
    };
    let input_json = serde_json::to_string(&input).context("serializing ClassifyInput")?;

    let python = resolve_python(&services_dir);

    info!(
        services_dir = %services_dir.display(),
        python = %python,
        batch = batch_size,
        timeout_s = cfg.classification_timeout_s,
        "spawning run_task_linker subprocess"
    );

    let rt = &cfg.runtime;
    let mut child = match Command::new(&python)
        .arg("-m")
        .arg("agents.run_task_linker")
        .env("LOG_LEVEL", &rt.log_level)
        .env("AGENT_AUTO_FLOOR", rt.agent_auto_floor.to_string())
        .env("AGENT_QUEUE_FLOOR", rt.agent_queue_floor.to_string())
        .env(
            "LLM_PREFER_LOCAL",
            if rt.llm_prefer_local { "1" } else { "0" },
        )
        .env("LLM_BUDGET_PCT", rt.llm_budget_pct.to_string())
        .current_dir(&services_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(python = %python, error = %e, "could not spawn run_task_linker — is python installed and hermes set up?");
            complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
            return Ok(());
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input_json.as_bytes())
            .await
            .context("writing to run_task_linker stdin")?;
    }

    let stdout_task = {
        use tokio::io::AsyncReadExt;
        let mut out = child.stdout.take().expect("stdout was piped");
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf).await;
            buf
        })
    };
    let stderr_task = {
        use tokio::io::AsyncReadExt;
        let mut err = child.stderr.take().expect("stderr was piped");
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf).await;
            buf
        })
    };

    let timeout_dur = std::time::Duration::from_secs(cfg.classification_timeout_s);
    let status = match tokio::time::timeout(timeout_dur, child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            warn!(error = %e, "run_task_linker subprocess IO error");
            stdout_task.abort();
            stderr_task.abort();
            complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
            return Ok(());
        }
        Err(_elapsed) => {
            warn!(
                timeout_s = cfg.classification_timeout_s,
                "run_task_linker subprocess timed out — killing"
            );
            let _ = child.kill().await;
            stdout_task.abort();
            stderr_task.abort();
            complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
            return Ok(());
        }
    };

    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !stderr_bytes.is_empty() {
        info!(stderr = %String::from_utf8_lossy(&stderr_bytes), "run_task_linker python stderr");
    }

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        warn!(
            exit_code = ?status.code(),
            stderr = %stderr,
            "run_task_linker exited with non-zero status"
        );
        complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
        return Ok(());
    }

    let classify_output: ClassifyOutput = match serde_json::from_slice(&stdout_bytes) {
        Ok(v) => v,
        Err(e) => {
            let raw = String::from_utf8_lossy(&stdout_bytes);
            warn!(
                error = %e,
                stdout = %raw,
                "run_task_linker stdout is not valid JSON — hermes may be printing to stdout; check observability setup"
            );
            complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
            return Ok(());
        }
    };

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
            debug!(
                session_id = r.session_id,
                task_key   = ?r.task_key,
                routing    = %r.routing,
                confidence = r.confidence,
                elapsed_s  = r.elapsed_s,
                method     = %r.method,
                "session classified"
            );
            links_written += 1;
        }
        Ok(())
    };
    if let Err(e) = write_result {
        complete_agent_run(pool, run_id, "failed", total_sessions, links_written).await?;
        return Err(e);
    }

    let elapsed = wall.elapsed().as_secs_f64();
    info!(
        sessions = total_sessions,
        links = links_written,
        elapsed = format!("{:.2}s", elapsed),
        "classification run complete"
    );

    complete_agent_run(pool, run_id, "success", total_sessions, links_written).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Backfill entry point — does NOT touch agent_cursor
// ---------------------------------------------------------------------------

/// Classify sessions in an explicit id range without advancing `agent_cursor`.
/// Safe to run while the daemon is active. Returns `(processed, linked)`.
#[tracing::instrument(skip(pool, cfg), fields(from_id, to_id = ?to_id, dry_run))]
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

    let pm_tasks = fetch_open_pm_tasks(pool).await?;

    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable: Vec<SessionPayload> = Vec::new();

    for (id, app_name, duration_s, wt_json, session_text_opt, category, confidence) in raw_sessions
    {
        let text = session_text_opt.unwrap_or_default();
        if text.trim().is_empty() {
            trivial_ids.push(id);
        } else {
            let window_titles =
                serde_json::from_str(&wt_json).unwrap_or(serde_json::Value::Array(vec![]));
            classifiable.push(SessionPayload {
                id,
                app_name,
                duration_s,
                session_text: text,
                window_titles,
                category,
                confidence,
            });
        }
    }

    let total = trivial_ids.len() + classifiable.len();
    let mut linked: usize = 0;

    if dry_run {
        for id in &trivial_ids {
            println!("  session {id}: overhead/skip (empty text — would write prefilter_trivial)");
        }
        for s in &classifiable {
            println!(
                "  session {}: {} — would classify via hermes",
                s.id, s.app_name
            );
        }
        return Ok((total, 0));
    }

    for id in trivial_ids {
        update_session_overhead(pool, id).await?;
        linked += 1;
    }

    if classifiable.is_empty() {
        return Ok((total, linked));
    }

    let services_dir = match find_services_dir(cfg) {
        Some(d) => d,
        None => {
            warn!("could not locate services/agents/run_task_linker.py — skipping hermes classification");
            return Ok((total, linked));
        }
    };

    let input = ClassifyInput {
        sessions: classifiable,
        pm_tasks,
        traceparent: crate::observability::current_traceparent(),
    };
    let input_json = serde_json::to_string(&input).context("serializing ClassifyInput")?;
    let python = resolve_python(&services_dir);

    let rt = &cfg.runtime;
    let mut child = match Command::new(&python)
        .arg("-m")
        .arg("agents.run_task_linker")
        .env("LOG_LEVEL", &rt.log_level)
        .env("AGENT_AUTO_FLOOR", rt.agent_auto_floor.to_string())
        .env("AGENT_QUEUE_FLOOR", rt.agent_queue_floor.to_string())
        .env(
            "LLM_PREFER_LOCAL",
            if rt.llm_prefer_local { "1" } else { "0" },
        )
        .env("LLM_BUDGET_PCT", rt.llm_budget_pct.to_string())
        .current_dir(&services_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(python = %python, error = %e, "could not spawn run_task_linker");
            return Ok((total, linked));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input_json.as_bytes())
            .await
            .context("writing to run_task_linker stdin")?;
    }

    let stdout_task = {
        use tokio::io::AsyncReadExt;
        let mut out = child.stdout.take().expect("stdout was piped");
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf).await;
            buf
        })
    };
    let stderr_task = {
        use tokio::io::AsyncReadExt;
        let mut err = child.stderr.take().expect("stderr was piped");
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf).await;
            buf
        })
    };

    let timeout_dur = std::time::Duration::from_secs(cfg.classification_timeout_s);
    let status = match tokio::time::timeout(timeout_dur, child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            warn!(error = %e, "run_task_linker subprocess IO error");
            stdout_task.abort();
            stderr_task.abort();
            return Ok((total, linked));
        }
        Err(_elapsed) => {
            warn!(
                timeout_s = cfg.classification_timeout_s,
                "run_task_linker timed out — killing"
            );
            let _ = child.kill().await;
            stdout_task.abort();
            stderr_task.abort();
            return Ok((total, linked));
        }
    };

    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !stderr_bytes.is_empty() {
        info!(stderr = %String::from_utf8_lossy(&stderr_bytes), "run_task_linker python stderr");
    }

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        warn!(exit_code = ?status.code(), stderr = %stderr, "run_task_linker exited non-zero");
        return Ok((total, linked));
    }

    let classify_output: ClassifyOutput = match serde_json::from_slice(&stdout_bytes) {
        Ok(v) => v,
        Err(e) => {
            let raw = String::from_utf8_lossy(&stdout_bytes);
            warn!(error = %e, stdout = %raw, "run_task_linker stdout not valid JSON");
            return Ok((total, linked));
        }
    };

    for r in &classify_output.results {
        update_session_task(pool, r).await?;
        write_dimensions(pool, r.session_id, &r.dimensions).await?;
        linked += 1;
    }

    Ok((total, linked))
}
