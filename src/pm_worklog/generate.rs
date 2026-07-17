//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The "Generate worklog" engine behind a day-task card: ONE provider-agnostic LLM
//! call that matches a day-level workstream to the existing tickets it advanced —
//! one or several — (or proposes a new one) and writes a high-level status update,
//! then — on approve — creates the ticket if proposed, posts the update as a plain
//! comment on each ticket, and links the day-task.
//!
//! # Partial delivery is a real state
//! Approving across several tickets is not atomic, and a comment cannot be
//! un-posted. Two can take the update and the third refuse, so each target carries
//! its own posted flag and a retry only posts the ones still outstanding. That is
//! why [`meridian_core::day_task_worklogs::targets`] is a table.
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
    self, DayTaskWorklogDraft, DraftUpsert, GeneratedWorklogPropose, GeneratedWorklogUpdate,
    TargetInput, WorklogSection,
};

use crate::config::Config;
use crate::llm::{self, parse_json_object, prompts, PromptRequest};

/// Token budget for the combined match/propose/update answer — a handful of short
/// fields plus a few bullet arrays. `pub(crate)` so the LLM-Lab replay
/// ([`crate::llm_experiment`]) rebuilds the identical request contract.
pub(crate) const GENERATE_MAX_TOKENS: u32 = 1500;

/// One open ticket the matcher may bind to, rendered for the prompt.
struct Candidate {
    task_key: String,
    provider: String,
    doc: String,
}

/// One ticket's outcome in an [`ApproveResult`].
#[derive(Debug, Clone, Serialize)]
pub struct PostedTarget {
    pub task_key: String,
    pub posted: bool,
    pub browse_url: Option<String>,
    /// Why this ticket specifically failed, when it did. The others may have
    /// succeeded — an approve across several tickets has no single outcome.
    pub error: Option<String>,
}

/// The result of [`approve`], serialized to the CLI's one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct ApproveResult {
    /// Every target posted. A partial success is `false` and retryable.
    pub posted: bool,
    pub targets: Vec<PostedTarget>,
    pub created_task_key: Option<String>,
    pub created: bool,
    pub error: Option<String>,
}

/// Generate (or regenerate) the draft for `(day_local, task_id)`. Loads the
/// day-task's whole-story summary, fetches THAT DAY'S PLANNED TASKS as candidates,
/// makes ONE structured LLM call (matches XOR propose + a status update), validates
/// it, and UPSERTs a `drafted` row (overwriting only a still-`drafted` one).
/// Returns the persisted draft.
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
        let (req, n_candidates) = generate_request(pool, day_local, task_id).await?;
        tracing::Span::current().record("n_candidates", n_candidates);

        let (out, provider) = llm::complete(&req)
            .await
            .map_err(|e| anyhow::anyhow!("generate-worklog LLM call failed: {e}"))?;
        tracing::Span::current().record("llm_provider", provider.as_str());

        let parsed = parse_answer(&out.text).context(
            "the AI answer could not be parsed into a match/propose/update - try regenerating",
        )?;

        // Resolve each branch. A match takes its provider from the matched ticket
        // (per-ticket: a plan can span two trackers); a proposal targets the first
        // configured provider.
        let (draft_provider, targets) = match (parsed.matches.is_empty(), &parsed.propose) {
            (false, None) => {
                let targets = resolve_targets(pool, &parsed.matches).await?;
                let keys: Vec<&str> = targets.iter().map(|t| t.task_key.as_str()).collect();
                tracing::Span::current().record("matched_task_key", keys.join(","));
                tracing::Span::current().record("proposed", false);
                // A draft-level provider is only consulted for a proposal, and this
                // isn't one; name the first target's so the field is never empty.
                (targets[0].provider.clone(), targets)
            }
            (true, Some(_)) => {
                let p = config
                    .pm_providers
                    .first()
                    .map(|p| p.provider_name().to_string())
                    .context("no PM tracker is connected - connect one first")?;
                // Record `matched_task_key` empty on this path too, so the span
                // attribute always exists in the trace stream's schema (OpenObserve
                // only knows a field once a record carries it — a dashboard that
                // filters on it errors until then). See worklog-generation.json.
                tracing::Span::current().record("matched_task_key", "");
                tracing::Span::current().record("proposed", true);
                (p, Vec::new())
            }
            // XOR violation: neither or both. Tolerated at the schema level, rejected here.
            _ => bail!("the AI must either match tickets or propose one, not both or neither - try regenerating"),
        };

        let now = chrono::Utc::now().to_rfc3339();
        let n_targets = targets.len();
        let upsert = DraftUpsert {
            provider: draft_provider,
            targets,
            propose: parsed.propose,
            update: parsed.update,
            reasoning: parsed.reasoning,
        };
        let draft = day_task_worklogs::upsert_draft(pool, day_local, task_id, upsert, &now)
            .await
            .context("persisting the generated worklog draft")?;

        tracing::info!(
            provider = draft.provider,
            state = draft.state,
            matched = n_targets,
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
/// plain comment on EVERY target, mark each one as it lands, and link the day-task.
///
/// Idempotent at both levels. An already-`posted` row short-circuits; and because a
/// comment cannot be un-posted, a partial delivery (two tickets took the comment,
/// the third refused) leaves the row `approved` with the two marked posted, so the
/// retry posts only the third. On failure the row is marked with the error and left
/// retry-safe (never a create-without-post that re-creates).
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
        target_keys = Empty,
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
                targets: recorded_targets(&draft),
                created_task_key: draft.created_task_key.clone(),
                created: false,
                error: None,
            });
        }

        let now = chrono::Utc::now().to_rfc3339();
        day_task_worklogs::mark_approved(pool, day_local, task_id, &now).await?;

        match approve_inner(pool, config, day_local, task_id, &draft).await {
            Ok(res) => {
                let keys: Vec<&str> = res.targets.iter().map(|t| t.task_key.as_str()).collect();
                tracing::Span::current().record("created", res.created);
                tracing::Span::current().record("posted", res.posted);
                tracing::Span::current().record("target_keys", keys.join(","));
                tracing::info!(
                    targets = res.targets.len(),
                    posted = res.targets.iter().filter(|t| t.posted).count(),
                    created = res.created,
                    "worklog: approve finished"
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
                    targets: recorded_targets(&draft),
                    created_task_key: draft.created_task_key.clone(),
                    created: false,
                    error: Some(msg),
                })
            }
        }
    }
    .instrument(span)
    .await
}

/// The draft's targets as an [`ApproveResult`] reports them, straight from what is
/// persisted. Used on the paths that post nothing (already-posted, hard failure) so
/// the caller still sees which tickets stand where.
fn recorded_targets(draft: &DayTaskWorklogDraft) -> Vec<PostedTarget> {
    draft
        .targets
        .iter()
        .map(|t| PostedTarget {
            task_key: t.task_key.clone(),
            posted: t.posted,
            browse_url: t.browse_url.clone(),
            error: t.error.clone(),
        })
        .collect()
}

/// The fallible body of [`approve`]: create-if-proposed → post to each target →
/// mark → link.
///
/// A hard failure (nothing to post to, the create itself failing) bubbles up so the
/// caller records it and leaves the row retry-safe. A failure posting to ONE ticket
/// does not: it is recorded on that target and the loop carries on to the rest,
/// because the alternative is letting one bad ticket suppress updates that would
/// have landed fine.
async fn approve_inner(
    pool: &SqlitePool,
    config: &Config,
    day_local: &str,
    task_id: &str,
    draft: &DayTaskWorklogDraft,
) -> Result<ApproveResult> {
    // Resolve what to post to. A proposal has no target until its ticket exists, so
    // create it first (unless a prior attempt already did — its key is persisted),
    // persisting the key BEFORE posting so a retry can never double-create. From
    // there both branches are just a list of tickets.
    let (targets, created, created_task_key) = match (draft.targets.is_empty(), &draft.propose) {
        (false, _) => (draft.targets.clone(), false, draft.created_task_key.clone()),
        (true, Some(p)) => {
            let provider = &draft.provider;
            let mut created = false;
            let key = match &draft.created_task_key {
                Some(existing) => existing.clone(),
                None => {
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
                    created = true;
                    key
                }
            };
            // Called on BOTH paths, not just after a fresh create, and idempotent
            // by construction (an UPDATE to the same key + an INSERT OR IGNORE).
            //
            // It is a two-step write: record the key, then add the target. A crash
            // between them leaves a row that knows what it created but has nothing
            // to post to — and the retry, seeing the key already there, used to skip
            // straight past this and find no targets at all. Re-running it repairs
            // that instead of stranding the row forever.
            let now = chrono::Utc::now().to_rfc3339();
            day_task_worklogs::mark_created(pool, day_local, task_id, &key, provider, &now)
                .await
                .context("persisting the created ticket key")?;
            // Re-read so the created ticket comes back as an ordinary target,
            // rather than being hand-assembled into the same shape a second time.
            let fresh = day_task_worklogs::get_day_task_worklog(pool, day_local, task_id)
                .await?
                .context("draft vanished mid-approve")?;
            (fresh.targets, created, Some(key))
        }
        // Every match dismissed and nothing proposed: there is genuinely nowhere to
        // put this. Say so rather than silently succeeding at nothing.
        (true, None) => {
            bail!("this update has no ticket to post to - pick one, or regenerate the draft")
        }
    };

    // Nothing to post to, on a path that was supposed to guarantee otherwise.
    // Explicit, because the natural reading of the loop below gets this wrong:
    // `all()` over an empty list is TRUE, so an empty target set would sail through
    // and report a successful post that never happened.
    if targets.is_empty() {
        bail!("this update has no ticket to post to - pick one, or regenerate the draft");
    }

    let body = render_update_body(&draft.update);
    let mut out: Vec<PostedTarget> = Vec::with_capacity(targets.len());

    for t in &targets {
        // Already live on the tracker: a comment cannot be un-posted, so a retry
        // must never touch this one again.
        if t.posted {
            out.push(PostedTarget {
                task_key: t.task_key.clone(),
                posted: true,
                browse_url: t.browse_url.clone(),
                error: None,
            });
            continue;
        }

        // A prior attempt on this ticket started and never recorded an outcome, so
        // the comment may already be live. Auto-retrying could double-post it, and
        // nothing here can tell — only a human looking at the ticket can. Report it
        // and move on to the tickets we CAN act on.
        if t.outcome_unknown {
            let msg = format!(
                "Meridian couldn't confirm whether this update posted to {} - open the ticket \
                 and check before trying again",
                t.task_key
            );
            tracing::warn!(
                task_key = t.task_key,
                "worklog: target outcome unknown - refusing to auto-retry"
            );
            out.push(PostedTarget {
                task_key: t.task_key.clone(),
                posted: false,
                browse_url: t.browse_url.clone(),
                error: Some(msg),
            });
            continue;
        }

        let now = chrono::Utc::now().to_rfc3339();
        // Write down the intent BEFORE the provider call. If this process dies
        // mid-request the claim survives, the target reads `outcome_unknown` on the
        // next pass, and the branch above refuses it — instead of `approved` letting
        // a retry silently post a second copy (post_comment has no dedup marker to
        // catch that after the fact, by design; see its module docs).
        if !day_task_worklogs::targets::begin_post(pool, day_local, task_id, &t.task_key, &now)
            .await
            .context("claiming a target for posting")?
        {
            // The CAS lost: another approve is in flight for this ticket, or it was
            // resolved between our read and now. Never post over that.
            tracing::warn!(
                task_key = t.task_key,
                "worklog: could not claim target - a concurrent approve may be in flight"
            );
            out.push(PostedTarget {
                task_key: t.task_key.clone(),
                posted: false,
                browse_url: None,
                error: Some(format!(
                    "another post to {} is already in progress - try again in a moment",
                    t.task_key
                )),
            });
            continue;
        }

        match super::post_comment::post_comment(config, &t.provider, &t.task_key, &body).await {
            Ok(comment_id) => {
                let browse = browse_url(config, &t.provider, &t.task_key);
                day_task_worklogs::mark_posted(
                    pool,
                    day_local,
                    task_id,
                    &t.task_key,
                    &comment_id,
                    browse.as_deref(),
                    &now,
                )
                .await
                .context("marking a target posted")?;
                out.push(PostedTarget {
                    task_key: t.task_key.clone(),
                    posted: true,
                    browse_url: browse,
                    error: None,
                });
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::warn!(task_key = t.task_key, error = %e, "worklog: posting to one target failed - others continue");
                // A returned error means the call definitely did not post — no
                // side effect, so releasing the claim is known-safe and lets a
                // normal retry re-attempt it. This is the ONLY safe place to
                // release: a crash never reaches here, which is the point.
                let _ =
                    day_task_worklogs::targets::revert_post(pool, day_local, task_id, &t.task_key)
                        .await;
                let _ = day_task_worklogs::targets::mark_error(
                    pool,
                    day_local,
                    task_id,
                    &t.task_key,
                    &msg,
                )
                .await;
                out.push(PostedTarget {
                    task_key: t.task_key.clone(),
                    posted: false,
                    browse_url: None,
                    error: Some(msg),
                });
            }
        }
    }

    let posted = out.iter().all(|t| t.posted);

    // Link the day-task card to the first ticket that took the update (best-effort
    // — the post already succeeded, so a link-write failure must not fail the
    // approve). `linked_ticket` is a single column, so a multi-ticket update shows
    // its first; the panel lists them all from the draft itself.
    if let Some(first) = out.iter().find(|t| t.posted) {
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = link_day_task(pool, day_local, task_id, &first.task_key, &now).await {
            tracing::warn!(error = %e, "worklog: could not set day_tasks.linked_ticket");
        }
    }

    Ok(ApproveResult {
        posted,
        targets: out,
        created_task_key,
        created,
        error: None,
    })
}

// ── Prompt assembly ───────────────────────────────────────────────────────────

/// The generate-worklog call's exact [`PromptRequest`] (plus the candidate count for
/// the caller's span) — extracted from [`generate`] so the LLM-Lab replay
/// ([`crate::llm_experiment`]) fans the byte-identical request across arbitrary
/// providers. Read-only: loads the day-task's whole-story report and THAT DAY'S
/// planned tasks as candidates, then assembles the user prompt.
pub(crate) async fn generate_request(
    pool: &SqlitePool,
    day_local: &str,
    task_id: &str,
) -> Result<(PromptRequest, usize)> {
    let report = load_workstream_report(pool, day_local, task_id).await?;
    let candidates = fetch_plan_candidates(pool, day_local).await?;
    let user = build_user_prompt(&report, &candidates);
    let req = PromptRequest {
        system: prompts::WORKLOG_GENERATE,
        user,
        schema: Some(prompts::worklog_generate_schema()),
        max_tokens: GENERATE_MAX_TOKENS,
        label: format!("worklog-generate {day_local} {task_id}"),
    };
    Ok((req, candidates.len()))
}

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

/// Build the user message: the workstream then the day's planned tasks.
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
        "=== TODAY'S PLANNED TASKS (the ONLY tickets you may match to - by task_key; any number, or none) ===\n",
    );
    if candidates.is_empty() {
        s.push_str(
            "(no tasks were planned for this day - propose a new one if the work warrants it)\n",
        );
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

/// The candidate tickets, rendered for the prompt: **the day's planned tasks**.
///
/// Not the board. What the dev committed to that morning is a far better prior
/// than every open ticket they own — the board invites the model to bind work to
/// tickets nobody intended to touch, and a long list buries the right answer
/// anyway. The plan is capped at [`meridian_core::plan::MAX_PLAN_TASKS`], so the
/// candidate list is short by construction and the old ">40 open tickets, refuse
/// to guess" gate is gone with it.
///
/// The trade: work on something unplanned comes back as a *proposal* rather than
/// a match. That is the intended behaviour, and the reason the user can retarget
/// a draft at any ticket by hand ([`meridian_core::day_task_worklogs::retarget_draft`]).
///
/// This owns NO query — [`meridian_core::plan::load_plan_candidates`] is the one
/// definition, shared with the plan cards, so the two can't drift.
///
/// Terminal candidates are kept deliberately: checking a task off the plan closes
/// the real ticket, and dropping it here would delete exactly the task the dev
/// just finished from the set used to log the work that finished it. Personal
/// (`'local'`) tasks are dropped, because this draft ends in `post_comment` to a
/// real tracker and a personal task has nowhere to post
/// (see [`meridian_core::task_create`]).
#[tracing::instrument(skip(pool))]
async fn fetch_plan_candidates(pool: &SqlitePool, day_local: &str) -> Result<Vec<Candidate>> {
    let planned = meridian_core::plan::load_plan_candidates(pool, day_local)
        .await
        .context("loading the day's planned tasks")?;
    let total = planned.len();
    let candidates: Vec<Candidate> = planned
        .into_iter()
        .filter(|t| t.provider != meridian_core::task_create::LOCAL_PROVIDER)
        .map(|t| Candidate {
            doc: render_doc(
                &t.issue_type,
                &t.title,
                t.epic.as_deref().unwrap_or_default(),
                &t.description,
            ),
            task_key: t.task_key,
            provider: t.provider,
        })
        .collect();
    tracing::debug!(
        planned = total,
        candidates = candidates.len(),
        "worklog: candidate set is the day's plan"
    );
    Ok(candidates)
}

/// Turn the model's matched keys into targets, resolving each ticket's own tracker.
///
/// A key that isn't on the board is DROPPED rather than fatal. The model can
/// hallucinate one key out of three, and failing the whole draft over it would
/// throw away two good matches and a written update to punish a typo. Only an
/// answer where nothing survives is an error — there is then genuinely nothing to
/// post to, and regenerating is the honest advice.
async fn resolve_targets(pool: &SqlitePool, matches: &[ParsedMatch]) -> Result<Vec<TargetInput>> {
    let mut out = Vec::with_capacity(matches.len());
    for m in matches {
        match resolve_provider_for_key(pool, &m.task_key).await? {
            Some(provider) => out.push(TargetInput {
                task_key: m.task_key.clone(),
                provider,
                confidence: m.confidence,
                manual: false,
            }),
            None => tracing::warn!(
                task_key = m.task_key,
                "worklog: the AI matched a ticket that is not on the board - dropping it"
            ),
        }
    }
    if out.is_empty() {
        bail!("the AI matched only tickets that are not on the board - try regenerating");
    }
    tracing::debug!(
        matched = matches.len(),
        resolved = out.len(),
        "worklog: matched tickets resolved"
    );
    Ok(out)
}

/// The provider that owns `task_key`, if it's a known ticket.
///
/// Defence in depth against a personal task reaching `post_comment`: the candidate
/// list already excludes them, but a model can hallucinate a key it was never shown,
/// and [`meridian_core::board::provider_for_key`]'s `COALESCE(provider,'jira')`
/// would then route a `LOCAL-3` "match" straight at Jira. Refuse it explicitly.
async fn resolve_provider_for_key(pool: &SqlitePool, task_key: &str) -> Result<Option<String>> {
    let row = meridian_core::board::provider_for_key(pool, task_key)
        .await
        .context("resolving provider for matched ticket")?;
    if row.as_deref() == Some(meridian_core::task_create::LOCAL_PROVIDER) {
        bail!("the AI matched a personal task, which has no tracker to post to - try regenerating");
    }
    Ok(row)
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

/// One ticket the model says this workstream advanced, before it has been checked
/// against the board.
struct ParsedMatch {
    task_key: String,
    confidence: f64,
}

/// The validated model answer.
struct ParsedAnswer {
    /// The tickets the model matched, in its own order (strongest first). Empty
    /// when it proposed instead.
    matches: Vec<ParsedMatch>,
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

    // Parse the dynamic `sections`: each `{heading, points[]}`; drop any section
    // with a blank heading or no non-empty points (see worklog-generate.md).
    let sections = |val: &serde_json::Value| -> Vec<WorklogSection> {
        val.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        let heading = s.get("heading")?.as_str()?.trim().to_string();
                        let points =
                            string_array(s.get("points").unwrap_or(&serde_json::Value::Null));
                        if heading.is_empty() || points.is_empty() {
                            return None;
                        }
                        Some(WorklogSection { heading, points })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    // matches — 0..N objects, each needing a non-empty task_key. Order is the
    // model's (strongest first) and is preserved all the way to the panel.
    //
    // `match` (singular object) is also read, because that is the shape every
    // draft generated before multi-match used, and some models keep emitting it
    // regardless of the schema. Treating it as a one-element list costs a line and
    // turns a silent no-match into a correct answer.
    let one_match = |m: &serde_json::Value| -> Option<ParsedMatch> {
        let task_key = m.get("task_key")?.as_str()?.trim().to_string();
        if task_key.is_empty() {
            return None;
        }
        let confidence = m.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0);
        Some(ParsedMatch {
            task_key,
            confidence: confidence.clamp(0.0, 1.0),
        })
    };
    let mut matches: Vec<ParsedMatch> = v
        .get("matches")
        .and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(&one_match).collect())
        .unwrap_or_default();
    if matches.is_empty() {
        matches.extend(v.get("match").and_then(&one_match));
    }
    // A key repeated across entries is one ticket; the target table's PK would
    // collapse them anyway, but dropping them here keeps the count honest in the
    // trace and the panel.
    let mut seen = std::collections::HashSet::new();
    matches.retain(|m| seen.insert(m.task_key.clone()));

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
            sections: u.get("sections").map(&sections).unwrap_or_default(),
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
        matches,
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
    // Render each model-named section as "Heading:\n- point" — headings are
    // dynamic (fit the work), so we iterate rather than hardcode Decisions/Arch.
    for sec in &update.sections {
        let heading = sec.heading.trim();
        let items: Vec<&String> = sec.points.iter().filter(|i| !i.trim().is_empty()).collect();
        if heading.is_empty() || items.is_empty() {
            continue;
        }
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&format!("{heading}:"));
        for it in items {
            s.push_str(&format!("\n- {}", it.trim()));
        }
    }
    if !update.status.trim().is_empty() {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&format!("Status: {}", update.status.trim()));
    }
    s
}

/// Fill in each target's `browse_url` when it's missing but resolvable.
/// `browse_url` is fully deterministic from provider + key, so recomputing it on
/// every read is safe and repairs rows persisted before an auth/URL fix (e.g. an
/// OAuth-Jira ticket posted before browse-URL resolution learned to read the site
/// URL from the token store). Called by the `worklog-generate-get` CLI path.
pub fn hydrate_browse_url(config: &Config, draft: &mut DayTaskWorklogDraft) {
    for t in &mut draft.targets {
        if t.browse_url.as_deref().unwrap_or_default().is_empty() {
            t.browse_url = browse_url(config, &t.provider, &t.task_key);
        }
    }
}

/// Best-effort human browse URL for a target key, without an auth round trip
/// (mirrors `intelligence::ticket_update::browse_url`). Empty → `None`.
fn browse_url(config: &Config, provider: &str, key: &str) -> Option<String> {
    use crate::config::PmProviderConfig as P;
    let url = config
        .pm_providers
        .iter()
        .find_map(|p| match (provider, p) {
            ("jira", P::Jira(c)) => {
                // An OAuth-only Jira leaves `base_url` empty (the struct doc says
                // so); the human-facing site URL then lives in the OAuth token
                // store's `accessible-resources` result. Read it synchronously
                // (no network) so the "Linked to KAN-…" chip becomes a real link.
                let base = if c.base_url.is_empty() {
                    crate::intelligence::oauth::store::load("jira")
                        .ok()
                        .map(|t| t.site_url)
                        .unwrap_or_default()
                } else {
                    c.base_url.clone()
                };
                Some(if base.is_empty() {
                    String::new()
                } else {
                    format!("{}/browse/{}", base.trim_end_matches('/'), key)
                })
            }
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
    use sqlx::sqlite::SqlitePoolOptions;

    /// In-memory DB with the real schema, for the candidate-query guards.
    async fn db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn seed(pool: &SqlitePool, key: &str, provider: &str) {
        sqlx::query(
            "INSERT INTO pm_tasks (task_key, provider, title, description_text, issue_type, \
                status_raw, is_terminal, url, updated_at) \
             VALUES (?, ?, 'A title', 'A description', 'Task', 'To Do', 0, '', \
                '2026-01-01T00:00:00Z')",
        )
        .bind(key)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Put `key` on the day's plan.
    async fn plan(pool: &SqlitePool, day: &str, key: &str) {
        sqlx::query(
            "INSERT INTO daily_plan (plan_date, task_key, position, origin, created_at, updated_at) \
             VALUES (?, ?, 0, 'manual', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(day)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn candidates_are_the_days_plan_not_the_board() {
        // The whole point of the change: a ticket can be wide open on the board and
        // still not be a candidate, because the dev didn't plan it today.
        let pool = db().await;
        seed(&pool, "KAN-1", "jira").await;
        seed(&pool, "KAN-2", "jira").await;
        plan(&pool, "2026-07-16", "KAN-1").await;

        let keys: Vec<String> = fetch_plan_candidates(&pool, "2026-07-16")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.task_key)
            .collect();
        assert_eq!(keys, vec!["KAN-1".to_string()]);
    }

    #[tokio::test]
    async fn a_checked_off_planned_task_is_still_a_candidate() {
        // Checking a task off the plan CLOSES the real ticket (is_terminal = 1).
        // Dropping it here would delete exactly the task the dev just finished from
        // the set used to log the work that finished it - the bug this guards.
        let pool = db().await;
        seed(&pool, "KAN-1", "jira").await;
        sqlx::query("UPDATE pm_tasks SET is_terminal = 1 WHERE task_key = 'KAN-1'")
            .execute(&pool)
            .await
            .unwrap();
        plan(&pool, "2026-07-16", "KAN-1").await;

        let keys: Vec<String> = fetch_plan_candidates(&pool, "2026-07-16")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.task_key)
            .collect();
        assert_eq!(
            keys,
            vec!["KAN-1".to_string()],
            "a task marked Done but still on the plan must stay matchable"
        );
    }

    #[tokio::test]
    async fn fetch_plan_candidates_excludes_personal_tasks() {
        // This draft ends in post_comment to a real tracker. A personal task has
        // nowhere to post, so offering one as a candidate could only ever produce a
        // draft that fails at the last step.
        let pool = db().await;
        seed(&pool, "KAN-1", "jira").await;
        seed(&pool, "LOCAL-1", "local").await;
        plan(&pool, "2026-07-16", "KAN-1").await;
        plan(&pool, "2026-07-16", "LOCAL-1").await;

        let keys: Vec<String> = fetch_plan_candidates(&pool, "2026-07-16")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.task_key)
            .collect();
        assert_eq!(
            keys,
            vec!["KAN-1".to_string()],
            "a personal task can be planned, but never posted to - so never a candidate"
        );
    }

    #[tokio::test]
    async fn an_unplanned_day_has_no_candidates() {
        // Not an error: the model proposes, and the user can retarget by hand.
        let pool = db().await;
        seed(&pool, "KAN-1", "jira").await;

        assert!(fetch_plan_candidates(&pool, "2026-07-16")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn resolve_provider_refuses_a_personal_task() {
        // Defence in depth: the model can hallucinate a key it was never shown, and
        // COALESCE(provider,'jira') would route it straight at Jira.
        let pool = db().await;
        seed(&pool, "LOCAL-1", "local").await;
        seed(&pool, "KAN-1", "jira").await;

        assert!(
            resolve_provider_for_key(&pool, "LOCAL-1").await.is_err(),
            "a hallucinated personal-task match must be refused, not posted to jira"
        );
        assert_eq!(
            resolve_provider_for_key(&pool, "KAN-1").await.unwrap(),
            Some("jira".to_string())
        );
    }

    fn matched(a: &ParsedAnswer) -> Vec<&str> {
        a.matches.iter().map(|m| m.task_key.as_str()).collect()
    }

    #[test]
    fn parse_reads_matches_and_clamps_confidence() {
        let a = parse_answer(
            r#"{"matches":[{"task_key":"KAN-12","confidence":1.7}],"propose":null,
                "update":{"summary":"Did X","sections":[{"heading":"Decisions","points":["d1"]}],"status":"WIP"},
                "reasoning":"clear"}"#,
        )
        .unwrap();
        assert_eq!(matched(&a), vec!["KAN-12"]);
        assert_eq!(a.matches[0].confidence, 1.0, "confidence clamped to <=1");
        assert!(a.propose.is_none());
        assert_eq!(a.update.summary, "Did X");
        assert_eq!(a.update.sections.len(), 1);
        assert_eq!(a.update.sections[0].heading, "Decisions");
        assert_eq!(a.update.sections[0].points, vec!["d1"]);
    }

    /// The point of the change: one strand of work can advance several planned
    /// tasks, and the model's order (strongest first) survives to the panel.
    #[test]
    fn parse_reads_several_matches_in_order() {
        let a = parse_answer(
            r#"{"matches":[{"task_key":"KAN-1","confidence":0.9},
                           {"task_key":"KAN-2","confidence":0.83}],
                "propose":null,"update":{"summary":"s","sections":[],"status":""},
                "reasoning":"both moved"}"#,
        )
        .unwrap();
        assert_eq!(matched(&a), vec!["KAN-1", "KAN-2"]);
    }

    /// Every draft generated before multi-match used the singular `match` object,
    /// and models keep emitting it regardless of the schema. Reading it as a
    /// one-element list turns a silent no-match into the right answer.
    #[test]
    fn parse_still_reads_a_singular_match_object() {
        let a = parse_answer(
            r#"{"match":{"task_key":"KAN-12","confidence":0.9},"propose":null,
                "update":{"summary":"s","sections":[],"status":""},"reasoning":"r"}"#,
        )
        .unwrap();
        assert_eq!(matched(&a), vec!["KAN-12"]);
    }

    /// One ticket named twice is one ticket. The target table's PK collapses them
    /// anyway; dropping them here keeps the count honest in the trace and panel.
    #[test]
    fn parse_dedupes_a_repeated_key() {
        let a = parse_answer(
            r#"{"matches":[{"task_key":"KAN-1","confidence":0.9},
                           {"task_key":"KAN-1","confidence":0.7}],
                "propose":null,"update":{"summary":"s","sections":[],"status":""},
                "reasoning":"r"}"#,
        )
        .unwrap();
        assert_eq!(matched(&a), vec!["KAN-1"]);
    }

    #[test]
    fn parse_drops_sections_with_blank_heading_or_no_points() {
        let a = parse_answer(
            r#"{"match":null,"propose":{"issue_type":"Task","title":"T","description":"d"},
                "update":{"summary":"s","sections":[
                    {"heading":"Edits","points":["cut intro","colour graded"]},
                    {"heading":"","points":["orphaned"]},
                    {"heading":"Empty","points":[]}
                ],"status":"WIP"},
                "reasoning":"r"}"#,
        )
        .unwrap();
        // Only the well-formed section survives; the model's headings are free-form.
        assert_eq!(a.update.sections.len(), 1);
        assert_eq!(a.update.sections[0].heading, "Edits");
        assert_eq!(a.update.sections[0].points.len(), 2);
    }

    #[test]
    fn parse_reads_a_proposal() {
        let a = parse_answer(
            r#"{"match":null,"propose":{"issue_type":"Bug","title":"Fix the crash",
                "description":"It crashes"},
                "update":{"summary":"s","sections":[],"status":""},
                "reasoning":"new work"}"#,
        )
        .unwrap();
        assert!(a.matches.is_empty());
        let p = a.propose.unwrap();
        assert_eq!(p.issue_type, "Bug");
        assert_eq!(p.title, "Fix the crash");
    }

    #[test]
    fn empty_match_object_is_treated_as_no_match() {
        // A model that emits an object with a blank task_key must NOT count as a match.
        let a = parse_answer(
            r#"{"matches":[{"task_key":"","confidence":0.9}],
                "propose":{"issue_type":"Task","title":"Do it","description":"d"},
                "update":{"summary":"s","sections":[],"status":""},
                "reasoning":"r"}"#,
        )
        .unwrap();
        assert!(a.matches.is_empty(), "blank task_key is not a match");
        assert!(a.propose.is_some());
    }

    /// A hallucinated key must not take the whole draft down with it: two good
    /// matches and a written update are worth more than punishing a typo.
    #[tokio::test]
    async fn resolve_targets_drops_a_ticket_that_is_not_on_the_board() {
        let pool = db().await;
        seed(&pool, "KAN-1", "jira").await;

        let out = resolve_targets(
            &pool,
            &[
                ParsedMatch {
                    task_key: "KAN-1".into(),
                    confidence: 0.9,
                },
                ParsedMatch {
                    task_key: "KAN-404".into(),
                    confidence: 0.8,
                },
            ],
        )
        .await
        .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].task_key, "KAN-1");
        assert_eq!(
            out[0].provider, "jira",
            "each ticket carries its own tracker"
        );
        assert!(!out[0].manual, "the model chose this, not the user");
    }

    /// When nothing survives there is genuinely nowhere to post - an error the user
    /// can act on beats a draft that fails at approve time.
    #[tokio::test]
    async fn resolve_targets_errors_when_no_ticket_survives() {
        let pool = db().await;
        assert!(resolve_targets(
            &pool,
            &[ParsedMatch {
                task_key: "KAN-404".into(),
                confidence: 0.9,
            }],
        )
        .await
        .is_err());
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
            sections: vec![
                WorklogSection {
                    heading: "Decisions".into(),
                    points: vec!["Chose one LLM call".into()],
                },
                WorklogSection {
                    heading: "Architecture".into(),
                    points: vec![],
                },
            ],
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
