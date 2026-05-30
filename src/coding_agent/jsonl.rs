// meridian — normalises screenpipe activity into structured app sessions
//
// JSONL normalisation layer for coding-agent sessions. Claude Code and Codex
// write different on-disk event schemas; each record is normalised to a common
// `NormRecord` (timestamp, cwd, is_turn, is_user, body) so everything
// downstream (segmentation, active-time, transcript) is agent-agnostic.
//
// This is a faithful port of services/coding_agent_indexer/jsonl_meta.py
// (the `_iter_claude` / `_iter_codex` / `_format_*` helpers). Parity with that
// module is enforced by tests; change both together.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use serde::Serialize;
use serde_json::ser::Formatter;
use serde_json::Value;

/// Truncation for noisy tool_result bodies (file dumps, large search outputs).
const TOOL_RESULT_CAP: usize = 800;
const CLAUDE_ASSISTANT_LABEL: &str = "claude-code";
const CODEX_ASSISTANT_LABEL: &str = "codex";

/// One raw JSONL record normalised across Claude and Codex schemas.
#[derive(Debug, Clone)]
pub struct NormRecord {
    pub timestamp: Option<String>,
    pub cwd: Option<String>,
    pub is_turn: bool,
    pub is_user: bool,
    /// True only for a REAL human prompt (text), not a tool-result. Claude
    /// records tool outputs as `type:user` too, so `is_user` alone can't anchor
    /// a conversation boundary — `is_user_prompt` is what the time-box split
    /// aligns to so a segment begins at a genuine user message.
    pub is_user_prompt: bool,
    pub role_label: Option<String>,
    pub body: String,
}

/// Is this user `content` a genuine prompt (has text), vs a tool-result-only
/// message? A real prompt is a non-empty string or a block list containing a
/// `text` block; a tool-result message carries only `tool_result` blocks.
fn is_real_user_prompt(content: &Value) -> bool {
    if let Some(s) = content.as_str() {
        return !s.trim().is_empty();
    }
    if let Some(arr) = content.as_array() {
        return arr.iter().any(|b| {
            b.as_object()
                .and_then(|o| o.get("type"))
                .and_then(|t| t.as_str())
                == Some("text")
        });
    }
    false
}

/// Infer the agent from a JSONL path (mirrors `_infer_agent`).
pub fn infer_agent(path: &Path) -> String {
    let names: Vec<String> = path
        .ancestors()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let has = |n: &str| names.iter().any(|x| x == n);
    if has("projects") && has(".claude") {
        return "claude_code".to_string();
    }
    if has("sessions") && has(".codex") {
        return "codex".to_string();
    }
    let s = path.to_string_lossy();
    if s.contains("/.claude/") {
        return "claude_code".to_string();
    }
    if s.contains("/.codex/") {
        return "codex".to_string();
    }
    "unknown".to_string()
}

/// Read each well-formed JSON object from the file. Tolerant of partial writes,
/// malformed lines, and IO errors — they are silently skipped (mirrors
/// `_iter_records`).
fn iter_records(path: &Path) -> Vec<Value> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let raw = match line {
            Ok(l) => l,
            Err(_) => break, // read/IO error — stop, like the Python OSError path
        };
        if raw.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if v.is_object() {
                out.push(v);
            }
        }
    }
    out
}

/// Yield canonical records for one source JSONL (agent-aware).
pub fn iter_normalised(path: &Path, agent: &str) -> Vec<NormRecord> {
    let records = iter_records(path);
    if agent == "codex" {
        records.iter().map(norm_codex).collect()
    } else {
        records.iter().map(norm_claude).collect()
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// Normalise one Claude Code record (mirrors `_iter_claude`).
fn norm_claude(raw: &Value) -> NormRecord {
    let ts = str_field(raw, "timestamp").map(|s| s.to_string());
    let cwd = str_field(raw, "cwd").map(|s| s.to_string());
    let rtype = str_field(raw, "type");

    let is_sidechain = raw
        .get("isSidechain")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_meta = raw.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false);
    let is_conv = matches!(rtype, Some("user") | Some("assistant"));

    if is_sidechain || is_meta || !is_conv {
        return NormRecord {
            timestamp: ts,
            cwd,
            is_turn: false,
            is_user: false,
            is_user_prompt: false,
            role_label: None,
            body: String::new(),
        };
    }

    let msg = raw.get("message").cloned().unwrap_or(Value::Null);
    let role_raw = str_field(&msg, "role")
        .or(rtype)
        .unwrap_or("user")
        .to_string();
    let is_user = role_raw == "user";
    let label = if is_user {
        role_raw.clone()
    } else {
        CLAUDE_ASSISTANT_LABEL.to_string()
    };
    let content = msg
        .get("content")
        .cloned()
        .unwrap_or(Value::String(String::new()));
    let is_user_prompt = is_user && is_real_user_prompt(&content);
    NormRecord {
        timestamp: ts,
        cwd,
        is_turn: true,
        is_user,
        is_user_prompt,
        role_label: Some(label),
        body: format_claude_content(&content),
    }
}

/// Normalise one Codex rollout record (mirrors `_iter_codex`).
fn norm_codex(raw: &Value) -> NormRecord {
    let ts = str_field(raw, "timestamp").map(|s| s.to_string());
    let payload = raw.get("payload").cloned().unwrap_or(Value::Null);
    let cwd = str_field(&payload, "cwd").map(|s| s.to_string());

    if str_field(raw, "type") != Some("event_msg") {
        return NormRecord {
            timestamp: ts,
            cwd,
            is_turn: false,
            is_user: false,
            is_user_prompt: false,
            role_label: None,
            body: String::new(),
        };
    }

    let sub = str_field(&payload, "type");
    let message = payload
        .get("message")
        .cloned()
        .unwrap_or(Value::String(String::new()));
    match sub {
        Some("user_message") => NormRecord {
            timestamp: ts,
            cwd,
            is_turn: true,
            is_user: true,
            is_user_prompt: true, // codex user_message is always a real prompt
            role_label: Some("user".to_string()),
            body: format_codex_message(&message),
        },
        Some("agent_message") => NormRecord {
            timestamp: ts,
            cwd,
            is_turn: true,
            is_user: false,
            is_user_prompt: false,
            role_label: Some(CODEX_ASSISTANT_LABEL.to_string()),
            body: format_codex_message(&message),
        },
        _ => NormRecord {
            timestamp: ts,
            cwd,
            is_turn: false,
            is_user: false,
            is_user_prompt: false,
            role_label: None,
            body: String::new(),
        },
    }
}

/// First `n` chars (Unicode scalar values), matching Python str slicing.
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Python truthiness for a JSON value (`if inp:`).
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Formatter matching CPython `json.dumps` default separators (`", "` / `": "`),
/// so tool_use input rendering matches the Python indexer char-for-char. (Object
/// key order still differs — serde_json sorts, CPython preserves insertion order
/// — but that's reordering of identical content: same length, cosmetic.)
struct PyFormatter;

impl Formatter for PyFormatter {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }
    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
}

/// Serialize a JSON value with CPython's `json.dumps(..., ensure_ascii=False)`
/// spelling (spaced separators, raw UTF-8).
fn py_json(v: &Value) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, PyFormatter);
    if v.serialize(&mut ser).is_err() {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

/// Best-effort `str(value)` for the rare non-text branches (mirrors Python's
/// `str(block)` / `str(p)` / `str(tr)`). Exact for str/bool/number/null;
/// compact JSON for arrays/objects (a documented minor divergence from CPython
/// repr, only hit by malformed/exotic content blocks).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Null => "None".into(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Render Claude `message.content` (string or typed blocks) to text
/// (mirrors `_format_claude_content`).
fn format_claude_content(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let arr = match content.as_array() {
        Some(a) => a,
        None => return String::new(),
    };
    let mut parts: Vec<String> = Vec::new();
    for block in arr {
        let obj = match block.as_object() {
            Some(o) => o,
            None => {
                parts.push(py_str(block));
                continue;
            }
        };
        match obj.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                parts.push(
                    obj.get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                );
            }
            Some("tool_use") => {
                let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let inp_repr = match obj.get("input") {
                    Some(v) if is_truthy(v) => take_chars(&py_json(v), 400),
                    _ => String::new(),
                };
                let s = format!("[tool_use: {} {}]", name, inp_repr);
                parts.push(s.trim_end().to_string());
            }
            Some("tool_result") => {
                let tr = obj
                    .get("content")
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                let tr_str = if let Some(list) = tr.as_array() {
                    list.iter()
                        .map(|p| match p.as_object() {
                            Some(o) => o
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            None => py_str(p),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    py_str(&tr)
                };
                let tr_str = tr_str.trim().to_string();
                let capped = if char_count(&tr_str) > TOOL_RESULT_CAP {
                    format!("{}…[truncated]", take_chars(&tr_str, TOOL_RESULT_CAP))
                } else {
                    tr_str
                };
                parts.push(format!("[tool_result: {}]", capped));
            }
            Some("thinking") => {
                let t = obj.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                if !t.is_empty() {
                    parts.push(format!("[thinking] {}", t));
                }
            }
            _ => {}
        }
    }
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a Codex event_msg message payload into plain text
/// (mirrors `_format_codex_message`).
fn format_codex_message(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let arr = match content.as_array() {
        Some(a) => a,
        None => return String::new(),
    };
    let mut parts: Vec<String> = Vec::new();
    for block in arr {
        if let Some(o) = block.as_object() {
            if let Some(text) = o.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    parts.push(text.to_string());
                    continue;
                }
            }
            if let Some(nested) = o.get("content").and_then(|v| v.as_str()) {
                if !nested.is_empty() {
                    parts.push(nested.to_string());
                }
            }
        } else if let Some(s) = block.as_str() {
            parts.push(s.to_string());
        }
    }
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
