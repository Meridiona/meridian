// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use crate::intelligence::classifier::{ClassifyRequest, LlmBackend, PmTaskRef};

// Sessions processed per run — avoids re-classifying erroring sessions on every poll
const BATCH_LIMIT: i64 = 20;

// Only skip these non-work apps entirely — everything else gets classified
const NON_WORK_APPS: &[&str] = &["Music", "TV", "Podcasts", "News", "Photos", "Maps"];

struct PmTask {
    task_key: String,
    title: String,
}

struct UnlinkedSession {
    id: i64,
    app_name: String,
    duration_s: i64,
    window_titles: String,
    ocr_samples: String,
    // fetched for potential future use in classification heuristics
    _category: String,
}

pub async fn settle_sessions(meridian: &SqlitePool, backend: &LlmBackend) -> Result<()> {
    // Load active (non-done) pm_tasks
    let tasks: Vec<PmTask> = sqlx::query_as::<_, (String, String)>(
        "SELECT task_key, title FROM pm_tasks
         WHERE status_category != 'done'
         ORDER BY task_key",
    )
    .fetch_all(meridian)
    .await
    .context("loading pm_tasks")?
    .into_iter()
    .map(|(task_key, title)| PmTask { task_key, title })
    .collect();

    if tasks.is_empty() {
        debug!("no pm_tasks to classify against — skipping settler");
        return Ok(());
    }

    let task_refs: Vec<PmTaskRef> = tasks
        .iter()
        .map(|t| PmTaskRef {
            key: t.task_key.clone(),
            title: t.title.clone(),
        })
        .collect();

    let valid_keys: HashSet<String> = tasks.iter().map(|t| t.task_key.clone()).collect();

    // Find sessions not yet in ticket_links
    let sessions: Vec<UnlinkedSession> =
        sqlx::query_as::<_, (i64, String, i64, String, String, String)>(
            "SELECT id, app_name, duration_s, window_titles,
                COALESCE(ocr_samples, '[]'), COALESCE(category, '')
         FROM app_sessions
         WHERE id NOT IN (SELECT session_id FROM ticket_links)
         ORDER BY id DESC
         LIMIT ?",
        )
        .bind(BATCH_LIMIT)
        .fetch_all(meridian)
        .await
        .context("loading unlinked sessions")?
        .into_iter()
        .map(
            |(id, app_name, duration_s, window_titles, ocr_samples, category)| UnlinkedSession {
                id,
                app_name,
                duration_s,
                window_titles,
                ocr_samples,
                _category: category,
            },
        )
        .collect();

    if sessions.is_empty() {
        debug!("all sessions already linked — settler idle");
        return Ok(());
    }

    info!(
        count = sessions.len(),
        backend = backend.name(),
        "classifying unlinked sessions"
    );

    for session in &sessions {
        let result = classify_session(session, backend, &task_refs, &valid_keys).await;

        match result {
            Ok((task_key, method)) => {
                let session_type = if task_key.is_some() {
                    "task"
                } else {
                    "overhead"
                };
                let routing = if task_key.is_some() { "auto" } else { "skip" };

                sqlx::query(
                    "INSERT OR IGNORE INTO ticket_links
                       (session_id, task_key, provider, method, confidence, session_type, routing)
                     VALUES (?, ?, 'jira', ?, 0.8, ?, ?)",
                )
                .bind(session.id)
                .bind(&task_key)
                .bind(&method)
                .bind(session_type)
                .bind(routing)
                .execute(meridian)
                .await
                .with_context(|| format!("inserting ticket_link for session {}", session.id))?;

                debug!(
                    session_id = session.id,
                    app = %session.app_name,
                    task_key = ?task_key,
                    method = %method,
                    "session linked"
                );
            }
            Err(e) => {
                warn!(
                    session_id = session.id,
                    error = %e,
                    "classification failed — queuing for manual review"
                );
                // Write a queue entry so it's not retried on every poll
                let _ = sqlx::query(
                    "INSERT OR IGNORE INTO ticket_links
                       (session_id, task_key, provider, method, confidence, session_type, routing)
                     VALUES (?, NULL, NULL, 'error', 0.0, 'unknown', 'queue')",
                )
                .bind(session.id)
                .execute(meridian)
                .await;
            }
        }
    }

    Ok(())
}

async fn classify_session(
    session: &UnlinkedSession,
    backend: &LlmBackend,
    task_refs: &[PmTaskRef],
    valid_keys: &HashSet<String>,
) -> Result<(Option<String>, String)> {
    // Hard-skip obvious non-work apps
    if NON_WORK_APPS.iter().any(|a| *a == session.app_name) {
        return Ok((None, "skipped_non_work".to_string()));
    }

    // Parse window titles from JSON — each entry may be {"title":…,"count":…} or {"window_name":…}
    let windows: Vec<String> =
        serde_json::from_str::<Vec<serde_json::Value>>(&session.window_titles)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| {
                v.get("title")
                    .or_else(|| v.get("window_name"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

    let ocr_snippet: String = serde_json::from_str::<Vec<serde_json::Value>>(&session.ocr_samples)
        .unwrap_or_default()
        .first()
        .and_then(|v| {
            v.get("text")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    let req = ClassifyRequest {
        app_name: session.app_name.clone(),
        duration_s: session.duration_s,
        windows,
        ocr_snippet,
        tasks: task_refs.to_vec(),
        valid_keys: valid_keys.clone(),
    };

    let resp = backend.classify(&req).await?;
    Ok((resp.task_key, resp.method))
}
