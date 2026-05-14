// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::config::Config;

// Maximum sessions per classification batch.
const BATCH_LIMIT: i64 = 50;

// ---------------------------------------------------------------------------
// Serialization structs — sent to and received from the Python subprocess
// ---------------------------------------------------------------------------

/// Full payload sent to `python3 -m agents.run_task_linker` via stdin.
#[derive(Serialize)]
struct ClassifyInput {
    sessions: Vec<SessionPayload>,
    pm_tasks: Vec<TaskPayload>,
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
    task_key: String,
    title: String,
    description_text: String,
    status: String,
    status_category: String,
}

/// Top-level response read from Python stdout.
#[derive(Deserialize)]
struct ClassifyOutput {
    results: Vec<SessionClassification>,
}

/// Per-session classification result returned by Python.
#[derive(Deserialize)]
struct SessionClassification {
    session_id: i64,
    task_key: Option<String>,
    confidence: f64,
    routing: String,
    #[allow(dead_code)]
    reasoning: String,
    method: String,
    #[serde(default)]
    dimensions: HashMap<String, Vec<String>>,
    #[allow(dead_code)]
    elapsed_s: f64,
}

// ---------------------------------------------------------------------------
// DB helpers — all inline, not exported to meridian.rs
// ---------------------------------------------------------------------------

/// Returns the max `id` in `app_sessions`, or `None` if the table is empty.
async fn get_max_session_id(pool: &SqlitePool) -> Result<Option<i64>> {
    let row = sqlx::query_as::<_, (Option<i64>,)>("SELECT MAX(id) FROM app_sessions")
        .fetch_one(pool)
        .await
        .context("reading max app_sessions id")?;
    Ok(row.0)
}

/// Returns the `last_session_id` from `agent_cursor` (row id=1), or 0 if absent.
async fn get_agent_cursor(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT last_session_id FROM agent_cursor WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("reading agent_cursor")?;

    Ok(row.map(|(v,)| v).unwrap_or(0))
}

/// Advance the cursor monotonically — only updates when `session_id` is strictly
/// greater than the stored value so out-of-order writes are safe.
async fn advance_agent_cursor(pool: &SqlitePool, session_id: i64) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE agent_cursor SET last_session_id = ?, updated_at = ? \
         WHERE id = 1 AND ? > last_session_id",
    )
    .bind(session_id)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("advancing agent_cursor to {}", session_id))?;
    Ok(())
}

/// Fetch up to `BATCH_LIMIT` sessions with id > `after_id` that have not yet
/// been classified (absent from `ticket_links`) and meet the minimum duration.
/// Returns tuples of `(id, app_name, duration_s, window_titles_json,
/// session_text_opt, category_opt, confidence_opt)`.
async fn fetch_unclassified_sessions(
    pool: &SqlitePool,
    after_id: i64,
    min_duration_s: i64,
) -> Result<Vec<(i64, String, i64, String, Option<String>, Option<String>, Option<f64>)>> {
    sqlx::query_as::<_, (i64, String, i64, String, Option<String>, Option<String>, Option<f64>)>(
        "SELECT id, app_name, duration_s, window_titles, session_text, category, confidence
         FROM app_sessions
         WHERE id > ?
           AND duration_s > ?
           AND id NOT IN (SELECT session_id FROM ticket_links)
         ORDER BY id ASC
         LIMIT ?",
    )
    .bind(after_id)
    .bind(min_duration_s)
    .bind(BATCH_LIMIT)
    .fetch_all(pool)
    .await
    .context("fetching unclassified sessions")
}

/// Fetch all open (non-done) PM tasks.
async fn fetch_open_pm_tasks(pool: &SqlitePool) -> Result<Vec<TaskPayload>> {
    sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT task_key, title,
                COALESCE(description_text, ''),
                COALESCE(status, ''),
                COALESCE(status_category, '')
         FROM pm_tasks
         WHERE LOWER(status_category) != 'done'",
    )
    .fetch_all(pool)
    .await
    .context("fetching open pm_tasks")?
    .into_iter()
    .map(|(task_key, title, description_text, status, status_category)| {
        Ok(TaskPayload {
            task_key,
            title,
            description_text,
            status,
            status_category,
        })
    })
    .collect()
}

/// Write a trivial (no session text) session as `overhead/skip` without calling
/// the LLM.
async fn write_overhead_link(pool: &SqlitePool, session_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO ticket_links \
         (session_id, task_key, provider, method, confidence, session_type, routing) \
         VALUES (?, NULL, NULL, 'prefilter_trivial', 0.0, 'overhead', 'skip')",
    )
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("writing overhead link for session {}", session_id))?;
    Ok(())
}

/// Persist a classification result from Python into `ticket_links`.
async fn write_ticket_link(pool: &SqlitePool, r: &SessionClassification) -> Result<()> {
    let session_type = if r.task_key.is_some() {
        "task"
    } else {
        "overhead"
    };
    sqlx::query(
        "INSERT OR IGNORE INTO ticket_links \
         (session_id, task_key, provider, method, confidence, session_type, routing) \
         VALUES (?, ?, 'jira', ?, ?, ?, ?)",
    )
    .bind(r.session_id)
    .bind(&r.task_key)
    .bind(&r.method)
    .bind(r.confidence)
    .bind(session_type)
    .bind(&r.routing)
    .execute(pool)
    .await
    .with_context(|| format!("writing ticket_link for session {}", r.session_id))?;
    Ok(())
}

/// Persist multi-label dimension tags returned by Python.
async fn write_dimensions(
    pool: &SqlitePool,
    session_id: i64,
    dims: &HashMap<String, Vec<String>>,
) -> Result<()> {
    for (dimension, values) in dims {
        for value in values {
            sqlx::query(
                "INSERT OR IGNORE INTO session_dimensions \
                 (session_id, dimension, value, confidence, source) \
                 VALUES (?, ?, ?, 0.75, 'hermes_standalone')",
            )
            .bind(session_id)
            .bind(dimension)
            .bind(value)
            .execute(pool)
            .await
            .with_context(|| {
                format!(
                    "writing dimension {}={} for session {}",
                    dimension, value, session_id
                )
            })?;
        }
    }
    Ok(())
}

/// Insert an `agent_runs` row with `status = 'running'` and return its id.
async fn start_agent_run(pool: &SqlitePool) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let row = sqlx::query_as::<_, (i64,)>(
        "INSERT INTO agent_runs (started_at, status) VALUES (?, 'running') RETURNING id",
    )
    .bind(&now)
    .fetch_one(pool)
    .await
    .context("inserting agent_run row")?;
    Ok(row.0)
}

/// Mark an `agent_runs` row as finished.
async fn complete_agent_run(
    pool: &SqlitePool,
    run_id: i64,
    status: &str,
    sessions: i64,
    links: i64,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE agent_runs \
         SET finished_at = ?, status = ?, sessions_processed = ?, links_written = ? \
         WHERE id = ?",
    )
    .bind(&now)
    .bind(status)
    .bind(sessions)
    .bind(links)
    .bind(run_id)
    .execute(pool)
    .await
    .with_context(|| format!("completing agent_run {}", run_id))?;
    Ok(())
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
    // 1. Explicit override from config
    if let Some(ref dir) = cfg.classification_services_dir {
        let p = std::path::Path::new(dir);
        if p.join("agents/run_task_linker.py").exists() {
            return Some(p.to_path_buf());
        }
    }

    // 2. Relative to the binary
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

    // 3. Current working directory
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
pub async fn run_task_linking(pool: &SqlitePool, cfg: &Config) -> Result<()> {
    if !cfg.classification_enabled {
        debug!("classification disabled — skipping");
        return Ok(());
    }

    let wall = Instant::now();
    let run_id = start_agent_run(pool).await?;

    let cursor = get_agent_cursor(pool).await?;

    // On first run (cursor == 0) skip all pre-existing sessions unless backfill
    // is explicitly enabled. This prevents classifying months of history.
    if cursor == 0 && !cfg.classification_backfill {
        if let Some(max_id) = get_max_session_id(pool).await? {
            info!(max_session_id = max_id, "first classification run — advancing cursor to skip historical sessions");
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

    info!(
        sessions = raw_sessions.len(),
        pm_tasks  = pm_tasks.len(),
        cursor,
        min_duration_s = cfg.min_classification_duration_s,
        "classification cycle started"
    );

    // Split into trivial (empty session_text) and classifiable.
    let mut trivial_ids: Vec<i64> = Vec::new();
    let mut classifiable: Vec<SessionPayload> = Vec::new();

    for (id, app_name, duration_s, wt_json, session_text_opt, category, confidence) in raw_sessions
    {
        let text = session_text_opt.unwrap_or_default();
        if text.trim().is_empty() {
            trivial_ids.push(id);
        } else {
            let window_titles = serde_json::from_str(&wt_json)
                .unwrap_or(serde_json::Value::Array(vec![]));
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

    // Handle trivial sessions immediately — no LLM needed.
    let trivial_count = trivial_ids.len() as i64;
    for id in &trivial_ids {
        debug!(session_id = id, "session skipped (empty session_text → overhead/skip)");
    }
    for id in trivial_ids {
        write_overhead_link(pool, id).await?;
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

    // Locate the Python services directory.
    let services_dir = match find_services_dir(cfg) {
        Some(d) => d,
        None => {
            warn!("could not locate services/agents/run_task_linker.py — skipping classification");
            complete_agent_run(pool, run_id, "failed", trivial_count, trivial_count).await?;
            return Ok(());
        }
    };

    // Serialize the payload.
    let batch_size = classifiable.len();
    let input = ClassifyInput {
        sessions: classifiable,
        pm_tasks,
    };
    let input_json = serde_json::to_string(&input).context("serializing ClassifyInput")?;

    // Resolve the Python executable: MERIDIAN_PYTHON > services/.venv/bin/python3 > python3
    let python = resolve_python(&services_dir);

    info!(
        services_dir = %services_dir.display(),
        python = %python,
        batch = batch_size,
        timeout_s = cfg.classification_timeout_s,
        "spawning run_task_linker subprocess"
    );

    // Spawn the Python subprocess.
    let mut child = match Command::new(&python)
        .arg("-m")
        .arg("agents.run_task_linker")
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

    // Write JSON payload to stdin then close the handle so the subprocess sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input_json.as_bytes())
            .await
            .context("writing to run_task_linker stdin")?;
        // stdin drops here, closing the pipe
    }

    // Drain stdout and stderr in background tasks so we can still kill the child on timeout
    // (wait_with_output() would consume the Child handle, making kill() unreachable).
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

    // Wait with timeout; kill the subprocess if it exceeds the deadline.
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

    // Parse the Python output.
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

    // Persist results and advance the cursor.
    let mut links_written: i64 = trivial_count;
    let total_sessions = trivial_count + classify_output.results.len() as i64;

    for r in &classify_output.results {
        write_ticket_link(pool, r).await?;
        write_dimensions(pool, r.session_id, &r.dimensions).await?;
        advance_agent_cursor(pool, r.session_id).await?;
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
// Unit tests — DB helpers only; no subprocess
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn seed_session(pool: &SqlitePool) -> i64 {
        sqlx::query(
            "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
             VALUES ('t', 0, 0, 'success')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO app_sessions (
                app_name, started_at, ended_at, duration_s,
                window_titles, audio_snippets, signals,
                min_frame_id, max_frame_id, frame_count,
                idle_frame_count, etl_run_id
             ) VALUES ('TestApp', 't', 't', 120, '[]', '[]', '{}', 1, 1, 1, 0, 1)",
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    // ── cursor ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_agent_cursor_returns_zero_on_fresh_db() {
        let pool = fresh_db().await;
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn advance_agent_cursor_is_monotonic() {
        let pool = fresh_db().await;
        advance_agent_cursor(&pool, 5).await.unwrap();
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 5);
        // Must not go backwards
        advance_agent_cursor(&pool, 3).await.unwrap();
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn advance_agent_cursor_advances_forward() {
        let pool = fresh_db().await;
        advance_agent_cursor(&pool, 10).await.unwrap();
        advance_agent_cursor(&pool, 20).await.unwrap();
        assert_eq!(get_agent_cursor(&pool).await.unwrap(), 20);
    }

    // ── write_overhead_link ───────────────────────────────────────────────

    #[tokio::test]
    async fn write_overhead_link_inserts_correct_row() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        write_overhead_link(&pool, session_id).await.unwrap();

        let row = sqlx::query_as::<_, (Option<String>, String, String, f64)>(
            "SELECT task_key, method, routing, confidence
             FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.0.is_none());
        assert_eq!(row.1, "prefilter_trivial");
        assert_eq!(row.2, "skip");
        assert_eq!(row.3, 0.0);
    }

    #[tokio::test]
    async fn write_overhead_link_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        write_overhead_link(&pool, session_id).await.unwrap();
        write_overhead_link(&pool, session_id).await.unwrap();
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    // ── write_ticket_link ─────────────────────────────────────────────────

    #[tokio::test]
    async fn write_ticket_link_task_match_stores_correct_row() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            task_key: Some("KAN-42".to_string()),
            confidence: 0.87,
            routing: "auto".to_string(),
            reasoning: "test".to_string(),
            method: "llm_standalone".to_string(),
            dimensions: HashMap::new(),
            elapsed_s: 0.5,
        };
        write_ticket_link(&pool, &r).await.unwrap();

        let row = sqlx::query_as::<_, (String, String, f64, String)>(
            "SELECT task_key, method, confidence, session_type
             FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "KAN-42");
        assert_eq!(row.1, "llm_standalone");
        assert!((row.2 - 0.87).abs() < 1e-9);
        assert_eq!(row.3, "task");
    }

    #[tokio::test]
    async fn write_ticket_link_overhead_when_no_task_key() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            task_key: None,
            confidence: 0.1,
            routing: "skip".to_string(),
            reasoning: "test".to_string(),
            method: "llm_standalone".to_string(),
            dimensions: HashMap::new(),
            elapsed_s: 0.2,
        };
        write_ticket_link(&pool, &r).await.unwrap();

        let row = sqlx::query_as::<_, (String,)>(
            "SELECT session_type FROM ticket_links WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "overhead");
    }

    #[tokio::test]
    async fn write_ticket_link_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let r = SessionClassification {
            session_id,
            task_key: Some("KAN-1".to_string()),
            confidence: 0.9,
            routing: "auto".to_string(),
            reasoning: "test".to_string(),
            method: "llm_standalone".to_string(),
            dimensions: HashMap::new(),
            elapsed_s: 0.1,
        };
        write_ticket_link(&pool, &r).await.unwrap();
        write_ticket_link(&pool, &r).await.unwrap();
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM ticket_links WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    // ── write_dimensions ──────────────────────────────────────────────────

    #[tokio::test]
    async fn write_dimensions_inserts_all_values() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let mut dims = HashMap::new();
        dims.insert(
            "activity".to_string(),
            vec!["coding".to_string(), "reviewing".to_string()],
        );
        dims.insert("tool".to_string(), vec!["cargo".to_string()]);
        write_dimensions(&pool, session_id, &dims).await.unwrap();

        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM session_dimensions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 3);
    }

    #[tokio::test]
    async fn write_dimensions_is_idempotent() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        let mut dims = HashMap::new();
        dims.insert("activity".to_string(), vec!["coding".to_string()]);
        write_dimensions(&pool, session_id, &dims).await.unwrap();
        write_dimensions(&pool, session_id, &dims).await.unwrap();
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM session_dimensions WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    // ── fetch_unclassified_sessions ───────────────────────────────────────

    #[tokio::test]
    async fn fetch_unclassified_filters_already_linked() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        sqlx::query(
            "UPDATE app_sessions SET session_text = 'hello world', duration_s = 60 WHERE id = ?",
        )
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(fetch_unclassified_sessions(&pool, 0, 10).await.unwrap().len(), 1);

        write_overhead_link(&pool, session_id).await.unwrap();
        assert_eq!(
            fetch_unclassified_sessions(&pool, 0, 10).await.unwrap().len(),
            0,
            "linked session must be excluded"
        );
    }

    #[tokio::test]
    async fn fetch_unclassified_filters_by_cursor() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        sqlx::query(
            "UPDATE app_sessions SET session_text = 'hello', duration_s = 60 WHERE id = ?",
        )
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

        let rows = fetch_unclassified_sessions(&pool, session_id, 10).await.unwrap();
        assert!(rows.is_empty(), "cursor at session_id must exclude it");
    }

    #[tokio::test]
    async fn fetch_unclassified_filters_short_duration() {
        let pool = fresh_db().await;
        let session_id = seed_session(&pool).await;
        sqlx::query(
            "UPDATE app_sessions SET session_text = 'hello', duration_s = 5 WHERE id = ?",
        )
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

        let rows = fetch_unclassified_sessions(&pool, 0, 10).await.unwrap();
        assert!(rows.is_empty(), "duration_s ≤ min must be excluded");
    }

    // ── agent_runs ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn agent_run_round_trip() {
        let pool = fresh_db().await;
        let run_id = start_agent_run(&pool).await.unwrap();
        complete_agent_run(&pool, run_id, "success", 5, 3).await.unwrap();

        let row = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT status, sessions_processed, links_written
             FROM agent_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "success");
        assert_eq!(row.1, 5);
        assert_eq!(row.2, 3);
    }
}
