// meridian — normalises screenpipe activity into structured app sessions
//
// Smoke-test binary: reads real app_sessions from meridian.db, runs categorize()
// on each row, and prints the result. Nothing is written to the DB.
//
// Usage:
//   cargo run --bin cat_smoke
//   cargo run --bin cat_smoke -- --limit 50
//   cargo run --bin cat_smoke -- --app "Google Chrome"

use anyhow::{Context, Result};
use meridian::db::screenpipe::{ElementSample, SignalEvent, WindowTitleCount};
use meridian::intelligence::categorizer::{categorize, SessionSignals};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Raw row from app_sessions
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: i64,
    app_name: String,
    duration_s: i64,
    window_titles: String,
    ocr_samples: Option<String>,
    elements_samples: Option<String>,
    audio_snippets: Option<String>,
    signals: Option<String>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Simple arg parsing — no external deps needed.
    let args: Vec<String> = std::env::args().collect();
    let limit = arg_value(&args, "--limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(100);
    let app_filter = arg_value(&args, "--app");

    let db_path = shellexpand::tilde("~/.meridian/meridian.db").into_owned();

    let pool = SqlitePool::connect_with(SqliteConnectOptions::from_str(&db_path)?.read_only(true))
        .await
        .context("failed to open meridian.db")?;

    let rows: Vec<SessionRow> = if let Some(ref app) = app_filter {
        sqlx::query_as(
            "SELECT id, app_name, duration_s, window_titles,
                    ocr_samples, elements_samples, audio_snippets, signals
             FROM app_sessions
             WHERE app_name = ?
             ORDER BY started_at DESC
             LIMIT ?",
        )
        .bind(app)
        .bind(limit)
        .fetch_all(&pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, app_name, duration_s, window_titles,
                    ocr_samples, elements_samples, audio_snippets, signals
             FROM app_sessions
             WHERE duration_s > 10
             ORDER BY started_at DESC
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&pool)
        .await?
    };

    println!(
        "{:<6} {:<22} {:<10} {:<20} {:<52} {:<5}",
        "ID", "APP", "DURATION", "CATEGORY", "TOP WINDOW", "CONF"
    );
    println!("{}", "-".repeat(120));

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for row in &rows {
        let window_titles: Vec<WindowTitleCount> =
            serde_json::from_str(&row.window_titles).unwrap_or_default();

        let ocr_text = concat_ocr(row.ocr_samples.as_deref());

        let elements: Vec<ElementSample> = row
            .elements_samples
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let signals: Vec<SignalEvent> = row
            .signals
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let audio_present = row
            .audio_snippets
            .as_deref()
            .map(|s| s != "[]" && !s.is_empty())
            .unwrap_or(false);

        let session_signals = SessionSignals {
            app_name: &row.app_name,
            window_titles: &window_titles,
            ocr_text: &ocr_text,
            elements: &elements,
            signals: &signals,
            audio_present,
            duration_secs: row.duration_s as u64,
        };

        let (kind, confidence) = categorize(&session_signals);
        let category = kind.display_name();

        *counts.entry(category.to_string()).or_insert(0) += 1;

        let top_window = window_titles
            .first()
            .map(|t| truncate(&t.window_name, 50))
            .unwrap_or_default();

        println!(
            "{:<6} {:<22} {:<10} {:<20} {:<52} {:.2}",
            row.id,
            truncate(&row.app_name, 20),
            format!("{}s", row.duration_s),
            category,
            top_window,
            confidence,
        );
    }

    println!("\n{}", "=".repeat(120));
    println!("SUMMARY ({} sessions)", rows.len());
    println!("{}", "-".repeat(40));
    let mut counts_vec: Vec<_> = counts.into_iter().collect();
    counts_vec.sort_by(|a, b| b.1.cmp(&a.1));
    for (cat, count) in counts_vec {
        println!("  {:<22} {}", cat, count);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn concat_ocr(ocr_json: Option<&str>) -> String {
    #[derive(serde::Deserialize)]
    struct OcrEntry {
        text: String,
    }
    let entries: Vec<OcrEntry> = ocr_json
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    entries
        .iter()
        .map(|e| e.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
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
