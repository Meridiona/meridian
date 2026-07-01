//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The approved-worklog poster. This is the ONLY path that writes to a real
// tracker. The hour-driven driver (`scheduler.rs`) only ever DRAFTS; a human
// reviews, optionally edits, and approves each worklog in the dashboard (which
// flips the row to `approved`). This sweep then picks approved rows up and posts
// them to whichever tracker the row belongs to (`pm_worklogs.provider`):
//
//   jira   → native worklog endpoint        (`jira.rs`)
//   linear → structured `commentCreate`      (`linear.rs`)  — no native worklog API
//   github → structured issue comment (REST) (`github.rs`)  — no native time tracking
//
// The sweep runs on a fast cadence independent of the hourly driver so "approve
// in the UI" feels close to immediate.
//
// Idempotent: a window already POSTED short-circuits (the `find_existing_worklog`
// backstop), so a restart mid-sweep can never double-post. A post failure leaves
// the row `approved` with `last_post_error` recorded, and the next sweep retries.

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::config::{
    AzureDevOpsConfig, Config, GitHubConfig, JiraConfig, LinearConfig, PmProviderConfig,
    TrelloConfig,
};

use super::config::PmWorklogConfig;
use super::{azure_devops, create, db, github, jira, linear, trello};

/// How often the approved-sweep runs. Short by design — this is the latency a
/// user feels between clicking "Approve" in the dashboard and the worklog
/// landing in their tracker.
const POST_SWEEP_INTERVAL_SECS: u64 = 60;

/// Outcome of one approved-sweep pass.
#[derive(Debug, Default, Clone)]
pub struct PostSweepSummary {
    pub approved_seen: u32,
    pub posted: u32,
    pub failed: u32,
}

/// Post every approved worklog, routing each to its provider. Rows whose
/// provider is not configured on this daemon are left in place (nothing to post
/// to) with a warning — the same forgiving behaviour the Jira-only poster had.
pub async fn post_approved(
    pool: &SqlitePool,
    config: &Config,
    cfg: &PmWorklogConfig,
) -> Result<PostSweepSummary> {
    let approved = db::fetch_approved_worklogs(pool).await?;
    let mut summary = PostSweepSummary {
        approved_seen: approved.len() as u32,
        ..Default::default()
    };
    if approved.is_empty() {
        return Ok(summary);
    }

    for w in approved {
        match post_one(pool, config, cfg, &w).await {
            Ok(true) => summary.posted += 1,
            Ok(false) => {} // ineligible/skipped — already recorded
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(
                    pm_worklog_id = w.id, task = %w.task_key, provider = %w.provider, error = %e,
                    "approved worklog post failed — left approved for retry"
                );
                db::mark_post_failed(pool, w.id, &format!("{e:#}")).await?;
            }
        }
    }
    Ok(summary)
}

/// Format a stored UTC ISO timestamp as local wall-clock — so the worklog_post
/// span (and the dashboard) shows lifecycle times in the dev's local zone.
/// Convert a local-time hour label (`YYYY-MM-DDTHH`) to a UTC ISO timestamp.
/// Used as a fallback when `window_start`/`window_end` are NULL on pre-050 rows.
/// `mm` and `ss` are the minute/second to set within the hour (0,0 → start; 59,59 → end).
fn local_hour_to_utc(label: &str, mm: u32, ss: u32) -> String {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let padded = format!("{}:{:02}:{:02}", label, mm, ss);
    let naive = NaiveDateTime::parse_from_str(&padded, "%Y-%m-%dT%H:%M:%S").unwrap_or_default();
    match Local.from_local_datetime(&naive).single() {
        Some(local_dt) => local_dt
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string(),
        // DST ambiguity: use the earlier interpretation
        None => chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string(),
    }
}

fn local_ts(iso_utc: &str) -> String {
    use chrono::{DateTime, Local};
    DateTime::parse_from_rfc3339(iso_utc)
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| iso_utc.to_string())
}

/// Now, as local wall-clock — recorded as `posted_at_local` when a post lands.
fn local_now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Post one approved worklog. Returns `Ok(true)` if it was posted, `Ok(false)` if
/// it was skipped as ineligible (too short / empty / no matching provider — each
/// recorded), or `Err` on a transient failure the caller should record for retry.
///
/// Wrapped in a `worklog_post` span so every post attempt is one trace carrying
/// the full lifecycle: when drafted, when approved, when posted (all local),
/// provider, final state, posted id, and time logged.
#[tracing::instrument(
    name = "worklog_post",
    skip_all,
    fields(
        task_key = %w.task_key,
        provider = %w.provider,
        time_spent_seconds = w.time_spent_seconds,
        post_attempt_count = w.post_attempt_count,
        window_start = %w.window_start,
        state = tracing::field::Empty,
        posted_worklog_id = tracing::field::Empty,
        drafted_at_local = tracing::field::Empty,
        approved_at_local = tracing::field::Empty,
        posted_at_local = tracing::field::Empty,
    )
)]
async fn post_one(
    pool: &SqlitePool,
    config: &Config,
    cfg: &PmWorklogConfig,
    w: &db::ApprovedWorklog,
) -> Result<bool> {
    let span = tracing::Span::current();
    if let Some(c) = &w.created_at {
        span.record("drafted_at_local", local_ts(c).as_str());
    }
    if let Some(a) = &w.approved_at {
        span.record("approved_at_local", local_ts(a).as_str());
    }

    if w.comment.trim().is_empty() {
        span.record("state", "failed");
        // Nothing to post — the user approved an empty draft. Terminal: editing
        // the worklog re-drafts it, so don't keep retrying.
        db::fail_worklog(pool, w.id, "approved worklog has an empty comment").await?;
        return Ok(false);
    }
    if w.time_spent_seconds < cfg.min_post_seconds {
        // Below the worklog floor — terminal; nothing the sweep can do.
        span.record("state", "failed");
        db::fail_worklog(
            pool,
            w.id,
            &format!(
                "time_spent={}s below the {}s minimum",
                w.time_spent_seconds, cfg.min_post_seconds
            ),
        )
        .await?;
        return Ok(false);
    }

    // Idempotency backstop: if this exact (task, window) was already posted,
    // adopt that worklog/comment id instead of posting again.
    if let Some((_, worklog_id)) =
        db::find_existing_worklog(pool, &w.task_key, &w.window_start, &w.window_end).await?
    {
        tracing::info!(
            pm_worklog_id = w.id, task = %w.task_key, %worklog_id,
            "window already posted — adopting existing worklog id"
        );
        db::mark_worklog_posted(pool, w.id, &worklog_id).await?;
        span.record("state", "posted");
        span.record("posted_worklog_id", worklog_id.as_str());
        span.record("posted_at_local", local_now().as_str());
        return Ok(true);
    }

    // Route to the provider this worklog was drafted against. A missing provider
    // config (e.g. the row is for Linear but only Jira is configured here) is not
    // a failure — leave it approved and warn, so it posts once configured.
    let comment = w.comment.trim();
    let (worklog_id, label) = match w.provider.as_str() {
        "jira" => {
            let Some(c) = jira_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = jira::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                comment,
            )
            .await?;
            (r.worklog_id, r.time_spent_jira)
        }
        "linear" => {
            let Some(c) = linear_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = linear::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                &w.window_end,
                comment,
            )
            .await?;
            (r.id, r.label)
        }
        "github" => {
            let Some(c) = github_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = github::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                &w.window_end,
                comment,
            )
            .await?;
            (r.id, r.label)
        }
        "trello" => {
            let Some(c) = trello_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = trello::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                &w.window_end,
                comment,
            )
            .await?;
            (r.id, r.label)
        }
        "azure_devops" => {
            let Some(c) = azure_devops_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = azure_devops::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                &w.window_end,
                comment,
            )
            .await?;
            (r.id, r.label)
        }
        other => {
            span.record("state", "failed");
            db::fail_worklog(pool, w.id, &format!("unknown provider '{other}'")).await?;
            return Ok(false);
        }
    };

    db::mark_worklog_posted(pool, w.id, &worklog_id).await?;
    span.record("state", "posted");
    span.record("posted_worklog_id", worklog_id.as_str());
    span.record("posted_at_local", local_now().as_str());
    tracing::info!(
        pm_worklog_id = w.id, task = %w.task_key, provider = %w.provider,
        worklog_id = %worklog_id, time_spent = %label, "approved worklog posted"
    );
    Ok(true)
}

/// The worklog's provider is not configured on this daemon: leave it approved
/// (nothing to post to) and warn once. Returns `Ok(false)` (not posted, not a
/// hard error — it will post when the provider is configured).
fn missing_provider(provider: &str, w: &db::ApprovedWorklog) -> Result<bool> {
    tracing::Span::current().record("state", "waiting");
    tracing::warn!(
        pm_worklog_id = w.id, task = %w.task_key, %provider,
        "approved worklog waiting but its provider is not configured — not posting"
    );
    Ok(false)
}

fn jira_cfg(config: &Config) -> Option<&JiraConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Jira(j) => Some(j),
        _ => None,
    })
}

fn github_cfg(config: &Config) -> Option<&GitHubConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::GitHub(g) => Some(g),
        _ => None,
    })
}

fn linear_cfg(config: &Config) -> Option<&LinearConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Linear(l) => Some(l),
        _ => None,
    })
}

fn trello_cfg(config: &Config) -> Option<&TrelloConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Trello(t) => Some(t),
        _ => None,
    })
}

fn azure_devops_cfg(config: &Config) -> Option<&AzureDevOpsConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::AzureDevOps(a) => Some(a),
        _ => None,
    })
}

/// Create real tickets for approved tier-3 proposals, then drop an approved
/// worklog row for each so the post sweep comments on the new ticket. Routes to
/// the user's connected provider (the first configured one). A create failure is
/// logged and the proposal is left approved for the next pass to retry — never a
/// hard error that stalls the loop.
#[tracing::instrument(skip(pool, config), name = "proposal.sweep")]
pub async fn process_approved_proposals(pool: &SqlitePool, config: &Config) -> Result<()> {
    let proposals = db::fetch_approved_proposals(pool).await?;
    if proposals.is_empty() {
        return Ok(());
    }
    let Some(provider) = config.pm_providers.first().map(|p| p.provider_name()) else {
        tracing::warn!("approved proposals waiting but no PM provider configured — skipping");
        return Ok(());
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    for p in proposals {
        // GitHub needs an existing repo to infer owner/repo; harmless elsewhere.
        let sample = db::fetch_sample_task_key(pool, provider)
            .await
            .ok()
            .flatten();
        let key = match create::create_ticket(
            config,
            provider,
            &p.title,
            &p.description,
            &p.issue_type,
            sample.as_deref(),
        )
        .await
        {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(
                    proposal_id = p.id, provider, error = %e,
                    "proposal ticket creation failed — left approved for retry"
                );
                continue;
            }
        };

        // Window + cycle fall back to the source hour when unset (pre-050 rows).
        // source_hour is a LOCAL-time label (YYYY-MM-DDTHH); convert to UTC ISO
        // so window_start/end in pm_worklogs are always UTC-anchored.
        let window_start = p
            .window_start
            .clone()
            .unwrap_or_else(|| local_hour_to_utc(&p.source_hour, 0, 0));
        let window_end = p
            .window_end
            .clone()
            .unwrap_or_else(|| local_hour_to_utc(&p.source_hour, 59, 59));
        let cycle_index: i64 = p
            .source_hour
            .get(11..13)
            .and_then(|h| h.parse().ok())
            .unwrap_or(0);

        // Stamp the real key into the drafted payload (it was minted with a
        // placeholder task_key) so the posted comment is grounded correctly.
        let payload_json = {
            let mut v: serde_json::Value = p
                .worklog_payload_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| serde_json::json!({ "summary": "" }));
            v["task_key"] = serde_json::Value::String(key.clone());
            v.to_string()
        };

        let wid = db::insert_proposal_worklog(
            pool,
            &key,
            provider,
            &p.day_utc,
            cycle_index,
            &window_start,
            &window_end,
            p.time_spent_seconds.max(60),
            p.confidence,
            &payload_json,
            &now,
        )
        .await?;
        db::mark_proposal_created(pool, p.id, &key, wid).await?;
        tracing::info!(
            proposal_id = p.id, task_key = %key, worklog_id = wid, provider,
            "proposal approved → ticket created + worklog queued for post"
        );
    }
    Ok(())
}

/// Daemon task: post approved worklogs on a short cadence until shutdown.
pub async fn run_post_loop(pool: SqlitePool, mut shutdown_rx: watch::Receiver<bool>) {
    let cfg = PmWorklogConfig::from_env();
    let interval = std::time::Duration::from_secs(POST_SWEEP_INTERVAL_SECS);
    tracing::info!(
        interval_s = interval.as_secs(),
        "pm-worklog approved-poster starting (approval is the only post gate)"
    );

    loop {
        let config = Config::from_env();
        // Mint real tickets for approved proposals FIRST, so the approved worklog
        // rows they create get posted in the same pass below.
        if let Err(e) = process_approved_proposals(&pool, &config).await {
            tracing::error!(error = %e, "approved-proposal sweep failed");
        }
        match post_approved(&pool, &config, &cfg).await {
            Ok(s) if s.posted > 0 || s.failed > 0 => tracing::info!(
                approved = s.approved_seen,
                posted = s.posted,
                failed = s.failed,
                "approved-poster pass complete"
            ),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "approved-poster pass failed"),
        }

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tokio::time::sleep(interval) => {}
        }
    }
    tracing::info!("pm-worklog approved-poster stopped");
}

/// One-shot CLI: `meridian worklog-post-approved` — post every approved worklog
/// now (the same sweep the daemon runs, for manual/testing use).
pub async fn cli_post_approved(pool: &SqlitePool) {
    let cfg = PmWorklogConfig::from_env();
    let config = Config::from_env();
    let providers: Vec<&str> = config
        .pm_providers
        .iter()
        .map(|p| p.provider_name())
        .collect();
    match post_approved(pool, &config, &cfg).await {
        Ok(s) => println!(
            "worklog-post-approved: approved={} posted={} failed={} (providers={:?})",
            s.approved_seen, s.posted, s.failed, providers,
        ),
        Err(e) => eprintln!("worklog-post-approved: sweep failed: {e:#}"),
    }
}
