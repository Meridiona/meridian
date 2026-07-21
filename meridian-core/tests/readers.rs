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
    // Codex: 11:00–11:05 = 300 s.
    // GitHub Copilot: 12:00–12:04 = 240 s.
    // Cursor Agent: 13:00–13:02 = 120 s.
    // No overlap across agents → total = 1560 s.
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
        (
            "GitHub Copilot",
            "2026-06-16T12:00:00+00:00",
            "2026-06-16T12:04:00+00:00",
        ),
        (
            "Cursor Agent",
            "2026-06-16T13:00:00+00:00",
            "2026-06-16T13:02:00+00:00",
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

    assert_eq!(r.total_s, 1560);
    assert_eq!(r.agents.len(), 4);
    // Sorted descending: Claude Code (900), Codex (300), GitHub Copilot (240), Cursor Agent (120).
    assert_eq!(r.agents[0].app, "Claude Code");
    assert_eq!(r.agents[0].total_s, 900);
    assert_eq!(r.agents[1].app, "Codex");
    assert_eq!(r.agents[1].total_s, 300);
    assert_eq!(r.agents[2].app, "GitHub Copilot");
    assert_eq!(r.agents[2].total_s, 240);
    assert_eq!(r.agents[3].app, "Cursor Agent");
    assert_eq!(r.agents[3].total_s, 120);
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

/// The CDM columns (migration 056) are surfaced by the reader when present,
/// including JSON-array hierarchy fields parsed into vecs.
#[tokio::test]
async fn tasks_exposes_cdm_columns_when_present() {
    let pool = make_pool().await;
    // Mirror migration 056: add the CDM columns to pm_tasks.
    for col in [
        "canonical_id",
        "status_category",
        "raw_payload",
        "reporter_name",
        "completed_at",
        "ancestor_path",
        "project_ids",
    ] {
        sqlx::query(&format!("ALTER TABLE pm_tasks ADD COLUMN {col} TEXT"))
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO pm_tasks (task_key, title, issue_type, status_raw, is_terminal, \
            canonical_id, status_category, reporter_name, completed_at, ancestor_path, project_ids) \
         VALUES ('JIRA-1','Task','Task','Done',1, \
            'jira:10001','done','Lead','2026-06-30T10:00:00Z', \
            '[\"jira:10000\"]','[\"jira:proj\"]')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let today = meridian_core::date::today_string();
    let now = chrono::Utc::now().to_rfc3339();
    let r = meridian_core::tasks::get_tasks(&pool, &today, &today, &now)
        .await
        .unwrap();
    let t = r
        .tasks
        .iter()
        .find(|t| t.key == "JIRA-1")
        .expect("task present");

    assert_eq!(t.canonical_id.as_deref(), Some("jira:10001"));
    assert_eq!(t.status_category.as_deref(), Some("done"));
    assert_eq!(t.reporter.as_deref(), Some("Lead"));
    assert_eq!(t.completed_at.as_deref(), Some("2026-06-30T10:00:00Z"));
    assert_eq!(t.ancestor_path, vec!["jira:10000"]);
    assert_eq!(t.project_ids, vec!["jira:proj"]);
}

/// On a DB that predates migration 056 (no CDM columns), the reader degrades
/// gracefully: the new fields come back None/empty rather than erroring.
#[tokio::test]
async fn tasks_cdm_columns_default_when_absent() {
    let pool = make_pool().await; // pm_tasks has no CDM columns
    sqlx::query("INSERT INTO pm_tasks (task_key, title, issue_type) VALUES ('OLD-1','Old','Task')")
        .execute(&pool)
        .await
        .unwrap();

    let today = meridian_core::date::today_string();
    let now = chrono::Utc::now().to_rfc3339();
    let r = meridian_core::tasks::get_tasks(&pool, &today, &today, &now)
        .await
        .unwrap();
    let t = r
        .tasks
        .iter()
        .find(|t| t.key == "OLD-1")
        .expect("task present");

    assert_eq!(t.canonical_id, None);
    assert_eq!(t.status_category, None);
    assert_eq!(t.reporter, None);
    assert_eq!(t.completed_at, None);
    assert!(t.ancestor_path.is_empty());
    assert!(t.project_ids.is_empty());
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

// ── plan reader + writes ──────────────────────────────────────────────────────

/// In-memory pool with the plan reader's full schema: `pm_tasks` (scoring
/// columns), `pm_task_curation`, `app_sessions`, and the `daily_plan` /
/// `daily_plan_meta` tables (incl. the 044 `task_snapshot` column).
async fn make_plan_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    for ddl in [
        r#"CREATE TABLE pm_tasks (
            task_key TEXT, title TEXT, provider TEXT, url TEXT, status_raw TEXT,
            is_terminal INTEGER, due_date TEXT, updated_at TEXT, description_text TEXT,
            epic_title TEXT, parent_key TEXT, priority TEXT, issue_type TEXT, story_points TEXT
        );"#,
        r#"CREATE TABLE pm_task_curation (task_key TEXT, decision TEXT);"#,
        r#"CREATE TABLE app_sessions (
            task_key TEXT, started_at TEXT, task_session_type TEXT
        );"#,
        r#"CREATE TABLE daily_plan (
            plan_date TEXT NOT NULL, task_key TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
            origin TEXT NOT NULL DEFAULT 'manual', task_snapshot TEXT,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            PRIMARY KEY (plan_date, task_key)
        );"#,
        r#"CREATE TABLE daily_plan_meta (
            plan_date TEXT NOT NULL PRIMARY KEY, confirmed_at TEXT,
            skipped INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );"#,
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    pool
}

/// Insert one board ticket (only the fields the scoring touches).
async fn seed_task(pool: &SqlitePool, key: &str, status: &str, due: Option<&str>, terminal: i64) {
    sqlx::query(
        "INSERT INTO pm_tasks (task_key, title, provider, url, status_raw, is_terminal, due_date, updated_at) \
         VALUES (?, ?, 'jira', '', ?, ?, ?, NULL)",
    )
    .bind(key)
    .bind(format!("Title {key}"))
    .bind(status)
    .bind(terminal)
    .bind(due)
    .execute(pool)
    .await
    .unwrap();
}

/// The plan clock the command resolves (now-ms + recent-work lookback bound).
fn plan_clock() -> (i64, String) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let recent_since = (chrono::Local::now()
        - Duration::days(meridian_core::plan::RECENT_WORK_DAYS))
    .format("%Y-%m-%d")
    .to_string();
    (now_ms, recent_since)
}

#[tokio::test]
async fn build_available_scores_sorts_and_excludes() {
    let pool = make_plan_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let due_today = date.clone();
    let due_far = (today + Duration::days(60)).format("%Y-%m-%d").to_string();

    // A: due today (350, "due_soon"); B: in-progress (300); E: far-future (0, manual)
    seed_task(&pool, "PROJ-A", "Backlog", Some(&due_today), 0).await;
    seed_task(&pool, "PROJ-B", "In Progress", None, 0).await;
    seed_task(&pool, "PROJ-E", "Backlog", Some(&due_far), 0).await;
    // C: terminal → dropped. D: curation-excluded → dropped.
    seed_task(&pool, "PROJ-C", "Done", None, 1).await;
    seed_task(&pool, "PROJ-D", "In Progress", None, 0).await;
    sqlx::query("INSERT INTO pm_task_curation (task_key, decision) VALUES ('PROJ-D','excluded')")
        .execute(&pool)
        .await
        .unwrap();

    let (now_ms, recent_since) = plan_clock();
    let avail = meridian_core::plan::build_available(&pool, &date, today, now_ms, &recent_since)
        .await
        .unwrap();

    // C (terminal) and D (excluded) are gone; A,B,E survive, sorted by score.
    let keys: Vec<&str> = avail.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, vec!["PROJ-A", "PROJ-B", "PROJ-E"]);
    assert_eq!(avail[0].score, 350);
    assert_eq!(avail[0].origin, "due_soon");
    assert_eq!(avail[0].reason, "Due today");
    assert_eq!(avail[1].score, 300);
    assert_eq!(avail[1].origin, "in_progress");
    assert_eq!(avail[2].score, 0);
    assert_eq!(avail[2].origin, "manual");

    // Suggestions drop the score-0 ticket.
    let resp = meridian_core::plan::build_plan_response(&pool, &date, today, avail)
        .await
        .unwrap();
    assert!(resp.has_table);
    assert!(!resp.confirmed && !resp.skipped);
    assert!(resp.plan.is_empty());
    let sug: Vec<&str> = resp.suggestions.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(sug, vec!["PROJ-A", "PROJ-B"]);
}

#[tokio::test]
async fn plan_write_confirm_then_reopen_roundtrip() {
    let pool = make_plan_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    seed_task(&pool, "PROJ-A", "Backlog", Some(&date), 0).await;
    seed_task(&pool, "PROJ-B", "In Progress", None, 0).await;

    let (now_ms, recent_since) = plan_clock();
    let now = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let avail = || {
        let pool = &pool;
        let recent_since = recent_since.clone();
        let date = date.clone();
        async move {
            meridian_core::plan::build_available(pool, &date, today, now_ms, &recent_since)
                .await
                .unwrap()
        }
    };

    // confirm [B, A] → committed in that order, meta confirmed.
    let body = meridian_core::plan::PlanBody {
        action: "confirm".to_string(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: Some(vec!["PROJ-B".to_string(), "PROJ-A".to_string()]),
    };
    let resp =
        meridian_core::plan::apply_plan_action(&pool, &body, &date, today, &now, avail().await)
            .await
            .unwrap();
    assert!(resp.confirmed && !resp.skipped);
    let plan_keys: Vec<&str> = resp.plan.iter().map(|p| p.task_key.as_str()).collect();
    assert_eq!(plan_keys, vec!["PROJ-B", "PROJ-A"]); // position order preserved
    assert_eq!(resp.plan[0].position, 0);
    assert_eq!(resp.plan[1].position, 1);
    // A committed ticket keeps its scored origin label (not bare "manual").
    assert_eq!(resp.plan[0].origin, "in_progress");
    // Confirmed tasks aren't re-suggested.
    assert!(resp.suggestions.is_empty());

    // reopen → confirmed cleared, committed rows kept.
    let reopen = meridian_core::plan::PlanBody {
        action: "reopen".to_string(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: None,
    };
    let resp =
        meridian_core::plan::apply_plan_action(&pool, &reopen, &date, today, &now, avail().await)
            .await
            .unwrap();
    assert!(!resp.confirmed && !resp.skipped);
    assert_eq!(resp.plan.len(), 2, "reopen keeps the committed rows");

    // remove one → drops just that key.
    let remove = meridian_core::plan::PlanBody {
        action: "remove".to_string(),
        date: Some(date.clone()),
        task_key: Some("PROJ-A".to_string()),
        task_keys: None,
    };
    let resp =
        meridian_core::plan::apply_plan_action(&pool, &remove, &date, today, &now, avail().await)
            .await
            .unwrap();
    let plan_keys: Vec<&str> = resp.plan.iter().map(|p| p.task_key.as_str()).collect();
    assert_eq!(plan_keys, vec!["PROJ-B"]);
}

#[tokio::test]
async fn plan_write_rejects_missing_keys_and_unready_storage() {
    let pool = make_plan_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let now = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    // confirm without task_keys → error (must not wipe the day).
    let bad = meridian_core::plan::PlanBody {
        action: "confirm".to_string(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: None,
    };
    let err = meridian_core::plan::apply_plan_action(&pool, &bad, &date, today, &now, vec![])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("task_keys array required"));

    // unknown action → error.
    let unknown = meridian_core::plan::PlanBody {
        action: "frobnicate".to_string(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: None,
    };
    let err = meridian_core::plan::apply_plan_action(&pool, &unknown, &date, today, &now, vec![])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown action"));
}

// ---------------------------------------------------------------------------
// current_task reader
// ---------------------------------------------------------------------------

/// Pool with the schema slices `current_task::get_current_task` needs.
async fn make_current_task_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    sqlx::query(
        r#"
        CREATE TABLE app_sessions (
            id INTEGER PRIMARY KEY, app_name TEXT, started_at TEXT, ended_at TEXT,
            duration_s INTEGER, task_key TEXT, task_session_type TEXT
        );
        CREATE TABLE pm_tasks (
            task_key TEXT PRIMARY KEY,
            title TEXT,
            status_category TEXT,
            priority TEXT,
            story_points TEXT NOT NULL DEFAULT ''
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn current_task_returns_most_recent_classified_task_today() {
    let pool = make_current_task_pool().await;
    let today = meridian_core::date::today_string();
    let (day_start, _) = meridian_core::date::local_day_bounds(&today);

    // Two task sessions today — the later one should win.
    let base: DateTime<chrono::Utc> = DateTime::parse_from_rfc3339(&day_start).unwrap().into();
    let earlier = base.to_rfc3339_opts(SecondsFormat::Millis, true);
    let later = (base + Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query("INSERT INTO app_sessions VALUES (1,'VS Code',?1,?1,1800,'MER-001','task')")
        .bind(&earlier)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_sessions VALUES (2,'VS Code',?1,?1,3600,'MER-002','task')")
        .bind(&later)
        .execute(&pool)
        .await
        .unwrap();
    // MER-002 has 3 story points → 3h budget; 3600s spent today → 33%.
    sqlx::query("INSERT INTO pm_tasks (task_key, story_points) VALUES ('MER-002','3')")
        .execute(&pool)
        .await
        .unwrap();

    let ct = meridian_core::current_task::get_current_task(&pool, &today)
        .await
        .unwrap()
        .expect("should resolve a current task");
    assert_eq!(ct.key, "MER-002");
    let pct = ct.percent.expect("story_points='3' and spent=3600 → ~33%");
    assert!((pct - 1.0 / 3.0).abs() < 0.01, "got {pct}");
}

#[tokio::test]
async fn current_task_none_without_task_sessions_today() {
    let pool = make_current_task_pool().await;
    let today = meridian_core::date::today_string();
    // Session exists but task_session_type is NULL → not a task session.
    let (day_start, _) = meridian_core::date::local_day_bounds(&today);
    let base: DateTime<chrono::Utc> = DateTime::parse_from_rfc3339(&day_start).unwrap().into();
    let ts = base.to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query("INSERT INTO app_sessions VALUES (1,'Slack',?1,?1,600,NULL,NULL)")
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    let ct = meridian_core::current_task::get_current_task(&pool, &today)
        .await
        .unwrap();
    assert!(ct.is_none(), "no task sessions today → should return None");
}

#[tokio::test]
async fn current_task_percent_none_without_story_points() {
    let pool = make_current_task_pool().await;
    let today = meridian_core::date::today_string();
    let (day_start, _) = meridian_core::date::local_day_bounds(&today);
    let base: DateTime<chrono::Utc> = DateTime::parse_from_rfc3339(&day_start).unwrap().into();
    let ts = base.to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query("INSERT INTO app_sessions VALUES (1,'VS Code',?1,?1,3600,'MER-003','task')")
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    // No pm_tasks row → story_points lookup returns None → percent is None.
    let ct = meridian_core::current_task::get_current_task(&pool, &today)
        .await
        .unwrap()
        .expect("should resolve a current task");
    assert_eq!(ct.key, "MER-003");
    assert!(
        ct.percent.is_none(),
        "no pm_tasks row → percent should be None"
    );
}

// ── plan_handled (tray auto-open gate) ────────────────────────────────────────

#[tokio::test]
async fn plan_handled_reads_meta_states() {
    let pool = make_plan_pool().await;
    let today = "2026-07-13";

    // No row for the date → not handled.
    assert!(!meridian_core::plan::plan_handled(&pool, today).await);

    // Row present but neither confirmed nor skipped → still not handled.
    sqlx::query(
        "INSERT INTO daily_plan_meta (plan_date, confirmed_at, skipped, created_at, updated_at) \
         VALUES (?, NULL, 0, 't', 't')",
    )
    .bind(today)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!meridian_core::plan::plan_handled(&pool, today).await);

    // Confirmed → handled.
    sqlx::query("UPDATE daily_plan_meta SET confirmed_at = 't' WHERE plan_date = ?")
        .bind(today)
        .execute(&pool)
        .await
        .unwrap();
    assert!(meridian_core::plan::plan_handled(&pool, today).await);

    // Skipped (not confirmed) → handled.
    sqlx::query("UPDATE daily_plan_meta SET confirmed_at = NULL, skipped = 1 WHERE plan_date = ?")
        .bind(today)
        .execute(&pool)
        .await
        .unwrap();
    assert!(meridian_core::plan::plan_handled(&pool, today).await);
}

#[tokio::test]
async fn plan_handled_false_without_table() {
    // A bare DB (no daily_plan_meta) must read as "not handled", not error.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    assert!(!meridian_core::plan::plan_handled(&pool, "2026-07-13").await);
}

// ── task_create: user-authored tasks (the `'local'` sentinel provider) ────────
//
// The invariant under test throughout: a personal task is a normal `pm_tasks` row
// whose `provider` no sync recognises, so it survives every prune AND is picked up
// by every unscoped reader (`build_available`, `load_plan`) for free.

/// A fuller `pm_tasks` than `make_plan_pool`'s: `task_key` UNIQUE (what
/// `insert_task`'s ON CONFLICT and the key mint rely on) + `project_key`.
async fn make_task_create_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    for ddl in [
        r#"CREATE TABLE pm_tasks (
            task_key TEXT NOT NULL UNIQUE, title TEXT NOT NULL, provider TEXT NOT NULL,
            url TEXT NOT NULL DEFAULT '', status_raw TEXT NOT NULL DEFAULT '',
            is_terminal INTEGER NOT NULL DEFAULT 0, due_date TEXT, updated_at TEXT NOT NULL,
            description_text TEXT NOT NULL DEFAULT '', epic_title TEXT, parent_key TEXT,
            priority TEXT NOT NULL DEFAULT '', issue_type TEXT NOT NULL DEFAULT '',
            story_points TEXT NOT NULL DEFAULT '', project_key TEXT NOT NULL DEFAULT ''
        );"#,
        r#"CREATE TABLE pm_task_curation (task_key TEXT, decision TEXT);"#,
        r#"CREATE TABLE app_sessions (
            task_key TEXT, started_at TEXT, task_session_type TEXT
        );"#,
        r#"CREATE TABLE daily_plan (
            plan_date TEXT NOT NULL, task_key TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
            origin TEXT NOT NULL DEFAULT 'manual', task_snapshot TEXT,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            PRIMARY KEY (plan_date, task_key)
        );"#,
        r#"CREATE TABLE daily_plan_meta (
            plan_date TEXT NOT NULL PRIMARY KEY, confirmed_at TEXT,
            skipped INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );"#,
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn local_task_mints_sequential_keys() {
    let pool = make_task_create_pool().await;
    for expected in ["LOCAL-1", "LOCAL-2", "LOCAL-3"] {
        let key = meridian_core::task_create::create_local_task(&pool, "T", "d", "Task", "t0")
            .await
            .unwrap();
        assert_eq!(key, expected);
    }
}

#[tokio::test]
async fn done_is_how_a_personal_task_leaves_the_board() {
    // There is no delete by design: marking done is the disposal path, exactly as it
    // is for a board ticket. It must drop the task out of build_available WITHOUT
    // removing the row (whose key must never be reissued).
    let pool = make_task_create_pool().await;
    let key = meridian_core::task_create::create_local_task(&pool, "T", "d", "Task", "t0")
        .await
        .unwrap();
    meridian_core::task_create::set_local_terminal(&pool, &key, true, "t1")
        .await
        .unwrap();

    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let recent_since = (chrono::Local::now()
        - Duration::days(meridian_core::plan::RECENT_WORK_DAYS))
    .format("%Y-%m-%d")
    .to_string();
    let available = meridian_core::plan::build_available(
        &pool,
        &date,
        today,
        chrono::Utc::now().timestamp_millis(),
        &recent_since,
    )
    .await
    .unwrap();
    assert!(
        !available.iter().any(|a| a.key == key),
        "a done personal task must leave the board"
    );
    assert!(
        meridian_core::task_create::task_exists(&pool, &key)
            .await
            .unwrap(),
        "…but the row must remain, so its key is never reissued"
    );

    // The key keeps climbing past the done task — no reuse.
    let next = meridian_core::task_create::create_local_task(&pool, "T2", "d", "Task", "t2")
        .await
        .unwrap();
    assert_eq!(next, "LOCAL-2");
}

#[tokio::test]
async fn local_task_appears_on_the_board_and_is_not_done() {
    // The payoff + the plan.rs:586 landmine, in one test. A plan row whose task has
    // NO pm_tasks row renders `is_terminal = true` and titled with its raw key; a
    // local task has a row, so `on_board` is true and neither applies.
    let pool = make_task_create_pool().await;
    let key =
        meridian_core::task_create::create_local_task(&pool, "Write the deck", "d", "Task", "t0")
            .await
            .unwrap();

    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let recent_since = (chrono::Local::now()
        - Duration::days(meridian_core::plan::RECENT_WORK_DAYS))
    .format("%Y-%m-%d")
    .to_string();

    let available =
        meridian_core::plan::build_available(&pool, &date, today, now_ms, &recent_since)
            .await
            .unwrap();
    assert!(
        available.iter().any(|a| a.key == key),
        "a personal task must appear in build_available - this is the whole payoff"
    );

    let body = meridian_core::plan::PlanBody {
        action: "confirm".into(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: Some(vec![key.clone()]),
    };
    let resp =
        meridian_core::plan::apply_plan_action(&pool, &body, &date, today, "t0", available.clone())
            .await
            .unwrap();
    let item = resp
        .plan
        .iter()
        .find(|p| p.task_key == key)
        .expect("in plan");
    assert_eq!(
        item.title, "Write the deck",
        "must not fall back to the raw key"
    );
    assert!(
        !item.is_terminal,
        "a fresh personal task must not render as done"
    );
    assert_eq!(item.provider, "local");
}

#[tokio::test]
async fn local_task_survives_a_provider_scoped_prune() {
    // THE load-bearing invariant. Every sync's prune is `WHERE provider = '<tracker>'`,
    // which is what makes it structurally unable to see a 'local' row. If a future
    // change ever makes a prune unscoped, this fails.
    let pool = make_task_create_pool().await;
    let local = meridian_core::task_create::create_local_task(&pool, "Mine", "d", "Task", "t0")
        .await
        .unwrap();
    meridian_core::task_create::insert_task(
        &pool,
        &meridian_core::task_create::NewTask {
            key: "KAN-1".into(),
            provider: "jira".into(),
            title: "Theirs".into(),
            description: String::new(),
            issue_type: "Task".into(),
            url: String::new(),
        },
        "t0",
    )
    .await
    .unwrap();

    // Verbatim shape of jira's prune (src/intelligence/providers/jira/mod.rs:335)
    // with an empty fetch set — the worst case, e.g. a totally failed sync.
    sqlx::query(
        "DELETE FROM pm_tasks WHERE provider = 'jira' AND task_key NOT IN ('') \
         AND task_key NOT IN (SELECT DISTINCT task_key FROM pm_tasks WHERE 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(
        meridian_core::task_create::task_exists(&pool, &local)
            .await
            .unwrap(),
        "a personal task MUST survive a tracker's prune"
    );
    assert!(
        !meridian_core::task_create::task_exists(&pool, "KAN-1")
            .await
            .unwrap(),
        "the jira row should have been pruned - proves the test discriminates"
    );
}

#[tokio::test]
async fn text_and_status_writes_refuse_a_board_row() {
    // A tracker's copy is authoritative; writing pm_tasks directly for a synced row
    // would be silently clobbered by the next sync's UPSERT.
    let pool = make_task_create_pool().await;
    meridian_core::task_create::insert_task(
        &pool,
        &meridian_core::task_create::NewTask {
            key: "KAN-1".into(),
            provider: "jira".into(),
            title: "Theirs".into(),
            description: String::new(),
            issue_type: "Task".into(),
            url: String::new(),
        },
        "t0",
    )
    .await
    .unwrap();

    assert!(
        !meridian_core::task_create::update_task_text(&pool, "KAN-1", Some("Hacked"), None, "t1")
            .await
            .unwrap(),
        "update_task_text must refuse a board ticket"
    );
    assert!(
        !meridian_core::task_create::set_local_terminal(&pool, "KAN-1", true, "t1")
            .await
            .unwrap(),
        "set_local_terminal must refuse a board ticket"
    );
    let (title,): (String,) = sqlx::query_as("SELECT title FROM pm_tasks WHERE task_key = 'KAN-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Theirs", "the board ticket must be untouched");
}

#[tokio::test]
async fn local_task_edits_round_trip() {
    let pool = make_task_create_pool().await;
    let key = meridian_core::task_create::create_local_task(&pool, "Old", "before", "Task", "t0")
        .await
        .unwrap();

    // COALESCE semantics: a None field is left alone, not blanked.
    assert!(
        meridian_core::task_create::update_task_text(&pool, &key, Some("New"), None, "t1")
            .await
            .unwrap()
    );
    let (title, desc): (String, String) =
        sqlx::query_as("SELECT title, description_text FROM pm_tasks WHERE task_key = ?")
            .bind(&key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "New");
    assert_eq!(desc, "before", "an omitted field must not be blanked");

    assert!(
        meridian_core::task_create::set_local_terminal(&pool, &key, true, "t2")
            .await
            .unwrap()
    );
    let (term,): (i64,) = sqlx::query_as("SELECT is_terminal FROM pm_tasks WHERE task_key = ?")
        .bind(&key)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(term, 1);
    assert_eq!(
        meridian_core::task_create::provider_of(&pool, &key)
            .await
            .unwrap()
            .as_deref(),
        Some("local")
    );
}

#[tokio::test]
async fn insert_task_is_idempotent_on_a_double_submit() {
    let pool = make_task_create_pool().await;
    let t = meridian_core::task_create::NewTask {
        key: "KAN-9".into(),
        provider: "jira".into(),
        title: "First".into(),
        description: String::new(),
        issue_type: "Task".into(),
        url: String::new(),
    };
    meridian_core::task_create::insert_task(&pool, &t, "t0")
        .await
        .unwrap();
    meridian_core::task_create::insert_task(&pool, &t, "t0")
        .await
        .expect("a re-insert must be a no-op, not an error");
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pm_tasks WHERE task_key = 'KAN-9'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

// ── The board is ONE definition ───────────────────────────────────────────────

/// The board is ONE query, and it answers one question: what is on your board.
///
/// It used to be two queries in two languages with two answers — the Tasks page
/// filtered `!is_terminal`, the worklog matcher additionally dropped
/// cleanup-excluded tickets — so a ticket could sit on your board and be
/// invisible to the model with nothing anywhere saying so. If this test fails,
/// that drift is back.
///
/// NOTE: this test USED to assert that the matcher's candidate set was this same
/// set. It deliberately no longer does. The matcher now compares a day's work
/// against that day's PLANNED tasks (`plan::load_plan_candidates`), which is a
/// different and better question — see
/// `meridian::pm_worklog::generate::fetch_plan_candidates` and its tests. What
/// this query feeds now is the Tasks page and the manual ticket picker.
#[tokio::test]
async fn the_board_is_one_definition() {
    let pool = make_board_pool().await;

    // Four tickets, one of each kind the predicate must judge.
    for (key, provider, terminal) in [
        ("KAN-1", "jira", 0),    // plain open ticket        → on board
        ("KAN-2", "jira", 1),    // Done / stamped off-board → hidden
        ("KAN-3", "jira", 0),    // open but cleanup-excluded → hidden
        ("LOCAL-1", "local", 0), // personal task         → on board, NOT postable
    ] {
        sqlx::query(
            "INSERT INTO pm_tasks (task_key, title, provider, status_raw, is_terminal, updated_at) \
             VALUES (?, 'T', ?, 'To Do', ?, '2026-07-16T00:00:00Z')",
        )
        .bind(key)
        .bind(provider)
        .bind(terminal)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO pm_task_curation (task_key, decision) VALUES ('KAN-3','excluded')")
        .execute(&pool)
        .await
        .unwrap();

    // The whole board — what the Tasks page shows.
    let all = meridian_core::board::fetch_open_board(&pool, false)
        .await
        .unwrap();
    let mut keys: Vec<&str> = all.iter().map(|t| t.task_key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["KAN-1", "LOCAL-1"],
        "the board is open + not-cleanup-excluded"
    );

    // The picker's set — identical, minus only what cannot be posted to.
    let postable = meridian_core::board::fetch_open_board(&pool, true)
        .await
        .unwrap();
    let keys: Vec<&str> = postable.iter().map(|t| t.task_key.as_str()).collect();
    assert_eq!(
        keys,
        ["KAN-1"],
        "you can't retarget a worklog at a personal task - nowhere to post a comment"
    );
}

// ── The plan is the matcher's candidate set ───────────────────────────────────

/// Seed one `daily_plan` row for `date`.
async fn plan_row(pool: &sqlx::SqlitePool, date: &str, key: &str, pos: i64) {
    sqlx::query(
        "INSERT INTO daily_plan (plan_date, task_key, position, origin, created_at, updated_at) \
         VALUES (?, ?, ?, 'manual', '2026-07-16T00:00:00Z', '2026-07-16T00:00:00Z')",
    )
    .bind(date)
    .bind(key)
    .bind(pos)
    .execute(pool)
    .await
    .unwrap();
}

/// A planned ticket that has LEFT the board still resolves — from the snapshot
/// captured onto its plan row at write time.
///
/// This is what makes "check a task off and still log work against it" work.
/// Ticking a plan task closes its real ticket, which drops it out of `pm_tasks`
/// entirely on the next sync; without the snapshot the matcher would see a
/// title-less husk (or nothing), for exactly the task the dev just finished.
#[tokio::test]
async fn a_planned_ticket_off_the_board_resolves_from_its_snapshot() {
    let pool = make_board_pool().await;
    // No pm_tasks row at all — the ticket is gone, only the snapshot remains.
    sqlx::query(
        "INSERT INTO daily_plan (plan_date, task_key, position, origin, task_snapshot, created_at, updated_at) \
         VALUES ('2026-07-16', 'KAN-9', 0, 'manual', ?, '2026-07-16T00:00:00Z', '2026-07-16T00:00:00Z')",
    )
    .bind(r#"{"title":"Ship the matcher","provider":"jira","description_text":"The long description","issue_type":"Task","epic_title":"Worklogs"}"#)
    .execute(&pool)
    .await
    .unwrap();

    let got = meridian_core::plan::load_plan_candidates(&pool, "2026-07-16")
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].title, "Ship the matcher");
    assert_eq!(got[0].description, "The long description");
    assert_eq!(got[0].epic.as_deref(), Some("Worklogs"));
    assert!(
        got[0].is_terminal,
        "off the board ⇒ done for the day's plan"
    );
}

/// The candidate description is the FULL text, not the plan card's excerpt.
///
/// `PlanItem.description` is truncated to ~130 chars for display. Feeding that to
/// the matcher would silently halve its context (`render_doc` allows 300) with
/// nothing saying so — the reason `PlanCandidate` exists as its own shape.
#[tokio::test]
async fn plan_candidates_carry_the_untruncated_description() {
    let pool = make_board_pool().await;
    let long = "x".repeat(400);
    sqlx::query(
        "INSERT INTO pm_tasks (task_key, title, provider, description_text, status_raw, is_terminal, updated_at) \
         VALUES ('KAN-1', 'T', 'jira', ?, 'To Do', 0, '2026-07-16T00:00:00Z')",
    )
    .bind(&long)
    .execute(&pool)
    .await
    .unwrap();
    plan_row(&pool, "2026-07-16", "KAN-1", 0).await;

    let got = meridian_core::plan::load_plan_candidates(&pool, "2026-07-16")
        .await
        .unwrap();
    assert_eq!(
        got[0].description.chars().count(),
        400,
        "the matcher must see the whole description, not the card's excerpt"
    );
}

/// The plan is per-day, and so is the candidate set. Yesterday's plan must never
/// leak into today's worklog.
#[tokio::test]
async fn plan_candidates_are_scoped_to_their_day() {
    let pool = make_board_pool().await;
    for key in ["KAN-1", "KAN-2"] {
        sqlx::query(
            "INSERT INTO pm_tasks (task_key, title, provider, status_raw, is_terminal, updated_at) \
             VALUES (?, 'T', 'jira', 'To Do', 0, '2026-07-16T00:00:00Z')",
        )
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    }
    plan_row(&pool, "2026-07-15", "KAN-1", 0).await;
    plan_row(&pool, "2026-07-16", "KAN-2", 0).await;

    let got = meridian_core::plan::load_plan_candidates(&pool, "2026-07-16")
        .await
        .unwrap();
    let keys: Vec<&str> = got.iter().map(|t| t.task_key.as_str()).collect();
    assert_eq!(keys, ["KAN-2"]);
}

/// The page's `on_board` flag is the same answer as the board query's `WHERE`.
/// Two code paths, one predicate — this is what stops them drifting.
#[tokio::test]
async fn on_board_flag_agrees_with_the_board_query() {
    use meridian_core::board::is_on_board;
    assert!(is_on_board(false, None));
    assert!(!is_on_board(true, None));
    assert!(!is_on_board(false, Some("excluded")));
}

async fn make_board_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    for ddl in [
        r#"CREATE TABLE pm_tasks (
            task_key TEXT NOT NULL UNIQUE, title TEXT NOT NULL, provider TEXT NOT NULL,
            url TEXT NOT NULL DEFAULT '', status_raw TEXT NOT NULL DEFAULT '',
            is_terminal INTEGER NOT NULL DEFAULT 0, due_date TEXT, updated_at TEXT NOT NULL,
            description_text TEXT NOT NULL DEFAULT '', epic_title TEXT, parent_key TEXT,
            priority TEXT NOT NULL DEFAULT '', issue_type TEXT NOT NULL DEFAULT '',
            story_points TEXT
        );"#,
        r#"CREATE TABLE pm_task_curation (task_key TEXT, decision TEXT);"#,
        // Migration 041 + 044. The matcher's candidate set reads this.
        r#"CREATE TABLE daily_plan (
            plan_date TEXT NOT NULL, task_key TEXT NOT NULL,
            position INTEGER NOT NULL DEFAULT 0, origin TEXT NOT NULL DEFAULT 'manual',
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL, task_snapshot TEXT,
            PRIMARY KEY (plan_date, task_key)
        );"#,
        r#"CREATE TABLE daily_plan_meta (
            plan_date TEXT PRIMARY KEY, confirmed_at TEXT,
            skipped INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );"#,
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    pool
}

// ── The plan cap ──────────────────────────────────────────────────────────────

/// Ten is the wall. The planner sends the whole list as `set`/`confirm`, so this
/// is the path that actually has to hold — the UI stop is only the faster of the
/// two, and a browser is not a guard.
#[tokio::test]
async fn a_whole_list_write_refuses_past_the_cap() {
    let pool = make_board_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();

    let keys: Vec<String> = (1..=meridian_core::plan::MAX_PLAN_TASKS + 1)
        .map(|i| format!("KAN-{i}"))
        .collect();
    for action in ["set", "confirm"] {
        let body = meridian_core::plan::PlanBody {
            action: action.to_string(),
            date: Some(date.clone()),
            task_key: None,
            task_keys: Some(keys.clone()),
        };
        let err = meridian_core::plan::apply_plan_action(
            &pool,
            &body,
            &date,
            today,
            "2026-07-16T00:00:00Z",
            Vec::new(),
        )
        .await
        .expect_err("11 tasks must be refused");
        assert!(
            err.to_string().contains("up to 10 tasks"),
            "{action}: the user must be told the limit, got: {err}"
        );
    }
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM daily_plan")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "a refused write must not half-apply");
}

/// Exactly the cap is fine — the limit is 10 tasks, not 9.
#[tokio::test]
async fn a_whole_list_write_accepts_exactly_the_cap() {
    let pool = make_board_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();

    let keys: Vec<String> = (1..=meridian_core::plan::MAX_PLAN_TASKS)
        .map(|i| format!("KAN-{i}"))
        .collect();
    let body = meridian_core::plan::PlanBody {
        action: "confirm".to_string(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: Some(keys),
    };
    meridian_core::plan::apply_plan_action(
        &pool,
        &body,
        &date,
        today,
        "2026-07-16T00:00:00Z",
        Vec::new(),
    )
    .await
    .expect("exactly the cap is legal");

    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM daily_plan")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, meridian_core::plan::MAX_PLAN_TASKS as i64);
}

/// The cap counts what would be WRITTEN, not what was sent. `replace_plan`
/// UPSERTs, so a payload repeating a key writes one row — counting the raw array
/// would refuse a plan that is actually legal.
#[tokio::test]
async fn the_cap_counts_distinct_keys() {
    let pool = make_board_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();

    let mut keys: Vec<String> = (1..=meridian_core::plan::MAX_PLAN_TASKS)
        .map(|i| format!("KAN-{i}"))
        .collect();
    keys.push("KAN-1".to_string()); // 11 entries, 10 distinct

    let body = meridian_core::plan::PlanBody {
        action: "set".to_string(),
        date: Some(date.clone()),
        task_key: None,
        task_keys: Some(keys),
    };
    meridian_core::plan::apply_plan_action(
        &pool,
        &body,
        &date,
        today,
        "2026-07-16T00:00:00Z",
        Vec::new(),
    )
    .await
    .expect("a repeated key is still 10 distinct tasks");
}

/// `add` at the cap: a NEW key is refused, but re-adding one already on the plan
/// stays the no-op it has always been (the INSERT is ON CONFLICT DO NOTHING), so
/// an idempotent retry can't start failing just because the day filled up.
#[tokio::test]
async fn add_refuses_a_new_key_at_the_cap_but_re_adding_is_still_a_no_op() {
    let pool = make_board_pool().await;
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    for i in 1..=meridian_core::plan::MAX_PLAN_TASKS {
        plan_row(&pool, &date, &format!("KAN-{i}"), i as i64).await;
    }

    let add = |key: &str| {
        let body = meridian_core::plan::PlanBody {
            action: "add".to_string(),
            date: Some(date.clone()),
            task_key: Some(key.to_string()),
            task_keys: None,
        };
        let pool = pool.clone();
        let date = date.clone();
        async move {
            meridian_core::plan::apply_plan_action(
                &pool,
                &body,
                &date,
                today,
                "2026-07-16T00:00:00Z",
                Vec::new(),
            )
            .await
        }
    };

    assert!(add("KAN-99").await.is_err(), "an 11th task must be refused");
    assert!(
        add("KAN-1").await.is_ok(),
        "re-adding a key already on the plan adds nothing, so it must not be refused"
    );
}

// ── llm_experiments (the dev-only LLM Lab reader) ────────────────────────────

/// In-memory pool with the two migration-064 LLM-Lab tables.
async fn make_llm_lab_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        r#"
        CREATE TABLE llm_experiments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            process TEXT NOT NULL, input_ref TEXT NOT NULL,
            input_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'running',
            n_variants INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL, finished_at TEXT
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TABLE llm_experiment_results (
            experiment_id INTEGER NOT NULL, variant_idx INTEGER NOT NULL,
            provider TEXT NOT NULL, model TEXT NOT NULL DEFAULT '',
            params_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'pending',
            output_text TEXT, output_rendered TEXT, error TEXT,
            input_tokens INTEGER, output_tokens INTEGER, elapsed_s REAL,
            started_at TEXT, finished_at TEXT,
            PRIMARY KEY (experiment_id, variant_idx)
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn llm_experiments_list_orders_newest_first_and_counts_done() {
    let pool = make_llm_lab_pool().await;
    for (process, input_ref, created) in [
        ("hour_report", "2026-07-15T14", "2026-07-15T15:00:00Z"),
        ("worklog_generate", "2026-07-15/T2", "2026-07-15T16:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO llm_experiments (process, input_ref, status, n_variants, created_at) \
             VALUES (?, ?, 'running', 2, ?)",
        )
        .bind(process)
        .bind(input_ref)
        .bind(created)
        .execute(&pool)
        .await
        .unwrap();
    }
    // Experiment 1: one terminal (ok) + one still running → n_done = 1.
    for (idx, status) in [(0i64, "ok"), (1i64, "running")] {
        sqlx::query(
            "INSERT INTO llm_experiment_results (experiment_id, variant_idx, provider, status) \
             VALUES (1, ?, 'claude', ?)",
        )
        .bind(idx)
        .bind(status)
        .execute(&pool)
        .await
        .unwrap();
    }

    let rows = meridian_core::llm_experiments::list_experiments(&pool, 20)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, 2, "newest (by created_at) first");
    assert_eq!(rows[0].n_done, 0);
    assert_eq!(rows[1].id, 1);
    assert_eq!(rows[1].n_done, 1, "ok counts, running does not");
    assert_eq!(rows[1].n_variants, 2);
}

#[tokio::test]
async fn llm_experiments_detail_carries_results_in_variant_order() {
    let pool = make_llm_lab_pool().await;
    sqlx::query(
        "INSERT INTO llm_experiments (process, input_ref, input_json, status, n_variants, created_at) \
         VALUES ('hour_report', '2026-07-15T14', '{\"user\":\"u\"}', 'done', 2, '2026-07-15T15:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO llm_experiment_results \
         (experiment_id, variant_idx, provider, model, status, output_text, output_rendered, \
          input_tokens, output_tokens, elapsed_s) \
         VALUES (1, 1, 'local', 'qwen', 'ok', 'raw', 'rendered', 100, 50, 2.5), \
                (1, 0, 'claude', '', 'rate_limited', NULL, NULL, NULL, NULL, NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let d = meridian_core::llm_experiments::get_experiment(&pool, 1)
        .await
        .unwrap()
        .expect("experiment exists");
    assert_eq!(d.input_json, "{\"user\":\"u\"}");
    assert_eq!(d.results.len(), 2);
    assert_eq!(d.results[0].variant_idx, 0, "idx order, not insert order");
    assert_eq!(d.results[0].provider, "claude");
    assert_eq!(d.results[0].status, "rate_limited");
    assert_eq!(d.results[1].model, "qwen");
    assert_eq!(d.results[1].output_rendered.as_deref(), Some("rendered"));
    assert_eq!(d.results[1].elapsed_s, Some(2.5));

    // Unknown id → None, not an error.
    assert!(meridian_core::llm_experiments::get_experiment(&pool, 99)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn llm_experiments_degrade_to_empty_on_a_pre_064_db() {
    // The tray opens meridian.db without running migrations — a DB without the
    // 064 tables must read as "no experiments", never error.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    assert!(meridian_core::llm_experiments::list_experiments(&pool, 20)
        .await
        .unwrap()
        .is_empty());
    assert!(meridian_core::llm_experiments::get_experiment(&pool, 1)
        .await
        .unwrap()
        .is_none());
}

// ── day_task_worklogs::set_draft_provider ────────────────────────────────────
//
// The propose branch's provider is assigned at generate time as "the first
// configured tracker", which is arbitrary for anyone on two boards. These cover
// the override that lets the user re-point it, and — more importantly — the three
// cases where it must REFUSE, because each one would otherwise make the row name a
// tracker the ticket is not actually on.

/// In-memory pool with the generated-worklog ledger (migrations 060/062/063) and
/// the `pm_tasks` the target read left-joins for titles.
async fn make_worklog_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    sqlx::query(
        r#"
        CREATE TABLE day_task_worklogs (
            day_local TEXT NOT NULL, task_id TEXT NOT NULL, provider TEXT NOT NULL,
            match_task_key TEXT, match_confidence REAL,
            propose_issue_type TEXT, propose_title TEXT, propose_description TEXT,
            update_summary TEXT NOT NULL DEFAULT '', update_json TEXT NOT NULL DEFAULT '{}',
            reasoning TEXT NOT NULL DEFAULT '', state TEXT NOT NULL DEFAULT 'drafted',
            target_key TEXT, created_task_key TEXT, posted_comment_id TEXT, browse_url TEXT,
            last_error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            PRIMARY KEY (day_local, task_id)
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TABLE day_task_worklog_targets (
            day_local TEXT NOT NULL, task_id TEXT NOT NULL, task_key TEXT NOT NULL,
            provider TEXT NOT NULL, confidence REAL NOT NULL DEFAULT 0,
            manual INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL DEFAULT 0,
            posted_comment_id TEXT, browse_url TEXT, posted_at TEXT, last_error TEXT,
            post_attempt_at TEXT, created_at TEXT NOT NULL,
            PRIMARY KEY (day_local, task_id, task_key)
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

/// Seed one draft. `propose` = the PROPOSE branch (a new ticket); otherwise the
/// row is a MATCH with one target ticket.
async fn seed_draft(pool: &SqlitePool, state: &str, provider: &str, propose: bool) {
    sqlx::query(
        "INSERT INTO day_task_worklogs (day_local, task_id, provider, propose_issue_type, \
            propose_title, propose_description, update_summary, update_json, reasoning, state, \
            created_at, updated_at) \
         VALUES ('2026-07-20', 'T1', ?, ?, ?, ?, 'did things', '{}', 'because', ?, 't0', 't0')",
    )
    .bind(provider)
    .bind(if propose { Some("Task") } else { None })
    .bind(if propose {
        Some("Ship the thing")
    } else {
        None
    })
    .bind(if propose { Some("A description") } else { None })
    .bind(state)
    .execute(pool)
    .await
    .unwrap();

    if !propose {
        sqlx::query(
            "INSERT INTO day_task_worklog_targets \
                (day_local, task_id, task_key, provider, confidence, manual, position, created_at) \
             VALUES ('2026-07-20', 'T1', 'KAN-1', ?, 0.9, 0, 0, 't0')",
        )
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn set_draft_provider_repoints_a_drafted_proposal() {
    let pool = make_worklog_pool().await;
    seed_draft(&pool, "drafted", "jira", true).await;

    let out = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T1",
        "github",
        "t1",
    )
    .await
    .expect("a drafted proposal is re-pointable");

    assert_eq!(out.provider, "github");
    // The proposal itself is untouched — this changes WHERE it lands, not WHAT.
    assert_eq!(out.propose.as_ref().unwrap().title, "Ship the thing");
    assert_eq!(out.state, "drafted");
}

#[tokio::test]
async fn set_draft_provider_stamps_updated_at_and_clears_a_stale_error() {
    let pool = make_worklog_pool().await;
    seed_draft(&pool, "drafted", "jira", true).await;
    sqlx::query("UPDATE day_task_worklogs SET last_error = 'jira rejected it'")
        .execute(&pool)
        .await
        .unwrap();

    let out = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T1",
        "github",
        "t1",
    )
    .await
    .unwrap();

    // A failure against the OLD provider must not be shown against the new one —
    // the user would read it as "GitHub rejected it", which never happened.
    assert_eq!(out.error, None);
    assert_eq!(out.updated_at, "t1");
}

#[tokio::test]
async fn set_draft_provider_is_idempotent_on_the_same_provider() {
    let pool = make_worklog_pool().await;
    seed_draft(&pool, "drafted", "jira", true).await;

    let out = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T1",
        "jira",
        "t1",
    )
    .await
    .expect("re-choosing the current provider is not an error");
    assert_eq!(out.provider, "jira");
}

#[tokio::test]
async fn set_draft_provider_refuses_a_match_draft() {
    let pool = make_worklog_pool().await;
    seed_draft(&pool, "drafted", "jira", false).await;

    let err = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T1",
        "github",
        "t1",
    )
    .await
    .expect_err("a matched draft's provider is per-target, not per-draft");
    assert!(
        err.to_string().contains("already exist"),
        "the message must say why, got: {err}"
    );

    // And the row is untouched — a refusal must not half-apply.
    let after: String =
        sqlx::query_scalar("SELECT provider FROM day_task_worklogs WHERE task_id = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, "jira");
}

#[tokio::test]
async fn set_draft_provider_refuses_once_approved() {
    let pool = make_worklog_pool().await;
    seed_draft(&pool, "approved", "jira", true).await;

    let err = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T1",
        "github",
        "t1",
    )
    .await
    .expect_err("an approved row may already have created the ticket");
    assert!(
        err.to_string().contains("already been approved"),
        "got: {err}"
    );
}

#[tokio::test]
async fn set_draft_provider_refuses_once_posted() {
    let pool = make_worklog_pool().await;
    seed_draft(&pool, "posted", "jira", true).await;

    let err = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T1",
        "github",
        "t1",
    )
    .await
    .expect_err("a posted comment is live on the old provider");
    assert!(
        err.to_string().contains("already been approved"),
        "got: {err}"
    );
}

#[tokio::test]
async fn set_draft_provider_errors_on_a_draft_that_does_not_exist() {
    let pool = make_worklog_pool().await;
    // No seed at all: nothing to re-point, and the read-back must not panic.
    let err = meridian_core::day_task_worklogs::set_draft_provider(
        &pool,
        "2026-07-20",
        "T404",
        "github",
        "t1",
    )
    .await
    .expect_err("no draft, no provider to set");
    assert!(err.to_string().contains("vanished"), "got: {err}");
}
