// meridian — normalises screenpipe activity into structured app sessions
//
// GitHub Copilot CLI source: `~/.copilot/session-state/<uuid>/events.jsonl`.
// One dir per session; the event log is append-only JSONL, one typed event per
// line, every event carrying a top-level ISO `timestamp`:
//
//   {"type":"user.message","data":{"content":"...", "transformedContent":...}, "timestamp":"..."}
//   {"type":"assistant.message","data":{"messageId":..., "model":..., "content":"..."}, ...}
//   {"type":"session.start","data":{"context":{"cwd":"/repo", ...}}, ...}
//
// Turns come from user.message / assistant.message. We read `data.content`
// (the raw text), NOT `transformedContent` — that variant is wrapped in
// injected <system_reminder>/<current_datetime> scaffolding that would pollute
// the transcript. Every other event type becomes a timestamp-only record so
// active-time still tracks tool/turn activity between messages. Tool events
// carry no rendered body yet (no agentic sample to pin a shape against).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::file_is_candidate;
use crate::coding_agent_session_ingest::jsonl::NormRecord;

pub const AGENT: &str = "copilot_cli";
const ASSISTANT_LABEL: &str = "copilot";

#[derive(Clone)]
pub struct CopilotCliSource {
    pub session_state_dir: PathBuf,
}

impl CopilotCliSource {
    pub fn from_env() -> Self {
        let raw = std::env::var("COPILOT_SESSION_STATE_DIR")
            .unwrap_or_else(|_| "~/.copilot/session-state".to_string());
        Self {
            session_state_dir: PathBuf::from(shellexpand::tilde(&raw).into_owned()),
        }
    }

    pub fn present(&self) -> bool {
        self.session_state_dir.is_dir()
    }

    /// Sessions whose events.jsonl mtime moved past the stored endpoint, as
    /// (session_uuid, events.jsonl path) pairs, oldest-changed first.
    pub fn changed_sessions(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<(String, PathBuf)> {
        let entries = match std::fs::read_dir(&self.session_state_dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<(f64, String, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let uuid = match dir.file_name().and_then(|n| n.to_str()) {
                Some(u) => u.to_string(),
                None => continue,
            };
            let events = dir.join("events.jsonl");
            let mtime = match events.metadata().and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue, // no events.jsonl → nothing to ingest
            };
            let stored = endpoints.get(&uuid).map(String::as_str);
            if !file_is_candidate(mtime, stored, now) {
                continue;
            }
            let epoch = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            out.push((epoch, uuid, events));
        }
        // Oldest-changed first, matching the JSONL indexer's ordering.
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.into_iter().map(|(_, u, p)| (u, p)).collect()
    }
}

/// Parse one events.jsonl into normalised records. Tolerant of partial writes
/// and malformed lines (skipped), mirroring the Claude/Codex JSONL readers.
pub(crate) fn parse_events_jsonl(path: &Path) -> Vec<NormRecord> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let raw = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if raw.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !v.is_object() {
            continue;
        }
        out.push(norm_event(&v));
    }
    out
}

fn norm_event(raw: &Value) -> NormRecord {
    let ts = raw
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    let etype = raw.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let data = raw.get("data").cloned().unwrap_or(Value::Null);

    // cwd only appears on session.start (data.context.cwd); harmless None elsewhere.
    let cwd = data
        .get("context")
        .and_then(|c| c.get("cwd"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());

    let content = |d: &Value| -> String {
        d.get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    };

    match etype {
        "user.message" => {
            let body = content(&data);
            NormRecord {
                timestamp: ts,
                cwd,
                is_turn: true,
                is_user: true,
                is_user_prompt: !body.trim().is_empty(),
                role_label: Some("user".to_string()),
                body,
                is_session_end: false,
            }
        }
        "assistant.message" => NormRecord {
            timestamp: ts,
            cwd,
            is_turn: true,
            is_user: false,
            is_user_prompt: false,
            role_label: Some(ASSISTANT_LABEL.to_string()),
            body: content(&data),
            is_session_end: false,
        },
        // Every other event (session.*, assistant.turn_*, tool activity, …)
        // anchors timing only: its timestamp extends the segment and feeds
        // active-time, but it contributes no transcript body. session.shutdown
        // — written on exit AND Ctrl+C — additionally marks the session ended,
        // so registration force-seals instead of waiting out the idle window.
        _ => NormRecord {
            timestamp: ts,
            cwd,
            is_session_end: etype == "session.shutdown" || etype == "session.end",
            ..Default::default()
        },
    }
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent_session_ingest::segment::{segment_records, SegmentParams};
    use chrono::TimeZone;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmpdir() -> PathBuf {
        static C: AtomicU64 = AtomicU64::new(0);
        let mut d = std::env::temp_dir();
        d.push(format!(
            "meridian_copilot_test_{}",
            C.fetch_add(1, Ordering::SeqCst)
        ));
        // The counter restarts every test run — clear leftovers from a
        // previous run so dir-scan assertions see only this test's files.
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ev(etype: &str, ts: &str, data: Value) -> String {
        serde_json::json!({"type": etype, "timestamp": ts, "data": data, "id": "x", "parentId": null})
            .to_string()
    }

    fn write_events(dir: &Path, uuid: &str, lines: &[String]) -> PathBuf {
        let sess = dir.join(uuid);
        std::fs::create_dir_all(&sess).unwrap();
        let p = sess.join("events.jsonl");
        let mut f = File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{}", l).unwrap();
        }
        p
    }

    fn sample_lines() -> Vec<String> {
        vec![
            ev(
                "session.start",
                "2026-05-29T06:14:06.353Z",
                serde_json::json!({"sessionId": "s", "context": {"cwd": "/repo", "branch": "main"}}),
            ),
            ev(
                "user.message",
                "2026-05-29T06:14:15.323Z",
                serde_json::json!({
                    "content": "what is an editorial?",
                    "transformedContent": "<current_datetime>…</current_datetime>\n\nwhat is an editorial?\n\n<system_reminder>…</system_reminder>"
                }),
            ),
            ev(
                "assistant.turn_start",
                "2026-05-29T06:14:15.500Z",
                serde_json::json!({}),
            ),
            ev(
                "assistant.message",
                "2026-05-29T06:14:20.000Z",
                serde_json::json!({"messageId": "m1", "model": "gpt-5-mini", "content": "An editorial is an opinion piece."}),
            ),
            ev(
                "assistant.turn_end",
                "2026-05-29T06:14:20.100Z",
                serde_json::json!({}),
            ),
        ]
    }

    #[test]
    fn parses_turns_and_skips_scaffolding() {
        let d = tmpdir();
        let p = write_events(&d, "u1", &sample_lines());
        let recs = parse_events_jsonl(&p);
        assert_eq!(recs.len(), 5);

        // session.start: non-turn, carries cwd.
        assert!(!recs[0].is_turn);
        assert_eq!(recs[0].cwd.as_deref(), Some("/repo"));

        // user.message: real prompt with RAW content (not transformedContent).
        assert!(recs[1].is_turn && recs[1].is_user && recs[1].is_user_prompt);
        assert_eq!(recs[1].body, "what is an editorial?");
        assert!(!recs[1].body.contains("system_reminder"));

        // assistant.message: labelled copilot.
        assert!(recs[3].is_turn && !recs[3].is_user);
        assert_eq!(recs[3].role_label.as_deref(), Some("copilot"));
        assert_eq!(recs[3].body, "An editorial is an opinion piece.");
    }

    #[test]
    fn segments_with_copilot_agent_tag() {
        let d = tmpdir();
        let p = write_events(&d, "u2", &sample_lines());
        let recs = parse_events_jsonl(&p);
        let (meta, segs) = segment_records(recs, "u2", AGENT, 0, &SegmentParams::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(meta.agent, AGENT);
        assert_eq!(segs[0].user_turns, 1);
        assert_eq!(segs[0].assistant_turns, 1);
        assert_eq!(segs[0].cwd.as_deref(), Some("/repo"));
        assert!(segs[0].transcript.contains("[user] what is an editorial?"));
        assert!(segs[0].transcript.contains("[copilot] An editorial"));
    }

    #[tokio::test]
    async fn register_records_writes_github_copilot_row() {
        use crate::coding_agent_session_ingest::indexer::register_records;
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;

        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();

        let d = tmpdir();
        let p = write_events(&d, "u3", &sample_lines());
        let recs = parse_events_jsonl(&p);
        // now = well after the last event → the (last) segment settles + seals.
        let now = chrono::Utc.with_ymd_and_hms(2026, 5, 29, 9, 0, 0).unwrap();
        register_records(&pool, "u3", AGENT, recs, false, now).await;

        let (app, src, method): (String, String, String) = sqlx::query_as(
            "SELECT app_name, session_text_source, task_method FROM app_sessions \
             WHERE claude_session_uuid = 'u3'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(app, "GitHub Copilot");
        assert_eq!(src, "copilot_events_jsonl");
        assert_eq!(method, "pending_summariser"); // sealed by idle
    }

    #[test]
    fn session_shutdown_marks_end_of_session() {
        let d = tmpdir();
        let mut lines = sample_lines();
        lines.push(ev(
            "session.shutdown",
            "2026-05-29T06:14:21.000Z",
            serde_json::json!({}),
        ));
        let p = write_events(&d, "u4", &lines);
        let recs = parse_events_jsonl(&p);
        assert!(recs.last().unwrap().is_session_end);
        assert!(!recs.last().unwrap().is_turn);
        // Turn records never carry the end flag.
        assert!(recs.iter().filter(|r| r.is_turn).all(|r| !r.is_session_end));
    }

    #[test]
    fn changed_sessions_discovers_today_touched_dirs() {
        let d = tmpdir();
        write_events(&d, "11111111-aaaa-bbbb-cccc-000000000001", &sample_lines());
        // A session dir without events.jsonl must be ignored.
        std::fs::create_dir_all(d.join("empty-session")).unwrap();

        let src = CopilotCliSource {
            session_state_dir: d.clone(),
        };
        assert!(src.present());
        // File mtime is "now" (just written) → today → candidate under the
        // backfill-today rule even with no stored endpoint.
        let refs = src.changed_sessions(&HashMap::new(), Utc::now());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "11111111-aaaa-bbbb-cccc-000000000001");

        // With a stored endpoint ahead of the file's mtime → not a candidate.
        let mut eps = HashMap::new();
        eps.insert(
            "11111111-aaaa-bbbb-cccc-000000000001".to_string(),
            "2099-01-01T00:00:00.000000+00:00".to_string(),
        );
        assert!(src.changed_sessions(&eps, Utc::now()).is_empty());
    }
}
