//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/active` ported to Rust — a faithful port of `ui/app/api/active/route.ts`.
//!
//! # What this is
//! The dashboard's view of the single in-progress block: the `active_session`
//! row reshaped with `elapsed_s` (now − started_at) and the JSON columns parsed
//! to values. Distinct from [`crate::get_active_session`] (the raw row the
//! daemon uses) — this is the computed shape the dashboard renders.
//!
//! # Who calls this
//! The tray `get_active` command → the dashboard `Sidebar`'s active-session pill.
//!
//! # Related
//! - [`crate::get_active_session`] returns the RAW `active_session` row (daemon
//!   shape); this module is the reshaped dashboard view — they are NOT
//!   interchangeable (different fields).
//! - [`crate::today`] also reports the active session, folded into today's totals.

use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::FromRow;
use tracing::Instrument;

/// Mirrors the route's `ActiveSessionRow` (ui/lib/types.ts).
#[derive(Debug, Clone, Serialize)]
pub struct ActiveView {
    pub app_name: String,
    pub started_at: String,
    pub last_seen_at: String,
    pub window_titles: Value,
    pub audio_snippets: Option<Value>,
    pub signals: Option<Value>,
    pub frame_count: i64,
    pub elapsed_s: i64,
    pub category: String,
    pub confidence: f64,
}

/// The `active_session` row as stored (JSON columns are raw text here, parsed
/// into [`ActiveView`] values below).
#[derive(FromRow)]
struct Raw {
    app_name: String,
    started_at: String,
    last_seen_at: String,
    window_titles: Option<String>,
    audio_snippets: Option<String>,
    signals: Option<String>,
    frame_count: i64,
    category: Option<String>,
    confidence: Option<f64>,
}

/// RFC3339 → epoch millis (`None` if unparseable), for the elapsed_s math.
fn ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Read the active session as the dashboard view, or `None` if nothing is
/// active. `now_iso` (RFC3339) drives `elapsed_s`; resolved by the caller (the
/// tray command) so this fn stays deterministic.
#[tracing::instrument(skip(pool))]
pub async fn get_active_view(
    pool: &SqlitePool,
    now_iso: &str,
) -> anyhow::Result<Option<ActiveView>> {
    let row: Option<Raw> = sqlx::query_as::<_, Raw>(
        r#"
        SELECT app_name, started_at, last_seen_at,
               window_titles, audio_snippets, signals, frame_count,
               category, confidence
        FROM active_session WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("active.read.active_session"))
    .await
    .context("active: fetch active_session")?;
    tracing::debug!(found = row.is_some(), "active.read.active_session");

    let Some(r) = row else {
        tracing::debug!("active served (no active session)");
        return Ok(None);
    };

    let now_ms = ms(now_iso).unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let started_ms = ms(&r.started_at).unwrap_or(now_ms);
    let elapsed_s = (now_ms - started_ms) / 1000;
    tracing::debug!(app = %r.app_name, elapsed_s, "active served");

    // window_titles defaults to [] (route: `JSON.parse(... || '[]')`); the
    // optional blobs stay null when absent/unparseable.
    let window_titles = r
        .window_titles
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| json!([]));
    let audio_snippets = r
        .audio_snippets
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let signals = r
        .signals
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());

    Ok(Some(ActiveView {
        app_name: r.app_name,
        started_at: r.started_at,
        last_seen_at: r.last_seen_at,
        window_titles,
        audio_snippets,
        signals,
        frame_count: r.frame_count,
        elapsed_s,
        category: r
            .category
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "idle_personal".to_string()),
        confidence: r.confidence.unwrap_or(0.0),
    }))
}
