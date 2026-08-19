//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! In-memory integration tests for [`meridian_core::usage_rollup`] — the
//! per-day product-action counters behind the tray's `daily_usage` analytics
//! event.
//!
//! Its own file rather than an append to `readers.rs` (already ~2300 lines) so
//! the repo's 500-line cap keeps meaning something; Cargo builds each file in
//! `tests/` as a separate integration binary, so nothing else changes.
//!
//! Every timestamp is placed **relative to `local_day_bounds(DAY)`** rather
//! than hardcoded, so the suite passes in any timezone — the seed and the
//! assertion derive from the same bound math the reader uses.

use meridian_core::date::local_day_bounds;
use meridian_core::usage_rollup::{usage_rollup, PlanState};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// The local day under test. Arbitrary, but fixed — the bounds are computed
/// from it at runtime so the actual UTC instants follow the host's timezone.
const DAY: &str = "2026-05-14";
/// The day before [`DAY`], used to seed rows that must fall OUTSIDE the window.
const PREV_DAY: &str = "2026-05-13";

/// Single-connection `:memory:` pool carrying just the columns the rollup and
/// its one reused reader ([`meridian_core::worklogs::get_worklogs`]) touch.
async fn make_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");

    for ddl in [
        r#"CREATE TABLE pm_worklogs (
            id INTEGER PRIMARY KEY AUTOINCREMENT, task_key TEXT, day_utc TEXT, provider TEXT,
            window_start TEXT, window_end TEXT, state TEXT, confidence REAL, coverage REAL,
            time_spent_seconds INTEGER, payload_json TEXT, posted_worklog_id TEXT,
            last_post_error TEXT, edited_at TEXT, posted_at TEXT, created_at TEXT
        );"#,
        r#"CREATE TABLE pm_tasks (
            task_key TEXT, title TEXT, url TEXT, issue_type TEXT, provider TEXT, updated_at TEXT
        );"#,
        r#"CREATE TABLE pm_proposed_tasks (
            id INTEGER, day_utc TEXT, source_hour TEXT, title TEXT, reasoning TEXT,
            issue_type TEXT, state TEXT, confidence REAL, time_spent_seconds INTEGER,
            window_start TEXT, window_end TEXT, worklog_payload_json TEXT, created_task_key TEXT
        );"#,
        r#"CREATE TABLE daily_plan (
            plan_date TEXT, task_key TEXT, position INTEGER, origin TEXT,
            created_at TEXT, updated_at TEXT
        );"#,
        r#"CREATE TABLE daily_plan_meta (
            plan_date TEXT, confirmed_at TEXT, skipped INTEGER, created_at TEXT, updated_at TEXT
        );"#,
        r#"CREATE TABLE day_tasks (
            day_local TEXT, task_id TEXT, title TEXT, minutes INTEGER, status TEXT
        );"#,
        r#"CREATE TABLE day_task_corrections (
            day_local TEXT, task_id TEXT, action TEXT, merged_into TEXT, created_at TEXT
        );"#,
        r#"CREATE TABLE day_summaries (day_local TEXT, summary_md TEXT);"#,
        r#"CREATE TABLE app_sessions (
            id INTEGER, app_name TEXT, started_at TEXT, ended_at TEXT, task_method TEXT
        );"#,
        r#"CREATE TABLE notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT, dedup_key TEXT, event_key TEXT,
            channels TEXT, delivered_native_at TEXT, banner_dismissed_at TEXT,
            attempts INTEGER NOT NULL DEFAULT 0, created_at TEXT
        );"#,
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    pool
}

/// An instant guaranteed to be inside `day`'s local bounds — its lower edge.
fn inside(day: &str) -> String {
    local_day_bounds(day).0
}

/// The LAST instant of `day` (23:59:59.999 local, as UTC). Exercises the `<=`
/// side of every bounded query — `inside()` only ever tests `>=`, so without
/// this a bound accidentally written `<` would pass the whole suite while
/// silently dropping every late-evening action.
fn last_instant(day: &str) -> String {
    local_day_bounds(day).1
}

#[tokio::test]
async fn counts_product_actions_across_every_surface() {
    let pool = make_pool().await;
    let t = inside(DAY);

    // Two successful posts to jira covering ONE ticket, plus one to linear.
    // tickets_updated must count distinct tickets (2), worklogs_posted the
    // individual posts (3).
    for (task, provider) in [("ENG-1", "jira"), ("ENG-1", "jira"), ("LIN-9", "linear")] {
        sqlx::query(
            "INSERT INTO pm_worklogs (task_key, day_utc, provider, state, posted_at, created_at,
                                      window_start, payload_json, time_spent_seconds)
             VALUES (?, ?, ?, 'posted', ?, ?, ?, '{}', 600)",
        )
        .bind(task)
        .bind(DAY)
        .bind(provider)
        .bind(&t)
        .bind(&t)
        .bind(&t)
        .execute(&pool)
        .await
        .unwrap();
    }
    // A worklog that was attempted and failed to post.
    sqlx::query(
        "INSERT INTO pm_worklogs (task_key, day_utc, provider, state, last_post_error, created_at,
                                  window_start, payload_json, time_spent_seconds)
         VALUES ('ENG-2', ?, 'jira', 'approved', 'HTTP 403', ?, ?, '{}', 300)",
    )
    .bind(DAY)
    .bind(&t)
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();
    // A draft still waiting at day close.
    sqlx::query(
        "INSERT INTO pm_worklogs (task_key, day_utc, provider, state, created_at,
                                  window_start, payload_json, time_spent_seconds)
         VALUES ('ENG-3', ?, 'jira', 'drafted', ?, ?, '{}', 900)",
    )
    .bind(DAY)
    .bind(&t)
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();

    // The plan ritual: confirmed, two items from different origins.
    sqlx::query("INSERT INTO daily_plan_meta (plan_date, confirmed_at, skipped) VALUES (?, ?, 0)")
        .bind(DAY)
        .bind(&t)
        .execute(&pool)
        .await
        .unwrap();
    for (key, origin) in [("ENG-1", "carryover"), ("ENG-3", "manual")] {
        sqlx::query("INSERT INTO daily_plan (plan_date, task_key, origin) VALUES (?, ?, ?)")
            .bind(DAY)
            .bind(key)
            .bind(origin)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Day tasks, one of which the user dismissed, plus a generated summary.
    for id in ["T1", "T2", "T3"] {
        sqlx::query("INSERT INTO day_tasks (day_local, task_id, title) VALUES (?, ?, 'x')")
            .bind(DAY)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO day_task_corrections (day_local, task_id, action, created_at)
         VALUES (?, 'T2', 'dismissed', ?)",
    )
    .bind(DAY)
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO day_summaries (day_local, summary_md) VALUES (?, 'done')")
        .bind(DAY)
        .execute(&pool)
        .await
        .unwrap();

    // A personal task edited today, and one coding-agent segment summarised.
    sqlx::query(
        "INSERT INTO pm_tasks (task_key, title, provider, updated_at) VALUES ('L-1', 'x', 'local', ?)",
    )
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO app_sessions (id, app_name, started_at, task_method)
         VALUES (1, 'Claude Code', ?, 'summarised')",
    )
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();

    let r = usage_rollup(&pool, DAY).await.expect("rollup");

    assert_eq!(r.tickets_updated, 2, "distinct tickets posted to");
    assert_eq!(r.worklogs_posted, 3, "individual successful posts");
    assert_eq!(r.posts_by_provider.get("jira"), Some(&2));
    assert_eq!(r.posts_by_provider.get("linear"), Some(&1));
    assert_eq!(r.worklogs_with_post_error, 1, "the HTTP 403 row");

    assert_eq!(r.worklogs_drafted, 1);
    assert_eq!(r.worklogs_approved, 1);

    assert!(r.plan_confirmed);
    assert!(!r.plan_skipped);
    assert_eq!(r.plan_items, 2);
    assert_eq!(r.plan_items_by_origin.get("carryover"), Some(&1));

    assert_eq!(r.day_tasks, 3);
    assert_eq!(r.day_task_corrections.get("dismissed"), Some(&1));
    assert!(r.day_summary_generated);

    assert_eq!(r.personal_tasks_touched, 1);
    assert_eq!(r.agent_sessions_summarised, 1);
}

#[tokio::test]
async fn posts_are_scoped_by_posted_at_not_the_work_day() {
    // A worklog covering YESTERDAY's work, posted TODAY. It is a today action
    // and must land in today's rollup — the whole reason the ticket counters
    // key on `posted_at` rather than `day_utc`.
    let pool = make_pool().await;
    sqlx::query(
        "INSERT INTO pm_worklogs (task_key, day_utc, provider, state, posted_at, created_at,
                                  window_start, payload_json, time_spent_seconds)
         VALUES ('ENG-7', ?, 'jira', 'posted', ?, ?, ?, '{}', 600)",
    )
    .bind(PREV_DAY)
    .bind(inside(DAY))
    .bind(inside(PREV_DAY))
    .bind(inside(PREV_DAY))
    .execute(&pool)
    .await
    .unwrap();

    let today = usage_rollup(&pool, DAY).await.expect("rollup today");
    assert_eq!(
        today.tickets_updated, 1,
        "a post that happened today counts today, whatever work day it covers"
    );

    let yesterday = usage_rollup(&pool, PREV_DAY).await.expect("rollup prev");
    assert_eq!(
        yesterday.tickets_updated, 0,
        "and must NOT also be counted on the day the work happened"
    );
}

#[tokio::test]
async fn banner_only_notifications_are_never_counted_as_native_failures() {
    // A banner-only row has `delivered_native_at` NULL forever by design. If
    // the failure counter ignored `channels`, every such row with a non-zero
    // attempt count would invent a permanent delivery failure that never was —
    // and notification health would read as broken on a perfectly working install.
    let pool = make_pool().await;
    let t = inside(DAY);

    sqlx::query(
        "INSERT INTO notifications (dedup_key, event_key, channels, attempts, created_at)
         VALUES ('a', 'plan.nudge', 'banner', 3, ?)",
    )
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();
    // A genuine native failure: tried, never landed.
    sqlx::query(
        "INSERT INTO notifications (dedup_key, event_key, channels, attempts, created_at)
         VALUES ('b', 'plan.nudge', 'native', 2, ?)",
    )
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();
    // A native success.
    sqlx::query(
        "INSERT INTO notifications (dedup_key, event_key, channels, attempts,
                                    delivered_native_at, created_at)
         VALUES ('c', 'plan.nudge', 'native,banner', 1, ?, ?)",
    )
    .bind(&t)
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();
    // A never-due row: zero attempts is not a failure.
    sqlx::query(
        "INSERT INTO notifications (dedup_key, event_key, channels, attempts, created_at)
         VALUES ('d', 'system.fault', 'native', 0, ?)",
    )
    .bind(&t)
    .execute(&pool)
    .await
    .unwrap();

    let r = usage_rollup(&pool, DAY).await.expect("rollup");

    let nudge = r.notifications.get("plan.nudge").expect("plan.nudge group");
    assert_eq!(nudge.enqueued, 3);
    assert_eq!(nudge.delivered, 1);
    assert_eq!(
        nudge.failed, 1,
        "only the native row that was attempted and never landed"
    );

    let fault = r.notifications.get("system.fault").expect("fault group");
    assert_eq!(
        fault.failed, 0,
        "attempts = 0 is 'not due yet', not a failure"
    );

    assert_eq!(r.notifications_enqueued(), 4);
    assert_eq!(r.notifications_delivered(), 1);
    assert_eq!(r.notifications_failed(), 1);
}

#[tokio::test]
async fn a_post_at_the_last_instant_of_the_day_still_counts() {
    // The realistic late-evening case, and the only test exercising the `<=`
    // side of the day window — see `last_instant`.
    let pool = make_pool().await;
    sqlx::query(
        "INSERT INTO pm_worklogs (task_key, day_utc, provider, state, posted_at, created_at,
                                  window_start, payload_json, time_spent_seconds)
         VALUES ('ENG-9', ?, 'jira', 'posted', ?, ?, ?, '{}', 600)",
    )
    .bind(DAY)
    .bind(last_instant(DAY))
    .bind(inside(DAY))
    .bind(inside(DAY))
    .execute(&pool)
    .await
    .unwrap();

    let r = usage_rollup(&pool, DAY).await.expect("rollup");
    assert_eq!(
        r.tickets_updated, 1,
        "a post at 23:59:59.999 local is still that day's action"
    );
}

#[tokio::test]
async fn plan_state_separates_never_seen_from_seen_and_undecided() {
    // The two states a pair of booleans cannot tell apart. "Never adopted the
    // daily plan" and "opened it and walked away" are different product
    // problems, and both read false/false.
    let pool = make_pool().await;

    let unseen = usage_rollup(&pool, DAY).await.expect("rollup");
    assert_eq!(unseen.plan_state, PlanState::Unseen);
    assert!(!unseen.plan_confirmed && !unseen.plan_skipped);

    sqlx::query(
        "INSERT INTO daily_plan_meta (plan_date, confirmed_at, skipped) VALUES (?, NULL, 0)",
    )
    .bind(DAY)
    .execute(&pool)
    .await
    .unwrap();

    let open = usage_rollup(&pool, DAY).await.expect("rollup");
    assert_eq!(
        open.plan_state,
        PlanState::Open,
        "a row with no decision is 'open', not 'unseen'"
    );
    assert!(
        !open.plan_confirmed && !open.plan_skipped,
        "and the booleans genuinely cannot distinguish it - which is why PlanState exists"
    );
}

#[tokio::test]
async fn a_pre_migration_database_yields_zeros_instead_of_an_error() {
    // The tray can run new code against a database the daemon has not migrated
    // yet (the daemon owns migrations; they are separate processes). Before the
    // sqlite_master guard this returned Err, and because `daily_usage` treats
    // every Err as retryable the tray would retry the same day every ~60s
    // forever, warning each time, until the daemon happened to restart.
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");

    let r = usage_rollup(&pool, DAY)
        .await
        .expect("an unmigrated database must not fail the rollup");

    assert_eq!(r.tickets_updated, 0);
    assert_eq!(r.day_tasks, 0);
    assert_eq!(r.notifications_enqueued(), 0);
    assert_eq!(
        r.plan_state,
        PlanState::Unseen,
        "absent plan tables read as 'never seen', which is the truth"
    );
}
