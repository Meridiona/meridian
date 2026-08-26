//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The shared sweep body [`super::maybe_auto_generate`] (clock-gated) and
//! [`super::generate_now`] (on demand) both call: draft a worklog for every
//! qualifying, not-yet-drafted day-task. Split out of `auto_generate.rs` when that
//! file passed the repo's 500-line cap — see `auto_generate/mod.rs`'s module doc
//! for the full feature description; this file is the mechanical loop only.

use sqlx::SqlitePool;

use crate::config::Config;
use crate::pm_worklog;

/// Fixed qualifying threshold — a day-task needs more than this many tracked
/// minutes to be auto-drafted. Not user-configurable: only WHEN Meridian checks
/// (`worklog_auto_generate_time`) is a choice; WHICH tasks qualify is not.
pub(super) const QUALIFYING_MINUTES: i64 = 30;

/// What one sweep did. Returned rather than only recorded onto the span so the
/// behaviour is assertable: "did this run at all" and "did it early-return" are
/// otherwise indistinguishable from outside, which is exactly how a tracker gate
/// silently disabled the whole feature for solo users once already.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SweepCounts {
    total: usize,
    qualifying: u32,
    already_drafted: u32,
    drafted: u32,
    failed: u32,
}

/// Draft a worklog for every qualifying, not-yet-drafted day-task of `day_local`,
/// recording per-run counts onto `span`. The shared body of
/// [`super::maybe_auto_generate`] (gated on the clock) and [`super::generate_now`]
/// (on demand); it assumes its caller has already decided a run is warranted.
///
/// Deliberately NOT gated on a connected tracker - a personal day-task is matched and
/// drafted like any ticket, and only the propose branch needs one (it already fails
/// per-task). See [`neither_sweep_is_gated_on_a_connected_tracker`].
pub(super) async fn draft_qualifying_tasks(
    pool: &SqlitePool,
    config: &Config,
    day_local: &str,
    span: &tracing::Span,
) -> SweepCounts {
    // Matching a day-task against `pm_tasks` wants current ticket state, and there is
    // no background poller keeping it fresh anymore — so this sweep asks for it.
    //
    // UNATTENDED on purpose. This runs on a clock (HH:03), which makes it the one
    // remaining path that could fire a Jira OAuth refresh with nobody at the machine
    // — the exact shape that permanently killed a production user's grant when a
    // refresh POST straddled a 28-minute suspend. `run_pm_sync_unattended` will use a
    // valid access token but never mint one, so an expired token simply defers to the
    // next attended request instead of betting the grant. Best-effort either way: a
    // sync failure must not block drafting against whatever is cached.
    if let Err(e) = crate::intelligence::run_pm_sync_unattended(pool, config).await {
        tracing::warn!(
            day = day_local, error = %e,
            "worklog: auto-generate pm sync failed — matching against cached tasks"
        );
    }

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
    use super::{draft_qualifying_tasks, SweepCounts, QUALIFYING_MINUTES};
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

    /// Neither entry point may re-acquire a "do they have a tracker" precondition.
    ///
    /// Source-level because the gate was a bare early return with no observable
    /// output to assert on - it just made the feature silently do nothing for every
    /// tracker-less user, in a build that otherwise looked healthy. Drafting works
    /// without a tracker (a personal day-task is matched and drafted like any
    /// ticket); only the propose branch needs one, and it already fails per-task.
    #[test]
    fn neither_sweep_is_gated_on_a_connected_tracker() {
        // Both files: `maybe_auto_generate`/`generate_now` (the entry points) live in
        // `mod.rs`, the actual per-task loop lives here — a gate could reappear in
        // either.
        let src = concat!(include_str!("mod.rs"), "\n", include_str!("sweep.rs"));
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
