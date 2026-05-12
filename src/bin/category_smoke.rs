// meridian — normalises screenpipe activity into structured app sessions
//
// Smoke-test binary: reads app sessions from meridian.db,
// runs the FM category classifier, and prints results.
// Nothing is written to the DB — read-only.
//
// Usage:
//   cargo run --bin category_smoke
//   cargo run --bin category_smoke -- --id 6925,6927
//   cargo run --bin category_smoke -- --limit 5

use std::str::FromStr;
use std::time::Instant;

use anyhow::{Context, Result};
use meridian::config::LlmBackendConfig;
use meridian::intelligence::classifier::backends::build_backend;
use meridian::intelligence::settler::{build_category_prompt, parse_category};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;

const CATEGORY_SYSTEM: &str = "\
You are an app session classifier. \
Given the app name, session duration, window titles, and optional page content, \
choose the single best category.\n\
\n\
  coding           — writing or editing code: VS Code, Xcode, JetBrains, Vim, terminal builds, localhost testing\n\
  code_review      — reviewing diffs, PRs, or merge requests on GitHub, GitLab, Gerrit\n\
  meeting          — Zoom, Google Meet, Teams, or any live video or audio call\n\
  communication    — Slack, email, Discord, Teams messages, chat\n\
  design           — Figma, Sketch, Adobe XD, Framer, Canva\n\
  documentation    — writing or editing docs: Notion, Confluence, Google Docs, GitBook\n\
  planning         — Jira, Linear, GitHub Issues, project boards, sprint planning\n\
  deployment_devops — CI/CD pipelines, cloud consoles, Kubernetes, monitoring dashboards\n\
  research         — reading docs, Stack Overflow, tutorials, GitHub repos, articles\n\
  idle_personal    — YouTube, social media, news, entertainment, shopping, games";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let limit = arg_value(&args, "--limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(5);
    let id_filter: Option<Vec<i64>> = arg_value(&args, "--id").map(|v| {
        v.split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect()
    });

    let db_path = shellexpand::tilde("~/.meridian/meridian.db").into_owned();
    let pool = SqlitePool::connect_with(SqliteConnectOptions::from_str(&db_path)?.read_only(true))
        .await
        .context("failed to open meridian.db")?;

    let rows: Vec<(i64, String, i64, String, String, String)> = if let Some(ids) = &id_filter {
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, app_name, duration_s, window_titles,
                    COALESCE(session_text, ''), category
             FROM app_sessions WHERE id IN ({}) ORDER BY id ASC",
            placeholders
        );
        let mut q = sqlx::query_as(&sql);
        for id in ids {
            q = q.bind(id);
        }
        q.fetch_all(&pool).await?
    } else {
        sqlx::query_as(
            "SELECT id, app_name, duration_s, window_titles,
                    COALESCE(session_text, ''), category
             FROM app_sessions
             WHERE duration_s >= 5
             ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&pool)
        .await?
    };

    if rows.is_empty() {
        eprintln!("No sessions found.");
        return Ok(());
    }

    let backend = build_backend(&LlmBackendConfig::FoundationModels);

    println!(
        "\nCategory smoke test (all apps) — backend: {}  sessions: {}\n",
        backend.name(),
        rows.len()
    );
    println!(
        "{:<6}  {:<16}  {:<5}  {:<15}  {:<20}  {:<6}  RAW",
        "ID", "APP", "DUR", "WAS", "→ CATEGORY", "MS"
    );
    println!("{}", "-".repeat(110));

    for (id, app_name, duration_s, window_titles, session_text, was_category) in &rows {
        let prompt = build_category_prompt(app_name, *duration_s, window_titles, session_text);

        let t0 = Instant::now();
        match backend.raw_generate(CATEGORY_SYSTEM, &prompt).await {
            Ok(text) => {
                let ms = t0.elapsed().as_millis();
                let category = parse_category(&text).unwrap_or("(unparseable)");
                println!(
                    "{:<6}  {:<16}  {:<5}  {:<15}  {:<20}  {:<6}  {}",
                    id,
                    truncate(app_name, 14),
                    format!("{}s", duration_s),
                    truncate(was_category, 13),
                    category,
                    format!("{}ms", ms),
                    truncate(&text.replace('\n', " "), 40),
                );
            }
            Err(e) => {
                println!(
                    "{:<6}  {:<16}  {:<5}  {:<15}  ERROR: {}",
                    id,
                    truncate(app_name, 14),
                    format!("{}s", duration_s),
                    truncate(was_category, 13),
                    e
                );
            }
        }
    }

    println!("{}", "-".repeat(110));
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
