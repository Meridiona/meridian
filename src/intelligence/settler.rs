// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use crate::db::meridian::update_session_category;
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
    session_text: String,
    _category: String,
}

#[tracing::instrument(
    skip_all,
    fields(
        backend = backend.name(),
        sessions_processed = tracing::field::Empty,
        sessions_linked = tracing::field::Empty,
    )
)]
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
                COALESCE(session_text, ''), COALESCE(category, '')
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
            |(id, app_name, duration_s, window_titles, session_text, category)| UnlinkedSession {
                id,
                app_name,
                duration_s,
                window_titles,
                session_text,
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

    let mut linked: i64 = 0;
    for session in &sessions {
        let result = classify_session(session, backend, &task_refs, &valid_keys).await;

        match result {
            Ok((task_key, method)) => {
                if task_key.is_some() {
                    linked += 1;
                }
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

    tracing::Span::current().record("sessions_processed", sessions.len() as i64);
    tracing::Span::current().record("sessions_linked", linked);

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

    let ocr_snippet: String = session
        .session_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect();

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

// ---------------------------------------------------------------------------
// Chrome / browser category settler
// ---------------------------------------------------------------------------

const SETTLE_BATCH_LIMIT: i64 = 20;

// 4 chars ≈ 1 token; caps keep total prompt under ~2,500 tokens (well within FM's 4,096 limit).
const WINDOW_CAP: usize = 500;
const CONTENT_CAP: usize = 1_500;

// Sentinel written when FM returns an unparseable response — prevents infinite retry.
const PARSE_ERROR_SENTINEL: &str = "fm_parse_error";

const VALID_CATEGORIES: &[&str] = &[
    "coding",
    "code_review",
    "meeting",
    "communication",
    "design",
    "documentation",
    "planning",
    "deployment_devops",
    "research",
    "idle_personal",
];

const CATEGORY_SYSTEM: &str = "\
You are an app session classifier. \
Given the app name, session duration, window titles, and optional page content, \
choose the single best category.\n\
\n\
  coding           — writing or editing code: VS Code, Xcode, JetBrains, Vim, terminal builds, localhost testing\n\
  code_review      — reviewing diffs, PRs, or merge requests on GitHub, GitLab, Gerrit\n\
  meeting          — Zoom, Google Meet, Teams, or any live video or audio call\n\
  communication    — Slack, email, Discord, Teams messages, chat\n\
  design           — Figma, Sketch, Adobe XD, Framer, Canva\n\
  documentation    — writing or editing docs: Notion, Confluence, Google Docs, GitBook\n\
  planning         — Jira, Linear, GitHub Issues, project boards, sprint planning\n\
  deployment_devops — CI/CD pipelines, cloud consoles, Kubernetes, monitoring dashboards\n\
  research         — reading docs, Stack Overflow, tutorials, GitHub repos, articles\n\
  idle_personal    — YouTube, social media, news, entertainment, shopping, games";

/// Re-classifies all sessions that still carry the rule-based category using
/// Foundation Models. Only runs when the configured backend is Foundation Models —
/// silently skips otherwise (category stays as-is until the backend is switched).
#[tracing::instrument(
    skip_all,
    fields(
        backend = backend.name(),
        sessions_processed = tracing::field::Empty,
    )
)]
pub async fn settle_all_categories(meridian: &SqlitePool, backend: &LlmBackend) -> Result<()> {
    if !backend.is_foundation_models() {
        debug!("category settler skipped — requires Foundation Models backend");
        return Ok(());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if !crate::intelligence::classifier::backends::foundation::FoundationBackend::is_available() {
        debug!("category settler skipped — Foundation Models not available on this OS");
        return Ok(());
    }

    let rows: Vec<(i64, String, i64, String, String)> = sqlx::query_as(
        "SELECT id, app_name, duration_s, window_titles,
                COALESCE(session_text, '')
         FROM app_sessions
         WHERE category_method = 'rule_based'
           AND duration_s >= 5
         ORDER BY id ASC
         LIMIT ?",
    )
    // Sessions with category_method != 'rule_based' (foundation_models, fm_parse_error, fm_skip)
    // are excluded by the WHERE clause and won't be retried.
    .bind(SETTLE_BATCH_LIMIT)
    .fetch_all(meridian)
    .await
    .context("loading sessions for category settler")?;

    if rows.is_empty() {
        debug!("no sessions pending category re-classification");
        return Ok(());
    }

    info!(
        count = rows.len(),
        "re-classifying sessions via Foundation Models"
    );
    tracing::Span::current().record("sessions_processed", rows.len() as i64);

    for (id, app_name, duration_s, window_titles, session_text) in &rows {
        let user = build_category_prompt(app_name, *duration_s, window_titles, session_text);
        let has_content = !session_text.trim().is_empty();
        match backend.generate_category(CATEGORY_SYSTEM, &user).await {
            Ok((cat, explanation)) => {
                let expl = if explanation.is_empty() {
                    None
                } else {
                    Some(explanation.as_str())
                };
                match parse_category(&cat) {
                    Some(valid_cat) => {
                        if let Err(e) =
                            update_session_category(meridian, *id, valid_cat, 0.9, expl).await
                        {
                            warn!(session_id = id, error = %e, "failed to update category");
                        } else {
                            debug!(session_id = id, app = %app_name, category = valid_cat, "category updated");
                        }
                    }
                    None => {
                        // Shouldn't happen — @Guide(.anyOf) constrains the value.
                        warn!(session_id = id, category = %cat, "unexpected category from structured generation — writing sentinel");
                        if let Err(e) =
                            update_session_category(meridian, *id, PARSE_ERROR_SENTINEL, 0.0, None)
                                .await
                        {
                            warn!(session_id = id, error = %e, "failed to write parse-error sentinel");
                        }
                    }
                }
            }
            Err(e) => {
                let msg = e.to_string();
                let is_language_error =
                    msg.contains("unsupported language") || msg.contains("unsupported Language");
                let is_permanent = is_language_error
                    || msg.contains("context window")
                    || msg.contains("deviceNotEligible")
                    || msg.contains("appleIntelligenceNotEnabled");

                if is_language_error && has_content {
                    // session_text contained non-Latin content that survived strip_non_latin
                    // — retry with titles only (no Content section)
                    warn!(
                        session_id = id,
                        "unsupported language in content — retrying with titles only"
                    );
                    let fallback = build_category_prompt(app_name, *duration_s, window_titles, "");
                    match backend.generate_category(CATEGORY_SYSTEM, &fallback).await {
                        Ok((cat, explanation)) => {
                            let expl = if explanation.is_empty() {
                                None
                            } else {
                                Some(explanation.as_str())
                            };
                            match parse_category(&cat) {
                                Some(valid_cat) => {
                                    if let Err(db_err) =
                                        update_session_category(meridian, *id, valid_cat, 0.8, expl)
                                            .await
                                    {
                                        warn!(session_id = id, error = %db_err, "failed to update category (fallback)");
                                    } else {
                                        debug!(session_id = id, app = %app_name, category = valid_cat, "category updated via titles-only fallback");
                                    }
                                }
                                None => {
                                    warn!(
                                        session_id = id,
                                        "unexpected category from fallback — writing sentinel"
                                    );
                                    let _ = update_session_category(
                                        meridian,
                                        *id,
                                        PARSE_ERROR_SENTINEL,
                                        0.0,
                                        None,
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(retry_err) => {
                            warn!(session_id = id, error = %retry_err, "titles-only fallback also failed — writing sentinel");
                            let _ = update_session_category(
                                meridian,
                                *id,
                                PARSE_ERROR_SENTINEL,
                                0.0,
                                None,
                            )
                            .await;
                        }
                    }
                } else if is_permanent {
                    warn!(session_id = id, error = %e, "FM permanent failure — writing sentinel");
                    if let Err(db_err) =
                        update_session_category(meridian, *id, PARSE_ERROR_SENTINEL, 0.0, None)
                            .await
                    {
                        warn!(session_id = id, error = %db_err, "failed to write FM error sentinel");
                    }
                } else {
                    // Transient error (throttle, network) — leave as rule_based so next tick retries
                    warn!(session_id = id, error = %e, "FM transient failure — will retry next tick");
                }
            }
        }
    }

    Ok(())
}

/// Removes non-Latin-script characters that cause Foundation Models to reject the prompt
/// with "unsupported language". Keeps printable ASCII plus common Latin-block symbols
/// (bullets •, arrows →, chevrons ›, registered ®, etc.). OCR artifacts like Thai Baht ฿
/// (U+0E3F) are the typical culprit.
fn strip_non_latin(s: &str) -> String {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            // Printable ASCII + Latin-1 Supplement (covers accented chars, ®, ©, etc.)
            // + General Punctuation block (bullets, dashes, arrows — U+2000..U+206F)
            // + Arrows block (→ etc. — U+2190..U+21FF)
            // + Enclosed Alphanumerics, box drawing, etc. up to U+25FF
            let keep = (0x0020..=0x00FF).contains(&cp) || (0x2000..=0x25FF).contains(&cp);
            if keep {
                c
            } else {
                ' '
            }
        })
        .collect()
}

pub fn build_category_prompt(
    app_name: &str,
    duration_s: i64,
    window_titles: &str,
    session_text: &str,
) -> String {
    let windows: String = serde_json::from_str::<Vec<serde_json::Value>>(window_titles)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            v.get("window_name")
                .or_else(|| v.get("title"))
                .and_then(|n| n.as_str())
                .filter(|s| !s.is_empty())
                .map(strip_non_latin)
        })
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .take(WINDOW_CAP)
        .collect();

    let content: String = strip_non_latin(session_text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(CONTENT_CAP)
        .collect();

    let mut prompt = format!(
        "App: {} ({}s)\nWindows: {}\n",
        app_name, duration_s, windows
    );
    if !content.is_empty() {
        prompt.push_str(&format!("Content: {}\n", content));
    }
    prompt
}

pub fn parse_category(text: &str) -> Option<&'static str> {
    let trimmed = text.trim().trim_matches('`');
    let value = if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        v.get("category")?.as_str()?.to_lowercase()
    } else {
        trimmed.to_lowercase()
    };
    VALID_CATEGORIES
        .iter()
        .copied()
        .find(|&c| c == value.as_str())
}
