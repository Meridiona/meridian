//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/today` ported to Rust — a faithful port of `ui/app/api/today/route.ts`.
//!
//! Two streams share `app_sessions`: the foreground screen-capture stream
//! (`claude_session_uuid IS NULL`) and the coding-agent transcript OVERLAY
//! (`IS NOT NULL`). Totals UNION intervals (never sum) via [`crate::intervals`];
//! "today" is the user's local day via [`crate::date`]. `category_explanation`
//! (migration 009) and `session_summary` (024) are detected at runtime and
//! substituted with `NULL` when absent — matching the TS route's graceful check.

use crate::date::local_day_bounds;
use crate::intervals::{
    count_switches, intersect_seconds, merge_intervals, session_interval, union_seconds, Interval,
    SwitchSession,
};
use crate::SqlitePool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::BTreeMap;
use tracing::Instrument;

/// Foreground sessions shorter than this are sub-second focus jitter, not real
/// context switches.
const SWITCH_MIN_DURATION_S: i64 = 15;

// ── Response types (field names match the TS JSON exactly for golden-compare) ──

#[derive(Debug, Clone, Serialize)]
pub struct TodaySession {
    pub id: i64,
    pub app: String,
    pub started_at: String,
    pub dur: i64,
    pub cat: String,
    pub titles: Vec<String>,
    pub explain: Option<String>,
    pub routing: Option<String>,
    pub session_type: Option<String>,
    pub task_key: Option<String>,
    pub candidates: Vec<String>,
    pub confidence: f64,
    pub method: String,
    pub link_method: Option<String>,
    pub link_confidence: Option<f64>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayActive {
    pub app: String,
    pub started_at: String,
    pub elapsed_s: i64,
    pub cat: String,
    pub titles: Vec<String>,
    pub confidence: f64,
    pub explain: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayGap {
    pub id: i64,
    pub kind: String,
    pub started_at: String,
    pub ended_at: String,
    pub dur: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskMeta {
    pub title: String,
    pub provider: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub started_at: String,
    pub dur: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayResponse {
    pub date: String,
    pub sessions: Vec<TodaySession>,
    pub active: Option<TodayActive>,
    pub gaps: Vec<TodayGap>,
    pub focus_s: i64,
    pub idle_s: i64,
    pub agent_s: i64,
    pub supervised_s: i64,
    pub autonomous_s: i64,
    pub presence_segments: Vec<Interval>,
    pub agent_segments: Vec<Interval>,
    pub session_count: i64,
    pub switch_count: i64,
    pub task_totals: BTreeMap<String, i64>,
    pub task_autonomous_s: BTreeMap<String, i64>,
    pub engaged_s: i64,
    pub task_meta: BTreeMap<String, TaskMeta>,
    pub task_agent_summaries: BTreeMap<String, Vec<AgentSummary>>,
}

// ── DB row shapes ─────────────────────────────────────────────────────────────

#[derive(FromRow)]
struct TodayRow {
    id: i64,
    app_name: String,
    started_at: String,
    ended_at: String,
    duration_s: i64,
    claude_session_uuid: Option<String>,
    category: Option<String>,
    confidence: Option<f64>,
    category_method: Option<String>,
    category_explanation: Option<String>,
    session_summary: Option<String>,
    window_titles: Option<String>,
    task_key: Option<String>,
    routing: Option<String>,
    session_type: Option<String>,
    link_method: Option<String>,
    link_confidence: Option<f64>,
}

#[derive(FromRow)]
struct ActiveRow {
    app_name: String,
    started_at: String,
    window_titles: Option<String>,
    category: Option<String>,
    confidence: Option<f64>,
    // active_session has no category_explanation column (only app_sessions does,
    // added in migration 013). The classifier writes it AFTER sealing — a live
    // block genuinely has no explanation yet.
}

#[derive(FromRow)]
struct GapRow {
    id: i64,
    kind: String,
    started_at: String,
    ended_at: String,
    duration_s: i64,
}

#[derive(FromRow)]
struct TaskMetaRow {
    task_key: String,
    title: Option<String>,
    provider: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct TitleEntry {
    window_name: Option<String>,
    title: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parsed window_titles → (top title falling back to `app`, list of non-empty names).
fn parse_titles(window_titles: &Option<String>, app: &str) -> (String, Vec<String>) {
    let entries: Vec<TitleEntry> = window_titles
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let name = |t: &TitleEntry| t.window_name.clone().or_else(|| t.title.clone());
    let top = entries
        .first()
        .and_then(name)
        .unwrap_or_else(|| app.to_string());
    let names: Vec<String> = if entries.is_empty() {
        vec![top.clone()]
    } else {
        entries
            .iter()
            .filter_map(|t| name(t).filter(|s| !s.is_empty()))
            .collect()
    };
    (top, names)
}

/// `cat`, with fm_parse_error/fm_skip and empty/NULL normalised to idle_personal.
fn normalize_cat(category: &Option<String>) -> String {
    match category.as_deref() {
        Some("fm_parse_error") | Some("fm_skip") => "idle_personal".to_string(),
        Some(c) if !c.is_empty() => c.to_string(),
        _ => "idle_personal".to_string(),
    }
}

// ── The route ─────────────────────────────────────────────────────────────────

/// Compute the Today dashboard payload. `now_iso` is "now" (passed in so the
/// caller controls the clock and `active.elapsed_s` is deterministic in tests).
#[tracing::instrument(skip(pool), fields(date = %date))]
pub async fn get_today(
    pool: &SqlitePool,
    date: &str,
    now_iso: &str,
) -> anyhow::Result<TodayResponse> {
    let (start, end) = local_day_bounds(date);
    tracing::debug!(start = %start, end = %end, "today: local-day window");

    // Optional columns added by later migrations — substitute NULL when absent
    // (mirrors the TS route's `hasExplanation`/`hasSummary` graceful checks).
    let cols: std::collections::HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info('app_sessions')")
            .fetch_all(pool)
            .instrument(tracing::debug_span!("today.read.columns"))
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
    tracing::debug!(columns = cols.len(), "today.read.columns");
    let expl_expr = if cols.contains("category_explanation") {
        "s.category_explanation"
    } else {
        "NULL AS category_explanation"
    };
    let summ_expr = if cols.contains("session_summary") {
        "s.session_summary"
    } else {
        "NULL AS session_summary"
    };

    let sql = format!(
        r#"
        SELECT s.id, s.app_name, s.started_at, s.ended_at, s.duration_s,
               s.claude_session_uuid, s.category, s.confidence, s.category_method,
               {expl_expr}, {summ_expr}, s.window_titles, s.task_key,
               s.task_routing      AS routing,
               s.task_session_type AS session_type,
               s.task_method       AS link_method,
               s.task_confidence   AS link_confidence
        FROM app_sessions s
        WHERE s.started_at >= ? AND s.started_at < ?
        ORDER BY s.started_at ASC
        "#
    );
    let all_rows: Vec<TodayRow> = sqlx::query_as::<_, TodayRow>(&sql)
        .bind(&start)
        .bind(&end)
        .fetch_all(pool)
        .instrument(tracing::debug_span!("today.read.app_sessions"))
        .await?;
    tracing::debug!(rows = all_rows.len(), "today.read.app_sessions");

    // Foreground sessions become the dashboard's `sessions`; the coding-agent
    // overlay drives the unioned focus/agent figures but is never its own row.
    let sessions: Vec<TodaySession> = all_rows
        .iter()
        .filter(|r| r.claude_session_uuid.is_none())
        .map(|r| {
            let (_top, titles) = parse_titles(&r.window_titles, &r.app_name);
            TodaySession {
                id: r.id,
                app: r.app_name.clone(),
                started_at: r.started_at.clone(),
                dur: r.duration_s,
                cat: normalize_cat(&r.category),
                titles,
                explain: r.category_explanation.clone(),
                routing: r.routing.clone(),
                session_type: r.session_type.clone(),
                task_key: r.task_key.clone(),
                candidates: r.task_key.clone().into_iter().collect(),
                confidence: r.confidence.unwrap_or(0.0),
                method: r
                    .category_method
                    .clone()
                    .unwrap_or_else(|| "rule_based".to_string()),
                link_method: r.link_method.clone(),
                link_confidence: r.link_confidence,
                summary: r.session_summary.clone(),
            }
        })
        .collect();

    // Active session (single row, id = 1). Same optional-column handling, and
    // graceful (logged warn on error) so a missing column/table → no active session.
    // active_session never has category_explanation — don't select it.
    let active_sql = "SELECT app_name, started_at, window_titles, category, confidence \
                      FROM active_session WHERE id = 1";
    let active: Option<TodayActive> = sqlx::query_as::<_, ActiveRow>(active_sql)
        .fetch_optional(pool)
        .instrument(tracing::debug_span!("today.read.active_session"))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "today: active_session read failed, treating as none");
            None
        })
        .map(|ar| {
            // Active session titles: map non-empty names, no [app_name] fallback —
            // the route returns [] when window_titles is empty (only foreground
            // sessions fall back to [topTitle]).
            let entries: Vec<TitleEntry> = ar
                .window_titles
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let titles: Vec<String> = entries
                .iter()
                .filter_map(|t| t.window_name.clone().or_else(|| t.title.clone()))
                .filter(|s| !s.is_empty())
                .collect();
            let elapsed_s = match (
                crate::intervals::parse_ms(now_iso),
                crate::intervals::parse_ms(&ar.started_at),
            ) {
                (Some(now), Some(s)) => (now - s) / 1000,
                _ => 0,
            };
            // Active category: plain null/empty → idle_personal only. The route
            // does NOT remap fm_parse_error/fm_skip for the live block (only for
            // sealed foreground sessions). normalize_cat is for app_sessions rows.
            let cat = ar
                .category
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| "idle_personal".to_string());
            TodayActive {
                app: ar.app_name,
                started_at: ar.started_at,
                elapsed_s,
                cat,
                titles,
                confidence: ar.confidence.unwrap_or(0.0),
                explain: None, // active_session never carries an explanation
            }
        });
    tracing::debug!(found = active.is_some(), "today.read.active_session");

    // Gaps for the day.
    let gaps: Vec<TodayGap> = sqlx::query_as::<_, GapRow>(
        r#"SELECT id, kind, started_at, ended_at, duration_s
           FROM gaps WHERE started_at >= ? AND started_at < ? ORDER BY started_at ASC"#,
    )
    .bind(&start)
    .bind(&end)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("today.read.gaps"))
    .await
    .unwrap_or_else(|e| {
        // gaps table may not exist on very old schemas
        tracing::warn!(error = %e, "today: gaps read failed, treating as empty");
        Vec::new()
    })
    .into_iter()
    .map(|g| TodayGap {
        id: g.id,
        kind: g.kind,
        started_at: g.started_at,
        ended_at: g.ended_at,
        dur: g.duration_s,
    })
    .collect();
    tracing::debug!(rows = gaps.len(), "today.read.gaps");

    // ── Presence (foreground stream) ──────────────────────────────────────────
    let mut presence_raw: Vec<Interval> = all_rows
        .iter()
        .filter(|r| r.claude_session_uuid.is_none())
        .map(|r| Interval {
            started_at: r.started_at.clone(),
            ended_at: r.ended_at.clone(),
        })
        .collect();
    if let Some(a) = &active {
        presence_raw.push(Interval {
            started_at: a.started_at.clone(),
            ended_at: now_iso.to_string(),
        });
    }
    let presence_segments = merge_intervals(&presence_raw);
    let focus_s = union_seconds(&presence_segments);

    // ── Agent overlay (capped to engaged duration_s via session_interval) ──────
    let agent_raw: Vec<Interval> = all_rows
        .iter()
        .filter(|r| r.claude_session_uuid.is_some())
        .map(|r| {
            session_interval(
                &r.started_at,
                &r.ended_at,
                r.duration_s,
                r.claude_session_uuid.as_deref(),
            )
        })
        .collect();
    let agent_segments = merge_intervals(&agent_raw);
    let agent_s = union_seconds(&agent_segments);
    let supervised_s = intersect_seconds(&agent_segments, &presence_segments);
    let autonomous_s = (agent_s - supervised_s).max(0);

    let idle_s: i64 = gaps
        .iter()
        .filter(|g| g.kind == "user_idle")
        .map(|g| g.dur)
        .sum();

    let switch_count = count_switches(
        &sessions
            .iter()
            .map(|s| SwitchSession {
                app: s.app.clone(),
                started_at: s.started_at.clone(),
                dur: s.dur,
            })
            .collect::<Vec<_>>(),
        SWITCH_MIN_DURATION_S,
    );

    // ── Per-task time = your foreground time + autonomous agent time ───────────
    let mut task_fg: BTreeMap<String, Vec<Interval>> = BTreeMap::new();
    let mut task_agent: BTreeMap<String, Vec<Interval>> = BTreeMap::new();
    for r in &all_rows {
        let (Some(k), Some("task")) = (r.task_key.as_deref(), r.session_type.as_deref()) else {
            continue;
        };
        let iv = session_interval(
            &r.started_at,
            &r.ended_at,
            r.duration_s,
            r.claude_session_uuid.as_deref(),
        );
        if r.claude_session_uuid.is_none() {
            task_fg.entry(k.to_string()).or_default().push(iv);
        } else {
            task_agent.entry(k.to_string()).or_default().push(iv);
        }
    }
    let mut task_totals: BTreeMap<String, i64> = BTreeMap::new();
    let mut task_autonomous_s: BTreeMap<String, i64> = BTreeMap::new();
    let keys: std::collections::BTreeSet<String> =
        task_fg.keys().chain(task_agent.keys()).cloned().collect();
    for k in keys {
        let your_s = union_seconds(task_fg.get(&k).map(|v| v.as_slice()).unwrap_or(&[]));
        let agent_iv = task_agent.get(&k).map(|v| v.as_slice()).unwrap_or(&[]);
        let auto_s =
            (union_seconds(agent_iv) - intersect_seconds(agent_iv, &presence_segments)).max(0);
        task_totals.insert(k.clone(), your_s + auto_s);
        task_autonomous_s.insert(k, auto_s);
    }
    let engaged_s = focus_s + autonomous_s;

    // ── Task metadata for keys touched today ───────────────────────────────────
    let today_keys: std::collections::BTreeSet<String> =
        all_rows.iter().filter_map(|r| r.task_key.clone()).collect();
    let mut task_meta: BTreeMap<String, TaskMeta> = BTreeMap::new();
    let meta_rows =
        sqlx::query_as::<_, TaskMetaRow>(r#"SELECT task_key, title, provider, url FROM pm_tasks"#)
            .fetch_all(pool)
            .instrument(tracing::debug_span!("today.read.pm_tasks"))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "today: pm_tasks read failed, no task metadata");
                Vec::new()
            });
    tracing::debug!(rows = meta_rows.len(), "today.read.pm_tasks");
    for t in meta_rows {
        if !today_keys.contains(&t.task_key) {
            continue;
        }
        let key = t.task_key.clone();
        task_meta.insert(
            key.clone(),
            TaskMeta {
                title: t.title.filter(|s| !s.is_empty()).unwrap_or(key),
                provider: t
                    .provider
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "jira".to_string()),
                url: t.url.unwrap_or_default(),
            },
        );
    }

    // ── Coding-agent summaries per task ─────────────────────────────────────────
    let mut task_agent_summaries: BTreeMap<String, Vec<AgentSummary>> = BTreeMap::new();
    for r in all_rows.iter().filter(|r| r.claude_session_uuid.is_some()) {
        if let (Some(k), Some(summary)) = (r.task_key.as_deref(), r.session_summary.as_deref()) {
            if summary.is_empty() {
                continue;
            }
            task_agent_summaries
                .entry(k.to_string())
                .or_default()
                .push(AgentSummary {
                    started_at: r.started_at.clone(),
                    dur: r.duration_s,
                    summary: summary.to_string(),
                });
        }
    }

    let session_count = sessions.len() as i64 + active.is_some() as i64;

    tracing::info!(
        sessions = sessions.len(),
        session_count,
        focus_s,
        idle_s,
        agent_s,
        supervised_s,
        autonomous_s,
        switch_count,
        gaps = gaps.len(),
        tasks = task_totals.len(),
        "today computed"
    );

    Ok(TodayResponse {
        date: date.to_string(),
        sessions,
        active,
        gaps,
        focus_s,
        idle_s,
        agent_s,
        supervised_s,
        autonomous_s,
        presence_segments,
        agent_segments,
        session_count,
        switch_count,
        task_totals,
        task_autonomous_s,
        engaged_s,
        task_meta,
        task_agent_summaries,
    })
}
