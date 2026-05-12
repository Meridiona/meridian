// meridian — normalises screenpipe activity into structured app sessions

mod common;

use meridian::intelligence::settler::{build_category_prompt, parse_category};

// ---------------------------------------------------------------------------
// parse_category — exact match only
// ---------------------------------------------------------------------------

#[test]
fn parse_category_json_exact_match() {
    assert_eq!(
        parse_category(r#"{"category": "research"}"#),
        Some("research")
    );
    assert_eq!(
        parse_category(r#"{"category": "code_review"}"#),
        Some("code_review")
    );
    assert_eq!(
        parse_category(r#"{"category": "idle_personal"}"#),
        Some("idle_personal")
    );
    assert_eq!(
        parse_category(r#"{"category": "deployment_devops"}"#),
        Some("deployment_devops")
    );
}

#[test]
fn parse_category_json_mixed_case() {
    // FM may capitalise the value — accept case-insensitive
    assert_eq!(
        parse_category(r#"{"category": "Research"}"#),
        Some("research")
    );
    assert_eq!(
        parse_category(r#"{"category": "CODE_REVIEW"}"#),
        Some("code_review")
    );
}

#[test]
fn parse_category_json_with_backticks() {
    // FM sometimes wraps response in markdown fences
    assert_eq!(
        parse_category("`{\"category\": \"planning\"}`"),
        Some("planning")
    );
}

#[test]
fn parse_category_unknown_value_returns_none() {
    assert_eq!(parse_category(r#"{"category": "gaming"}"#), None);
    assert_eq!(parse_category(r#"{"category": "not_a_category"}"#), None);
    assert_eq!(parse_category(r#"{"category": "development"}"#), None);
}

#[test]
fn parse_category_rejects_substring_supersets() {
    // "code_review_and_more" must NOT match "code_review" (exact match required)
    assert_eq!(parse_category(r#"{"category": "code_review_extra"}"#), None);
    // "idle" alone must NOT match "idle_personal"
    assert_eq!(parse_category(r#"{"category": "idle"}"#), None);
}

#[test]
fn parse_category_empty_returns_none() {
    assert_eq!(parse_category(""), None);
    assert_eq!(parse_category("{}"), None);
}

#[test]
fn parse_category_all_valid_categories_round_trip() {
    let categories = [
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
    for cat in categories {
        let json = format!(r#"{{"category": "{}"}}"#, cat);
        assert_eq!(
            parse_category(&json),
            Some(cat),
            "expected round-trip for {cat}"
        );
    }
}

// ---------------------------------------------------------------------------
// build_category_prompt — correct field extraction
// ---------------------------------------------------------------------------

#[test]
fn build_category_prompt_window_name_key() {
    let titles = r#"[{"window_name":"GitHub - Pull Requests","count":3}]"#;
    let prompt = build_category_prompt("Chrome", 120, titles, None);
    assert!(
        prompt.contains("GitHub - Pull Requests"),
        "should include window_name value; prompt was:\n{prompt}"
    );
}

#[test]
fn build_category_prompt_title_key_fallback() {
    let titles = r#"[{"title":"Notion - Project Plan","count":2}]"#;
    let prompt = build_category_prompt("Notion", 60, titles, None);
    assert!(
        prompt.contains("Notion - Project Plan"),
        "should include title value; prompt was:\n{prompt}"
    );
}

#[test]
fn build_category_prompt_includes_duration() {
    let prompt = build_category_prompt("Xcode", 999, "[]", None);
    assert!(prompt.contains("999s"), "duration in seconds should appear");
}

#[test]
fn build_category_prompt_includes_app_name() {
    let prompt = build_category_prompt("Visual Studio Code", 120, "[]", None);
    assert!(
        prompt.contains("Visual Studio Code"),
        "app name should appear; prompt was:\n{prompt}"
    );
}

// ---------------------------------------------------------------------------
// settle_all_categories — DB integration: sentinel prevents retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settle_all_categories_sentinel_prevents_retry() {
    let db = common::make_meridian_db().await;

    // Insert a session already marked with the parse-error sentinel
    sqlx::query(
        "INSERT INTO app_sessions
           (app_name, started_at, ended_at, duration_s,
            window_titles, min_frame_id, max_frame_id, frame_count,
            idle_frame_count, etl_run_id, category, confidence, category_method)
         VALUES
           ('Visual Studio Code', '2024-01-01T10:00:00Z', '2024-01-01T10:05:00Z', 300,
            '[{\"window_name\":\"main.rs\"}]', 1, 5, 5,
            0, 1, 'fm_parse_error', 0.0, 'foundation_models')",
    )
    .execute(&db)
    .await
    .unwrap();

    // The settler query only picks up category_method = 'rule_based', so this row is skipped.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_sessions WHERE category_method = 'rule_based'",
    )
    .fetch_one(&db)
    .await
    .unwrap();

    assert_eq!(
        count, 0,
        "sentinel session should not be in the rule_based queue"
    );
}

#[tokio::test]
async fn settle_all_categories_short_sessions_excluded() {
    let db = common::make_meridian_db().await;

    // Session with duration_s = 4 (below threshold of 5)
    sqlx::query(
        "INSERT INTO app_sessions
           (app_name, started_at, ended_at, duration_s,
            window_titles, min_frame_id, max_frame_id, frame_count,
            idle_frame_count, etl_run_id, category, confidence, category_method)
         VALUES
           ('Xcode', '2024-01-01T10:00:00Z', '2024-01-01T10:00:04Z', 4,
            '[{\"window_name\":\"AppDelegate.swift\"}]', 1, 2, 2,
            0, 1, 'coding', 1.0, 'rule_based')",
    )
    .execute(&db)
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_sessions
         WHERE category_method = 'rule_based' AND duration_s >= 5",
    )
    .fetch_one(&db)
    .await
    .unwrap();

    assert_eq!(
        count, 0,
        "session shorter than 5s should be excluded from classification queue"
    );
}

#[tokio::test]
async fn settle_all_categories_all_apps_included() {
    let db = common::make_meridian_db().await;

    // Insert sessions from diverse app types — all should be in the settler queue
    for (app, category) in [
        ("Google Chrome", "research"),
        ("Visual Studio Code", "coding"),
        ("Xcode", "coding"),
        ("Figma", "design"),
        ("Slack", "communication"),
        ("Terminal", "coding"),
    ] {
        sqlx::query(
            "INSERT INTO app_sessions
               (app_name, started_at, ended_at, duration_s,
                window_titles, min_frame_id, max_frame_id, frame_count,
                idle_frame_count, etl_run_id, category, confidence, category_method)
             VALUES
               (?, '2024-01-01T10:00:00Z', '2024-01-01T10:05:00Z', 300,
                '[{\"window_name\":\"Test\"}]', 1, 5, 5,
                0, 1, ?, 0.8, 'rule_based')",
        )
        .bind(app)
        .bind(category)
        .execute(&db)
        .await
        .unwrap();
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_sessions
         WHERE category_method = 'rule_based' AND duration_s >= 5",
    )
    .fetch_one(&db)
    .await
    .unwrap();

    assert_eq!(
        count, 6,
        "all 6 app sessions should be in the settler queue"
    );
}
