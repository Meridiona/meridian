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
use std::time::{Duration, Instant};

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

/// Hard bound on the single in-process embedding forward pass. An hour with an unusually
/// large volume of on-screen text (many sessions, little repetition) can push the lexically-
/// deduped span batch well beyond a typical hour's size; on CPU-only inference the forward
/// pass cost grows with batch size, so an outsized hour can take minutes rather than seconds.
/// Hours run strictly sequentially, so an unbounded embed here would stall every later hour
/// behind it. Timing out degrades this one hour to lexical-only (same degrade path as a load
/// failure) rather than blocking the queue — see [`embed_lex`].
static EMBED_TIMEOUT: Lazy<Duration> =
    Lazy::new(|| Duration::from_secs_f64(env_num("DISTILLER_EMBED_TIMEOUT_SECS", 210.0)));

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

/// One retry after a real (non-timeout) error — covers transient failures (a momentary
/// file-read glitch loading weights, a poisoned-mutex hiccup on first touch) that a second
/// attempt can plausibly clear. Deliberately NOT retried on the caller's timeout branch:
/// a timeout means the computation itself is too slow for the input size, and an identical
/// retry would just re-run the same slow computation and cost another full timeout budget
/// for no chance of a different outcome — that path degrades immediately instead.
const EMBED_MAX_ATTEMPTS: u32 = 2;
const EMBED_RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Embed the lexically-deduped span lines. Gated on [`embedder::is_ready`]; a failure
/// degrades to empty vectors (SemDeDup + floc become lexical-only). Times the stage under
/// a `distil.embed` child span. An empty `lex` short-circuits to empty vectors without
/// touching the embedder at all — nothing to embed is not a failure, so it's excluded from
/// the `degraded` flag below.
async fn embed_lex(lex: &[Span]) -> (Vec<Vec<f32>>, bool) {
    let span = tracing::debug_span!("distil.embed", n_spans = lex.len());
    async {
        let start = Instant::now();
        if lex.is_empty() {
            tracing::debug!(
                n_spans = 0,
                elapsed_ms = 0,
                degraded = false,
                "distil: embedded spans"
            );
            return (Vec::new(), false);
        }
        let vecs = if embedder::is_ready() {
            let n = lex.len();
            // Retries share ONE outer timeout budget rather than each getting their own —
            // a retry that would blow past EMBED_TIMEOUT is still bounded by it, so the
            // worst case for this hour is exactly EMBED_TIMEOUT, never attempts * timeout.
            let attempts = async {
                let mut last_err = None;
                for attempt in 1..=EMBED_MAX_ATTEMPTS {
                    let texts: Vec<String> = lex.iter().map(|s| s.line.clone()).collect();
                    match embedder::embed_batch(texts).await {
                        Ok(v) => return Ok(v),
                        Err(e) => {
                            tracing::warn!(
                                attempt,
                                max_attempts = EMBED_MAX_ATTEMPTS,
                                error = %e,
                                "distil: embed attempt failed"
                            );
                            last_err = Some(e);
                            if attempt < EMBED_MAX_ATTEMPTS {
                                tokio::time::sleep(EMBED_RETRY_BACKOFF).await;
                            }
                        }
                    }
                }
                Err(last_err.expect("loop ran at least once"))
            };
            match tokio::time::timeout(*EMBED_TIMEOUT, attempts).await {
                Ok(result) => {
                    // This is the only embed_batch call in the hour's whole run (one
                    // distil pass = one batch), so the model has no more work until
                    // next hour — drop it now rather than leaving it resident in
                    // memory until then.
                    embedder::unload();
                    match result {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                attempts = EMBED_MAX_ATTEMPTS,
                                "distil: embed failed after all retries — degrading to lexical-only"
                            );
                            Vec::new()
                        }
                    }
                }
                Err(_) => {
                    // The abandoned spawn_blocking task keeps running to completion on
                    // its own thread (not cancelled) and will release the embedder mutex
                    // when done — the NEXT hour's embed_batch call just waits on it like
                    // any other lock contention. What matters is THIS hour's async
                    // pipeline is freed to continue rather than stalling the sequential
                    // hour queue behind an outsized batch.
                    tracing::warn!(
                        n_spans = n,
                        timeout_s = EMBED_TIMEOUT.as_secs_f64(),
                        "distil: embed timed out — degrading to lexical-only"
                    );
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

#[cfg(test)]
mod bench {
    //! Manual diagnostic, not part of CI — measures the embedder's own memory/time cost
    //! against REAL session data for one hour on this machine, to compare against the
    //! Python predecessor's ~1 GB / batch_size=32 behaviour after the chunking fix.
    //! Run: `cargo test --release -p meridian distiller::bench::embed_only_bench -- \
    //! --ignored --nocapture` (optionally with `HOUR_START`/`HOUR_END`/`MERIDIAN_DB` env
    //! overrides; defaults to the heavy hour this fix was diagnosed against).
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    fn rss_kb(pid: u32) -> u64 {
        std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    #[tokio::test]
    #[ignore]
    async fn embed_only_bench() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        let db_path = std::env::var("MERIDIAN_DB").unwrap_or_else(|_| {
            format!(
                "{}/.meridian/meridian.db",
                std::env::var("HOME").expect("HOME set")
            )
        });
        let hs =
            std::env::var("HOUR_START").unwrap_or_else(|_| "2026-07-18T10:30:00+00:00".to_string());
        let he =
            std::env::var("HOUR_END").unwrap_or_else(|_| "2026-07-18T11:30:00+00:00".to_string());

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&format!("sqlite://{db_path}"))
            .await
            .expect("connect to real meridian.db");

        embedder::ensure_weights()
            .await
            .expect("embedder weights present");
        assert!(embedder::is_ready(), "embedder must be ready to bench it");

        let rows = rows::load_sessions(&pool, &hs, &he).await;
        assert!(
            !rows.is_empty(),
            "no sessions in [{hs}, {he}) — pick an hour that has data via HOUR_START/HOUR_END"
        );
        println!("loaded {} sessions for [{hs}, {he})", rows.len());

        // The model unloads again (inside embed_lex) before distil() returns, so a
        // point-in-time RSS sample taken after the call would miss the peak — poll in
        // a background thread for the call's duration and track the high-water mark.
        let pid = std::process::id();
        let peak_kb = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (peak_clone, stop_clone) = (Arc::clone(&peak_kb), Arc::clone(&stop));
        let sampler = std::thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                peak_clone.fetch_max(rss_kb(pid), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(50));
            }
        });

        let rss_before = rss_kb(pid);
        let t0 = Instant::now();
        let (body, stats) = distil(rows, "HOUR 10:00", "bench").await;
        let elapsed = t0.elapsed();

        stop.store(true, Ordering::Relaxed);
        sampler.join().ok();
        let rss_after = rss_kb(pid);

        println!("=== embed-only bench ===");
        println!(
            "nsess={} n_after_junk={} n_after_df={} n_after_lex={} n_after_sem={} n_selected={}",
            stats.nsess,
            stats.n_after_junk,
            stats.n_after_df,
            stats.n_after_lex,
            stats.n_after_sem,
            stats.n_selected
        );
        println!(
            "total distil() time (all stages, embed included): {:.2}s",
            elapsed.as_secs_f64()
        );
        println!(
            "rss before: {} MB, peak during: {} MB (delta {} MB), after (post-unload): {} MB",
            rss_before / 1024,
            peak_kb.load(Ordering::Relaxed) / 1024,
            (peak_kb.load(Ordering::Relaxed).saturating_sub(rss_before)) / 1024,
            rss_after / 1024
        );
        println!(
            "body preview: {}",
            &body.chars().take(200).collect::<String>()
        );
    }
}
