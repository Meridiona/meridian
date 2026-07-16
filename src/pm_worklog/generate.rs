//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The "Generate worklog" engine behind a day-task card: ONE provider-agnostic LLM
//! call that matches a day-level workstream to an existing ticket (or proposes a
//! new one) and writes a high-level status update, then — on approve — creates the
//! ticket if proposed, posts the update as a plain comment, and links the day-task.
//!
//! This re-implements centrally, in Rust and through the user's chosen LLM
//! provider, the match/propose logic that previously lived only in an orphaned
//! MLX-bound Python pipeline — so it runs with first-class tracing
//! ([`crate::llm::complete`]'s free `llm.*` subspans) and posts through the same
//! provider write-back the rest of the pipeline uses.
//!
//! # Who calls this
//! The `worklog-generate` / `worklog-generate-approve` CLI subcommands (`main.rs`)
//! → the tray `generate_day_task_worklog` / `approve_day_task_worklog` commands →
//! the day-task detail panel.
//!
//! # Related
//! - [`meridian_core::day_task_worklogs`] — the ledger this reads/writes.
//! - [`meridian_core::day_tasks`] — the day-task workstream this drafts a worklog for.
//! - [`super::post_comment`] — the plain-comment primitive approve posts through.
//! - [`super::create`] — the ticket-CREATE dispatcher approve calls for a proposal.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

use meridian_core::day_task_worklogs::{
    self, DayTaskWorklogDraft, DraftUpsert, GeneratedWorklogMatch, GeneratedWorklogPropose,
    GeneratedWorklogUpdate,
};

use crate::config::Config;
use crate::llm::{self, parse_json_object, prompts, PromptRequest};

/// Candidate cap handed to the model. Enough breadth for the day's board without
/// blowing the context window; mirrors the Python matcher's lean candidate set.
const MAX_CANDIDATES: usize = 30;

/// Token budget for the combined match/propose/update answer — a handful of short
/// fields plus a few bullet arrays.
const GENERATE_MAX_TOKENS: u32 = 1500;

/// One open ticket the matcher may bind to, rendered for the prompt.
struct Candidate {
    task_key: String,
    provider: String,
    doc: String,
}

/// The result of [`approve`], serialized to the CLI's one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct ApproveResult {
    pub posted: bool,
    pub target_key: Option<String>,
    pub created_task_key: Option<String>,
    pub created: bool,
    pub browse_url: Option<String>,
    pub error: Option<String>,
}

/// Generate (or regenerate) the draft for `(day_local, task_id)`. Loads the
/// day-task's whole-story summary, fetches the open PM tickets as candidates, makes
/// ONE structured LLM call (match XOR propose + a status update), validates it, and
/// UPSERTs a `drafted` row (overwriting only a still-`drafted` one). Returns the
/// persisted draft.
#[tracing::instrument(skip(pool, config))]
pub async fn generate(
    pool: &SqlitePool,
    config: &Config,
    day_local: &str,
    task_id: &str,
) -> Result<DayTaskWorklogDraft> {
    let span = tracing::info_span!(
        "worklog.generate",
        day = day_local,
        task_id,
        llm_provider = Empty,
        matched_task_key = Empty,
        proposed = Empty,
        n_candidates = Empty,
    );
    async move {
        let report = load_workstream_report(pool, day_local, task_id).await?;
        let candidates = fetch_open_candidates(pool).await?;
        tracing::Span::current().record("n_candidates", candidates.len());

        let user = build_user_prompt(&report, &candidates);
        let req = PromptRequest {
            system: prompts::WORKLOG_GENERATE,
            user,
            schema: Some(prompts::worklog_generate_schema()),
            max_tokens: GENERATE_MAX_TOKENS,
            label: format!("worklog-generate {day_local} {task_id}"),
        };

        let (out, provider) = llm::complete(&req)
            .await
            .map_err(|e| anyhow::anyhow!("generate-worklog LLM call failed: {e}"))?;
        tracing::Span::current().record("llm_provider", provider.as_str());

        let parsed = parse_answer(&out.text).context(
            "the AI answer could not be parsed into a match/propose/update - try regenerating",
        )?;

        // Resolve the target provider: a match takes its provider from the matched
        // ticket; a proposal targets the first configured provider.
        let (draft_provider, target_key) = match (&parsed.match_, &parsed.propose) {
            (Some(m), None) => {
                let p = resolve_provider_for_key(pool, &m.task_key).await?.context(
                    "the AI matched a ticket that is not on the board - try regenerating",
                )?;
                tracing::Span::current().record("matched_task_key", m.task_key.as_str());
                tracing::Span::current().record("proposed", false);
                (p, Some(m.task_key.clone()))
            }
            (None, Some(_)) => {
                let p = config
                    .pm_providers
                    .first()
                    .map(|p| p.provider_name().to_string())
                    .context("no PM tracker is connected - connect one first")?;
                tracing::Span::current().record("proposed", true);
                (p, None)
            }
            // XOR violation: neither or both. Tolerated at the schema level, rejected here.
            _ => bail!("the AI must return exactly one of match or propose - try regenerating"),
        };

        let now = chrono::Utc::now().to_rfc3339();
        let upsert = DraftUpsert {
            provider: draft_provider,
            match_: parsed.match_,
            propose: parsed.propose,
            update: parsed.update,
            reasoning: parsed.reasoning,
            target_key,
        };
        let draft = day_task_worklogs::upsert_draft(pool, day_local, task_id, upsert, &now)
            .await
            .context("persisting the generated worklog draft")?;

        tracing::info!(
            provider = draft.provider,
            state = draft.state,
            matched = draft.match_.is_some(),
            proposed = draft.propose.is_some(),
            input_tokens = out.input_tokens,
            output_tokens = out.output_tokens,
            elapsed_s = out.elapsed_s,
            "worklog: draft generated"
        );
        Ok(draft)
    }
    .instrument(span)
    .await
}

/// Approve the current draft: create the proposed ticket if needed (persisting its
/// key BEFORE posting so a retry never re-creates), post the status update as a
/// plain comment, mark the row `posted`, and link the day-task. Idempotent — an
/// already-`posted` row short-circuits. On failure the row is marked with the error
/// and left retry-safe (never a create-without-post that re-creates).
#[tracing::instrument(skip(pool, config))]
pub async fn approve(
    pool: &SqlitePool,
    config: &Config,
    day_local: &str,
    task_id: &str,
) -> Result<ApproveResult> {
    let span = tracing::info_span!(
        "worklog.generate.approve",
        day = day_local,
        task_id,
        provider = Empty,
        created = Empty,
        target_key = Empty,
        posted = Empty,
    );
    async move {
        let draft = day_task_worklogs::get_day_task_worklog(pool, day_local, task_id)
            .await?
            .context("no generated draft to approve - generate one first")?;
        tracing::Span::current().record("provider", draft.provider.as_str());

        // Idempotent: already posted → return the recorded result, no re-post.
        if draft.state == "posted" {
            tracing::info!("worklog: approve is a no-op (already posted)");
            tracing::Span::current().record("posted", true);
            return Ok(ApproveResult {
                posted: true,
                target_key: draft.target_key.clone(),
                created_task_key: draft.created_task_key.clone(),
                created: false,
                browse_url: draft.browse_url.clone(),
                error: None,
            });
        }

        let now = chrono::Utc::now().to_rfc3339();
        day_task_worklogs::mark_approved(pool, day_local, task_id, &now).await?;

        match approve_inner(pool, config, day_local, task_id, &draft).await {
            Ok(res) => {
                tracing::Span::current().record("created", res.created);
                tracing::Span::current().record("posted", res.posted);
                if let Some(k) = &res.target_key {
                    tracing::Span::current().record("target_key", k.as_str());
                }
                tracing::info!(
                    target_key = res.target_key.as_deref().unwrap_or(""),
                    created = res.created,
                    "worklog: approved and posted"
                );
                Ok(res)
            }
            Err(e) => {
                let now = chrono::Utc::now().to_rfc3339();
                let msg = format!("{e:#}");
                // Record the failure; leave the row `approved` for a safe retry.
                let _ = day_task_worklogs::mark_error(pool, day_local, task_id, &msg, &now).await;
                tracing::warn!(error = %e, "worklog: approve failed - left retry-safe");
                tracing::Span::current().record("posted", false);
                Ok(ApproveResult {
                    posted: false,
                    target_key: draft.target_key.clone(),
                    created_task_key: draft.created_task_key.clone(),
                    created: false,
                    browse_url: None,
                    error: Some(msg),
                })
            }
        }
    }
    .instrument(span)
    .await
}

/// The fallible body of [`approve`]: create-if-proposed → post → mark → link.
/// Every failure bubbles up so the caller records it and leaves the row retry-safe.
async fn approve_inner(
    pool: &SqlitePool,
    config: &Config,
    day_local: &str,
    task_id: &str,
    draft: &DayTaskWorklogDraft,
) -> Result<ApproveResult> {
    let provider = &draft.provider;

    // Resolve the target key. For a proposal, create the ticket first (unless a
    // prior attempt already created it — its key is persisted), then persist the
    // key BEFORE posting so a retry can never double-create.
    let (target_key, created) = match (&draft.match_, &draft.propose) {
        (Some(m), None) => (m.task_key.clone(), false),
        (None, Some(p)) => {
            if let Some(existing) = &draft.created_task_key {
                (existing.clone(), false)
            } else {
                let sample = fetch_sample_task_key(pool, provider).await;
                let key = crate::pm_worklog::create::create_ticket(
                    config,
                    provider,
                    &p.title,
                    &p.description,
                    &p.issue_type,
                    sample.as_deref(),
                )
                .await
                .context("creating the proposed ticket on the tracker")?;
                let now = chrono::Utc::now().to_rfc3339();
                day_task_worklogs::mark_created(pool, day_local, task_id, &key, &now)
                    .await
                    .context("persisting the created ticket key")?;
                (key, true)
            }
        }
        _ => bail!("draft has neither a match nor a proposal to post"),
    };

    let body = render_update_body(&draft.update);
    let comment_id = super::post_comment::post_comment(config, provider, &target_key, &body)
        .await
        .context("posting the status-update comment")?;

    let browse = browse_url(config, provider, &target_key);
    let now = chrono::Utc::now().to_rfc3339();
    day_task_worklogs::mark_posted(
        pool,
        day_local,
        task_id,
        &target_key,
        &comment_id,
        browse.as_deref(),
        &now,
    )
    .await
    .context("marking the draft posted")?;

    // Link the day-task card to the ticket (best-effort — the post already
    // succeeded, so a link-write failure must not fail the approve).
    if let Err(e) = link_day_task(pool, day_local, task_id, &target_key, &now).await {
        tracing::warn!(error = %e, "worklog: could not set day_tasks.linked_ticket");
    }

    // For a proposal the target key IS the created key (created this call or a
    // prior one); a match creates nothing.
    let created_task_key = if draft.propose.is_some() {
        Some(target_key.clone())
    } else {
        None
    };
    Ok(ApproveResult {
        posted: true,
        target_key: Some(target_key),
        created_task_key,
        created,
        browse_url: browse,
        error: None,
    })
}

// ── Prompt assembly ───────────────────────────────────────────────────────────

/// A day-task's whole-story report as the matcher sees it.
struct WorkstreamReport {
    title: String,
    summary: Vec<String>,
    minutes: i64,
}

/// Load the day-task workstream, or a clean error if it doesn't exist.
async fn load_workstream_report(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> Result<WorkstreamReport> {
    let resp = meridian_core::day_tasks::get_day_tasks(pool, day_local)
        .await
        .context("loading day tasks")?;
    let task = resp
        .tasks
        .into_iter()
        .find(|t| t.id == task_id)
        .with_context(|| format!("no day-task {task_id} on {day_local}"))?;
    Ok(WorkstreamReport {
        title: task.title,
        summary: task.summary,
        minutes: task.minutes,
    })
}

/// Build the user message: the workstream then the candidate tickets.
fn build_user_prompt(report: &WorkstreamReport, candidates: &[Candidate]) -> String {
    let mut s = String::new();
    s.push_str("=== WORKSTREAM (one strand of the day's work) ===\n");
    s.push_str(&format!("TITLE: {}\n", report.title));
    s.push_str("WHAT WAS DONE:\n");
    if report.summary.is_empty() {
        s.push_str("- (no summary captured)\n");
    } else {
        for line in &report.summary {
            s.push_str(&format!("- {line}\n"));
        }
    }
    s.push_str(&format!(
        "(measured ~{} min across the day)\n\n",
        report.minutes
    ));

    s.push_str(
        "=== CANDIDATE TICKETS (open, non-terminal - match to one by task_key, or none) ===\n",
    );
    if candidates.is_empty() {
        s.push_str("(no open tickets on the board - propose a new one if the work warrants it)\n");
    } else {
        for c in candidates {
            s.push_str(&format!("- {} [{}] {}\n", c.task_key, c.provider, c.doc));
        }
    }
    s
}

/// Render one ticket into the doc line the matcher sees (mirrors the Python
/// `render_doc`): `[Type] Title. Epic: E. Desc`. Description is trimmed so a long
/// ticket can't dominate the prompt.
fn render_doc(issue_type: &str, title: &str, epic: &str, desc: &str) -> String {
    let desc = desc.replace('\n', " ");
    let desc = if desc.chars().count() > 300 {
        let truncated: String = desc.chars().take(300).collect();
        format!("{truncated}…")
    } else {
        desc
    };
    let it = if issue_type.is_empty() {
        "Task"
    } else {
        issue_type
    };
    format!("[{it}] {title}. Epic: {epic}. {desc}")
        .trim()
        .to_string()
}

// ── DB reads ──────────────────────────────────────────────────────────────────

/// Fetch open (non-terminal, non-curation-excluded) tickets as candidates, each
/// carrying its provider. Degrades to a curation-free query on a pre-038 DB.
async fn fetch_open_candidates(pool: &SqlitePool) -> Result<Vec<Candidate>> {
    let sql_with_curation = "SELECT pm_tasks.task_key, COALESCE(pm_tasks.provider,'jira'), \
             COALESCE(title,''), COALESCE(issue_type,'Task'), COALESCE(epic_title,''), \
             COALESCE(description_text,'') \
         FROM pm_tasks \
         LEFT JOIN pm_task_curation c ON c.task_key = pm_tasks.task_key \
         WHERE COALESCE(pm_tasks.is_terminal,0)=0 \
           AND (c.decision IS NULL OR c.decision != 'excluded') \
         LIMIT ?";
    let sql_plain = "SELECT task_key, COALESCE(provider,'jira'), COALESCE(title,''), \
             COALESCE(issue_type,'Task'), COALESCE(epic_title,''), COALESCE(description_text,'') \
         FROM pm_tasks WHERE COALESCE(is_terminal,0)=0 LIMIT ?";

    let rows = match sqlx::query_as::<_, (String, String, String, String, String, String)>(
        sql_with_curation,
    )
    .bind(MAX_CANDIDATES as i64)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("day_task_worklogs.read.candidates"))
    .await
    {
        Ok(rows) => rows,
        Err(_) => sqlx::query_as::<_, (String, String, String, String, String, String)>(sql_plain)
            .bind(MAX_CANDIDATES as i64)
            .fetch_all(pool)
            .await
            .context("fetching candidate tickets")?,
    };
    tracing::debug!(rows = rows.len(), "day_task_worklogs.read.candidates");

    Ok(rows
        .into_iter()
        .map(
            |(task_key, provider, title, issue_type, epic, desc)| Candidate {
                doc: render_doc(&issue_type, &title, &epic, &desc),
                task_key,
                provider,
            },
        )
        .collect())
}

/// The provider that owns `task_key`, if it's a known ticket.
async fn resolve_provider_for_key(pool: &SqlitePool, task_key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(provider,'jira') FROM pm_tasks WHERE task_key = ?")
            .bind(task_key)
            .fetch_optional(pool)
            .await
            .context("resolving provider for matched ticket")?;
    Ok(row.map(|(p,)| p))
}

/// Any existing task_key of `provider` — GitHub needs one to infer owner/repo on
/// create. Harmless elsewhere. Best-effort: `None` on any error.
async fn fetch_sample_task_key(pool: &SqlitePool, provider: &str) -> Option<String> {
    sqlx::query_as::<_, (String,)>("SELECT task_key FROM pm_tasks WHERE provider = ? LIMIT 1")
        .bind(provider)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|(k,)| k)
}

/// Set `day_tasks.linked_ticket = target_key` for the card.
async fn link_day_task(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
    target_key: &str,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE day_tasks SET linked_ticket = ?, updated_at = ? WHERE day_local = ? AND task_id = ?",
    )
    .bind(target_key)
    .bind(now)
    .bind(day_local)
    .bind(task_id)
    .execute(pool)
    .await
    .context("linking day-task to ticket")?;
    Ok(())
}

// ── Parsing / rendering ───────────────────────────────────────────────────────

/// The validated model answer.
struct ParsedAnswer {
    match_: Option<GeneratedWorklogMatch>,
    propose: Option<GeneratedWorklogPropose>,
    update: GeneratedWorklogUpdate,
    reasoning: String,
}

/// Tolerantly parse the model's JSON and normalise match/propose to at-most-one.
/// Returns `None` only when there was no usable object at all (the caller turns
/// that into a clean "could not parse" error, never a panic).
fn parse_answer(text: &str) -> Option<ParsedAnswer> {
    let v = parse_json_object(text)?;

    let string_array = |val: &serde_json::Value| -> Vec<String> {
        val.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .filter(|s| !s.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };

    // match — only a non-null object with a non-empty task_key counts.
    let match_ = v.get("match").and_then(|m| {
        let task_key = m.get("task_key")?.as_str()?.trim().to_string();
        if task_key.is_empty() {
            return None;
        }
        let confidence = m.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0);
        Some(GeneratedWorklogMatch {
            task_key,
            confidence: confidence.clamp(0.0, 1.0),
        })
    });

    // propose — only a non-null object with a non-empty title counts.
    let propose = v.get("propose").and_then(|p| {
        let title = p.get("title")?.as_str()?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        let description = p
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let issue_type = p
            .get("issue_type")
            .and_then(|t| t.as_str())
            .unwrap_or("Task")
            .to_string();
        Some(GeneratedWorklogPropose {
            issue_type,
            title,
            description,
        })
    });

    let update = v
        .get("update")
        .map(|u| GeneratedWorklogUpdate {
            summary: u
                .get("summary")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            decisions: u.get("decisions").map(&string_array).unwrap_or_default(),
            architecture: u.get("architecture").map(&string_array).unwrap_or_default(),
            status: u
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
        })
        .unwrap_or_default();

    let reasoning = v
        .get("reasoning")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    Some(ParsedAnswer {
        match_,
        propose,
        update,
        reasoning,
    })
}

/// Render the status update into the plain-text comment body posted to the tracker.
/// Empty sections are skipped; bullets use a plain hyphen.
fn render_update_body(update: &GeneratedWorklogUpdate) -> String {
    let mut s = String::new();
    if !update.summary.trim().is_empty() {
        s.push_str(update.summary.trim());
    }
    let section = |s: &mut String, heading: &str, items: &[String]| {
        let items: Vec<&String> = items.iter().filter(|i| !i.trim().is_empty()).collect();
        if items.is_empty() {
            return;
        }
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(heading);
        for it in items {
            s.push_str(&format!("\n- {}", it.trim()));
        }
    };
    section(&mut s, "Decisions:", &update.decisions);
    section(&mut s, "Architecture:", &update.architecture);
    if !update.status.trim().is_empty() {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&format!("Status: {}", update.status.trim()));
    }
    s
}

/// Best-effort human browse URL for a target key, without an auth round trip
/// (mirrors `intelligence::ticket_update::browse_url`). Empty → `None`.
fn browse_url(config: &Config, provider: &str, key: &str) -> Option<String> {
    use crate::config::PmProviderConfig as P;
    let url = config
        .pm_providers
        .iter()
        .find_map(|p| match (provider, p) {
            ("jira", P::Jira(c)) => Some(if c.base_url.is_empty() {
                String::new()
            } else {
                format!("{}/browse/{}", c.base_url.trim_end_matches('/'), key)
            }),
            ("linear", P::Linear(_)) => Some(format!("https://linear.app/issue/{key}")),
            ("github", P::GitHub(_)) => {
                // key is owner/repo#number
                key.rsplit_once('#').and_then(|(repo_path, num)| {
                    repo_path
                        .split_once('/')
                        .map(|(o, r)| format!("https://github.com/{o}/{r}/issues/{num}"))
                })
            }
            ("trello", P::Trello(_)) => Some(format!("https://trello.com/c/{key}")),
            ("azure_devops", P::AzureDevOps(c)) => key.rsplit_once('#').map(|(_, id)| {
                if c.api_base.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{}/{}/_workitems/edit/{}",
                        c.api_base.trim_end_matches('/'),
                        c.project,
                        id
                    )
                }
            }),
            _ => None,
        })?;
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prefers_match_and_clamps_confidence() {
        let a = parse_answer(
            r#"{"match":{"task_key":"KAN-12","confidence":1.7},"propose":null,
                "update":{"summary":"Did X","decisions":["d1"],"architecture":[],"status":"WIP"},
                "reasoning":"clear"}"#,
        )
        .unwrap();
        let m = a.match_.unwrap();
        assert_eq!(m.task_key, "KAN-12");
        assert_eq!(m.confidence, 1.0, "confidence clamped to <=1");
        assert!(a.propose.is_none());
        assert_eq!(a.update.summary, "Did X");
        assert_eq!(a.update.decisions, vec!["d1"]);
    }

    #[test]
    fn parse_reads_a_proposal() {
        let a = parse_answer(
            r#"{"match":null,"propose":{"issue_type":"Bug","title":"Fix the crash",
                "description":"It crashes"},
                "update":{"summary":"s","decisions":[],"architecture":[],"status":""},
                "reasoning":"new work"}"#,
        )
        .unwrap();
        assert!(a.match_.is_none());
        let p = a.propose.unwrap();
        assert_eq!(p.issue_type, "Bug");
        assert_eq!(p.title, "Fix the crash");
    }

    #[test]
    fn empty_match_object_is_treated_as_no_match() {
        // A model that emits an object with a blank task_key must NOT count as a match.
        let a = parse_answer(
            r#"{"match":{"task_key":"","confidence":0.9},
                "propose":{"issue_type":"Task","title":"Do it","description":"d"},
                "update":{"summary":"s","decisions":[],"architecture":[],"status":""},
                "reasoning":"r"}"#,
        )
        .unwrap();
        assert!(a.match_.is_none(), "blank task_key is not a match");
        assert!(a.propose.is_some());
    }

    #[test]
    fn garbage_answer_is_none_not_a_panic() {
        assert!(parse_answer("I could not decide.").is_none());
        assert!(parse_answer("").is_none());
    }

    #[test]
    fn render_body_skips_empty_sections_and_uses_plain_hyphens() {
        let u = GeneratedWorklogUpdate {
            summary: "Shipped the matcher".into(),
            decisions: vec!["Chose one LLM call".into()],
            architecture: vec![],
            status: "In progress".into(),
        };
        let body = render_update_body(&u);
        assert!(body.contains("Shipped the matcher"));
        assert!(body.contains("Decisions:\n- Chose one LLM call"));
        assert!(!body.contains("Architecture:"), "empty section omitted");
        assert!(body.contains("Status: In progress"));
        assert!(!body.contains('—'), "no em-dash in posted text");
    }

    #[test]
    fn render_doc_trims_long_descriptions() {
        let long = "x".repeat(500);
        let doc = render_doc("Task", "Title", "Epic", &long);
        assert!(doc.contains("[Task] Title. Epic: Epic."));
        assert!(doc.chars().count() < 400, "description trimmed");
    }
}
