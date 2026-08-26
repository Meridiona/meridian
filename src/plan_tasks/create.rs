//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Creating a task from the daily plan — personal, or synced to the user's tracker —
//! and putting it straight into today's focus.
//!
//! # The two branches converge
//! `personal` mints a `LOCAL-<n>` key; `sync` files a real ticket through
//! [`crate::pm_worklog::create::create_ticket`]. Both end at ONE `pm_tasks` row and
//! ONE `plan add`, so everything downstream (the board, carryover, the plan's
//! rendering) treats them identically.
//!
//! # Why the ordering is load-bearing
//! `create_ticket` writes NOTHING to our DB — only a sync inserts, and that is gated
//! at 5 minutes. Until a `pm_tasks` row exists, `plan.rs`'s `load_plan` sees
//! `on_board = false` and renders the card **struck-through "done" and titled with its
//! raw key** (`is_terminal: if on_board { … } else { true }` — it ignores the
//! `task_snapshot`'s own flag, so the 044 snapshot column cannot rescue this).
//!
//! Pre-inserting a `provider='jira'` row to dodge that is a TRAP: the force-sync's own
//! prune (`WHERE provider='jira' AND task_key NOT IN (…fetched…)`) would delete it
//! whenever self-assign failed. So: create → force-sync → **verify** → and only if the
//! row still isn't there, write a `provider='local'` shadow row. A prune cannot touch
//! that, and a later successful sync UPSERTs over it and flips the provider back,
//! converging on truth by itself.
//!
//! # Who calls this
//! [`super::cli`]'s `plan-task-create` → the tray's `create_plan_task` → the composer.
//!
//! # Related
//! - [`meridian_core::task_create`] — the row writer + the `'local'` invariant.
//! - [`meridian_core::plan`] — `apply_plan_action("add")`, which puts the key in today.

use anyhow::{Context, Result};
use meridian_core::pm_sync_requests::SyncMode;
use meridian_core::task_create::{self, NewTask};
use serde::Serialize;
use sqlx::SqlitePool;
use std::time::Duration;
use tracing::field::Empty;
use tracing::Instrument;

use crate::config::Config;
use crate::intelligence::sync_delegate::Delegation;

/// How long to wait for the post-create sync before falling through to the shadow row.
/// Must stay comfortably inside the tray's 90 s `WRITE_TIMEOUT` for `plan-task-create`,
/// since a blown tray timeout looks to the user like a failed create even though the
/// ticket was filed.
const POST_CREATE_SYNC_BUDGET: Duration = Duration::from_secs(60);

/// Where a new task should live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Personal — Meridian only. The single option for a user with no tracker.
    Personal,
    /// File a real ticket on this provider, then track it like any board ticket.
    Tracker(String),
}

impl Target {
    /// Parse the CLI's `--target` (`local` | a provider id).
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "" | "local" | "personal" => Self::Personal,
            provider => Self::Tracker(provider.to_string()),
        }
    }
}

/// The result of [`create`], serialized to the CLI's one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedTask {
    pub task_key: String,
    /// The provider the row ended up with — `local` for a personal task, and also for
    /// a synced ticket that hasn't appeared in our mirror yet (see [`note`]).
    pub provider: String,
    /// True when a real ticket was filed on a tracker.
    pub synced: bool,
    /// A soft, user-facing caveat — NOT an error. Set when the ticket was created but
    /// hasn't landed in our mirror, which is usually a failed self-assign.
    pub note: Option<String>,
}

/// Create a task and add it to `day`'s plan. See the module header for the ordering.
///
/// Errors only when the task could not be created at all (e.g. the tracker rejected
/// it, or we are offline). A partial success — ticket filed but not yet mirrored —
/// returns `Ok` with `note` set, because the user's task exists and works.
#[tracing::instrument(skip(pool, config, title, description))]
pub async fn create(
    pool: &SqlitePool,
    config: &Config,
    target: &Target,
    title: &str,
    description: &str,
    issue_type: &str,
    day: &str,
) -> Result<CreatedTask> {
    let span = tracing::info_span!(
        "plan_task.create",
        day = day,
        synced = Empty,
        provider = Empty,
        task_key = Empty,
        shadowed = Empty,
    );
    async move {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("a title is required to create a task");
        }
        let issue_type = normalise_issue_type(issue_type);
        let now = chrono::Utc::now().to_rfc3339();

        // Check the plan cap BEFORE filing anything. `add_to_plan` below can now
        // fail (the day is capped at MAX_PLAN_TASKS), and `create_on_tracker`
        // files a REAL ticket on a REAL board first - so checking after would
        // leave a live Jira ticket the user never asked to exist and never sees.
        // The cap is the plan's rule, so a full plan must refuse before we act on
        // the outside world, not after.
        ensure_plan_has_room(pool, day).await?;

        let created = match target {
            Target::Personal => {
                let key =
                    task_create::create_local_task(pool, title, description, issue_type, &now)
                        .await
                        .context("creating the personal task")?;
                tracing::Span::current().record("synced", false);
                CreatedTask {
                    task_key: key,
                    provider: task_create::LOCAL_PROVIDER.to_string(),
                    synced: false,
                    note: None,
                }
            }
            Target::Tracker(provider) => {
                create_on_tracker(pool, config, provider, title, description, issue_type, &now)
                    .await?
            }
        };

        tracing::Span::current().record("provider", created.provider.as_str());
        tracing::Span::current().record("task_key", created.task_key.as_str());

        add_to_plan(pool, &created.task_key, day, &now)
            .await
            .context("adding the new task to today's plan")?;

        tracing::info!(
            task_key = created.task_key,
            provider = created.provider,
            synced = created.synced,
            "plan_task: created and added to the plan"
        );
        Ok(created)
    }
    .instrument(span)
    .await
}

/// File a real ticket, then make sure we have a row for it. The verify-then-shadow
/// dance is explained in the module header.
#[allow(clippy::too_many_arguments)]
async fn create_on_tracker(
    pool: &SqlitePool,
    config: &Config,
    provider: &str,
    title: &str,
    description: &str,
    issue_type: &str,
    now: &str,
) -> Result<CreatedTask> {
    // GitHub infers owner/repo from an existing key, and an OAuth Jira has no
    // configured project_keys — both rely on this sample. Best-effort by design.
    let sample = fetch_sample_task_key(pool, provider).await;
    let key = crate::pm_worklog::create::create_ticket(
        config,
        provider,
        title,
        description,
        issue_type,
        sample.as_deref(),
    )
    .await
    .with_context(|| format!("creating the ticket on {provider}"))?;
    tracing::Span::current().record("synced", true);

    // Pull the authoritative row in immediately rather than waiting out the 5-minute
    // sync gate. Best-effort: a failed sync is not a failed create - it just means the
    // `task_exists` check below falls through to the shadow row.
    //
    // Delegated to the daemon (see `intelligence::sync_delegate`) because this is a
    // short-lived CLI process and the rotating Jira OAuth token has exactly one safe
    // writer. Unlike the edit/done paths this WAITS for the outcome: the very next line
    // reads `pm_tasks`, so returning before the sync landed would shadow every created
    // ticket. The budget sits inside the tray's 90 s `WRITE_TIMEOUT` for this command.
    match crate::intelligence::sync_delegate::sync_and_wait(
        pool,
        config,
        SyncMode::Force,
        "plan-task-create",
        POST_CREATE_SYNC_BUDGET,
    )
    .await
    {
        Delegation::Synced { .. } => {}
        Delegation::Failed { error } => {
            tracing::warn!(%error, "plan_task: post-create sync failed - will shadow the row");
        }
        Delegation::Pending => {
            tracing::warn!("plan_task: post-create sync still running - will shadow the row");
        }
    }

    if task_create::task_exists(pool, &key).await? {
        tracing::Span::current().record("shadowed", false);
        return Ok(CreatedTask {
            task_key: key,
            provider: provider.to_string(),
            synced: true,
            note: None,
        });
    }

    // The ticket is real but our mirror hasn't got it. Overwhelmingly this means one
    // of `create_ticket`'s best-effort follow-ups failed — the self-assign (every
    // provider's sync only fetches issues assigned to the user) or, on GitHub, the
    // board add as well (its sync walks project items, so an issue that isn't ON the
    // board is invisible however it's assigned). Neither heals by waiting. Shadow it
    // as 'local' — prune cannot touch that, and a later sync UPSERTs over it.
    tracing::Span::current().record("shadowed", true);
    tracing::warn!(
        task_key = key,
        provider,
        "plan_task: created ticket is not in pm_tasks - writing a local shadow row"
    );
    task_create::insert_task(
        pool,
        &NewTask {
            key: key.clone(),
            provider: task_create::LOCAL_PROVIDER.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            issue_type: issue_type.to_string(),
            url: String::new(),
        },
        now,
    )
    .await
    .context("writing the shadow row for the created ticket")?;

    Ok(CreatedTask {
        task_key: key,
        provider: task_create::LOCAL_PROVIDER.to_string(),
        synced: true,
        note: Some(shadow_note(provider)),
    })
}

/// The soft caveat shown when a real ticket was filed but hasn't reached our
/// mirror. Provider-specific because the thing the user has to fix differs: every
/// tracker's sync needs the ticket assigned to them, but GitHub's ALSO needs it to
/// be an item on a configured Projects v2 board — so telling a GitHub user to
/// check the assignee sends them to look at something that is already correct.
fn shadow_note(provider: &str) -> String {
    match provider {
        "github" => "Created on GitHub - it'll appear here once it's on your project board and \
                     assigned to you. If it isn't, reconnect GitHub in Settings so Meridian can \
                     add it for you."
            .to_string(),
        other => {
            format!("Created in {other} - it'll appear on your board once it's assigned to you.")
        }
    }
}

/// Refuse early when `day`'s plan is already full.
///
/// A guard against an outward-facing side effect we cannot take back: creating a
/// task files a real ticket on a shared board, and only then adds it to the plan.
/// The message is the plan's own, so the user reads the same sentence here as
/// when they drag an eleventh card.
async fn ensure_plan_has_room(pool: &SqlitePool, day: &str) -> Result<()> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_plan WHERE plan_date = ?")
        .bind(day)
        .fetch_one(pool)
        .await
        .context("counting today's planned tasks")?;
    if n as usize >= meridian_core::plan::MAX_PLAN_TASKS {
        anyhow::bail!(
            "{}",
            meridian_core::plan::PlanWriteError::TooManyTasks(n as usize + 1)
        );
    }
    Ok(())
}

/// Put `task_key` at the end of `day`'s plan, reusing the plan's own `add` action so
/// position/origin/snapshot handling stays in one place.
async fn add_to_plan(pool: &SqlitePool, task_key: &str, day: &str, now: &str) -> Result<()> {
    let today = chrono::Local::now().date_naive();
    let body = meridian_core::plan::PlanBody {
        action: "add".to_string(),
        date: Some(day.to_string()),
        task_key: Some(task_key.to_string()),
        task_keys: None,
    };
    // `add` only needs `available` to resolve an origin, and defaults an unknown key
    // to "manual" — which is exactly right for a task the user just authored.
    meridian_core::plan::apply_plan_action(pool, &body, day, today, now, Vec::new())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

/// Any existing key of `provider` — GitHub needs one to infer owner/repo, Jira to
/// infer the project. Best-effort: `None` on any error.
async fn fetch_sample_task_key(pool: &SqlitePool, provider: &str) -> Option<String> {
    sqlx::query_as::<_, (String,)>("SELECT task_key FROM pm_tasks WHERE provider = ? LIMIT 1")
        .bind(provider)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|(k,)| k)
}

/// Clamp to the pair every provider's create understands. Mirrors the composer's own
/// options and `pm_worklog::create`'s `norm_issue_type`.
fn normalise_issue_type(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "bug" => "Bug",
        _ => "Task",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parses_personal_and_providers() {
        assert_eq!(Target::parse("local"), Target::Personal);
        assert_eq!(Target::parse("personal"), Target::Personal);
        assert_eq!(Target::parse(""), Target::Personal, "absent = personal");
        assert_eq!(Target::parse("  "), Target::Personal);
        assert_eq!(
            Target::parse("jira"),
            Target::Tracker("jira".to_string()),
            "a provider id means file it for real"
        );
    }

    /// GitHub's sync gate is board membership, not just the assignee — the old
    /// one-size note sent a GitHub user to check something already correct.
    #[test]
    fn the_github_shadow_note_names_the_project_board() {
        let note = shadow_note("github");
        assert!(note.contains("project board"), "{note}");
        assert!(note.contains("reconnect GitHub"), "{note}");
    }

    #[test]
    fn other_providers_keep_the_assignee_note() {
        assert!(shadow_note("jira").contains("assigned to you"));
        assert!(shadow_note("jira").contains("jira"));
    }

    /// User-facing copy: plain hyphens only (see the repo's Hard Rules).
    #[test]
    fn shadow_notes_use_plain_hyphens() {
        for p in ["github", "jira", "linear", "trello"] {
            let note = shadow_note(p);
            assert!(!note.contains('—') && !note.contains('–'), "{note}");
        }
    }

    #[test]
    fn issue_type_clamps_to_task_or_bug() {
        assert_eq!(normalise_issue_type("Bug"), "Bug");
        assert_eq!(normalise_issue_type("bug"), "Bug");
        for raw in ["Task", "Story", "", "epic", "improvement"] {
            assert_eq!(
                normalise_issue_type(raw),
                "Task",
                "{raw} should clamp to Task"
            );
        }
    }
}
