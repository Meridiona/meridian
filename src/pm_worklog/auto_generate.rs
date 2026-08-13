//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Auto-generate worklog DRAFTS, once a day, at a user-chosen local clock time —
//! the opt-in behind `settings.worklog_auto_generate_time`.
//!
//! This runs the exact same action as the day-task detail panel's "Generate
//! worklog" button (`pm_worklog::generate`), automatically, for any task past the
//! fixed [`QUALIFYING_MINUTES`] threshold that doesn't have a draft yet. It NEVER
//! approves or posts — approving is always a deliberate human click
//! (`pm_worklog::approve`, the panel's "Approve & post"). Auto-generate only saves
//! the user the click that starts the draft; the tracker is never touched without
//! them.
//!
//! Off by default: the field is `None` until the user picks a time from the
//! Timeline nudge or Settings → Worklogs, so nothing here ever runs unattended for
//! someone who hasn't said yes.
//!
//! # A TRACKER IS NOT REQUIRED — and used to be, wrongly
//! Both entry points opened with `if config.pm_providers.is_empty() { return }`,
//! on the premise that "there is nothing to match against, so the sweep would only
//! fail every call". That premise is false, and stopped being true the moment
//! personal tasks became first-class: [`crate::pm_worklog::generate`] matches a
//! PERSONAL (`local`) day-task exactly like a real ticket and, on approve, posts to
//! that task's own row (`post_to_local_task`) — no tracker anywhere in the path.
//! Only the PROPOSE branch needs a configured provider, and that branch failing is
//! already a per-task `warn!` the loop walks past.
//!
//! So the gate silently disabled the whole feature for every tracker-less user —
//! the cohort Meridian now leads with — and Settings mirrored it with a dead-end
//! card offering nothing. Removed at both ends. Keep it that way: the correct floor
//! is "does this task have qualifying minutes and no draft yet", which is what the
//! sweep already checks, not "does this user have Jira".
//!
//! # Once a day, from the chosen time onward — not continuously, and self-healing
//! [`maybe_auto_generate`] is called every clock-aligned wake (HH:03, same as the
//! rest of the worklog pipeline), but only actually scans once the current local
//! hour has REACHED the chosen time's hour — so a task crossing the threshold at
//! 2pm with a 6pm setting waits for 6pm, not the next tick after 2pm.
//!
//! Deliberately `>=`, not `==`: if the Mac is asleep, off, or the app isn't
//! running at exactly 9pm, there is no tick at 9pm to miss — the daemon simply
//! catches up on its next wake that same day (10pm, or whenever the machine comes
//! back), because every tick from the chosen hour through local midnight re-runs
//! the same scan. This is safe to repeat because already-drafted tasks are
//! skipped (see below) — a caught-up run only ever drafts what genuinely wasn't
//! drafted yet, it never redoes work. The one thing this does NOT do is reach
//! back across local midnight: if the whole rest of the day was slept through,
//! that day's catch-up is skipped, matching this pipeline's standing "no
//! backfill past today" policy — the manual "Generate worklog" button in the
//! panel always still works for any past day.
//!
//! # Fires once per task, not on every qualifying tick
//! Once a task has ANY draft (auto- or manually generated), this leaves it alone
//! for good — it does not keep re-drafting an already-drafted task on a later
//! tick, whether that's later the same evening (the catch-up case above) or a
//! future day. If the user keeps working the task past the auto-draft, getting
//! an up-to-date version is a manual "Regenerate" click in the panel (which
//! shows when the draft was last generated, via `DayTaskWorklogDraft::
//! updated_at`) — auto-generate hands off to that rather than looping forever.
//!
//! # One task at a time
//! Tasks are processed in a plain sequential loop, not fanned out — each `generate`
//! call is a real LLM request, and running them one after another (rather than
//! concurrently) keeps this predictable and easy to reason about from the trace.
//!
//! # Observability
//! The whole run is one `worklog.auto_generate.run` span (see
//! [`maybe_auto_generate`]) carrying the gate outcome and per-run counts as
//! attributes — `gate` names exactly why a run did or didn't scan (`disabled`,
//! `before_chosen_hour`, `malformed_time`, or `ran`), so "why didn't this fire
//! tonight" is answerable from the trace alone, not by reading code. Each task
//! that actually gets drafted (or fails to) gets its own `info!`/`warn!` — real,
//! individually actionable events — but a task skipped for already having a
//! draft or not yet qualifying is silent (folded into the summary counts only),
//! so a quiet evening with nothing to do doesn't spam the log with one line per
//! task.
//!
//! # Who calls this
//! [`crate::worklog_pipeline::run_loop`], every clock-aligned wake (HH:03), right
//! after the hourly activity-report pass.
//!
//! # Related
//! - [`crate::pm_worklog::generate`] — the exact function the manual button calls;
//!   this module never calls [`crate::pm_worklog::approve`].
//! - [`meridian_core::day_tasks::get_day_tasks`] — the read model this scans;
//!   `DayTask::minutes` is the deterministic measured total.
//! - [`meridian_core::day_task_worklogs::get_day_task_worklog`] — the "has this
//!   task already been drafted" check that makes auto-generate fire exactly once
//!   per task.

use chrono::{Local, Timelike};
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

use crate::config::Config;
use crate::pm_worklog;

/// Fixed qualifying threshold — a day-task needs more than this many tracked
/// minutes to be auto-drafted. Not user-configurable: only WHEN Meridian checks
/// (`worklog_auto_generate_time`) is a choice; WHICH tasks qualify is not.
const QUALIFYING_MINUTES: i64 = 30;

/// Parse "HH:MM" into `(hour, minute)`. `None` for anything malformed — the
/// settings write path (`update_settings`) already rejects a bad value, so this
/// only has to be defensive against a hand-edited file.
fn parse_hh_mm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// Once the current local hour has reached the user's chosen hour (today), scan
/// `day_local`'s day-tasks and auto-generate a worklog DRAFT (never approve,
/// never post) for any past [`QUALIFYING_MINUTES`] that doesn't have a draft yet.
/// A no-op when `worklog_auto_generate_time` is unset, or before that hour. Safe
/// to call again later the same day (e.g. the machine was asleep at the chosen
/// time) — see the module docs. Tasks are processed one at a time, in order —
/// never concurrently.
#[tracing::instrument(skip(pool, config))]
pub async fn maybe_auto_generate(pool: &SqlitePool, config: &Config, day_local: &str) {
    let span = tracing::info_span!(
        "worklog.auto_generate.run",
        day = day_local,
        chosen_time = Empty,
        gate = Empty,
        tasks_total = Empty,
        tasks_qualifying = Empty,
        tasks_already_drafted = Empty,
        tasks_drafted = Empty,
        tasks_failed = Empty,
    );
    run(pool, config, day_local).instrument(span).await
}

async fn run(pool: &SqlitePool, config: &Config, day_local: &str) {
    let current_span = tracing::Span::current();

    let settings = meridian_core::settings::load_runtime_settings();
    let Some(chosen_time) = settings.worklog_auto_generate_time else {
        current_span.record("gate", "disabled");
        return;
    };
    current_span.record("chosen_time", chosen_time.as_str());

    let Some((chosen_hour, _)) = parse_hh_mm(&chosen_time) else {
        current_span.record("gate", "malformed_time");
        tracing::warn!(
            chosen_time,
            "worklog: auto-generate time is malformed — skipping"
        );
        return;
    };
    if Local::now().hour() < chosen_hour {
        current_span.record("gate", "before_chosen_hour");
        return;
    }
    current_span.record("gate", "ran");

    draft_qualifying_tasks(pool, config, day_local, &current_span).await;
}

/// The "Generate now" path — the same day-task draft sweep as [`maybe_auto_generate`],
/// but WITHOUT the time-of-day gate, run because the user asked for it on the spot
/// (the daily-summary screen's "Generate now" button) rather than because the clock
/// reached their chosen time. Everything else is identical: drafts only, never
/// approves/posts, and skips any task that already has a draft, so it is safe to run
/// alongside or before the scheduled pass. Independent of `worklog_auto_generate_time`:
/// a user who never set a time can still generate on demand.
#[tracing::instrument(skip(pool, config))]
pub async fn generate_now(pool: &SqlitePool, config: &Config, day_local: &str) {
    let span = tracing::info_span!(
        "worklog.generate_now.run",
        day = day_local,
        gate = Empty,
        tasks_total = Empty,
        tasks_qualifying = Empty,
        tasks_already_drafted = Empty,
        tasks_drafted = Empty,
        tasks_failed = Empty,
    );
    async {
        let current_span = tracing::Span::current();
        current_span.record("gate", "ran");
        draft_qualifying_tasks(pool, config, day_local, &current_span).await;
    }
    .instrument(span)
    .await
}

/// What one sweep did. Returned rather than only recorded onto the span so the
/// behaviour is assertable: "did this run at all" and "did it early-return" are
/// otherwise indistinguishable from outside, which is exactly how a tracker gate
/// silently disabled the whole feature for solo users once already.
#[derive(Debug, Default, PartialEq, Eq)]
struct SweepCounts {
    total: usize,
    qualifying: u32,
    already_drafted: u32,
    drafted: u32,
    failed: u32,
}

/// Draft a worklog for every qualifying, not-yet-drafted day-task of `day_local`,
/// recording per-run counts onto `span` and returning them. The shared body of
/// [`maybe_auto_generate`] (gated on the clock) and [`generate_now`] (on demand); it
/// assumes its caller has already decided a run is warranted.
///
/// Deliberately NOT gated on a connected tracker - a personal day-task is matched and
/// drafted like any ticket, and only the propose branch needs one (it already fails
/// per-task). See [`neither_sweep_is_gated_on_a_connected_tracker`].
async fn draft_qualifying_tasks(
    pool: &SqlitePool,
    config: &Config,
    day_local: &str,
    span: &tracing::Span,
) -> SweepCounts {
    let tasks = match meridian_core::day_tasks::get_day_tasks(pool, day_local).await {
        Ok(resp) => resp.tasks,
        Err(e) => {
            tracing::warn!(
                day = day_local, error = %e,
                "worklog: auto-generate day-task read failed — skipping this run"
            );
            return SweepCounts::default();
        }
    };
    span.record("tasks_total", tasks.len());
    let total = tasks.len();

    let mut qualifying = 0u32;
    let mut already_drafted = 0u32;
    let mut drafted = 0u32;
    let mut failed = 0u32;

    for task in tasks {
        if task.minutes < QUALIFYING_MINUTES {
            continue;
        }
        qualifying += 1;

        // Already drafted — by an earlier auto-generate run, or by the user
        // clicking "Generate worklog" themselves. Either way this task is now the
        // user's to drive (Regenerate in the panel), never auto-generate's again.
        match meridian_core::day_task_worklogs::get_day_task_worklog(pool, day_local, &task.id)
            .await
        {
            Ok(Some(_)) => {
                already_drafted += 1;
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    day = day_local, task_id = %task.id, error = %e,
                    "worklog: auto-generate existing-draft check failed — skipping this task this run"
                );
                continue;
            }
        }

        tracing::info!(
            day = day_local, task_id = %task.id, minutes = task.minutes,
            "worklog: threshold crossed — drafting"
        );
        match pm_worklog::generate(pool, config, day_local, &task.id).await {
            Ok(draft) => {
                drafted += 1;
                tracing::info!(
                    day = day_local, task_id = %task.id, state = draft.state,
                    "worklog: auto-generated (draft only — never posted)"
                );
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    day = day_local, task_id = %task.id, error = %e,
                    "worklog: auto-generate failed — the user can still generate it manually"
                );
            }
        }
    }

    span.record("tasks_qualifying", qualifying);
    span.record("tasks_already_drafted", already_drafted);
    span.record("tasks_drafted", drafted);
    span.record("tasks_failed", failed);
    tracing::info!(
        day = day_local,
        qualifying,
        already_drafted,
        drafted,
        failed,
        "worklog: auto-generate run complete"
    );

    SweepCounts {
        total,
        qualifying,
        already_drafted,
        drafted,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::{draft_qualifying_tasks, parse_hh_mm, SweepCounts, QUALIFYING_MINUTES};
    use crate::config::Config;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    const DAY: &str = "2026-08-07";

    /// The two tables the sweep reads: day-tasks to enumerate, worklogs to skip
    /// ones already drafted. `pm_providers` is a CONFIG field, not a table, so a
    /// tracker-less user is modelled by the empty `Config` below.
    async fn seeded() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE day_tasks (day_local TEXT NOT NULL, task_id TEXT NOT NULL, \
                title TEXT, summary TEXT, hours_json TEXT, segments_json TEXT, \
                minutes INTEGER, status TEXT, linked_ticket TEXT, \
                PRIMARY KEY (day_local, task_id))",
            // Mirrors `meridian-core/src/readers/day_task_worklogs/tests.rs`'s fixture -
            // `get_day_task_worklog` reads the targets table too, and a column short of
            // it the read fails and every task counts as `failed` rather than
            // `already_drafted`, which would make this test pass for the wrong reason.
            "CREATE TABLE day_task_worklogs (day_local TEXT NOT NULL, task_id TEXT NOT NULL, \
                provider TEXT NOT NULL DEFAULT 'local', state TEXT NOT NULL DEFAULT 'drafted', \
                update_summary TEXT NOT NULL DEFAULT '', update_json TEXT NOT NULL DEFAULT '{}', \
                reasoning TEXT NOT NULL DEFAULT '', propose_issue_type TEXT, propose_title TEXT, \
                propose_description TEXT, created_task_key TEXT, last_error TEXT, \
                create_attempt_at TEXT, drafted_minutes INTEGER, \
                created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '', \
                PRIMARY KEY (day_local, task_id))",
            "CREATE TABLE day_task_worklog_targets (day_local TEXT NOT NULL, \
                task_id TEXT NOT NULL, task_key TEXT NOT NULL, provider TEXT NOT NULL, \
                confidence REAL NOT NULL DEFAULT 0, manual INTEGER NOT NULL DEFAULT 0, \
                position INTEGER NOT NULL DEFAULT 0, posted_comment_id TEXT, browse_url TEXT, \
                posted_at TEXT, last_error TEXT, post_attempt_at TEXT, \
                created_at TEXT NOT NULL DEFAULT '', update_json TEXT, \
                PRIMARY KEY (day_local, task_id, task_key))",
            "CREATE TABLE pm_tasks (task_key TEXT PRIMARY KEY, title TEXT NOT NULL)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn put_task(pool: &SqlitePool, task_id: &str, minutes: i64) {
        sqlx::query(
            "INSERT INTO day_tasks (day_local, task_id, title, minutes) VALUES (?, ?, ?, ?)",
        )
        .bind(DAY)
        .bind(task_id)
        .bind(format!("Task {task_id}"))
        .bind(minutes)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn put_draft(pool: &SqlitePool, task_id: &str) {
        sqlx::query("INSERT INTO day_task_worklogs (day_local, task_id) VALUES (?, ?)")
            .bind(DAY)
            .bind(task_id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// A user with NO tracker connected - the exact configuration the removed
    /// gate used to abandon.
    fn no_tracker_config() -> Config {
        Config {
            meridian_db: ":memory:".into(),
            poll_interval_secs: 60,
            pm_providers: Vec::new(),
            jira_update_enabled: false,
            jira_update_interval_s: 14_400,
            jira_office_start_hour: 9,
            jira_office_end_hour: 17,
            runtime: Default::default(),
        }
    }

    /// The functional half of [`neither_sweep_is_gated_on_a_connected_tracker`].
    ///
    /// That test greps the source for the gate's return; this one proves the sweep
    /// actually walks its task list with `pm_providers` empty. The two together
    /// close the hole: a gate written a DIFFERENT way (a different field, an early
    /// `return` above the read) would slip past the grep, but not past this.
    ///
    /// Every task here is pre-drafted on purpose, so the loop reaches its
    /// already-drafted branch and returns without ever calling `pm_worklog::generate`
    /// - no LLM, no network, no CLI spawn. Counting the tasks it CONSIDERED is
    /// enough: an early return yields zeroes, and nothing else does.
    #[tokio::test]
    async fn the_sweep_walks_its_tasks_with_no_tracker_connected() {
        let pool = seeded().await;
        put_task(&pool, "T1", QUALIFYING_MINUTES + 5).await;
        put_task(&pool, "T2", QUALIFYING_MINUTES + 90).await;
        put_draft(&pool, "T1").await;
        put_draft(&pool, "T2").await;

        let counts =
            draft_qualifying_tasks(&pool, &no_tracker_config(), DAY, &tracing::Span::none()).await;

        assert_eq!(
            counts,
            SweepCounts {
                total: 2,
                qualifying: 2,
                already_drafted: 2,
                drafted: 0,
                failed: 0,
            },
            "a tracker-less user's tasks must still be considered"
        );
    }

    /// The threshold still applies - "not gated on a tracker" must not become
    /// "not gated at all".
    #[tokio::test]
    async fn short_tasks_are_still_below_the_threshold() {
        let pool = seeded().await;
        put_task(&pool, "T1", QUALIFYING_MINUTES - 1).await;
        put_draft(&pool, "T1").await;

        let counts =
            draft_qualifying_tasks(&pool, &no_tracker_config(), DAY, &tracing::Span::none()).await;

        assert_eq!(counts.total, 1);
        assert_eq!(counts.qualifying, 0, "under the threshold, never drafted");
        assert_eq!(counts.already_drafted, 0);
    }

    /// A day with nothing on it returns cleanly rather than erroring.
    #[tokio::test]
    async fn an_empty_day_sweeps_to_zero() {
        let pool = seeded().await;
        let counts =
            draft_qualifying_tasks(&pool, &no_tracker_config(), DAY, &tracing::Span::none()).await;
        assert_eq!(counts, SweepCounts::default());
    }

    #[test]
    fn parses_valid_times() {
        assert_eq!(parse_hh_mm("00:00"), Some((0, 0)));
        assert_eq!(parse_hh_mm("18:00"), Some((18, 0)));
        assert_eq!(parse_hh_mm("23:59"), Some((23, 59)));
        assert_eq!(parse_hh_mm("09:05"), Some((9, 5)));
    }

    #[test]
    fn rejects_out_of_range_or_malformed_values() {
        assert_eq!(parse_hh_mm("24:00"), None);
        assert_eq!(parse_hh_mm("18:60"), None);
        assert_eq!(parse_hh_mm("18"), None);
        assert_eq!(parse_hh_mm("18:00:00"), None);
        assert_eq!(parse_hh_mm(""), None);
        assert_eq!(parse_hh_mm("not-a-time"), None);
        assert_eq!(parse_hh_mm("ab:cd"), None);
    }

    /// Neither entry point may re-acquire a "do they have a tracker" precondition.
    ///
    /// Source-level because the gate was a bare early return with no observable
    /// output to assert on - it just made the feature silently do nothing for every
    /// tracker-less user, in a build that otherwise looked healthy. Drafting works
    /// without a tracker (a personal day-task is matched and drafted like any
    /// ticket); only the propose branch needs one, and it already fails per-task.
    #[test]
    fn neither_sweep_is_gated_on_a_connected_tracker() {
        let src = include_str!("auto_generate.rs");
        // Comment lines dropped, not just their markers - the module doc above
        // quotes the removed gate verbatim to explain why it went.
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // Assembled, not written out: a literal would match itself on this very line.
        let gate = format!("pm_providers{}", ".is_empty()");
        assert!(
            !code.contains(&gate),
            "the tracker gate is back - it disables auto-generate for every solo user"
        );
    }
}
