//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Segmentation: a single coding-agent JSONL is sliced into SEGMENTS aligned to
// UTC clock hours so that one hour of coding work maps to exactly one
// `app_sessions` row, bucketed by `started_at` exactly like screen-capture rows.
//
// The indivisible unit is an EXCHANGE: a real user prompt followed by all the
// assistant/tool work it triggers, up to (but not including) the next prompt.
// An exchange is never split mid-stream — it belongs WHOLE to the clock hour of
// its COMPLETION (its last/most-recent record). So a prompt at 10:55 whose reply
// lands at 11:03 is all hour 11. Consecutive exchanges that complete in the same
// hour share one segment; a new completion hour opens a new segment. A >1h idle
// silence also ends an exchange (a "walked away" boundary), but since that gap
// necessarily crosses an hour it never merges into the same segment anyway.
//
// `segment_started_at`/`started_at` is the COMPLETION-HOUR FLOOR (e.g.
// `…T10:00:00`), which makes the `(uuid, segment_started_at)` key precisely
// "one row per conversation per hour" and every downstream hour-bucket query
// (`distil_hour`, the worklog readiness gate, the ledger) exact with no special
// casing. `ended_at` stays the real last-completion instant and `duration_s`
// stays real active-seconds — a boundary-crossing exchange's full active time
// counts toward its completion hour.
//
// Originally ported from the Python indexer's `parse_session_segments`.

use std::path::Path;

use chrono::{DateTime, Timelike, Utc};

use super::jsonl::{infer_agent, iter_normalised_with_title, NormRecord};

// Defaults mirror the former Python indexer/config.py.
pub const ACTIVE_TIME_GAP_CAP_SECONDS: i64 = 120;
pub const SEGMENT_GAP_SECONDS: i64 = 3600;

/// Tunables for one parse pass.
#[derive(Debug, Clone)]
pub struct SegmentParams {
    pub agent: Option<String>,
    pub active_gap_cap_seconds: i64,
    /// A gap longer than this between two consecutive records ends the current
    /// exchange (a "walked away" boundary). Such a gap always crosses an hour,
    /// so it also lands the next exchange in a fresh segment.
    pub segment_gap_seconds: i64,
    pub start_after_ts: Option<String>,
}

impl Default for SegmentParams {
    fn default() -> Self {
        Self {
            agent: None,
            active_gap_cap_seconds: ACTIVE_TIME_GAP_CAP_SECONDS,
            segment_gap_seconds: SEGMENT_GAP_SECONDS,
            start_after_ts: None,
        }
    }
}

/// One UTC clock-hour of a session's work. One app_sessions row per segment,
/// keyed on (session_uuid, segment_started_at) where `segment_started_at` is the
/// completion-hour floor. `is_last` marks the final segment — the only one that
/// may still be live (unsealed); its hour may not have elapsed yet.
#[derive(Debug, Clone)]
pub struct Segment {
    pub session_uuid: String,
    pub agent: String,
    pub cwd: Option<String>,
    pub segment_started_at: String,
    pub started_at: String,
    pub ended_at: String,
    pub user_turns: u32,
    pub assistant_turns: u32,
    pub active_seconds: i64,
    pub transcript: String,
    pub is_last: bool,
    /// The agent's own session name, when its store has one (Cursor
    /// `composerData.name`, VS Code chat `customTitle`, cursor-agent meta
    /// `name`, Claude `summary` records). Written to `window_titles` as a
    /// single-entry `[{"window_name": …, "count": 1}]`; None → `[]`.
    pub title: Option<String>,
}

impl Segment {
    pub fn is_valid(&self) -> bool {
        self.user_turns + self.assistant_turns > 0
            && !self.segment_started_at.is_empty()
            && !self.ended_at.is_empty()
    }
}

/// Overall metadata for a JSONL — useful for daemon-level cursors.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub session_uuid: String,
    pub agent: String,
    pub cwd: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub user_turns: u32,
    pub assistant_turns: u32,
    pub total_records: u64,
    pub jsonl_bytes: u64,
    pub active_seconds: i64,
}

impl SessionMeta {
    pub fn is_valid(&self) -> bool {
        self.user_turns + self.assistant_turns > 0
            && !self.started_at.is_empty()
            && !self.ended_at.is_empty()
            && self.started_at <= self.ended_at
    }
}

// ──────────────────────── Time helpers ─────────────────────────────────────

/// Parse any ISO-8601 string (…Z / …±offset) to UTC (mirrors `_parse_iso`).
pub fn parse_iso(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Canonical app_sessions timestamp: ISO-8601 UTC, microseconds, `+00:00`
/// (mirrors `iso_utc`). `%6f` matches CPython's `%f` (6-digit microseconds).
pub fn iso_utc(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S.%6f+00:00").to_string()
}

/// Normalise any ISO string to `iso_utc`; fall back to the input unchanged if
/// it can't be parsed (mirrors `norm_iso`).
pub fn norm_iso(ts: &str) -> String {
    match parse_iso(ts) {
        Some(dt) => iso_utc(dt),
        None => ts.to_string(),
    }
}

/// Signed seconds between two instants as float, preserving sub-second
/// precision — matches Python's `(a - b).total_seconds()` so gap boundary
/// comparisons are byte-for-byte equivalent.
fn delta_secs(a: DateTime<Utc>, b: DateTime<Utc>) -> f64 {
    (a - b).num_milliseconds() as f64 / 1000.0
}

/// Floor an instant to the top of its UTC hour — the segment's bucket key.
fn hour_floor(dt: DateTime<Utc>) -> DateTime<Utc> {
    // with_minute/second/nanosecond(0) only fail on out-of-range inputs (never
    // for 0); fall back to the input unchanged on the impossible None.
    dt.with_minute(0)
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(dt)
}

// ──────────────────────── Builder ──────────────────────────────────────────

/// One exchange: a real user prompt and the work it triggers, up to the next
/// prompt (or a >1h silence). The indivisible unit of attribution — it belongs
/// whole to the clock hour of `completion`.
struct Exchange {
    records: Vec<(Option<String>, NormRecord)>,
    completion: DateTime<Utc>, // max record ts → robust to out-of-order timestamps
    user_turns: u32,
    assistant_turns: u32,
}

impl Exchange {
    fn new(anchor: DateTime<Utc>) -> Self {
        Self {
            records: Vec::new(),
            completion: anchor,
            user_turns: 0,
            assistant_turns: 0,
        }
    }

    /// Push a record, advancing the completion instant (max, not last-seen, so
    /// non-monotonic JSONL timestamps can't drag the bucket hour backwards).
    fn push(&mut self, ts: Option<String>, cur_dt: Option<DateTime<Utc>>, rec: NormRecord) {
        if let Some(d) = cur_dt {
            if d > self.completion {
                self.completion = d;
            }
        }
        if rec.is_turn {
            if rec.is_user {
                self.user_turns += 1;
            } else {
                self.assistant_turns += 1;
            }
        }
        self.records.push((ts, rec));
    }
}

/// Active seconds over a segment's record stream: the sum of inter-record gaps,
/// each capped, counting only forward (positive) gaps. Matches the legacy
/// per-segment accumulation — the cap absorbs idle pauses, and the gap between
/// two same-hour exchanges still counts (capped) since they share a segment.
fn active_seconds_of(records: &[(Option<String>, NormRecord)], gap_cap: i64) -> i64 {
    let mut acc = 0.0_f64;
    let mut prev: Option<DateTime<Utc>> = None;
    for (ts, _) in records {
        if let Some(dt) = ts.as_deref().and_then(parse_iso) {
            if let Some(p) = prev {
                let gap = delta_secs(dt, p);
                if gap > 0.0 {
                    acc += gap.min(gap_cap as f64);
                }
            }
            prev = Some(dt);
        }
    }
    acc as i64 // int() truncation toward zero
}

// ──────────────────────── Public API ───────────────────────────────────────

/// Single pass over the JSONL; returns (overall SessionMeta, Segment list).
pub fn parse_session_segments(path: &Path, params: &SegmentParams) -> (SessionMeta, Vec<Segment>) {
    let agent = params.agent.clone().unwrap_or_else(|| infer_agent(path));

    let session_uuid = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let jsonl_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (records, title) = iter_normalised_with_title(path, &agent);
    let (meta, mut segments) = segment_records(records, &session_uuid, &agent, jsonl_bytes, params);
    // Stamp the session's own name on every segment (one read produced both).
    if let Some(t) = title.as_deref().and_then(clean_title) {
        for seg in segments.iter_mut() {
            seg.title = Some(t.clone());
        }
    }
    (meta, segments)
}

/// Normalise a raw session title for storage: trim, drop when empty, cap length.
/// Shared by both title-stamping sites — `parse_session_segments` (the Claude
/// JSONL path) and `indexer::stamp_title` (the source-adapter path) — so the two
/// cannot diverge on trimming, empty-filtering, or the length cap.
pub(crate) fn clean_title(raw: &str) -> Option<String> {
    const TITLE_CAP: usize = 200;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(TITLE_CAP).collect())
}

/// Segment an already-normalised record stream — the source-agnostic core of
/// `parse_session_segments`. Non-JSONL sources (Cursor's vscdb, Copilot's
/// event log) normalise their on-disk format into `NormRecord`s and feed them
/// here, so segmentation, active-time, and transcript rendering stay identical
/// across every agent.
pub fn segment_records(
    records: Vec<NormRecord>,
    session_uuid: &str,
    agent: &str,
    jsonl_bytes: u64,
    params: &SegmentParams,
) -> (SessionMeta, Vec<Segment>) {
    let start_after_dt: Option<DateTime<Utc>> =
        params.start_after_ts.as_deref().and_then(parse_iso);

    let mut cwd: Option<String> = None;
    let mut started_overall: Option<String> = None;
    let mut ended_overall: Option<String> = None;
    let mut user_turns_overall: u32 = 0;
    let mut assistant_turns_overall: u32 = 0;
    let mut total_records: u64 = 0;
    let mut prev_dt: Option<DateTime<Utc>> = None; // ts of previous KEPT record (gap calc)

    // Phase 1 — group records into exchanges. A new exchange opens at a real
    // user prompt, on the first kept record, or after a >1h silence.
    let mut exchanges: Vec<Exchange> = Vec::new();
    let mut cur: Option<Exchange> = None;

    for rec in records {
        total_records += 1;
        let ts = rec.timestamp.clone();
        if cwd.is_none() {
            if let Some(c) = &rec.cwd {
                cwd = Some(c.clone());
            }
        }

        let cur_dt: Option<DateTime<Utc>> = ts.as_deref().and_then(parse_iso);

        // Already-sealed content is immutable history — skip it and reset the
        // gap anchor so the first kept record opens a fresh exchange.
        if let (Some(c), Some(sa)) = (cur_dt, start_after_dt) {
            if c <= sa {
                prev_dt = None;
                continue;
            }
        }

        if cur_dt.is_some() {
            if started_overall.is_none() {
                started_overall = ts.clone();
            }
            ended_overall = ts.clone();
        }

        // Records with no usable timestamp can't anchor an exchange; attach to
        // the current one if it exists (so a body isn't lost), else drop.
        let cur_dt = match cur_dt {
            Some(d) => d,
            None => {
                if let Some(ex) = cur.as_mut() {
                    ex.push(ts, None, rec);
                }
                continue;
            }
        };

        let gap_break = prev_dt
            .map(|p| delta_secs(cur_dt, p) > params.segment_gap_seconds as f64)
            .unwrap_or(false);
        if cur.is_none() || rec.is_user_prompt || gap_break {
            if let Some(ex) = cur.take() {
                exchanges.push(ex);
            }
            cur = Some(Exchange::new(cur_dt));
        }

        if rec.is_turn {
            if rec.is_user {
                user_turns_overall += 1;
            } else {
                assistant_turns_overall += 1;
            }
        }
        cur.as_mut().unwrap().push(ts, Some(cur_dt), rec);
        prev_dt = Some(cur_dt);
    }
    if let Some(ex) = cur.take() {
        exchanges.push(ex);
    }

    // Phase 2 — fold consecutive same-completion-hour exchanges into segments.
    let mut segments: Vec<Segment> = Vec::new();
    let mut active_total: i64 = 0;
    let mut cur_hour: Option<DateTime<Utc>> = None;
    let mut buf: Vec<(Option<String>, NormRecord)> = Vec::new();
    let mut buf_user = 0_u32;
    let mut buf_asst = 0_u32;
    let mut buf_last: Option<DateTime<Utc>> = None;

    let flush = |hour: DateTime<Utc>,
                 records: Vec<(Option<String>, NormRecord)>,
                 user_turns: u32,
                 assistant_turns: u32,
                 last: DateTime<Utc>,
                 segments: &mut Vec<Segment>,
                 active_total: &mut i64| {
        let active = active_seconds_of(&records, params.active_gap_cap_seconds);
        *active_total += active;
        let seg_start = iso_utc(hour);
        segments.push(Segment {
            session_uuid: session_uuid.to_string(),
            agent: agent.to_string(),
            cwd: cwd.clone(),
            segment_started_at: seg_start.clone(),
            started_at: seg_start,
            ended_at: iso_utc(last),
            user_turns,
            assistant_turns,
            active_seconds: active,
            transcript: render_records(&records),
            is_last: false, // fixed up after the loop
            title: None,
        });
    };

    for ex in exchanges {
        let hour = hour_floor(ex.completion);
        if cur_hour != Some(hour) {
            if let Some(h) = cur_hour.take() {
                flush(
                    h,
                    std::mem::take(&mut buf),
                    buf_user,
                    buf_asst,
                    buf_last.unwrap_or(h),
                    &mut segments,
                    &mut active_total,
                );
                buf_user = 0;
                buf_asst = 0;
                buf_last = None;
            }
            cur_hour = Some(hour);
        }
        buf.extend(ex.records);
        buf_user += ex.user_turns;
        buf_asst += ex.assistant_turns;
        buf_last = Some(buf_last.map_or(ex.completion, |l| l.max(ex.completion)));
    }
    if let Some(h) = cur_hour {
        flush(
            h,
            std::mem::take(&mut buf),
            buf_user,
            buf_asst,
            buf_last.unwrap_or(h),
            &mut segments,
            &mut active_total,
        );
    }
    if let Some(last) = segments.last_mut() {
        last.is_last = true;
    }

    let meta = SessionMeta {
        session_uuid: session_uuid.to_string(),
        agent: agent.to_string(),
        cwd,
        started_at: started_overall.as_deref().map(norm_iso).unwrap_or_default(),
        ended_at: ended_overall.as_deref().map(norm_iso).unwrap_or_default(),
        user_turns: user_turns_overall,
        assistant_turns: assistant_turns_overall,
        total_records,
        jsonl_bytes,
        active_seconds: active_total,
    };
    (meta, segments)
}

/// Flatten (timestamp, record) pairs to a timestamped transcript:
/// `[<raw ts>] [role] body` per turn (mirrors `_render_records`). The timestamp
/// prefix is the RAW JSONL ts (not normalised), matching the Python behaviour.
fn render_records(records: &[(Option<String>, NormRecord)]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for (ts, rec) in records {
        if !rec.body.trim().is_empty() {
            let prefix = match ts {
                Some(t) => format!("[{}] ", t),
                None => String::new(),
            };
            let role = rec.role_label.as_deref().unwrap_or("user");
            blocks.push(format!("{}[{}] {}", prefix, role, rec.body));
        }
    }
    blocks.join("\n\n")
}

// ──────────────────────── Tests (parity with test_segmentation.py) ──────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use std::io::Write;

    #[test]
    fn clean_title_trims_filters_and_caps() {
        assert_eq!(clean_title("  hello  ").as_deref(), Some("hello"));
        assert_eq!(clean_title("   ").as_deref(), None);
        assert_eq!(clean_title("").as_deref(), None);
        // Capped at 200 chars (counts chars, not bytes).
        let long = "x".repeat(250);
        assert_eq!(clean_title(&long).map(|t| t.chars().count()), Some(200));
        // Multibyte stays char-correct (no panic on a byte boundary).
        let emoji = "🚀".repeat(250);
        assert_eq!(clean_title(&emoji).map(|t| t.chars().count()), Some(200));
    }

    fn base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap()
    }

    /// Source 'ms + Z' shape that real Claude JSONLs write (mirrors test `_iso`).
    fn iso_z(dt: DateTime<Utc>) -> String {
        dt.format("%Y-%m-%dT%H:%M:%S.%3fZ").to_string()
    }

    /// One Claude Code JSONL record `offset_s` after BASE.
    fn rec(offset_s: i64, role: &str, text: &str) -> String {
        let ts = iso_z(base() + Duration::seconds(offset_s));
        serde_json::json!({
            "type": role,
            "timestamp": ts,
            "cwd": "/repo",
            "message": { "role": role, "content": text }
        })
        .to_string()
    }

    /// A Claude `type:user` record that is a TOOL RESULT (not a human prompt) —
    /// content is a tool_result block, so is_user_prompt must be false.
    fn rec_tool_result(offset_s: i64) -> String {
        let ts = iso_z(base() + Duration::seconds(offset_s));
        serde_json::json!({
            "type": "user",
            "timestamp": ts,
            "cwd": "/repo",
            "message": { "role": "user", "content": [{"type": "tool_result", "content": "output"}] }
        })
        .to_string()
    }

    fn write_claude_jsonl(dir: &Path, uuid: &str, lines: &[String]) -> std::path::PathBuf {
        // Path must look like ~/.claude/projects/<proj>/<uuid>.jsonl for agent inference.
        let proj = dir.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let p = proj.join(format!("{}.jsonl", uuid));
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{}", l).unwrap();
        }
        p
    }

    fn tmp() -> std::path::PathBuf {
        let mut d = std::env::temp_dir();
        // unique-ish dir without Math.random/time: use a static counter
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        d.push(format!(
            "meridian_seg_test_{}",
            C.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Hour floor of BASE+offset, as the canonical `…+00:00` string segments use.
    fn hour_floor_iso(offset_s: i64) -> String {
        iso_utc(hour_floor(base() + Duration::seconds(offset_s)))
    }

    #[test]
    fn single_segment_no_split() {
        let d = tmp();
        // base = 08:00:00; all three records inside hour 8 → one segment, floored.
        let p = write_claude_jsonl(
            &d,
            "u1",
            &[
                rec(0, "user", "first"),
                rec(60, "assistant", "working"),
                rec(120, "user", "thanks"),
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].user_turns, 2);
        assert_eq!(segs[0].assistant_turns, 1);
        assert!(segs[0].is_last);
        // started_at is the completion-hour FLOOR (08:00:00), not the first ts.
        assert_eq!(segs[0].segment_started_at, iso_utc(base()));
        assert_eq!(segs[0].started_at, "2026-05-20T08:00:00.000000+00:00");
        // ended_at stays the real last-completion instant.
        assert_eq!(segs[0].ended_at, iso_utc(base() + Duration::seconds(120)));
    }

    #[test]
    fn exchange_crossing_hour_belongs_to_completion_hour() {
        let d = tmp();
        // BASE 08:00. A single exchange: prompt at 08:55, reply at 09:03. The
        // whole exchange (prompt + reply) belongs to hour 9 (its completion).
        let p = write_claude_jsonl(
            &d,
            "x1",
            &[
                rec(55 * 60, "user", "do the thing"), // 08:55
                rec(63 * 60, "assistant", "done"),    // 09:03 → completion hour 9
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(
            segs.len(),
            1,
            "one exchange → one segment, not split mid-exchange"
        );
        assert_eq!(segs[0].started_at, hour_floor_iso(63 * 60)); // 09:00:00 floor
        assert_eq!(segs[0].started_at, "2026-05-20T09:00:00.000000+00:00");
        assert_eq!(
            segs[0].ended_at,
            iso_utc(base() + Duration::seconds(63 * 60))
        );
        // The 08:55 prompt is still in the transcript even though the row is hour 9.
        assert!(segs[0].transcript.contains("[user] do the thing"));
    }

    #[test]
    fn two_exchanges_split_at_completion_hour() {
        let d = tmp();
        // Exchange A completes in hour 8, exchange B (next prompt) completes in
        // hour 9 → two segments, each floored to its completion hour.
        let p = write_claude_jsonl(
            &d,
            "x2",
            &[
                rec(10 * 60, "user", "a-prompt"),     // 08:10
                rec(15 * 60, "assistant", "a-reply"), // 08:15 → hour 8
                rec(58 * 60, "user", "b-prompt"),     // 08:58
                rec(65 * 60, "assistant", "b-reply"), // 09:05 → hour 9
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].started_at, "2026-05-20T08:00:00.000000+00:00");
        assert!(!segs[0].is_last);
        assert!(segs[0].transcript.contains("a-reply"));
        assert!(!segs[0].transcript.contains("b-prompt"));
        assert_eq!(segs[1].started_at, "2026-05-20T09:00:00.000000+00:00");
        assert!(segs[1].is_last);
        assert!(segs[1].transcript.contains("[user] b-prompt"));
        assert!(segs[1].transcript.contains("b-reply"));
    }

    #[test]
    fn late_start_creates_no_sliver() {
        let d = tmp();
        // A session whose first (and only) exchange starts at 08:56 but completes
        // at 09:10 → ONE hour-9 row, NO 4-minute hour-8 sliver.
        let p = write_claude_jsonl(
            &d,
            "x3",
            &[
                rec(56 * 60, "user", "start late"),    // 08:56
                rec(70 * 60, "assistant", "finished"), // 09:10 → hour 9
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 1, "no sliver segment in the start hour");
        assert_eq!(segs[0].started_at, "2026-05-20T09:00:00.000000+00:00");
    }

    #[test]
    fn long_autonomous_exchange_goes_to_completion_hour() {
        let d = tmp();
        // One prompt, then the agent runs autonomously every 10 min for ~2.5h
        // with NO new user prompt and no >1h gap → ONE exchange → ONE segment in
        // the FINAL completion hour (Option-A: whole exchange by completion).
        let mut lines = vec![rec(0, "user", "go")]; // 08:00
        for i in 1..16 {
            lines.push(rec(i * 600, "assistant", &format!("step {i}"))); // up to 10:30
        }
        let p = write_claude_jsonl(&d, "x4", &lines);
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(
            segs.len(),
            1,
            "no user prompt → single exchange → one segment"
        );
        // 15*600 = 9000s after 08:00 → 10:30 → hour 10.
        assert_eq!(segs[0].started_at, "2026-05-20T10:00:00.000000+00:00");
    }

    #[test]
    fn out_of_order_timestamps_do_not_drag_bucket_back() {
        let d = tmp();
        // A late assistant record stamped EARLIER than a prior one (clock skew).
        // Completion = max ts, so the bucket hour can't go backwards and the
        // segment stays valid (started_at floor <= ended_at).
        let p = write_claude_jsonl(
            &d,
            "x5",
            &[
                rec(50 * 60, "user", "q"),           // 08:50
                rec(65 * 60, "assistant", "late"),   // 09:05 (real completion)
                rec(62 * 60, "assistant", "skewed"), // 09:02 (out of order, earlier)
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].started_at, "2026-05-20T09:00:00.000000+00:00");
        assert_eq!(
            segs[0].ended_at,
            iso_utc(base() + Duration::seconds(65 * 60))
        );
        assert!(parse_iso(&segs[0].started_at).unwrap() <= parse_iso(&segs[0].ended_at).unwrap());
    }

    #[test]
    fn claude_custom_title_record_becomes_segment_title() {
        let d = tmp();
        // Real Claude format: `custom-title` records, rewritten on rename —
        // the LAST one wins. (Verified against a live session JSONL 2026-06-07.)
        let p = write_claude_jsonl(
            &d,
            "u_title",
            &[
                r#"{"type":"custom-title","customTitle":"old name","sessionId":"u_title"}"#
                    .to_string(),
                rec(0, "user", "first"),
                r#"{"type":"custom-title","customTitle":"multi-agent-ingest","sessionId":"u_title"}"#
                    .to_string(),
                rec(60, "assistant", "working"),
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(
            segs[0].title.as_deref(),
            Some("multi-agent-ingest"),
            "last custom-title record wins"
        );

        // Fallback: older/compacted sessions use `summary` records.
        let p2 = write_claude_jsonl(
            &d,
            "u_summary",
            &[
                r#"{"type":"summary","summary":"Compacted title","leafUuid":"y"}"#.to_string(),
                rec(0, "user", "hi"),
            ],
        );
        let (_m2, segs2) = parse_session_segments(&p2, &SegmentParams::default());
        assert_eq!(segs2[0].title.as_deref(), Some("Compacted title"));

        // Neither record → no title.
        let p3 = write_claude_jsonl(&d, "u_untitled", &[rec(0, "user", "hello")]);
        let (_m3, segs3) = parse_session_segments(&p3, &SegmentParams::default());
        assert!(segs3[0].title.is_none());
    }

    #[test]
    fn split_on_gap_over_threshold() {
        let d = tmp();
        // 2h gap (7200s > 3600s): the silence ends the first exchange and the
        // afternoon work lands two hours later → two segments, each floored.
        let p = write_claude_jsonl(
            &d,
            "u2",
            &[
                rec(0, "user", "morning work"),            // 08:00 → hour 8
                rec(60, "assistant", "done"),              // 08:01
                rec(60 + 7200, "user", "afternoon work"),  // 10:01 → hour 10
                rec(60 + 7200 + 30, "assistant", "done2"), // 10:01:30
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 2);
        assert!(!segs[0].is_last);
        assert!(segs[1].is_last);
        assert_eq!(
            segs[0].segment_started_at,
            "2026-05-20T08:00:00.000000+00:00"
        );
        assert_eq!(
            segs[1].segment_started_at,
            "2026-05-20T10:00:00.000000+00:00"
        );
    }

    #[test]
    fn continuous_session_splits_into_hourly_chunks() {
        let d = tmp();
        // 16 records 10 min apart = 150 min continuous, no >1h gap. Exchanges
        // (user→assistant pairs) complete across hours 8, 9, 10 → 3 segments,
        // each floored to its hour, each spanning < 1h of wall clock.
        let mut lines = Vec::new();
        for i in 0..16 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            lines.push(rec(i * 600, role, &format!("turn {}", i)));
        }
        let p = write_claude_jsonl(&d, "u4", &lines);
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 3, "150min continuous → 3 hourly chunks");
        assert_eq!(segs[0].started_at, "2026-05-20T08:00:00.000000+00:00");
        assert_eq!(segs[1].started_at, "2026-05-20T09:00:00.000000+00:00");
        assert_eq!(segs[2].started_at, "2026-05-20T10:00:00.000000+00:00");
        // Each row's real completion stays within its bucket hour.
        for s in &segs {
            let start = parse_iso(&s.started_at).unwrap();
            let end = parse_iso(&s.ended_at).unwrap();
            assert!(end >= start && delta_secs(end, start) < 3600.0);
        }
    }

    #[test]
    fn tool_result_does_not_open_a_new_exchange() {
        let d = tmp();
        // A tool_result `user` record is NOT a real prompt, so it must not start a
        // new exchange. All within hour 8 → exactly one segment.
        let p = write_claude_jsonl(
            &d,
            "u_tool",
            &[
                rec(0, "user", "do the task"), // real prompt → exchange 1
                rec(100, "assistant", "calling tool"),
                rec_tool_result(150), // tool_result user → NOT a new exchange
                rec(200, "assistant", "more work"),
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(
            segs.len(),
            1,
            "tool_result must not split into a new segment"
        );
        assert_eq!(
            segs[0].user_turns, 2,
            "the real prompt + the tool_result user turn"
        );
        assert!(segs[0].transcript.contains("more work"));
    }

    #[test]
    fn timestamps_normalised_to_canonical_plus0000() {
        let d = tmp();
        let p = write_claude_jsonl(
            &d,
            "u6",
            &[rec(0, "user", "hi"), rec(30, "assistant", "yo")],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        // Input was '...Z' (ms); output must be '...+00:00' (µs).
        assert!(segs[0].started_at.ends_with("+00:00"));
        assert!(!segs[0].started_at.contains('Z'));
        assert_eq!(segs[0].started_at, "2026-05-20T08:00:00.000000+00:00");
    }

    #[test]
    fn transcript_has_timestamped_turns_with_full_body() {
        let d = tmp();
        let long_body = "x".repeat(5000);
        let p = write_claude_jsonl(
            &d,
            "u7",
            &[
                rec(0, "user", "please fix the bug"),
                rec(30, "assistant", &long_body),
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        let t = &segs[0].transcript;
        assert!(t.contains("[user] please fix the bug"));
        assert!(t.contains("[claude-code]"));
        // "full text, not half": a 5000-char message body is preserved in full.
        assert!(
            t.contains(&long_body),
            "assistant body must be stored in full"
        );
    }

    #[test]
    fn start_after_ts_excludes_sealed_content() {
        let d = tmp();
        let p = write_claude_jsonl(
            &d,
            "u8",
            &[
                rec(0, "user", "sealed already"),
                rec(60, "assistant", "sealed reply"),
                rec(120, "user", "fresh turn"),
            ],
        );
        // Seal everything up to and including the 60s record.
        let cutoff = iso_z(base() + Duration::seconds(60));
        let params = SegmentParams {
            start_after_ts: Some(cutoff),
            ..Default::default()
        };
        let (_m, segs) = parse_session_segments(&p, &params);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].transcript.contains("fresh turn"));
        assert!(!segs[0].transcript.contains("sealed"));
        // The fresh turn (08:02) completes in hour 8 → floored to 08:00:00.
        assert_eq!(
            segs[0].segment_started_at,
            "2026-05-20T08:00:00.000000+00:00"
        );
        assert_eq!(segs[0].ended_at, iso_utc(base() + Duration::seconds(120)));
    }
}
