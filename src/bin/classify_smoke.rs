// meridian — normalises screenpipe activity into structured app sessions
//
// Smoke-test binary: reads real app_sessions + pm_tasks from meridian.db,
// runs the LLM classifier on each session, and prints results.
// Nothing is written to the DB — read-only.
//
// Usage:
//   cargo run --bin classify_smoke
//   cargo run --bin classify_smoke -- --limit 10
//   cargo run --bin classify_smoke -- --app "Antigravity"
//   cargo run --bin classify_smoke -- --backend ollama

use std::collections::HashSet;
use std::str::FromStr;
use std::time::Instant;

use anyhow::{Context, Result};
use meridian::config::LlmBackendConfig;
use meridian::intelligence::classifier::backends::build_backend;
use meridian::intelligence::classifier::{ClassifyRequest, PmTaskRef};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: i64,
    app_name: String,
    duration_s: i64,
    window_titles: String,
    category: String,
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    task_key: String,
    title: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let limit = arg_value(&args, "--limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(10);
    let app_filter = arg_value(&args, "--app");
    let id_filter: Option<Vec<i64>> = arg_value(&args, "--id").map(|v| {
        v.split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect()
    });
    let backend_override = arg_value(&args, "--backend");

    let db_path = shellexpand::tilde("~/.meridian/meridian.db").into_owned();
    let pool = SqlitePool::connect_with(SqliteConnectOptions::from_str(&db_path)?.read_only(true))
        .await
        .context("failed to open meridian.db")?;

    // Load pm_tasks
    let task_rows: Vec<TaskRow> = sqlx::query_as(
        "SELECT task_key, title FROM pm_tasks WHERE status_category != 'done' ORDER BY task_key",
    )
    .fetch_all(&pool)
    .await
    .context("loading pm_tasks")?;

    if task_rows.is_empty() {
        eprintln!("No pm_tasks found — configure JIRA_BASE_URL/JIRA_EMAIL/JIRA_API_TOKEN and run the daemon first.");
        return Ok(());
    }

    let task_refs: Vec<PmTaskRef> = task_rows
        .iter()
        .map(|t| PmTaskRef {
            key: t.task_key.clone(),
            title: t.title.clone(),
        })
        .collect();
    let valid_keys: HashSet<String> = task_rows.iter().map(|t| t.task_key.clone()).collect();

    // Load sessions
    let sessions: Vec<SessionRow> = if let Some(ids) = &id_filter {
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, app_name, duration_s, window_titles,
                    category
             FROM app_sessions WHERE id IN ({}) ORDER BY started_at DESC",
            placeholders
        );
        let mut q = sqlx::query_as(&sql);
        for id in ids {
            q = q.bind(id);
        }
        q.fetch_all(&pool).await?
    } else if let Some(app) = app_filter {
        sqlx::query_as(
            "SELECT id, app_name, duration_s, window_titles,
                    category
             FROM app_sessions WHERE app_name = ? AND duration_s > 10
             ORDER BY started_at DESC LIMIT ?",
        )
        .bind(app)
        .bind(limit)
        .fetch_all(&pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, app_name, duration_s, window_titles,
                    category
             FROM app_sessions WHERE duration_s > 10
             ORDER BY started_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&pool)
        .await?
    };

    // Build backend
    let backend_cfg = match backend_override {
        Some("ollama") => LlmBackendConfig::OpenAiCompat {
            base_url: std::env::var("LLM_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string()),
            model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "qwen2.5:3b".to_string()),
        },
        Some("claude") => LlmBackendConfig::Claude {
            api_key: std::env::var("ANTHROPIC_API_KEY")
                .or_else(|_| std::env::var("LLM_API_KEY"))
                .unwrap_or_default(),
            model: std::env::var("LLM_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string()),
        },
        _ => LlmBackendConfig::FoundationModels,
    };
    let backend = build_backend(&backend_cfg);

    println!(
        "\nClassifier smoke test — backend: {}  sessions: {}  issues: {}\n",
        backend.name(),
        sessions.len(),
        task_rows.len()
    );
    println!(
        "{:<6}  {:<16}  {:<5}  {:<10}  {:<8}  {:<6}  TITLE",
        "ID", "APP", "DUR", "WAS", "→ ISSUE", "MS"
    );
    println!("{}", "-".repeat(100));

    let mut matched = 0usize;
    let mut nulled = 0usize;

    for s in &sessions {
        let windows: Vec<String> = serde_json::from_str::<Vec<serde_json::Value>>(&s.window_titles)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| {
                v.get("window_name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.to_string())
            })
            .collect();

        let req = ClassifyRequest {
            app_name: s.app_name.clone(),
            duration_s: s.duration_s,
            windows,
            ocr_snippet: String::new(),
            tasks: task_refs.clone(),
            valid_keys: valid_keys.clone(),
        };

        let t0 = Instant::now();
        match backend.classify(&req).await {
            Ok(resp) => {
                let ms = t0.elapsed().as_millis();
                let issue = resp.task_key.as_deref().unwrap_or("null");
                let title = resp
                    .task_key
                    .as_ref()
                    .and_then(|k| task_rows.iter().find(|t| &t.task_key == k))
                    .map(|t| truncate(&t.title, 40))
                    .unwrap_or_default();

                if resp.task_key.is_some() {
                    matched += 1;
                } else {
                    nulled += 1;
                }

                println!(
                    "{:<6}  {:<16}  {:<5}  {:<10}  {:<8}  {:<6}  {}",
                    s.id,
                    truncate(&s.app_name, 14),
                    format!("{}s", s.duration_s),
                    truncate(&s.category, 10),
                    issue,
                    format!("{}ms", ms),
                    title,
                );
            }
            Err(e) => {
                println!(
                    "{:<6}  {:<16}  {:<5}  {:<10}  ERROR: {}",
                    s.id,
                    truncate(&s.app_name, 14),
                    format!("{}s", s.duration_s),
                    &s.category,
                    e
                );
            }
        }
    }

    println!("{}", "-".repeat(100));
    println!(
        "matched: {}  null/overhead: {}  total: {}",
        matched,
        nulled,
        sessions.len()
    );

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].as_str())
}
