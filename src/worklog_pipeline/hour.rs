//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The Rust-owned hour report — distil locally, summarise through the user's chosen AI.
//!
//! The hour is orchestrated here so its generative work goes through the centralised
//! provider layer ([`crate::llm`]) — the user's chosen AI (Claude / Codex / Cursor /
//! Copilot) writes the hourly summary, per the "one global choice for everything except
//! coding-agent summaries" design.
//!
//! **Distillation runs fully in-process** ([`super::distiller`]): a pure-Rust embedder
//! ([`crate::embedder`]) compresses the hour's screen OCR before the summary call — no
//! external service, no HTTP hop.
//!
//! The report is a SINGLE generative call (see [`crate::llm::prompts::ACTIVITY_REPORT`]):
//! the model returns the activities as prose (no clock times in the words) AND a minute
//! estimate per activity, together in one structured answer. Code
//! ([`super::hour_input::assemble_report`]) then clamps each estimate to the hour's real
//! measured span and fills any omission — the model's minutes are a ratio, the total is
//! ours. (This replaced an earlier two-call split — summary, then times — which existed
//! because the weak on-device 2B mixed prose quality with invented durations; a capable
//! provider plus a structured `minutes` field removes that failure, so one call suffices.)

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::Instrument;

use crate::llm::{self, prompts, LlmProvider, PromptRequest};
use crate::pm_worklog::PmWorklogConfig;

use super::hour_db;
use super::hour_input::{
    assemble_report, compose_report_input, hour_span_minutes, parse_activities,
};
use super::{distiller, task_db, workstream};

/// Token ceiling for the single report call: "under 300 words" of prose plus a short
/// per-activity minutes list, as one JSON object. Generous enough not to truncate.
/// `pub(crate)` so the LLM-Lab replay ([`crate::llm_experiment`]) rebuilds the identical
/// request contract for this process.
pub(crate) const REPORT_MAX_TOKENS: u32 = 1536;

/// The embedder-compressed hour body + a few metrics the pipeline reports on.
struct Distilled {
    body: String,
    out_chars: i64,
    reduction_pct: f64,
    nsess: i64,
}

/// Distil the hour's screen sessions in-process via [`super::distiller`] (a pure-Rust
/// port of the old Python embedder path). Never fails — an empty hour or an unavailable
/// embedder yields an empty / lower-reduction body rather than an error.
async fn distill(pool: &SqlitePool, hs: &str, he: &str, hour: &str) -> Distilled {
    let (body, stats) = distiller::distil_hour(pool, hs, he, hour).await;
    Distilled {
        body,
        out_chars: stats.out_chars as i64,
        reduction_pct: stats.reduction_pct,
        nsess: stats.nsess as i64,
    }
}

/// The single hourly report call: a short, high-level, plain-text summary of the hour —
/// one `"<minutes> min  <activity>"` line per activity. Through the user's chosen provider
/// (with the resolver's retry / rate-limit / on-device fallback).
///
/// Plain text, no schema: every provider rides the same prompt contract and the answer is
/// parsed by [`parse_report`]. Returns the ordered activity prose and a 1-based
/// `position -> minutes` map; a line with no leading minute count is skipped, and if
/// NOTHING parses it falls back to reading numbered prose with no minutes — so the hour
/// never fails outright and [`assemble_report`] still owns the final numbers.
#[allow(clippy::type_complexity)]
async fn build_report(
    report_input: String,
    label: &str,
) -> Result<(
    Vec<String>,
    std::collections::BTreeMap<usize, i64>,
    std::collections::BTreeMap<usize, String>,
    LlmProvider,
)> {
    let req = report_request(report_input, label);
    let (out, provider) = llm::complete(&req)
        .await
        .map_err(|e| anyhow::anyhow!("activity report failed: {e}"))?;
    let (activities, minutes, stamps) = parse_report(&out.text);
    Ok((activities, minutes, stamps, provider))
}

/// The hour report's exact [`PromptRequest`] — extracted from [`build_report`] so the
/// LLM-Lab replay ([`crate::llm_experiment`]) fans the byte-identical request across
/// arbitrary providers.
pub(crate) fn report_request(report_input: String, label: &str) -> PromptRequest {
    PromptRequest {
        system: prompts::ACTIVITY_REPORT,
        user: report_input,
        // No schema: the report is plain text now — one "<minutes> min  <activity>" line
        // per activity, parsed by `parse_report`. Dropping `--json-schema` removes the
        // constrained-decoding cost, and `assemble_report` still owns/clamps the minutes,
        // so code keeps the time even though the model writes it inline.
        schema: None,
        max_tokens: REPORT_MAX_TOKENS,
        label: format!("activity-report {label}"),
        interactive: false,
    }
}

/// Parse the plain-text report into ordered activity prose + a 1-based `position -> minutes`
/// map + a 1-based `position -> time` map. Each line the model emits is
/// `"<HH:MM-HH:MM>  <minutes> min  <activity>"` (a rough local start-end range, copied from
/// the timeline, for downstream attachment); a stray leading list marker ("1. ", "- ") is
/// tolerated and the time is optional (a start-only `HH:MM` also parses). A line with no
/// leading minute count is skipped
/// (a header, blank line, or trailing note). If NOTHING parses, falls back to reading the
/// text as numbered prose (minutes/stamps empty, so [`assemble_report`] even-splits the
/// span) — the hour never fails outright.
pub(crate) fn parse_report(
    text: &str,
) -> (
    Vec<String>,
    std::collections::BTreeMap<usize, i64>,
    std::collections::BTreeMap<usize, String>,
) {
    let mut activities = Vec::new();
    let mut minutes = std::collections::BTreeMap::new();
    let mut stamps = std::collections::BTreeMap::new();
    for raw in text.lines() {
        if let Some((stamp, mins, prose)) = parse_report_line(raw) {
            activities.push(prose);
            // 1-based index of the activity we just pushed.
            let i = activities.len();
            minutes.insert(i, mins.max(1));
            if let Some(s) = stamp {
                stamps.insert(i, s);
            }
        }
    }
    if activities.is_empty() {
        return (
            parse_activities(text),
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
        );
    }
    (activities, minutes, stamps)
}

/// Parse one `"<HH:MM-HH:MM>  <minutes> min  <activity>"` line into `(Some(stamp), minutes,
/// prose)`. The leading time is optional (older/looser answers omit it → `None`) and may be
/// a single `HH:MM` start or a `HH:MM-HH:MM` range.
/// Tolerates a leading list marker the model may add against instructions ("1. ", "1) ",
/// "- ", "* "), a literal `<...>` wrapped around the timestamp (the prompt's
/// `<HH:MM-HH:MM>` is prompt-writing notation for "fill this in" — Claude omits the
/// brackets, but cursor-agent 2026.06.04 was observed emitting them literally) — the
/// bracket tolerance lives inside [`take_hhmm`] and fires ONLY around a real stamp, so a
/// bracketed non-time aside is not turned into a phantom entry — the unit written as
/// `min` / `mins` / `minutes`, and arbitrary surrounding whitespace. Returns `None` for
/// any line without a leading minute count so headers and notes drop.
fn parse_report_line(raw: &str) -> Option<(Option<String>, i64, String)> {
    let s = strip_list_marker(raw.trim());
    let (stamp, s) = take_hhmm(s);
    let s = s.trim_start();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let mins: i64 = digits.parse().ok()?;
    let rest = s[digits.len()..].trim_start();
    // Consume the unit word and require a word boundary after it, so "minimal" (or any
    // prose word starting with "min") can't be mistaken for the duration unit.
    let lower = rest.to_ascii_lowercase();
    let unit = ["minutes", "mins", "min"]
        .into_iter()
        .find(|u| lower.starts_with(u))?;
    let after = &rest[unit.len()..];
    if after.chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let prose = after.split_whitespace().collect::<Vec<_>>().join(" ");
    if prose.is_empty() {
        return None;
    }
    Some((stamp, mins, prose))
}

/// Split a leading local time off the front of a line: either a single `HH:MM` or a
/// `HH:MM-HH:MM` start-end range (1-2 digit hour, `:`, 2-digit minute each). Returns
/// `(Some(span), rest)` with `span` the matched string verbatim (brackets stripped), or
/// `(None, original)` if the line doesn't start with one. The `-HH:MM` end is optional — a
/// looser answer that gives only a start still parses. Purely lexical — it does not
/// validate the clock values (a rough time is all downstream needs).
///
/// A literal `<…>` wrapper is tolerated but ONLY when it actually wraps a real `HH:MM`:
/// the leading `<` is consumed only if an `HH:MM` follows it, and the trailing `>` only if
/// the `<` was consumed. This is deliberately gated so a bracketed non-time aside such as
/// `"<15 min gap, no activity>"` is returned untouched (`None`, original) instead of being
/// unwrapped into something the minute parser below would misread as an entry.
fn take_hhmm(s: &str) -> (Option<String>, &str) {
    let had_open = s.starts_with('<');
    let body = if had_open { &s[1..] } else { s };
    let Some(start_end) = hhmm_len(body) else {
        // No real stamp — leave the original untouched, brackets and all.
        return (None, s);
    };
    // Optional "-HH:MM" end.
    let tail = &body[start_end..];
    let (stamp, rest) = match tail
        .strip_prefix('-')
        .and_then(|r| hhmm_len(r).map(|l| (r, l)))
    {
        Some((_, end_len)) => {
            let end = start_end + 1 + end_len;
            (body[..end].to_string(), &body[end..])
        }
        None => (body[..start_end].to_string(), &body[start_end..]),
    };
    // Consume the closing bracket only if we consumed the opening one.
    let rest = if had_open {
        rest.strip_prefix('>').unwrap_or(rest)
    } else {
        rest
    };
    (Some(stamp), rest)
}

/// Byte length of a leading `HH:MM` (1-2 digit hour, `:`, 2-digit minute), or `None` if `s`
/// does not start with one. Shared by [`take_hhmm`] for both the start and the optional end.
fn hhmm_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut h = 0;
    while h < b.len() && h < 2 && b[h].is_ascii_digit() {
        h += 1;
    }
    if h == 0 || h >= b.len() || b[h] != b':' {
        return None;
    }
    let mstart = h + 1;
    let mut m = 0;
    while mstart + m < b.len() && m < 2 && b[mstart + m].is_ascii_digit() {
        m += 1;
    }
    if m != 2 {
        return None;
    }
    Some(mstart + m)
}

/// Strip a single leading list marker — "1. ", "12) ", "- ", or "* " — if present; the
/// prompt bans them, but a model occasionally adds one anyway. Nothing else is touched.
fn strip_list_marker(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
        return rest.trim_start();
    }
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &s[digits.len()..];
        if let Some(rest) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            return rest.trim_start();
        }
    }
    s
}

/// Process one completed local hour end to end.
///
/// `hs` / `he` are the hour's UTC bounds (`…+00:00`), as the driver already computes them;
/// `hs` doubles as the `pm_worklog_hours` ledger key for persistence. Returns an error only
/// on a real fault (distil unreachable, summary provider down with no fallback) so the
/// driver leaves the hour pending and retries; a legitimately empty hour is `Ok(())`.
pub async fn run_hour(
    pool: &SqlitePool,
    // `_cfg` / `_db_path` are retained only for signature stability with the driver; the
    // in-process distiller uses `pool`/`hs`/`he` (they were needed by the old HTTP call).
    _cfg: &PmWorklogConfig,
    _db_path: &str,
    hour: &str,
    hs: &str,
    he: &str,
) -> Result<()> {
    // ── distil (in-process embedder) ─────────────────────────────────────────
    let sess_span =
        tracing::info_span!("worklog.sessions", hour, body_chars = tracing::field::Empty);
    let d = distill(pool, hs, he, hour)
        .instrument(sess_span.clone())
        .await;
    let coding = hour_db::fetch_coding_summaries(pool, hs, he).await;
    let timeline = hour_db::fetch_hour_timeline(pool, hs, he).await;
    sess_span.record("body_chars", d.body.len());
    link_session_formation_traces(&timeline);

    // Burst filter + block rendering + the 3-part join live in `compose_report_input`
    // (shared with the LLM-Lab replay); `timeline` (full set) still feeds the span/time
    // math below, so a burst loses its place in the prose without losing its minutes.
    let composed = compose_report_input(&d.body, &timeline, &coding);
    tracing::info!(
        hour,
        ocr_nsess = d.nsess,
        coding_nsess = composed.coding_kept,
        coding_dropped_short = composed.coding_dropped,
        timeline_dropped_short = composed.timeline_dropped,
        body_chars = d.body.len(),
        "worklog: hour distilled"
    );

    // Persist the distilled INPUT for the dashboard's hour-detail panel, for EVERY hour
    // that distils — not only ones that produce a report.
    hour_db::persist_hour_text(pool, hs, &d.body, d.out_chars, d.reduction_pct).await?;

    // A truly empty hour: nothing to summarise. Persist an empty report and finish clean.
    if d.body.trim().is_empty() && composed.coding_block_empty {
        tracing::info!(hour, "worklog: hour has no sessions — skipping report");
        hour_db::persist_hour_report(pool, hs, "").await?;
        return Ok(());
    }

    // ── report (global provider) ─────────────────────────────────────────────
    let report_span = tracing::info_span!(
        "worklog.report",
        hour,
        llm_provider = tracing::field::Empty,
        n_activities = tracing::field::Empty,
        // 1 (activity-summary only) or 2 (+ activity-time) — matches the number of
        // child `llm.call` spans under this one.
        n_calls = tracing::field::Empty
    );
    let coding_folded = !composed.coding_block_empty;
    let report_input = composed.text;
    let report = async {
        if report_input.trim().is_empty() {
            return Ok::<String, anyhow::Error>(String::new());
        }

        // ONE call — activities (prose) + per-activity minutes + local start times, together.
        let (activities, minutes, stamps, provider) = build_report(report_input, hour).await?;
        tracing::Span::current().record("llm_provider", provider.as_str());
        tracing::Span::current().record("n_activities", activities.len());
        tracing::Span::current().record("n_calls", 1);

        Ok(assemble_report(
            &activities,
            &minutes,
            &stamps,
            hour_span_minutes(&timeline),
        ))
    }
    .instrument(report_span.clone())
    .await?;

    tracing::info!(
        hour,
        report_chars = report.chars().count(),
        coding_folded,
        "worklog: hour report built"
    );
    hour_db::persist_hour_report(pool, hs, &report).await?;

    // ── Workstream Builder (global provider) ─────────────────────────────────
    // Fold this hour's report into the running 1-5 day tasks (workstreams), THEN record its
    // per-hour marker row. `hour` is the local `YYYY-MM-DDTHH` label; its date prefix is the
    // local day key. Order matters: `upsert_hour_span`'s row is the fold's idempotency
    // marker (`task_db::hour_folded_marker_exists`), so it must land only *after* the fold
    // succeeds — a fold that errors short-circuits here via `?`, writes no marker, and the
    // driver retries the whole hour. `span_min` itself is write-only now (minutes come from
    // segments), so nothing reads it back on the failed-fold path.
    let day_local = hour.get(..10).unwrap_or(hour);
    let span_min = hour_span_minutes(&timeline);
    workstream::run(pool, day_local, hour, &report).await?;
    task_db::upsert_hour_span(pool, day_local, hour, span_min).await?;
    Ok(())
}

/// Add an OTel span Link from the current span (`worklog.hour`, via the caller's
/// `.instrument()`) to each distinct session-formation trace among this hour's
/// timeline rows, so a `worklog.hour` trace in OpenObserve can backtrack to
/// exactly how each contributing session was formed by the ETL. Sessions with no
/// `traceparent` (pre-migration-010 rows, or a formation that predates OTel
/// capture being enabled) are silently skipped — this is best-effort lineage,
/// never load-bearing for the hour itself.
fn link_session_formation_traces(timeline: &[crate::worklog_pipeline::hour_input::TimelineRow]) {
    use std::collections::HashSet;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let mut seen = HashSet::new();
    let mut linked = 0u32;
    let current = tracing::Span::current();
    for tp in timeline.iter().filter_map(|r| r.traceparent.as_deref()) {
        if !seen.insert(tp) {
            continue;
        }
        if let Some(sc) = crate::observability::span_context_from_traceparent(tp) {
            current.add_link(sc);
            linked += 1;
        }
    }
    if linked > 0 {
        tracing::debug!(linked, "worklog: linked session formation traces");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_report_line_reads_start_end_range() {
        let (stamp, mins, prose) =
            parse_report_line("08:01-08:46  45 min  Built the thing").unwrap();
        assert_eq!(stamp.as_deref(), Some("08:01-08:46"));
        assert_eq!(mins, 45);
        assert_eq!(prose, "Built the thing");
    }

    #[test]
    fn parse_report_line_accepts_start_only_and_no_time() {
        let (stamp, mins, _) = parse_report_line("08:20  5 min  Watched YouTube").unwrap();
        assert_eq!(stamp.as_deref(), Some("08:20"));
        assert_eq!(mins, 5);
        // No leading time: still parses minutes, stamp is None.
        let (stamp, mins, prose) = parse_report_line("9 mins  Reviewed the PR").unwrap();
        assert_eq!(stamp, None);
        assert_eq!(mins, 9);
        assert_eq!(prose, "Reviewed the PR");
    }

    #[test]
    fn parse_report_line_tolerates_a_stray_bullet_and_skips_headers() {
        let (stamp, mins, _) = parse_report_line("- 08:00-08:10  3 min  Did a thing").unwrap();
        assert_eq!(stamp.as_deref(), Some("08:00-08:10"));
        assert_eq!(mins, 3);
        // A header line with no minute count is dropped.
        assert!(parse_report_line("## Summary of the hour").is_none());
    }

    #[test]
    fn parse_report_line_tolerates_a_literal_angle_bracket_wrapper() {
        // Verbatim output from a live cursor-agent 2026.06.04 call against the real
        // ACTIVITY_REPORT prompt: it took the prompt's "<HH:MM-HH:MM>" notation literally
        // and emitted the brackets, instead of writing a bare "HH:MM-HH:MM" like Claude
        // does. Before this fix the leading '<' isn't a digit, so the line dropped and the
        // whole hour's report came out empty with no error anywhere in the path.
        let (stamp, mins, prose) = parse_report_line(
            "<12:43-12:53>  10 min  Launched the staging macOS release on pre-main (PR #443), \
             monitoring the GitHub Actions pipeline through Apple signing and notarization \
             until the staging DMG is ready for team testing.",
        )
        .unwrap();
        assert_eq!(
            stamp.as_deref(),
            Some("12:43-12:53"),
            "brackets must not leak into the stamp"
        );
        assert_eq!(mins, 10);
        assert!(prose.starts_with("Launched the staging macOS release"));

        // A single HH:MM (no range) wrapped the same way.
        let (stamp, mins, _) = parse_report_line("<08:20>  5 min  Watched YouTube").unwrap();
        assert_eq!(stamp.as_deref(), Some("08:20"));
        assert_eq!(mins, 5);

        // A stray leading '<' with no real timestamp behind it must not swallow real
        // prose or otherwise misparse — it still requires digits right after.
        assert!(parse_report_line("<not a timestamp>  Did a thing").is_none());

        // A bracketed aside shaped like a duration ("<15 min gap, no activity>") is NOT a
        // timestamped entry. The bracket strip is now gated on a real HH:MM, so the '<'
        // stays put and the line drops — where an unconditional strip would have unwrapped
        // it and let the minute parser read "15 min" as a phantom entry with a leaked '>'.
        assert!(parse_report_line("<15 min gap, no activity>").is_none());
    }

    #[test]
    fn parse_report_maps_positions_to_minutes_and_stamps() {
        let text = "08:01-08:46  45 min  A\n08:50  9 min  B";
        let (acts, mins, stamps) = parse_report(text);
        assert_eq!(acts, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(mins.get(&1), Some(&45));
        assert_eq!(mins.get(&2), Some(&9));
        assert_eq!(stamps.get(&1).map(String::as_str), Some("08:01-08:46"));
        assert_eq!(stamps.get(&2).map(String::as_str), Some("08:50"));
    }
}
