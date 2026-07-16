//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The Rust-owned hour report — distil locally, summarise through the user's chosen AI.
//!
//! This replaces the single Python `/worklog_hour` call. The hour is now orchestrated
//! here so its generative work goes through the centralised provider layer
//! ([`crate::llm`]) — the user's chosen AI (Claude / Codex / Cursor / Copilot / on-device)
//! writes the hourly summary, per the "one global choice for everything except coding-agent
//! summaries" design. Only two things still touch the local MLX server:
//!
//! * **distil** (`/distill_hour`) — the embedder, which no CLI provider offers, so it stays
//!   local and is the one call that holds [`crate::llm_gate`] (the single-GPU permit).
//! * **the on-device fallback** — when the chosen provider fails, [`crate::llm::complete`]
//!   routes to the local model, which self-gates.
//!
//! A CLI provider therefore leaves the GPU free while it thinks — the permit is held only
//! around distillation, not the whole multi-minute hour.
//!
//! The report is a SINGLE generative call (see [`crate::llm::prompts::ACTIVITY_REPORT`]):
//! the model returns the activities as prose (no clock times in the words) AND a minute
//! estimate per activity, together in one structured answer. Code
//! ([`super::hour_input::assemble_report`]) then clamps each estimate to the hour's real
//! measured span and fills any omission — the model's minutes are a ratio, the total is
//! ours. (This replaced an earlier two-call split — summary, then times — which existed
//! because the weak on-device 2B mixed prose quality with invented durations; a capable
//! provider plus a structured `minutes` field removes that failure, so one call suffices.)

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::Instrument;

use crate::llm::{self, prompts, LlmProvider, PromptRequest};
use crate::pm_worklog::PmWorklogConfig;

use super::hour_db;
use super::hour_input::{
    assemble_report, format_coding_block, format_timeline_block, hour_span_minutes,
    parse_activities, CodingRow, TimelineRow,
};
use super::{task_db, workstream};

/// Token ceiling for the single report call: "under 300 words" of prose plus a short
/// per-activity minutes list, as one JSON object. Generous enough not to truncate.
const REPORT_MAX_TOKENS: u32 = 1536;

/// A session shorter than this is a burst — a brief alt-tab into an app, a one-line
/// coding-agent exchange — that clutters the report without representing reportable
/// work. Such sessions are dropped from the REPORT INPUT (both the screen timeline and
/// the coding-agent blocks). The span / time math keeps the full set, so excluding a
/// burst from the prose never removes its measured minutes from the deterministic totals.
const REPORT_MIN_DURATION_S: i64 = 180;

/// What `/distill_hour` returns (the embedder-compressed hour body + metrics).
struct Distilled {
    body: String,
    out_chars: i64,
    reduction_pct: f64,
    nsess: i64,
}

/// Call the local MLX embedder to distil the hour's screen sessions. Holds the single GPU
/// permit for exactly this call — NOT the surrounding pipeline, so a CLI summary that
/// follows doesn't sit behind a resource it isn't using.
async fn distill(cfg: &PmWorklogConfig, db_path: &str, hour: &str) -> Result<Distilled> {
    let _permit = crate::llm_gate::acquire().await;

    let url = format!("http://{}:{}/distill_hour", cfg.mlx_host, cfg.mlx_port);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .build()
        .context("building distill_hour http client")?;

    let traceparent = crate::observability::current_traceparent();
    let resp = client
        .post(&url)
        .json(&json!({ "hour": hour, "db_path": db_path, "traceparent": traceparent }))
        .send()
        .await
        .with_context(|| {
            format!("/distill_hour unreachable at {url} — is the MLX server running?")
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let preview: String = body.chars().take(200).collect();
        anyhow::bail!("/distill_hour returned {status}: {preview}");
    }

    let v: Value = resp
        .json()
        .await
        .context("/distill_hour response not JSON")?;
    Ok(Distilled {
        body: v["body"].as_str().unwrap_or_default().to_string(),
        out_chars: v["out_chars"].as_i64().unwrap_or(0),
        reduction_pct: v["reduction_pct"].as_f64().unwrap_or(0.0),
        nsess: v["nsess"].as_i64().unwrap_or(0),
    })
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
    let req = PromptRequest {
        system: prompts::ACTIVITY_REPORT,
        user: report_input,
        // No schema: the report is plain text now — one "<minutes> min  <activity>" line
        // per activity, parsed by `parse_report`. Dropping `--json-schema` removes the
        // constrained-decoding cost, and `assemble_report` still owns/clamps the minutes,
        // so code keeps the time even though the model writes it inline.
        schema: None,
        max_tokens: REPORT_MAX_TOKENS,
        label: format!("activity-report {label}"),
    };
    let (out, provider) = llm::complete(&req)
        .await
        .map_err(|e| anyhow::anyhow!("activity report failed: {e}"))?;
    let (activities, minutes, stamps) = parse_report(&out.text);
    Ok((activities, minutes, stamps, provider))
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
fn parse_report(
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
/// brackets, but cursor-agent 2026.06.04 was observed emitting them literally, which
/// otherwise made the line start with `<` instead of a digit and drop silently), the unit
/// written as `min` / `mins` / `minutes`, and arbitrary surrounding whitespace. Returns
/// `None` for any line without a leading minute count so headers and notes drop.
fn parse_report_line(raw: &str) -> Option<(Option<String>, i64, String)> {
    let s = strip_list_marker(raw.trim());
    let s = s.strip_prefix('<').unwrap_or(s);
    let (stamp, s) = take_hhmm(s);
    let s = s.strip_prefix('>').unwrap_or(s);
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
/// `(Some(span), rest)` with `span` the matched string verbatim, or `(None, original)` if
/// the line doesn't start with one. The `-HH:MM` end is optional — a looser answer that
/// gives only a start still parses. Purely lexical — it does not validate the clock values
/// (a rough time is all downstream needs).
fn take_hhmm(s: &str) -> (Option<String>, &str) {
    let Some(start_end) = hhmm_len(s) else {
        return (None, s);
    };
    // Optional "-HH:MM" end.
    let tail = &s[start_end..];
    if let Some(rest) = tail.strip_prefix('-') {
        if let Some(end_len) = hhmm_len(rest) {
            let end = start_end + 1 + end_len;
            return (Some(s[..end].to_string()), &s[end..]);
        }
    }
    (Some(s[..start_end].to_string()), &s[start_end..])
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
    cfg: &PmWorklogConfig,
    db_path: &str,
    hour: &str,
    hs: &str,
    he: &str,
) -> Result<()> {
    // ── distil (local embedder) ──────────────────────────────────────────────
    let sess_span =
        tracing::info_span!("worklog.sessions", hour, body_chars = tracing::field::Empty);
    let d = distill(cfg, db_path, hour)
        .instrument(sess_span.clone())
        .await?;
    let coding = hour_db::fetch_coding_summaries(pool, hs, he).await;
    let timeline = hour_db::fetch_hour_timeline(pool, hs, he).await;
    sess_span.record("body_chars", d.body.len());

    // Drop sub-REPORT_MIN_DURATION_S bursts from the report input only — `timeline`
    // (full set) still feeds the span/time math below, so a burst loses its place in
    // the prose without losing its measured minutes.
    let report_timeline: Vec<TimelineRow> = timeline
        .iter()
        .filter(|r| r.duration_s > REPORT_MIN_DURATION_S)
        .cloned()
        .collect();
    let report_coding: Vec<CodingRow> = coding
        .iter()
        .filter(|c| c.duration_s > REPORT_MIN_DURATION_S)
        .cloned()
        .collect();

    let coding_block = format_coding_block(&report_coding);
    let timeline_block = format_timeline_block(&report_timeline);
    tracing::info!(
        hour,
        ocr_nsess = d.nsess,
        coding_nsess = report_coding.len(),
        coding_dropped_short = coding.len() - report_coding.len(),
        timeline_dropped_short = timeline.len() - report_timeline.len(),
        body_chars = d.body.len(),
        "worklog: hour distilled"
    );

    // Persist the distilled INPUT for the dashboard's hour-detail panel, for EVERY hour
    // that distils — not only ones that produce a report.
    hour_db::persist_hour_text(pool, hs, &d.body, d.out_chars, d.reduction_pct).await?;

    // A truly empty hour: nothing to summarise. Persist an empty report and finish clean.
    if d.body.trim().is_empty() && coding_block.is_empty() {
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
    let report = async {
        let report_input = [
            timeline_block.as_str(),
            d.body.as_str(),
            coding_block.as_str(),
        ]
        .iter()
        .filter(|p| !p.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");

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
        coding_folded = !coding_block.is_empty(),
        "worklog: hour report built"
    );
    hour_db::persist_hour_report(pool, hs, &report).await?;

    // ── Workstream Builder (global provider) ─────────────────────────────────
    // Record this hour's measured active minutes (the deterministic time source),
    // then fold its report into the running 1-5 day tasks (workstreams). `hour` is
    // the local `YYYY-MM-DDTHH` label; its date prefix is the local day key.
    let day_local = hour.get(..10).unwrap_or(hour);
    let span_min = hour_span_minutes(&timeline);
    task_db::upsert_hour_span(pool, day_local, hour, span_min).await?;
    workstream::run(pool, day_local, hour, &report).await?;
    Ok(())
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
