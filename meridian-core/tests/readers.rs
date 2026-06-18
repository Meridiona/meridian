//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! In-memory integration tests for the ported DB readers. A single-connection
//! `:memory:` pool (so the schema persists across queries) is seeded with hand-
//! computable rows, then the reader's output is asserted. Complements the
//! `intervals`/`date` unit tests (the math) by checking the SQL + composition.

use chrono::{DateTime, Duration, SecondsFormat};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// Single-connection in-memory pool with just the columns the readers touch.
async fn make_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");

    sqlx::query(
        r#"
        CREATE TABLE app_sessions (
            app_name TEXT, started_at TEXT, ended_at TEXT, duration_s INTEGER,
            coding_agent_session_uuid TEXT, category TEXT, task_key TEXT,
            task_session_type TEXT, task_method TEXT
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE pm_tasks (
            task_key TEXT, title TEXT, description_text TEXT, issue_type TEXT,
            status_raw TEXT, is_terminal INTEGER, provider TEXT, url TEXT,
            parent_key TEXT, epic_title TEXT, due_date TEXT, start_date TEXT
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    pool
}

/// In-memory pool with the `today` reader's full schema: `app_sessions` in its
/// migrated shape (WITH `category_explanation`) and `active_session` in its real
/// shape (WITHOUT `category_explanation`, WITH `last_seen_at`).
async fn make_today_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    sqlx::query(
        r#"
        CREATE TABLE app_sessions (
            id INTEGER, app_name TEXT, started_at TEXT, ended_at TEXT, duration_s INTEGER,
            coding_agent_session_uuid TEXT, category TEXT, confidence REAL, category_method TEXT,
            category_explanation TEXT, session_summary TEXT, window_titles TEXT, task_key TEXT,
            task_routing TEXT, task_session_type TEXT, task_method TEXT, task_confidence REAL
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"CREATE TABLE active_session (
            id INTEGER, app_name TEXT, started_at TEXT, last_seen_at TEXT,
            window_titles TEXT, category TEXT, confidence REAL
        );"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn coding_agents_unions_overlap_per_agent_and_total() {
    let pool = make_pool().await;
    // Claude Code: 10:00–10:10 ∪ 10:05–10:15 → 10:00–10:15 = 900 s.
    // Codex: 11:00–11:05 = 300 s. No CC/Codex overlap → total = 1200 s.
    let rows = [
        (
            "Claude Code",
            "2026-06-16T10:00:00+00:00",
            "2026-06-16T10:10:00+00:00",
        ),
        (
            "Claude Code",
            "2026-06-16T10:05:00+00:00",
            "2026-06-16T10:15:00+00:00",
        ),
        (
            "Codex",
            "2026-06-16T11:00:00+00:00",
            "2026-06-16T11:05:00+00:00",
        ),
    ];
    for (app, s, e) in rows {
        sqlx::query(
            "INSERT INTO app_sessions (app_name, started_at, ended_at, coding_agent_session_uuid) VALUES (?,?,?,?)",
        )
        .bind(app).bind(s).bind(e).bind("uuid")
        .execute(&pool).await.unwrap();
    }

    let r = meridian_core::coding_agents::get_coding_agents(&pool, "2026-06-16")
        .await
        .unwrap();

    assert_eq!(r.total_s, 1200);
    assert_eq!(r.agents.len(), 2);
    // Sorted descending: Claude Code (900) before Codex (300).
    assert_eq!(r.agents[0].app, "Claude Code");
    assert_eq!(r.agents[0].total_s, 900);
    assert_eq!(r.agents[1].app, "Codex");
    assert_eq!(r.agents[1].total_s, 300);
}

#[tokio::test]
async fn tasks_autonomous_excludes_supervised_agent_time() {
    let pool = make_pool().await;
    let today = meridian_core::date::today_string();
    // Place rows relative to the computed local-day start so the test is
    // timezone-independent (the reader filters on local_day_bounds(today)).
    let (start, _end) = meridian_core::date::local_day_bounds(&today);
    let base = DateTime::parse_from_rfc3339(&start).unwrap();
    let at = |h: f64| {
        (base + Duration::milliseconds((h * 3_600_000.0) as i64))
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    };

    sqlx::query("INSERT INTO pm_tasks (task_key, title, issue_type) VALUES ('X','Task X','Task')")
        .execute(&pool)
        .await
        .unwrap();

    // Foreground task session: base+1h .. base+2h (presence = 1h, your_s = 3600).
    sqlx::query(
        "INSERT INTO app_sessions (app_name, started_at, ended_at, duration_s, coding_agent_session_uuid, category, task_key, task_session_type) \
         VALUES ('Code', ?, ?, 3600, NULL, 'coding', 'X', 'task')",
    )
    .bind(at(1.0)).bind(at(2.0))
    .execute(&pool).await.unwrap();

    // Agent task session: starts base+1.5h, capped to duration 3600 → base+2.5h.
    // Overlaps presence base+1.5h..base+2h (1800 s supervised); the other 1800 s
    // (base+2h..base+2.5h) ran while away → autonomous.
    sqlx::query(
        "INSERT INTO app_sessions (app_name, started_at, ended_at, duration_s, coding_agent_session_uuid, task_key, task_session_type) \
         VALUES ('Claude Code', ?, ?, 3600, 'uuid1', 'X', 'task')",
    )
    .bind(at(1.5)).bind(at(3.0))
    .execute(&pool).await.unwrap();

    let now = chrono::Utc::now().to_rfc3339();
    let r = meridian_core::tasks::get_tasks(&pool, &today, &today, &now)
        .await
        .unwrap();

    let x = r
        .tasks
        .iter()
        .find(|t| t.key == "X")
        .expect("task X present");
    assert_eq!(x.today_autonomous_s, 1800, "agent time outside presence");
    assert_eq!(x.today_s, 5400, "your 3600 + autonomous 1800");
    assert_eq!(x.session_count, 1, "foreground sessions only");
    assert_eq!(x.cats.get("coding").copied(), Some(3600));
}

/// Regression for the active-session column-guard bug: `app_sessions` HAS
/// `category_explanation` (post-migration) but `active_session` does NOT. The
/// old code guarded the active query on `app_sessions`' columns, injecting the
/// non-existent column → the read failed and the live block silently vanished.
/// With the fix (always `NULL`), the active session is returned.
#[tokio::test]
async fn today_returns_active_session_when_app_sessions_has_explanation_column() {
    let pool = make_today_pool().await;

    let today = meridian_core::date::today_string();
    let (start, _end) = meridian_core::date::local_day_bounds(&today);
    let base = DateTime::parse_from_rfc3339(&start).unwrap();
    let at = |h: f64| {
        (base + Duration::milliseconds((h * 3_600_000.0) as i64))
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    };

    // Live block: last_seen_at == now, so presence runs started_at → now (1 h).
    sqlx::query(
        "INSERT INTO active_session (id, app_name, started_at, last_seen_at, window_titles, category, confidence) \
         VALUES (1, 'Code', ?, ?, '[]', 'coding', 0.9)",
    )
    .bind(at(1.0))
    .bind(at(2.0))
    .execute(&pool)
    .await
    .unwrap();

    let now = at(2.0); // one hour after the active session started
    let r = meridian_core::today::get_today(&pool, &today, &now)
        .await
        .unwrap();

    let active = r
        .active
        .as_ref()
        .expect("active session must survive the column guard");
    assert_eq!(active.app, "Code");
    assert_eq!(active.elapsed_s, 3600);
    assert!(
        active.explain.is_none(),
        "active session never carries an explanation"
    );
    // A healthy live block contributes its full extent (started → now) to focus.
    assert_eq!(r.focus_s, 3600, "live active block counts as 1 h of focus");
}

/// Regression for the "50h focused in one day" bug: a stale `active_session`
/// left open by a stopped daemon (its `last_seen_at` days old, never advanced)
/// must NOT inflate today's focus. Presence is capped at `last_seen_at` and
/// clamped to the current day, so a block that last advanced on a prior day
/// contributes 0 to `focus_s` — even though the card still renders.
#[tokio::test]
async fn today_focus_excludes_stale_active_block() {
    let pool = make_today_pool().await;

    let today = meridian_core::date::today_string();
    let (start, _end) = meridian_core::date::local_day_bounds(&today);
    let base = DateTime::parse_from_rfc3339(&start).unwrap();
    let at = |h: f64| {
        (base + Duration::milliseconds((h * 3_600_000.0) as i64))
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    };

    // Block started 50 h before today began and never advanced (last_seen ==
    // started) — the stopped-daemon shape that produced "50h 6m focused".
    let stale = (base - Duration::hours(50)).to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query(
        "INSERT INTO active_session (id, app_name, started_at, last_seen_at, window_titles, category, confidence) \
         VALUES (1, 'Code', ?, ?, '[]', 'coding', 0.9)",
    )
    .bind(&stale)
    .bind(&stale)
    .execute(&pool)
    .await
    .unwrap();

    let now = at(2.0); // 2 h into today, ~52 h after the block opened
    let r = meridian_core::today::get_today(&pool, &today, &now)
        .await
        .unwrap();

    assert_eq!(
        r.focus_s, 0,
        "a stale block from a prior day must not count toward today's focus"
    );
    // The card itself still renders (clamping focus is a separate concern from
    // whether to surface a stale active session).
    assert!(r.active.is_some(), "active card still present");
}
