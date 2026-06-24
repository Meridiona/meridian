//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/logs` GET ported to Rust — last N lines from the daemon's JSONL log.
//!
//! Reads `~/.meridian/logs/meridian-rust.jsonl.<today>`, falling back to
//! yesterday's file if today's doesn't exist yet. Each line is parsed from the
//! `tracing-subscriber` JSON format into a [`LogEntry`]. Returns at most
//! `limit` entries (default 200, max 1000), matching the TS route.
//!
//! # Who calls this
//! - Command: `get_logs` (registered in `lib.rs`)
//! - Frontend: `ui/components/views/LogsView.tsx` on mount (initial load before
//!   the SSE tail takes over) — swapped to
//!   `load('/api/logs', 'get_logs', { limit: 200 })`.
//!
//! # Related
//! - The SSE tail (`/api/logs/stream`) is NOT ported here; it becomes a Tauri
//!   event in the SSE-migration phase.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};

/// A parsed daemon log line. Field names match the TS `LogEntry` interface
/// in `ui/lib/log-tail.ts` so the frontend component needs no changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
    pub fields: HashMap<String, serde_json::Value>,
}

/// The last N log entries (the ported `/api/logs` GET).
///
/// `limit` defaults to 200, clamped to 1000 — mirrors `Math.min(parseInt(…) ?? 200, 1000)`.
#[tauri::command]
#[tracing::instrument]
pub async fn get_logs(limit: Option<u32>) -> Result<Vec<LogEntry>, String> {
    let n = limit.unwrap_or(200).min(1000) as usize;
    let entries = read_recent_lines(n);
    tracing::info!(entries = entries.len(), "logs read");
    Ok(entries)
}

fn log_dir() -> String {
    std::env::var("MERIDIAN_LOG_DIR").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/.meridian/logs", home)
    })
}

/// Today's JSONL log path. `pub(crate)` so the poll loop's log tailer
/// ([`crate::poll`]) follows the same file the snapshot read uses.
pub(crate) fn today_log_path() -> String {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    format!("{}/meridian-rust.jsonl.{}", log_dir(), date)
}

fn yesterday_log_path() -> String {
    let date = (chrono::Local::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    format!("{}/meridian-rust.jsonl.{}", log_dir(), date)
}

/// Parse one `tracing-subscriber` JSON log line into a [`LogEntry`].
/// `pub(crate)` so the poll loop's log tailer reuses the exact same parsing the
/// snapshot read uses (one source of truth for the log shape).
pub(crate) fn parse_line(raw: &str) -> Option<LogEntry> {
    let obj: serde_json::Value = serde_json::from_str(raw).ok()?;
    let timestamp = obj.get("timestamp")?.as_str()?.to_string();
    let level = obj
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("INFO")
        .to_uppercase();
    let message = obj
        .get("fields")
        .and_then(|f| f.get("message"))
        .and_then(|m| m.as_str())
        .or_else(|| obj.get("message").and_then(|m| m.as_str()))
        .unwrap_or("")
        .to_string();
    let target = obj
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let span = obj
        .get("span")
        .and_then(|s| s.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    // Strip `message` and `scope` from fields (matches the TS route's rest-spread).
    let fields: HashMap<String, serde_json::Value> = obj
        .get("fields")
        .and_then(|f| f.as_object())
        .map(|m| {
            m.iter()
                .filter(|(k, _)| k.as_str() != "message" && k.as_str() != "scope")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();

    Some(LogEntry {
        timestamp,
        level,
        message,
        target,
        span,
        fields,
    })
}

fn read_recent_lines(n: usize) -> Vec<LogEntry> {
    let candidates = [today_log_path(), yesterday_log_path()];
    let log_path = candidates.iter().find(|p| std::path::Path::new(p).exists());
    let Some(path) = log_path else {
        return Vec::new();
    };

    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };

    let entries: Vec<LogEntry> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.is_empty())
        .filter_map(|l| parse_line(&l))
        .collect();

    // Take the last n entries (matches `entries.slice(-n)` in TS).
    if entries.len() <= n {
        entries
    } else {
        entries[entries.len() - n..].to_vec()
    }
}
