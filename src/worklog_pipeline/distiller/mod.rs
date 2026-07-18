//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Session distiller — compress an hour of `app_sessions` into a structured,
//! noise-reduced activity excerpt for the hour report.
//!
//! Runs fully in-process — the one capability with no CLI-provider equivalent, so it
//! stays local. ~85-92% char reduction while preserving named facts (ticket keys, PR
//! numbers, file paths), via an 11-stage pipeline: segment → junk gate → prose gate →
//! DF cut → lexical dedup → SemDeDup → facility-location → entity rescue →
//! empty-session rescue → format → header/stats.
//!
//! # Embedder degradation
//! SemDeDup + facility-location's diversity path are the only stages needing the
//! [`crate::embedder`]. When it is not ready (first-run weights still downloading, or a
//! load failure), those stages degrade to no-op / longest-first — the hour still yields
//! a (lower-reduction) body, never an error.
//!
//! # Who calls this
//! [`crate::worklog_pipeline::hour::run_hour`] via [`distil_hour`], in-process — it
//! replaced the HTTP POST to the Python `/distill_hour`.
//!
//! # Observability
//! Emits a `distil.run` span + a structured `tracing::info!` carrying the same
//! `distil_*` attributes the Python root span/log set, so the OpenObserve distiller
//! dashboards keep rendering. A `distil.embed` child times the embedding stage.

mod dedup;
mod format;
mod rows;
mod segment;
mod select;

use std::collections::HashMap;
use std::time::Instant;

use once_cell::sync::Lazy;
use regex::Regex;
use sqlx::SqlitePool;
use tracing::Instrument;

use crate::embedder;
use meridian_core::date::utc_to_local_hhmm;

use segment::{
    clean_window_title, fails_prose_gate, is_fallback_candidate, is_hard_junk, norm, segment,
    strip_url_prefix,
};
use select::Item;

/// Cosine-similarity dot product for two L2-normalized embedding vectors — shared by
/// [`dedup`]'s SemDeDup mask and [`select`]'s facility-location diversity pick, the two
/// stages that compare embeddings.
pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ── Tunables (env-overridable) ──────────────────────────────────────────────────

/// Coding-agent apps whose transcripts are folded in as clean summaries elsewhere, so
/// they never reach this OCR-tuned compressor.
pub(crate) const EXCLUDE_APPS: [&str; 4] =
    ["Claude Code", "Codex", "GitHub Copilot", "Cursor Agent"];

/// Sessions shorter than this many seconds are dropped as alt-tab flicker.
pub(crate) static MIN_SESSION_DUR_S: Lazy<i64> =
    Lazy::new(|| env_num("DISTILLER_MIN_SESSION_DUR", 15.0) as i64);
/// Cosine threshold above which SemDeDup treats two spans as near-duplicates.
static SEM_DEDUP_THR: Lazy<f64> = Lazy::new(|| env_num("DISTILLER_SEM_DEDUP_THR", 0.86));
/// A span recurring across this fraction of sessions is boilerplate (DF cut).
static DF_FRAC: Lazy<f64> = Lazy::new(|| env_num("DISTILLER_DF_FRAC", 0.25));

/// Per-session facility-location span-count floor / ceiling, and entity-rescue cap.
const FLOOR: usize = 3;
const CEIL: usize = 14;
pub(crate) const ENTITY_RESCUE_CAP: usize = 4;

fn env_num(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `[HH:MM:SS] <rest>` line marker — a UTC time-of-day the capture stamps on each line.
static MARKER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[(\d\d:\d\d:\d\d)\]\s*(.*)").expect("MARKER_RE"));

// ── Shared span type ────────────────────────────────────────────────────────────

/// One kept on-screen text span, tagged with its session, app, window, and local time.
#[derive(Clone)]
pub(crate) struct Span {
    pub sid: i64,
    pub app: String,
    pub win: String,
    pub t: String,
    pub line: String,
}

/// Per-run metrics — the same set the Python `DistilStats` carried, surfaced on the
/// `distil.run` span and log.
#[derive(Debug, Clone, Default)]
pub struct DistilStats {
    pub label: String,
    pub nsess: usize,
    pub raw_chars: usize,
    pub out_chars: usize,
    pub reduction_pct: f64,
    pub n_after_junk: usize,
    pub n_after_df: usize,
    pub n_after_lex: usize,
    pub n_after_sem: usize,
    pub n_selected: usize,
    pub n_session_rescued: usize,
    pub n_entity_rescued: usize,
    pub elapsed_s: f64,
}

// ── Public API ──────────────────────────────────────────────────────────────────

/// Compress one local calendar hour of screen sessions into a structured excerpt.
///
/// `hs`/`he` are the hour's UTC `+00:00` bounds (as the driver computes them); `hour` is
/// the `YYYY-MM-DDTHH` local label, used only for the header/label. Returns
/// `("", empty stats)` for an hour with no qualifying sessions.
pub async fn distil_hour(
    pool: &SqlitePool,
    hs: &str,
    he: &str,
    hour: &str,
) -> (String, DistilStats) {
    let rows = rows::load_sessions(pool, hs, he).await;
    if rows.is_empty() {
        tracing::warn!(hour, "distil: no sessions for hour");
        return (
            String::new(),
            DistilStats {
                label: hour.to_string(),
                ..Default::default()
            },
        );
    }
    let header_prefix = format!("HOUR {}:00", hour.get(11..).unwrap_or(""));
    let run_span = tracing::info_span!("distil.run", hour);
    async {
        let (body, stats) = distil(rows, &header_prefix, hour).await;
        record_run(&stats, &body);
        (body, stats)
    }
    .instrument(run_span)
    .await
}

// ── Core pipeline ───────────────────────────────────────────────────────────────

async fn distil(
    rows: Vec<rows::SessionRow>,
    header_prefix: &str,
    label: &str,
) -> (String, DistilStats) {
    let t_start = Instant::now();
    let nsess = rows.len();
    let raw_chars: usize = rows.iter().map(|r| r.session_text.chars().count()).sum();

    // ── stages 1-3: build spans + per-session fallback + meta ────────────────────
    let mut spans: Vec<Span> = Vec::new();
    let mut fallback_by_sid: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    // sid → (app, window, first local HH:MM)
    let mut meta: HashMap<i64, (String, String, String)> = HashMap::new();

    for r in &rows {
        let window = clean_window_title(&pick_window(&r.window_titles));
        let t0 = utc_to_local_hhmm(&r.started_at);
        meta.insert(r.id, (r.app_name.clone(), window.clone(), t0.clone()));

        let mut fallback: Vec<(String, String)> = Vec::new();
        let mut cur_time = t0.clone();
        let marker_date = r.started_at.split('T').next().unwrap_or("");
        for raw_line in r.session_text.lines() {
            let mut line = raw_line.to_string();
            if let Some(caps) = MARKER_RE.captures(raw_line) {
                cur_time = utc_to_local_hhmm(&format!("{marker_date}T{}", &caps[1]));
                line = caps[2].to_string();
            }
            let line = strip_url_prefix(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            for seg in segment(line) {
                if is_fallback_candidate(&seg) {
                    fallback.push((cur_time.clone(), seg.clone()));
                }
                if is_hard_junk(&seg) || fails_prose_gate(&seg) {
                    continue;
                }
                spans.push(Span {
                    sid: r.id,
                    app: r.app_name.clone(),
                    win: window.clone(),
                    t: cur_time.clone(),
                    line: seg,
                });
            }
        }
        fallback_by_sid.insert(r.id, fallback);
    }
    let n_after_junk = spans.len();

    // ── stages 4-6: DF cut → lexical dedup → SemDeDup ────────────────────────────
    let spans = dedup::df_cut(spans, nsess, *DF_FRAC);
    let n_after_df = spans.len();
    let lex = dedup::lexical_dedup(spans);
    let n_after_lex = lex.len();

    let (vecs, degraded) = embed_lex(&lex).await;
    let keep = dedup::sem_dedup_mask(&lex, &vecs, *SEM_DEDUP_THR);
    let mut sem: Vec<Span> = Vec::new();
    let mut v_sem: Vec<Vec<f32>> = Vec::new();
    for (i, sp) in lex.into_iter().enumerate() {
        if keep[i] {
            if !vecs.is_empty() {
                v_sem.push(vecs[i].clone());
            }
            sem.push(sp);
        }
    }
    let n_after_sem = sem.len();

    // ── stages 7-9: per-session floc pick, entity rescue, empty rescue ───────────
    let mut by_sid: HashMap<i64, Vec<Item>> = HashMap::new();
    for (i, sp) in sem.iter().enumerate() {
        let vec = if v_sem.is_empty() {
            None
        } else {
            Some(v_sem[i].clone())
        };
        by_sid.entry(sp.sid).or_default().push(Item {
            span: sp.clone(),
            vec,
        });
    }

    let mut selected: Vec<Span> = Vec::new();
    let mut discarded_by_sid: Vec<(i64, Vec<Span>)> = Vec::new();
    let mut n_session_rescued = 0usize;

    for r in &rows {
        let sid = r.id;
        let session_items = by_sid.remove(&sid).unwrap_or_default();
        if !session_items.is_empty() {
            let cap = FLOOR.max(CEIL.min(session_items.len()));
            let (picked, discarded) = select::floc_pick(session_items, cap);
            selected.extend(picked);
            discarded_by_sid.push((sid, discarded));
        } else {
            let (app, window, t0) = meta.get(&sid).cloned().unwrap_or_default();
            let mut fb = fallback_by_sid.get(&sid).cloned().unwrap_or_default();
            fb.sort_by_key(|(_, line)| std::cmp::Reverse(line.chars().count()));
            let mut seen2 = std::collections::HashSet::new();
            let mut added = 0usize;
            for (t, line) in fb {
                let key: String = norm(&line).chars().take(80).collect();
                if !seen2.insert(key) {
                    continue;
                }
                selected.push(Span {
                    sid,
                    app: app.clone(),
                    win: window.clone(),
                    t,
                    line,
                });
                added += 1;
                if added >= 2 {
                    break;
                }
            }
            if added > 0 {
                n_session_rescued += 1;
            } else {
                selected.push(Span {
                    sid,
                    app: app.clone(),
                    win: window.clone(),
                    t: t0,
                    line: format!("(no readable on-screen text; {app} window '{window}')"),
                });
            }
        }
    }

    let (selected, n_entity_rescued) = select::entity_rescue(selected, &discarded_by_sid);
    let n_selected = selected.len();

    // ── stage 10: format ─────────────────────────────────────────────────────────
    let span_start = utc_to_local_hhmm(&rows[0].started_at);
    let span_end = utc_to_local_hhmm(&rows[rows.len() - 1].started_at);
    let active_mins: i64 = rows.iter().map(|r| r.duration_s).sum::<i64>() / 60;
    let body = format::render(
        selected,
        header_prefix,
        nsess,
        active_mins,
        &span_start,
        &span_end,
    );

    let out_chars = body.chars().count();
    let reduction_pct =
        (100.0 * (1.0 - out_chars as f64 / raw_chars.max(1) as f64) * 10.0).round() / 10.0;
    let stats = DistilStats {
        label: label.to_string(),
        nsess,
        raw_chars,
        out_chars,
        reduction_pct,
        n_after_junk,
        n_after_df,
        n_after_lex,
        n_after_sem,
        n_selected,
        n_session_rescued,
        n_entity_rescued,
        elapsed_s: t_start.elapsed().as_secs_f64(),
    };
    let _ = degraded; // recorded on the distil.embed span; not a run-level stat
    (body, stats)
}

/// Embed the lexically-deduped span lines. Gated on [`embedder::is_ready`]; a failure
/// degrades to empty vectors (SemDeDup + floc become lexical-only). Times the stage under
/// a `distil.embed` child span.
async fn embed_lex(lex: &[Span]) -> (Vec<Vec<f32>>, bool) {
    let span = tracing::debug_span!("distil.embed", n_spans = lex.len());
    async {
        let start = Instant::now();
        let vecs = if embedder::is_ready() && !lex.is_empty() {
            let texts: Vec<String> = lex.iter().map(|s| s.line.clone()).collect();
            match embedder::embed_batch(&texts).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "distil: embed failed — degrading to lexical-only");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let degraded = vecs.is_empty();
        tracing::debug!(
            n_spans = lex.len(),
            elapsed_ms = (start.elapsed().as_secs_f64() * 1000.0) as i64,
            degraded,
            "distil: embedded spans"
        );
        (vecs, degraded)
    }
    .instrument(span)
    .await
}

/// Pick the most-frequent window title out of the `window_titles` JSON blob
/// (`[{"window_name": …, "count": n}, …]`). Empty/malformed → `""`.
fn pick_window(window_titles_json: &str) -> String {
    let val: serde_json::Value =
        serde_json::from_str(window_titles_json).unwrap_or(serde_json::Value::Null);
    val.as_array()
        .and_then(|arr| {
            arr.iter().max_by_key(|d| {
                d.get("count")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0)
            })
        })
        .and_then(|d| d.get("window_name").and_then(serde_json::Value::as_str))
        .unwrap_or("")
        .to_string()
}

/// Stamp every `distil_*` attribute on the current `distil.run` span AND emit the
/// structured `tracing::info!` the OpenObserve distiller dashboards query. Every field is
/// recorded always (including zeros) so a dashboard filter never errors on a missing key.
fn record_run(s: &DistilStats, body: &str) {
    let preview: String = body.chars().take(400).collect();
    tracing::info!(
        distil_label = %s.label,
        distil_nsess = s.nsess,
        distil_raw_chars = s.raw_chars,
        distil_out_chars = s.out_chars,
        distil_input_tokens_est = s.raw_chars / 4,
        distil_output_tokens_est = s.out_chars / 4,
        distil_reduction_pct = s.reduction_pct,
        distil_n_after_junk = s.n_after_junk,
        distil_n_after_df = s.n_after_df,
        distil_n_after_lex = s.n_after_lex,
        distil_n_after_sem = s.n_after_sem,
        distil_n_selected = s.n_selected,
        distil_n_session_rescued = s.n_session_rescued,
        distil_n_entity_rescued = s.n_entity_rescued,
        distil_elapsed_s = s.elapsed_s,
        distil_body_preview = %preview,
        "session_distiller: distilled hour"
    );
}
