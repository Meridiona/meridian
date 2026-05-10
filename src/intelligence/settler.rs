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

// ---------------------------------------------------------------------------
// Chrome / browser category settler
// ---------------------------------------------------------------------------

const BROWSER_BATCH_LIMIT: i64 = 10;

// 4 chars ≈ 1 token; caps keep total prompt under ~2,500 tokens (well within FM's 4,096 limit).
const OCR_CAP: usize = 0; // OCR disabled — concatenated screencap text triggers FM language detector
const WINDOW_CAP: usize = 500;
const ELEMENTS_CAP: usize = 1_500;

// Sentinel written when FM returns an unparseable response — prevents infinite retry.
const PARSE_ERROR_SENTINEL: &str = "fm_parse_error";

const VALID_CATEGORIES: &[&str] = &[
    "code_review",
    "research",
    "documentation",
    "planning",
    "communication",
    "deployment_devops",
    "idle_personal",
];

const CATEGORY_SYSTEM: &str = "\
You are a JSON-only classifier. Given a Chrome browser session return exactly \
{\"category\": \"VALUE\", \"why\": \"one sentence reason\"}.\n\
\n\
Valid values:\n\
  code_review      — PR diffs, GitHub pull requests, code comments, merge requests\n\
  research         — docs, Stack Overflow, GitHub repos (reading), tutorials, articles\n\
  documentation    — writing/editing: Notion, Confluence, Google Docs, GitBook\n\
  planning         — Jira, Linear, GitHub Issues, project boards, sprint planning\n\
  communication    — Gmail, Slack web, Discord web, email, chat\n\
  deployment_devops — CI/CD dashboards, cloud consoles, deploy logs, monitoring\n\
  idle_personal    — YouTube, social media, news, entertainment, shopping\n\
\n\
Return ONLY {\"category\": \"VALUE\", \"why\": \"one sentence reason\"}. No explanation outside the JSON.";

/// Re-classifies browser sessions that still carry the rule-based category using
/// Foundation Models. Only runs when the configured backend is Foundation Models —
/// silently skips otherwise (category stays as-is until the backend is switched).
pub async fn settle_chrome_categories(meridian: &SqlitePool, backend: &LlmBackend) -> Result<()> {
    if !backend.is_foundation_models() {
        debug!("Chrome category settler skipped — requires Foundation Models backend");
        return Ok(());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if !crate::intelligence::classifier::backends::foundation::FoundationBackend::is_available() {
        debug!("Chrome category settler skipped — Foundation Models not available on this OS");
        return Ok(());
    }

    let rows: Vec<(i64, String, i64, String, String, String)> = sqlx::query_as(
        "SELECT id, app_name, duration_s, window_titles,
                COALESCE(ocr_samples, '[]'), COALESCE(elements_samples, '[]')
         FROM app_sessions
         WHERE category_method = 'rule_based'
           AND duration_s >= 5
           AND (lower(app_name) LIKE '%chrome%'
                OR lower(app_name) LIKE '%safari%'
                OR lower(app_name) LIKE '%firefox%'
                OR lower(app_name) LIKE '%arc%'
                OR lower(app_name) LIKE '%edge%'
                OR lower(app_name) LIKE '%brave%')
         ORDER BY id ASC
         LIMIT ?",
    )
    // Sessions with category_method != 'rule_based' (foundation_models, fm_parse_error, fm_skip)
    // are excluded by the WHERE clause and won't be retried.
    .bind(BROWSER_BATCH_LIMIT)
    .fetch_all(meridian)
    .await
    .context("loading browser sessions for category settler")?;

    if rows.is_empty() {
        debug!("no browser sessions pending category re-classification");
        return Ok(());
    }

    info!(
        count = rows.len(),
        "re-classifying browser sessions via Foundation Models"
    );

    for (id, app_name, duration_s, window_titles, ocr_samples, elements_samples) in &rows {
        let user = build_category_prompt(*duration_s, window_titles, ocr_samples, elements_samples);
        match backend.raw_generate(CATEGORY_SYSTEM, &user).await {
            Ok(text) => match parse_category_response(&text) {
                Some(resp) => {
                    if let Err(e) = update_session_category(meridian, *id, resp.category, 0.9).await
                    {
                        warn!(session_id = id, error = %e, "failed to update category");
                    } else {
                        debug!(session_id = id, app = %app_name, category = resp.category, why = %resp.why, "category updated");
                    }
                }
                None => {
                    warn!(session_id = id, raw = %text, "could not parse category response — writing sentinel");
                    if let Err(e) =
                        update_session_category(meridian, *id, PARSE_ERROR_SENTINEL, 0.0).await
                    {
                        warn!(session_id = id, error = %e, "failed to write parse-error sentinel");
                    }
                }
            },
            Err(e) => {
                let msg = e.to_string();
                let is_permanent = msg.contains("unsupported language")
                    || msg.contains("unsupported Language")
                    || msg.contains("context window")
                    || msg.contains("deviceNotEligible")
                    || msg.contains("appleIntelligenceNotEnabled");
                if is_permanent {
                    // Write sentinel so this session is never retried
                    warn!(session_id = id, error = %e, "FM permanent failure — writing sentinel");
                    if let Err(db_err) =
                        update_session_category(meridian, *id, PARSE_ERROR_SENTINEL, 0.0).await
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
    duration_s: i64,
    window_titles: &str,
    ocr_samples: &str,
    elements_samples: &str,
) -> String {
    let windows: String = serde_json::from_str::<Vec<serde_json::Value>>(window_titles)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            // window_titles may use either "window_name" (browser sessions) or "title" (general)
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

    // Use only the first OCR sample capped at OCR_CAP chars.
    // Multiple frames or large single frames create dense concatenated text that triggers
    // FM's language detector even when the content is English.
    let ocr: String = serde_json::from_str::<Vec<serde_json::Value>>(ocr_samples)
        .unwrap_or_default()
        .into_iter()
        .next()
        .and_then(|v| v.get("text")?.as_str().map(strip_non_latin))
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(OCR_CAP)
        .collect();

    let elements: String = serde_json::from_str::<Vec<serde_json::Value>>(elements_samples)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.get("text")?.as_str().map(strip_non_latin))
        .collect::<Vec<_>>()
        .join(", ")
        .chars()
        .take(ELEMENTS_CAP)
        .collect();

    let mut prompt = format!("Chrome session ({}s)\nWindows: {}\n", duration_s, windows);
    if !ocr.is_empty() {
        prompt.push_str(&format!("Screen:\n{}\n", ocr));
    }
    if !elements.is_empty() {
        prompt.push_str(&format!("UI elements: {}\n", elements));
    }
    prompt
}

pub struct CategoryResult {
    pub category: &'static str,
    pub why: String,
}

pub fn parse_category_response(text: &str) -> Option<CategoryResult> {
    // Strip optional markdown fences: ```json ... ``` or `...`
    let trimmed = text.trim();
    let trimmed = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches('`')
            .trim_start_matches(|c: char| c.is_alphabetic()) // strip optional language tag (json)
            .trim_end_matches('`')
            .trim()
    } else {
        trimmed.trim_matches('`').trim()
    };
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let value = v.get("category")?.as_str()?.to_lowercase();
    let category = VALID_CATEGORIES
        .iter()
        .copied()
        .find(|&c| c == value.as_str())?;
    let why = v
        .get("why")
        .and_then(|w| w.as_str())
        .unwrap_or("")
        .to_string();
    Some(CategoryResult { category, why })
}

pub fn parse_category(text: &str) -> Option<&'static str> {
    parse_category_response(text).map(|r| r.category)
}
