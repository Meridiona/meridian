// meridian — normalises screenpipe activity into structured app sessions
//
// Segmentation: a single coding-agent JSONL is sliced into SEGMENTS split on
// idle gaps > `segment_gap_seconds` (default 1h) AND on a `max_segment_seconds`
// time-box (default 1h), so a long continuous burst still seals on a
// predictable cadence. One `app_sessions` row per segment, keyed on
// (claude_session_uuid, segment_started_at).
//
// Faithful port of services/coding_agent_indexer/jsonl_meta.py
// `parse_session_segments`. Parity is enforced by the tests below; the Python
// `tests/test_segmentation.py` cases are mirrored here. Change both together.

use std::path::Path;

use chrono::{DateTime, Utc};

use super::jsonl::{infer_agent, iter_normalised, NormRecord};

// Defaults mirror services/coding_agent_indexer/config.py.
pub const ACTIVE_TIME_GAP_CAP_SECONDS: i64 = 120;
pub const SEGMENT_GAP_SECONDS: i64 = 3600;
pub const MAX_SEGMENT_SECONDS: i64 = 3600;

/// Tunables for one parse pass (mirrors the keyword args of the Python fn).
#[derive(Debug, Clone)]
pub struct SegmentParams {
    pub agent: Option<String>,
    pub active_gap_cap_seconds: i64,
    pub segment_gap_seconds: i64,
    pub max_segment_seconds: i64,
    pub start_after_ts: Option<String>,
}

impl Default for SegmentParams {
    fn default() -> Self {
        Self {
            agent: None,
            active_gap_cap_seconds: ACTIVE_TIME_GAP_CAP_SECONDS,
            segment_gap_seconds: SEGMENT_GAP_SECONDS,
            max_segment_seconds: MAX_SEGMENT_SECONDS,
            start_after_ts: None,
        }
    }
}

/// One continuous work burst of a session. One app_sessions row per segment,
/// keyed on (session_uuid, segment_started_at). `is_last` marks the final
/// segment — the only one that may still be live (unsealed).
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
/// precision — matches Python's `(a - b).total_seconds()` so gap/time-box
/// boundary comparisons are byte-for-byte equivalent.
fn delta_secs(a: DateTime<Utc>, b: DateTime<Utc>) -> f64 {
    (a - b).num_milliseconds() as f64 / 1000.0
}

// ──────────────────────── Builder ──────────────────────────────────────────

struct SegBuilder {
    segment_started_at: String,
    ended_at: Option<String>,
    user_turns: u32,
    assistant_turns: u32,
    active_seconds: f64,
    records: Vec<(Option<String>, NormRecord)>,
}

impl SegBuilder {
    fn new(segment_started_at: String) -> Self {
        Self {
            segment_started_at,
            ended_at: None,
            user_turns: 0,
            assistant_turns: 0,
            active_seconds: 0.0,
            records: Vec::new(),
        }
    }

    fn add_turn(&mut self, ts: Option<String>, rec: NormRecord) {
        if rec.is_user {
            self.user_turns += 1;
        } else {
            self.assistant_turns += 1;
        }
        self.records.push((ts, rec));
    }
}

// ──────────────────────── Public API ───────────────────────────────────────

/// Single pass over the JSONL; returns (overall SessionMeta, Segment list).
pub fn parse_session_segments(path: &Path, params: &SegmentParams) -> (SessionMeta, Vec<Segment>) {
    let agent = params.agent.clone().unwrap_or_else(|| infer_agent(path));

    let start_after_dt: Option<DateTime<Utc>> =
        params.start_after_ts.as_deref().and_then(parse_iso);

    let session_uuid = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let jsonl_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let mut cwd: Option<String> = None;
    let mut started_overall: Option<String> = None;
    let mut ended_overall: Option<String> = None;
    let mut user_turns_overall: u32 = 0;
    let mut assistant_turns_overall: u32 = 0;
    let mut total_records: u64 = 0;
    let mut prev_dt: Option<DateTime<Utc>> = None; // ts of previous KEPT record (gap calc)
    let mut seg_start_dt: Option<DateTime<Utc>> = None; // start ts of current segment (time-box)
    let mut builders: Vec<SegBuilder> = Vec::new();
    let mut cur: Option<usize> = None; // index into `builders`

    for rec in iter_normalised(path, &agent) {
        total_records += 1;
        let ts = rec.timestamp.clone();
        if cwd.is_none() {
            if let Some(c) = &rec.cwd {
                cwd = Some(c.clone());
            }
        }

        let cur_dt: Option<DateTime<Utc>> = ts.as_deref().and_then(parse_iso);

        // Already-sealed content is immutable history — skip it and reset the
        // gap anchor so the first kept record opens a fresh segment.
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

        // Records with no usable timestamp can't anchor a segment; attach to the
        // current one if it exists (so a body isn't lost), else drop.
        let cur_dt = match cur_dt {
            Some(d) => d,
            None => {
                if let Some(ci) = cur {
                    if rec.is_turn {
                        builders[ci].add_turn(ts, rec);
                    }
                }
                continue;
            }
        };

        // Time-box: once a segment has run for max_segment_seconds, it should
        // split — but only AT THE NEXT REAL USER PROMPT, so the prior row ends on
        // a complete assistant turn and the new row opens on a user message
        // (continuity). Splitting on raw time would cut mid-exchange. Tool-result
        // `user` records don't count (is_user_prompt is false for them). If the
        // agent runs autonomously past the box with no new prompt, the segment
        // simply extends until the next prompt (or a >1h gap closes it).
        let time_box_due = params.max_segment_seconds > 0
            && seg_start_dt.is_some()
            && delta_secs(cur_dt, seg_start_dt.unwrap()) >= params.max_segment_seconds as f64;
        let start_new = cur.is_none()
            || prev_dt.is_none()
            || delta_secs(cur_dt, prev_dt.unwrap()) > params.segment_gap_seconds as f64
            || (time_box_due && rec.is_user_prompt);

        if start_new {
            builders.push(SegBuilder::new(ts.clone().unwrap_or_default()));
            cur = Some(builders.len() - 1);
            seg_start_dt = Some(cur_dt);
        } else {
            let gap = delta_secs(cur_dt, prev_dt.unwrap());
            if gap > 0.0 {
                let ci = cur.unwrap();
                builders[ci].active_seconds += gap.min(params.active_gap_cap_seconds as f64);
            }
        }

        let ci = cur.unwrap();
        builders[ci].ended_at = ts.clone();
        prev_dt = Some(cur_dt);

        if rec.is_turn {
            let is_user = rec.is_user;
            builders[ci].add_turn(ts, rec);
            if is_user {
                user_turns_overall += 1;
            } else {
                assistant_turns_overall += 1;
            }
        }
    }

    let n = builders.len();
    let mut segments: Vec<Segment> = Vec::with_capacity(n);
    let mut active_total: i64 = 0;
    for (i, b) in builders.into_iter().enumerate() {
        let active = b.active_seconds as i64; // int() truncation toward zero
        active_total += active;
        let seg_start = norm_iso(&b.segment_started_at);
        let ended = norm_iso(b.ended_at.as_deref().unwrap_or(&b.segment_started_at));
        segments.push(Segment {
            session_uuid: session_uuid.clone(),
            agent: agent.clone(),
            cwd: cwd.clone(),
            segment_started_at: seg_start.clone(),
            started_at: seg_start,
            ended_at: ended,
            user_turns: b.user_turns,
            assistant_turns: b.assistant_turns,
            active_seconds: active,
            transcript: render_records(&b.records),
            is_last: i == n - 1,
        });
    }

    let meta = SessionMeta {
        session_uuid,
        agent,
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

    #[test]
    fn single_segment_no_split() {
        let d = tmp();
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
        assert_eq!(segs[0].segment_started_at, iso_utc(base()));
    }

    #[test]
    fn split_on_gap_over_threshold() {
        let d = tmp();
        // 2h gap (7200s > 3600s) → two segments.
        let p = write_claude_jsonl(
            &d,
            "u2",
            &[
                rec(0, "user", "morning work"),
                rec(60, "assistant", "done"),
                rec(60 + 7200, "user", "afternoon work"),
                rec(60 + 7200 + 30, "assistant", "done2"),
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 2);
        assert!(!segs[0].is_last);
        assert!(segs[1].is_last);
        assert_eq!(segs[0].segment_started_at, iso_utc(base()));
        assert_eq!(
            segs[1].segment_started_at,
            iso_utc(base() + Duration::seconds(7260))
        );
    }

    #[test]
    fn no_split_at_exact_threshold() {
        let d = tmp();
        // Exactly 3600s gap, time-box disabled → stays one segment (strict >).
        let p = write_claude_jsonl(
            &d,
            "u3",
            &[rec(0, "user", "a"), rec(3600, "assistant", "b")],
        );
        let params = SegmentParams {
            max_segment_seconds: 0,
            ..Default::default()
        };
        let (_m, segs) = parse_session_segments(&p, &params);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn time_box_splits_continuous_session() {
        let d = tmp();
        // 16 records, 10 min apart = 150 min continuous, no >1h gap.
        // Time-box at 3600s → boundaries at 0, 3600, 7200 → 3 segments.
        let mut lines = Vec::new();
        for i in 0..16 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            lines.push(rec(i * 600, role, &format!("turn {}", i)));
        }
        let p = write_claude_jsonl(&d, "u4", &lines);
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(
            segs.len(),
            3,
            "150min continuous should time-box into 3 hourly chunks"
        );
        // No segment spans >= 1h.
        for s in &segs {
            let span = delta_secs(
                parse_iso(&s.ended_at).unwrap(),
                parse_iso(&s.started_at).unwrap(),
            );
            assert!(span < 3600.0, "segment span {} must be < time-box", span);
        }
    }

    #[test]
    fn time_box_splits_at_user_prompt_not_mid_exchange() {
        let d = tmp();
        // seg starts at 0; time-box boundary = 3600. After the boundary the agent
        // is mid-work (assistant turns + a tool_result), and the next REAL user
        // prompt is at 4000 → the split must land there, not at the raw boundary.
        let p = write_claude_jsonl(
            &d,
            "u",
            &[
                rec(0, "user", "do the task"),
                rec(100, "assistant", "working"),
                rec(3500, "assistant", "still working"), // before boundary
                rec(3700, "assistant", "ran a tool"),    // after boundary, not a prompt
                rec_tool_result(3750),                   // tool_result user → not a prompt
                rec(3800, "assistant", "more work"),     // last assistant before the prompt
                rec(4000, "user", "next request"),       // real prompt after boundary → SPLIT
                rec(4100, "assistant", "on it"),
            ],
        );
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 2, "exactly one split, at the user prompt");

        // Prior row ends on the complete assistant turn (3800), keeps the in-flight
        // exchange, and does NOT contain the next prompt.
        assert_eq!(segs[0].ended_at, iso_utc(base() + Duration::seconds(3800)));
        assert!(segs[0].transcript.contains("more work"));
        assert!(!segs[0].transcript.contains("next request"));

        // New row opens AT the user prompt.
        assert_eq!(
            segs[1].segment_started_at,
            iso_utc(base() + Duration::seconds(4000))
        );
        assert!(segs[1].transcript.contains("[user] next request"));
        assert!(!segs[1].transcript.contains("more work"));
    }

    #[test]
    fn time_box_extends_when_no_user_prompt_after_boundary() {
        let d = tmp();
        // Agent runs autonomously ~3h past the box with no new prompt and no >1h
        // gap → stays ONE segment (continuity beats a mid-stream cut).
        let mut lines = vec![rec(0, "user", "go")];
        for i in 1..20 {
            lines.push(rec(i * 600, "assistant", &format!("step {i}")));
        }
        let p = write_claude_jsonl(&d, "u", &lines);
        let (_m, segs) = parse_session_segments(&p, &SegmentParams::default());
        assert_eq!(segs.len(), 1, "no user prompt after the box → no split");
    }

    #[test]
    fn time_box_disabled_keeps_one_segment() {
        let d = tmp();
        let mut lines = Vec::new();
        for i in 0..16 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            lines.push(rec(i * 600, role, &format!("turn {}", i)));
        }
        let p = write_claude_jsonl(&d, "u5", &lines);
        let params = SegmentParams {
            max_segment_seconds: 0,
            ..Default::default()
        };
        let (_m, segs) = parse_session_segments(&p, &params);
        assert_eq!(segs.len(), 1);
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
        assert_eq!(
            segs[0].segment_started_at,
            iso_utc(base() + Duration::seconds(120))
        );
    }
}
