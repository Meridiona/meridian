// meridian — normalises screenpipe activity into structured app sessions

mod db;
mod db_write;

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
    fetch_sessions_in_range, fetch_unclassified_sessions, get_agent_cursor, get_max_session_id,
};
use db_write::{
    advance_agent_cursor, complete_agent_run, start_agent_run, update_session_overhead,
    update_session_task, write_dimensions,
};

// One session per daemon tick — at 30-60s cadence there is typically one new
// session. The backfill binary handles bulk catch-up after downtime.
pub(super) const BATCH_LIMIT: i64 = 1;

// ---------------------------------------------------------------------------
// Startup preflight check
// ---------------------------------------------------------------------------

pub fn check_classification_ready(cfg: &Config) -> Result<()> {
    if !cfg.classification_enabled {
        return Ok(());
    }

    let services_dir = find_services_dir(cfg).ok_or_else(|| {
        anyhow::anyhow!(
            "classification is enabled but services/agents/run_task_linker.py not found\n\
             Fix: run  bash scripts/setup-services.sh  from the repo root"
        )
    })?;

    let hermes_config = services_dir.join(".hermes/config.yaml");
    if !hermes_config.exists() {
        anyhow::bail!(
            "services/.hermes/config.yaml not found — hermes is not configured\n\
             Fix: cd services && bash scripts/setup-hermes.sh"
        );
    }

    let hermes_env = services_dir.join(".hermes/.env");
    if !hermes_env.exists() {
        anyhow::bail!(
            "services/.hermes/.env not found\n\
             Fix: cd services && bash scripts/setup-hermes.sh"
        );
    }

    let python = resolve_python(&services_dir);
    let check = std::process::Command::new(&python)
        .arg("-c")
        .arg("import run_agent")
        .current_dir(&services_dir)
        .output();

    match check {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "Python cannot import 'run_agent' — hermes is not installed in the venv\n\
                 Fix: cd services && bash scripts/setup-services.sh\n\
                 Detail: {}",
                stderr.trim()
            );
        }
        Err(e) => {
            anyhow::bail!(
                "Cannot run Python binary '{}'\n\
                 Fix: cd services && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt\n\
                 Detail: {}",
                python,
                e
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Serialization types — sent to and received from the Python subprocess
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SessionPayload {
    id: i64,
    app_name: String,
    duration_s: i64,
    session_text: String,
    session_text_source: String,
    window_titles: String,
    started_at: String,
    ended_at: String,
    category: Option<String>,
    confidence: Option<f64>,
    audio_snippets: Vec<String>,
}

#[derive(Serialize)]
struct TaskPayload {
    task_key: String,
    title: String,
    description_text: String,
    status: String,
    status_category: String,
    issue_type: String,
    epic_title: String,
    sprint_name: String,
}

#[derive(Serialize)]
struct ClassifyInput {
    sessions: Vec<SessionPayload>,
    pm_tasks: Vec<TaskPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    traceparent: Option<String>,
}

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

#[derive(Deserialize)]
pub(super) struct SessionClassification {
    pub(super) session_id: i64,
    pub(super) task_key: Option<String>,
    pub(super) confidence: f64,
    #[serde(default = "default_routing")]
    pub(super) routing: String,
    #[serde(default = "default_session_type")]
    pub(super) session_type: String,
    pub(super) reasoning: String,
    pub(super) method: String,
    #[serde(default)]
    pub(super) dimensions: HashMap<String, Vec<String>>,
    #[allow(dead_code)]
    pub(super) elapsed_s: f64,
}

// ---------------------------------------------------------------------------
// Services directory and Python executable resolution
// ---------------------------------------------------------------------------

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
// DB helpers
// ---------------------------------------------------------------------------

async fn fetch_open_pm_tasks(pool: &SqlitePool) -> Result<Vec<TaskPayload>> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ),
    >(
        "SELECT task_key,
                COALESCE(title, ''),
                COALESCE(description_text, ''),
                COALESCE(status, ''),
                COALESCE(status_category, ''),
                COALESCE(issue_type, ''),
                COALESCE(epic_title, ''),
                COALESCE(sprint_name, '')
         FROM pm_tasks
         WHERE LOWER(COALESCE(status_category, '')) != 'done'",
    )
    .fetch_all(pool)
    .await
    .context("fetching open pm_tasks")?;

    Ok(rows
        .into_iter()
        .map(
            |(
                task_key,
                title,
                description_text,
                status,
                status_category,
                issue_type,
                epic_title,
                sprint_name,
            )| TaskPayload {
                task_key,
                title,
                description_text,
                status,
                status_category,
                issue_type,
                epic_title,
                sprint_name,
            },
        )
        .collect())
}

// ---------------------------------------------------------------------------
// Subprocess helper
// ---------------------------------------------------------------------------

async fn run_subprocess(
    python: &str,
    services_dir: &std::path::Path,
    input_json: &str,
    cfg: &Config,
) -> Result<Option<ClassifyOutput>> {
    let rt = &cfg.runtime;
    let mut child = match Command::new(python)
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
        .current_dir(services_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(python = %python, error = %e, "could not spawn run_task_linker");
            return Ok(None);
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
            return Ok(None);
        }
        Err(_elapsed) => {
            warn!(
                timeout_s = cfg.classification_timeout_s,
                "run_task_linker subprocess timed out — killing"
            );
            let _ = child.kill().await;
            stdout_task.abort();
            stderr_task.abort();
            return Ok(None);
        }
    };

    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !stderr_bytes.is_empty() {
        debug!(
            stderr = %String::from_utf8_lossy(&stderr_bytes),
            "run_task_linker python stderr"
        );
    }

    if !status.success() {
        warn!(
            exit_code = ?status.code(),
            stderr = %String::from_utf8_lossy(&stderr_bytes),
            "run_task_linker exited with non-zero status"
        );
        return Ok(None);
    }

    match serde_json::from_slice(&stdout_bytes) {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            warn!(
                error = %e,
                stdout = %String::from_utf8_lossy(&stdout_bytes),
                "run_task_linker stdout is not valid JSON — hermes may be printing to stdout"
            );
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

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
        cursor,
        min_duration_s = cfg.min_classification_duration_s,
        "classification cycle started"
    );

    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable: Vec<SessionPayload> = Vec::new();

    for (
        id,
        app_name,
        duration_s,
        wt_json,
        session_text_opt,
        started_at,
        ended_at,
        category,
        confidence,
        text_source,
    ) in raw_sessions
    {
        if session_text_opt.as_deref().unwrap_or("").trim().is_empty() {
            trivial_ids.push(id);
        } else {
            classifiable.push(SessionPayload {
                id,
                app_name,
                duration_s,
                session_text: session_text_opt.unwrap_or_default(),
                session_text_source: text_source,
                window_titles: wt_json,
                started_at,
                ended_at,
                category,
                confidence,
                audio_snippets: vec![],
            });
        }
    }

    let trivial_count = trivial_ids.len() as i64;
    for id in trivial_ids {
        debug!(
            session_id = id,
            "session skipped (empty session_text → overhead/skip)"
        );
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

    let python = resolve_python(&services_dir);
    let batch_size = classifiable.len();

    let input = ClassifyInput {
        sessions: classifiable,
        pm_tasks,
        traceparent: crate::observability::current_traceparent(),
    };
    let input_json = serde_json::to_string(&input).context("serializing ClassifyInput")?;

    info!(
        services_dir = %services_dir.display(),
        python = %python,
        batch = batch_size,
        "spawning run_task_linker subprocess"
    );

    let classify_output = match run_subprocess(&python, &services_dir, &input_json, cfg).await? {
        Some(out) => out,
        None => {
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

    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable: Vec<SessionPayload> = Vec::new();

    for (
        id,
        app_name,
        duration_s,
        wt_json,
        session_text_opt,
        started_at,
        ended_at,
        category,
        confidence,
        text_source,
    ) in raw_sessions
    {
        if session_text_opt.as_deref().unwrap_or("").trim().is_empty() {
            trivial_ids.push(id);
        } else {
            classifiable.push(SessionPayload {
                id,
                app_name,
                duration_s,
                session_text: session_text_opt.unwrap_or_default(),
                session_text_source: text_source,
                window_titles: wt_json,
                started_at,
                ended_at,
                category,
                confidence,
                audio_snippets: vec![],
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
            println!("  session {}: would classify via hermes", s.id);
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

    let pm_tasks = fetch_open_pm_tasks(pool).await?;
    let python = resolve_python(&services_dir);

    let input = ClassifyInput {
        sessions: classifiable,
        pm_tasks,
        traceparent: crate::observability::current_traceparent(),
    };
    let input_json = serde_json::to_string(&input).context("serializing ClassifyInput")?;

    let classify_output = match run_subprocess(&python, &services_dir, &input_json, cfg).await? {
        Some(out) => out,
        None => return Ok((total, linked)),
    };

    for r in &classify_output.results {
        update_session_task(pool, r).await?;
        write_dimensions(pool, r.session_id, &r.dimensions).await?;
        linked += 1;
    }

    Ok((total, linked))
}
