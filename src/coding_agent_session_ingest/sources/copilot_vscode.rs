// meridian — normalises screenpipe activity into structured app sessions
//
// GitHub Copilot Chat (VS Code) source. VS Code persists each chat session as
// an op-log JSONL:
//
//   workspaceStorage/<ws-hash>/chatSessions/<session-uuid>.jsonl
//   globalStorage/emptyWindowChatSessions/<session-uuid>.jsonl
//
// Line 1 is a full snapshot; later lines mutate it:
//
//   {"kind":0, "v":{...session state: requests[], customTitle, ...}}
//   {"kind":1, "k":["requests",0,"modelState"], "v":{...}}     SET at path
//   {"kind":2, "k":["requests",0,"response"], "v":[...parts]}  APPEND at path
//
// New user turns arrive as kind-2 appends to ["requests"]; streaming response
// parts as kind-2 appends to ["requests",N,"response"]. Replaying the ops
// rebuilds the final session state, from which requests[] yields the
// conversation: message.text + timestamp (epoch ms) per user turn,
// modelState.completedAt + response parts per assistant turn. Response parts
// are typed by `kind`: plain text parts carry NO kind (markdown `value`),
// thinking / toolInvocationSerialized / textEditGroup are rendered in the
// Claude transcript dialect; UI-only parts (undoStop, codeblockUri,
// inlineReference, …) are skipped.
//
// Pinned against a real store (VS Code 1.10x / copilot-chat 0.4x, 2026-06):
// every chatSessions file opened with kind-0; multi-request sessions appended
// requests via kind-2 at ["requests"]; plain-text response parts had no kind.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::file_is_candidate;
use crate::coding_agent_session_ingest::jsonl::NormRecord;

pub const AGENT: &str = "copilot_vscode";
const ASSISTANT_LABEL: &str = "copilot";
/// Caps mirror the Claude JSONL renderer: tool messages 400 chars.
const TOOL_MSG_CAP: usize = 400;

#[derive(Clone)]
pub struct CopilotVscodeSource {
    pub user_dir: PathBuf,
}

impl CopilotVscodeSource {
    pub fn from_env() -> Self {
        let raw = std::env::var("VSCODE_USER_DIR")
            .unwrap_or_else(|_| "~/Library/Application Support/Code/User".to_string());
        Self {
            user_dir: PathBuf::from(shellexpand::tilde(&raw).into_owned()),
        }
    }

    pub fn present(&self) -> bool {
        self.user_dir.is_dir()
    }

    /// Chat-session files whose mtime moved past the stored endpoint, as
    /// (session_uuid, file path) pairs, oldest-changed first. Walks every
    /// workspace's chatSessions dir plus the empty-window global dir.
    pub fn changed_sessions(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<(String, PathBuf)> {
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = fs::read_dir(self.user_dir.join("workspaceStorage")) {
            for entry in entries.flatten() {
                let dir = entry.path().join("chatSessions");
                if dir.is_dir() {
                    roots.push(dir);
                }
            }
        }
        let empty_window = self
            .user_dir
            .join("globalStorage")
            .join("emptyWindowChatSessions");
        if empty_window.is_dir() {
            roots.push(empty_window);
        }

        let mut out: Vec<(f64, String, PathBuf)> = Vec::new();
        for root in roots {
            let entries = match fs::read_dir(&root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let uuid = match path.file_stem().and_then(|n| n.to_str()) {
                    Some(u) => u.to_string(),
                    None => continue,
                };
                let mtime = match path.metadata().and_then(|m| m.modified()) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let stored = endpoints.get(&uuid).map(String::as_str);
                if !file_is_candidate(mtime, stored, now) {
                    continue;
                }
                let epoch = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                out.push((epoch, uuid, path));
            }
        }
        // Oldest-changed first, matching the JSONL indexer's ordering.
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.into_iter().map(|(_, u, p)| (u, p)).collect()
    }
}

/// Parse one chat-session op-log into normalised records: replay the ops,
/// then walk the rebuilt requests[]. Tolerant of malformed lines (skipped)
/// and unknown op kinds (ignored) — a partially-written log still yields the
/// turns recorded so far.
pub(crate) fn parse_chat_jsonl(path: &Path) -> Vec<NormRecord> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let session = replay_ops(&raw);
    norm_requests(&session)
}

/// Replay the op-log into the final session state.
fn replay_ops(raw: &str) -> Value {
    let mut state = Value::Object(serde_json::Map::new());
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let op: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = op.get("kind").and_then(Value::as_i64);
        match kind {
            Some(0) => {
                if let Some(v) = op.get("v") {
                    state = v.clone();
                }
            }
            Some(1) => {
                if let (Some(path), Some(v)) = (op.get("k").and_then(Value::as_array), op.get("v"))
                {
                    apply_set(&mut state, path, v.clone());
                }
            }
            Some(2) => {
                if let (Some(path), Some(Value::Array(items))) =
                    (op.get("k").and_then(Value::as_array), op.get("v"))
                {
                    apply_append(&mut state, path, items.clone());
                }
            }
            _ => {} // unknown op kind → skip, stay tolerant
        }
    }
    state
}

/// Walk `path` to the parent of its last element. A missing key/index aborts
/// the op (None) rather than creating structure — a set/append whose target
/// was never snapshotted is dropped, matching the "tolerant of partial logs"
/// contract.
fn resolve_parent<'a>(root: &'a mut Value, path: &[Value]) -> Option<&'a mut Value> {
    let mut cur = root;
    for step in &path[..path.len().saturating_sub(1)] {
        cur = match step {
            Value::String(key) => cur.get_mut(key.as_str())?,
            Value::Number(n) => cur.get_mut(n.as_u64()? as usize)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn apply_set(root: &mut Value, path: &[Value], v: Value) {
    let Some(last) = path.last() else { return };
    let Some(parent) = resolve_parent(root, path) else {
        return;
    };
    match (last, parent) {
        (Value::String(key), Value::Object(map)) => {
            map.insert(key.clone(), v);
        }
        (Value::Number(n), Value::Array(arr)) => {
            if let Some(i) = n.as_u64().map(|i| i as usize) {
                if i < arr.len() {
                    arr[i] = v;
                } else if i == arr.len() {
                    arr.push(v);
                }
            }
        }
        _ => {}
    }
}

fn apply_append(root: &mut Value, path: &[Value], items: Vec<Value>) {
    let mut cur = root;
    for step in path {
        cur = match step {
            Value::String(key) => match cur.get_mut(key.as_str()) {
                Some(v) => v,
                None => return,
            },
            Value::Number(n) => match n.as_u64().and_then(|i| cur.get_mut(i as usize)) {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
    }
    if let Value::Array(arr) = cur {
        arr.extend(items);
    }
}

/// requests[] → records: one user record + one assistant record per request.
fn norm_requests(session: &Value) -> Vec<NormRecord> {
    let requests = match session.get("requests").and_then(Value::as_array) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(requests.len() * 2);
    for req in requests {
        let user_ts = req.get("timestamp").and_then(Value::as_i64);
        let user_text = req
            .get("message")
            .and_then(|m| m.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let has_prompt = !user_text.trim().is_empty();
        out.push(NormRecord {
            timestamp: user_ts.and_then(epoch_ms_to_iso),
            cwd: None, // not stored per-session; the workspace hash is opaque
            is_turn: has_prompt,
            is_user: true,
            is_user_prompt: has_prompt,
            role_label: has_prompt.then(|| "user".to_string()),
            body: user_text,
        });

        let body = render_response(req.get("response").and_then(Value::as_array));
        // Assistant timing: completedAt when the turn finished; else fall back
        // to the request timestamp so the record still anchors inside the
        // segment (an in-flight turn has no completion stamp yet).
        let done_ts = req
            .get("modelState")
            .and_then(|m| m.get("completedAt"))
            .and_then(Value::as_i64)
            .or(user_ts);
        let has_body = !body.is_empty();
        out.push(NormRecord {
            timestamp: done_ts.and_then(epoch_ms_to_iso),
            cwd: None,
            is_turn: has_body,
            is_user: false,
            is_user_prompt: false,
            role_label: has_body.then(|| ASSISTANT_LABEL.to_string()),
            body,
        });
    }
    out
}

/// Render response parts in the Claude transcript dialect (same spelling as
/// cursor.rs / jsonl.rs so the summariser sees one dialect across agents).
fn render_response(parts: Option<&Vec<Value>>) -> String {
    let parts = match parts {
        Some(p) => p,
        None => return String::new(),
    };
    let mut rendered: Vec<String> = Vec::new();
    for p in parts {
        let kind = p.get("kind").and_then(Value::as_str);
        match kind {
            // Plain markdown text parts carry no `kind`.
            None => {
                if let Some(text) = p.get("value").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        rendered.push(text.to_string());
                    }
                }
            }
            Some("thinking") => {
                // `value` is a string mid-stream but can be an array shell in
                // some snapshots — only the string form carries prose.
                if let Some(text) = p.get("value").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        rendered.push(format!("[thinking] {}", text));
                    }
                }
            }
            Some("toolInvocationSerialized") => {
                let tool = p.get("toolId").and_then(Value::as_str).unwrap_or("?");
                // invocationMessage is a plain string or a {value} object.
                let msg = p
                    .get("invocationMessage")
                    .map(|m| match m {
                        Value::String(s) => s.clone(),
                        other => other
                            .get("value")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                    .unwrap_or_default();
                rendered.push(
                    format!("[tool_use: {} {}]", tool, take_chars(&msg, TOOL_MSG_CAP))
                        .trim_end()
                        .to_string(),
                );
            }
            Some("textEditGroup") => {
                if let Some(path) = p
                    .get("uri")
                    .and_then(|u| u.get("fsPath"))
                    .and_then(Value::as_str)
                {
                    rendered.push(format!("[edit: {}]", path));
                }
            }
            // undoStop, codeblockUri, inlineReference, references, … are UI
            // bookkeeping — no transcript value.
            _ => {}
        }
    }
    rendered.join("\n")
}

/// Epoch milliseconds → canonical ISO-8601 (`2026-06-05T08:00:00.000Z`).
fn epoch_ms_to_iso(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent_session_ingest::segment::{segment_records, SegmentParams};
    use serde_json::json;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmpdir() -> PathBuf {
        static C: AtomicU64 = AtomicU64::new(0);
        let mut d = std::env::temp_dir();
        d.push(format!(
            "meridian_copilot_vscode_test_{}",
            C.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Base instant (epoch ms) for synthetic sessions: 2026-06-05T08:00:00Z.
    const T0: i64 = 1_780_646_400_000;

    fn request(ts: i64, text: &str) -> Value {
        json!({
            "requestId": format!("request_{ts}"),
            "timestamp": ts,
            "message": {"text": text},
            "response": [],
            "modelId": "copilot/auto",
        })
    }

    /// An op-log exercising every op kind: snapshot with one request, streamed
    /// response parts (kind 2 at requests.0.response), completion stamp
    /// (kind 1 set), then a second request appended (kind 2 at requests).
    fn sample_oplog() -> String {
        let lines = vec![
            json!({"kind": 0, "v": {
                "version": 3, "sessionId": "s1", "creationDate": T0,
                "customTitle": "fix the bug",
                "requests": [request(T0, "fix the login bug")],
            }}),
            json!({"kind": 2, "k": ["requests", 0, "response"], "v": [
                {"kind": "thinking", "value": "the null check is missing"},
                {"kind": "toolInvocationSerialized",
                 "invocationMessage": "Reading auth.ts",
                 "toolId": "copilot_readFile", "isComplete": true},
            ]}),
            json!({"kind": 2, "k": ["requests", 0, "response"], "v": [
                {"kind": "textEditGroup", "uri": {"fsPath": "/repo/auth.ts"}, "edits": []},
                {"value": "Added the null check in auth.ts.", "supportThemeIcons": false},
                {"kind": "undoStop", "id": "u1"},
            ]}),
            json!({"kind": 1, "k": ["requests", 0, "modelState"],
                   "v": {"value": 1, "completedAt": T0 + 30_000}}),
            // Second user turn appended mid-session.
            json!({"kind": 2, "k": ["requests"], "v": [request(T0 + 60_000, "now add a test")]}),
            json!({"kind": 2, "k": ["requests", 1, "response"], "v": [
                {"value": "Added auth.test.ts with the regression test."},
            ]}),
            json!({"kind": 1, "k": ["requests", 1, "modelState"],
                   "v": {"value": 1, "completedAt": T0 + 95_000}}),
        ];
        lines
            .into_iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn write_session(user_dir: &Path, ws: &str, uuid: &str, content: &str) -> PathBuf {
        let dir = user_dir
            .join("workspaceStorage")
            .join(ws)
            .join("chatSessions");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{uuid}.jsonl"));
        let mut f = std::fs::File::create(&p).unwrap();
        write!(f, "{}", content).unwrap();
        p
    }

    #[test]
    fn replays_oplog_into_full_conversation() {
        let d = tmpdir();
        let p = write_session(&d, "ws1", "11111111-aaaa", &sample_oplog());
        let recs = parse_chat_jsonl(&p);
        assert_eq!(recs.len(), 4, "2 requests → 2 user + 2 assistant records");

        // Request 0: user prompt at T0, assistant completed at T0+30s.
        assert!(recs[0].is_user && recs[0].is_user_prompt);
        assert_eq!(recs[0].body, "fix the login bug");
        assert_eq!(
            recs[0].timestamp.as_deref(),
            Some("2026-06-05T08:00:00.000Z")
        );
        assert!(!recs[1].is_user && recs[1].is_turn);
        assert_eq!(
            recs[1].timestamp.as_deref(),
            Some("2026-06-05T08:00:30.000Z")
        );
        assert!(recs[1]
            .body
            .contains("[thinking] the null check is missing"));
        assert!(recs[1]
            .body
            .contains("[tool_use: copilot_readFile Reading auth.ts]"));
        assert!(recs[1].body.contains("[edit: /repo/auth.ts]"));
        assert!(recs[1].body.contains("Added the null check in auth.ts."));
        assert!(!recs[1].body.contains("undoStop"), "UI parts skipped");

        // Request 1 arrived purely via kind-2 append at ["requests"].
        assert_eq!(recs[2].body, "now add a test");
        assert!(recs[3].body.contains("auth.test.ts"));
        assert_eq!(
            recs[3].timestamp.as_deref(),
            Some("2026-06-05T08:01:35.000Z")
        );

        // End-to-end through the shared segmenter.
        let (meta, segs) =
            segment_records(recs, "11111111-aaaa", AGENT, 0, &SegmentParams::default());
        assert_eq!(meta.agent, AGENT);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].user_turns, 2);
        assert_eq!(segs[0].assistant_turns, 2);
        assert!(segs[0].transcript.contains("[user] fix the login bug"));
        assert!(segs[0].transcript.contains("[copilot] [thinking]"));
    }

    #[test]
    fn snapshot_only_log_and_malformed_lines_are_tolerated() {
        let d = tmpdir();
        // Single-line snapshot (a freshly-opened chat), then junk.
        let content = format!(
            "{}\nnot json at all\n{}",
            json!({"kind": 0, "v": {"requests": [request(T0, "hello copilot")]}}),
            json!({"kind": 9, "v": "future op kind"}),
        );
        let p = write_session(&d, "ws1", "22222222-bbbb", &content);
        let recs = parse_chat_jsonl(&p);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].body, "hello copilot");
        // No response yet → assistant record is a timing-only placeholder.
        assert!(!recs[1].is_turn && recs[1].body.is_empty());
    }

    #[test]
    fn ops_against_missing_paths_are_dropped_not_fatal() {
        let d = tmpdir();
        let content = format!(
            "{}\n{}\n{}",
            json!({"kind": 0, "v": {"requests": [request(T0, "q")]}}),
            // Set + append against request index 5, which doesn't exist.
            json!({"kind": 1, "k": ["requests", 5, "modelState"], "v": {"completedAt": T0}}),
            json!({"kind": 2, "k": ["requests", 5, "response"], "v": [{"value": "lost"}]}),
        );
        let p = write_session(&d, "ws1", "33333333-cccc", &content);
        let recs = parse_chat_jsonl(&p);
        assert_eq!(recs.len(), 2, "the one real request survives");
        assert!(!recs.iter().any(|r| r.body.contains("lost")));
    }

    #[test]
    fn changed_sessions_walks_workspace_and_empty_window_dirs() {
        let d = tmpdir();
        write_session(&d, "ws1", "44444444-dddd", &sample_oplog());
        write_session(&d, "ws2", "55555555-eeee", &sample_oplog());
        // Empty-window chats live under globalStorage, not workspaceStorage.
        let ew = d.join("globalStorage").join("emptyWindowChatSessions");
        std::fs::create_dir_all(&ew).unwrap();
        std::fs::write(ew.join("66666666-ffff.jsonl"), sample_oplog()).unwrap();
        // Non-jsonl files are ignored.
        std::fs::write(ew.join("notes.txt"), "ignore me").unwrap();

        let src = CopilotVscodeSource { user_dir: d };
        assert!(src.present());
        let got = src.changed_sessions(&HashMap::new(), Utc::now());
        let uuids: Vec<&str> = got.iter().map(|(u, _)| u.as_str()).collect();
        assert_eq!(got.len(), 3);
        assert!(uuids.contains(&"44444444-dddd"));
        assert!(uuids.contains(&"55555555-eeee"));
        assert!(uuids.contains(&"66666666-ffff"));

        // Endpoint ahead of mtime → no longer a candidate.
        let mut eps = HashMap::new();
        for u in &uuids {
            eps.insert(
                u.to_string(),
                "2099-01-01T00:00:00.000000+00:00".to_string(),
            );
        }
        assert!(src.changed_sessions(&eps, Utc::now()).is_empty());
    }
}
