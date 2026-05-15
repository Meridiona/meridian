// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::db::meridian::update_session_category;
use crate::intelligence::category_llm::LlmBackend;

async fn get_settler_cursor(pool: &SqlitePool) -> Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT last_settled_session_id FROM agent_cursor WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .context("reading settler cursor")?;
    Ok(row.0)
}

async fn advance_settler_cursor(pool: &SqlitePool, session_id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE agent_cursor
         SET last_settled_session_id = MAX(last_settled_session_id, ?),
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE id = 1",
    )
    .bind(session_id)
    .execute(pool)
    .await
    .context("advancing settler cursor")?;
    Ok(())
}

async fn get_max_session_id(pool: &SqlitePool) -> Result<Option<i64>> {
    let row: (Option<i64>,) = sqlx::query_as("SELECT MAX(id) FROM app_sessions")
        .fetch_one(pool)
        .await
        .context("reading max session id")?;
    Ok(row.0)
}

// ---------------------------------------------------------------------------
// Category settler — re-classifies rule_based sessions via Foundation Models
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

/// Re-classifies sessions that still carry the rule-based category using
/// Foundation Models. Only processes sessions created after the daemon started —
/// historical sessions are skipped on first run unless `backfill` is true.
/// Only runs when the configured backend is Foundation Models.
#[tracing::instrument(
    skip_all,
    fields(
        backend = backend.name(),
        sessions_processed = tracing::field::Empty,
    )
)]
pub async fn settle_all_categories(
    meridian: &SqlitePool,
    backend: &LlmBackend,
    min_duration_s: i64,
    backfill: bool,
) -> Result<()> {
    if !backend.is_foundation_models() {
        debug!("category settler skipped — requires Foundation Models backend");
        return Ok(());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if !crate::intelligence::category_llm::backends::foundation::FoundationBackend::is_available() {
        debug!("category settler skipped — Foundation Models not available on this OS");
        return Ok(());
    }

    let cursor = get_settler_cursor(meridian).await?;

    // First run: fast-forward cursor to current max session id to skip history.
    if cursor == 0 && !backfill {
        if let Some(max_id) = get_max_session_id(meridian).await? {
            info!(
                max_session_id = max_id,
                "first category settler run — advancing cursor to skip historical sessions"
            );
            advance_settler_cursor(meridian, max_id).await?;
        }
        return Ok(());
    }

    let rows: Vec<(i64, String, i64, String, String)> = sqlx::query_as(
        "SELECT id, app_name, duration_s, window_titles,
                COALESCE(session_text, '')
         FROM app_sessions
         WHERE category_method = 'rule_based'
           AND duration_s >= ?
           AND id > ?
         ORDER BY id ASC
         LIMIT ?",
    )
    // Sessions with category_method != 'rule_based' (foundation_models, fm_parse_error, fm_skip)
    // are excluded by the WHERE clause and won't be retried.
    .bind(min_duration_s)
    .bind(cursor)
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

    // Track the highest id definitively processed (success or permanent failure).
    // Transient failures keep the cursor where it is so next tick retries them.
    let mut max_settled: i64 = cursor;

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
                            max_settled = max_settled.max(*id);
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
                        } else {
                            max_settled = max_settled.max(*id);
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
                                        max_settled = max_settled.max(*id);
                                    }
                                }
                                None => {
                                    warn!(
                                        session_id = id,
                                        "unexpected category from fallback — writing sentinel"
                                    );
                                    if update_session_category(
                                        meridian, *id, PARSE_ERROR_SENTINEL, 0.0, None,
                                    )
                                    .await
                                    .is_ok()
                                    {
                                        max_settled = max_settled.max(*id);
                                    }
                                }
                            }
                        }
                        Err(retry_err) => {
                            warn!(session_id = id, error = %retry_err, "titles-only fallback also failed — writing sentinel");
                            if update_session_category(
                                meridian, *id, PARSE_ERROR_SENTINEL, 0.0, None,
                            )
                            .await
                            .is_ok()
                            {
                                max_settled = max_settled.max(*id);
                            }
                        }
                    }
                } else if is_permanent {
                    warn!(session_id = id, error = %e, "FM permanent failure — writing sentinel");
                    if update_session_category(meridian, *id, PARSE_ERROR_SENTINEL, 0.0, None)
                        .await
                        .is_ok()
                    {
                        max_settled = max_settled.max(*id);
                    }
                } else {
                    // Transient error (throttle, network) — do NOT advance cursor; retry next tick.
                    warn!(session_id = id, error = %e, "FM transient failure — will retry next tick");
                }
            }
        }
    }

    if max_settled > cursor {
        advance_settler_cursor(meridian, max_settled).await?;
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
    let candidate = if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        v.get("category")?.as_str()?.to_lowercase()
    } else {
        trimmed.to_lowercase()
    };

    // Exact match (fast path — structured generation should land here)
    if let Some(cat) = VALID_CATEGORIES
        .iter()
        .copied()
        .find(|&c| c == candidate.as_str())
    {
        return Some(cat);
    }

    // Unambiguous near-misses the model emits instead of exact category names
    let alias = match candidate.as_str() {
        "development" | "developer" => "coding",
        "devops" | "infra" | "infrastructure" | "ci/cd" => "deployment_devops",
        "docs" | "doc" | "wiki" => "documentation",
        "personal" | "entertainment" => "idle_personal",
        _ => "",
    };
    if !alias.is_empty() {
        return VALID_CATEGORIES.iter().copied().find(|&c| c == alias);
    }

    // Last resort: find any valid category as a whole token in verbose prose responses.
    // Handles "Based on the session… the primary activity appears to be coding."
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    VALID_CATEGORIES.iter().copied().find(|&cat| {
        let cb = cat.as_bytes();
        let cl = cb.len();
        bytes.windows(cl).enumerate().any(|(i, w)| {
            w == cb
                && (i == 0 || !matches!(bytes[i - 1], b'a'..=b'z' | b'0'..=b'9' | b'_'))
                && (i + cl >= bytes.len()
                    || !matches!(bytes[i + cl], b'a'..=b'z' | b'0'..=b'9' | b'_'))
        })
    })
}
