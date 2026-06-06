// meridian — normalises screenpipe activity into structured app sessions
//
// Cursor source: agent conversations live in Cursor's global KV store
// (`~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`,
// table cursorDiskKV), NOT in per-session files:
//
//   composerData:<composerId>   session metadata — name, createdAt /
//                               lastUpdatedAt (epoch ms), and
//                               fullConversationHeadersOnly: the ORDERED
//                               bubble list (the conversation's authority —
//                               key order in the KV table is hash order)
//   bubbleId:<composerId>:<id>  one message — type (1=user, 2=assistant),
//                               text, ISO createdAt, thinking.text,
//                               toolFormerData {name, rawArgs, result, …}
//
// Cursor also writes per-session agent-transcript JSONLs under
// ~/.cursor/projects/, but those carry NO timestamps — useless for
// segmentation. The vscdb is the only timestamped store, hence the sqlx
// read-only attach (WAL, same pattern as the screenpipe DB).
//
// Pinned against a real store (Cursor 2.x, 2026-06): headers order matched
// bubble-createdAt order on every session; all bubbles of all 19 composers
// were present in the global store (workspace vscdbs held none); empty
// `fullConversationHeadersOnly` marks a draft composer (skipped).

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;

use super::epoch_is_candidate;
use crate::coding_agent_session_ingest::jsonl::NormRecord;

pub const AGENT: &str = "cursor";
const ASSISTANT_LABEL: &str = "cursor";
/// Caps mirror the Claude JSONL renderer (jsonl.rs): tool args 400, result 800.
const TOOL_ARGS_CAP: usize = 400;
const TOOL_RESULT_CAP: usize = 800;

pub struct CursorSource {
    pub vscdb_path: PathBuf,
}

impl CursorSource {
    pub fn from_env() -> Self {
        let raw = std::env::var("CURSOR_STATE_VSCDB").unwrap_or_else(|_| {
            "~/Library/Application Support/Cursor/User/globalStorage/state.vscdb".to_string()
        });
        Self {
            vscdb_path: PathBuf::from(shellexpand::tilde(&raw).into_owned()),
        }
    }

    pub fn present(&self) -> bool {
        let exists = self.vscdb_path.is_file();
        tracing::debug!(
            path = %self.vscdb_path.display(),
            exists,
            "cursor vscdb device gate check"
        );
        exists
    }

    /// Discover + load changed sessions in one pass (one short-lived read-only
    /// pool per sweep; Cursor holds the write side — WAL readers don't block).
    pub async fn collect_changed(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<(String, Vec<NormRecord>, Option<String>)> {
        let pool = match self.open_ro().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, db = %self.vscdb_path.display(), "cursor vscdb open failed");
                return Vec::new();
            }
        };
        let out = collect_from_pool(&pool, endpoints, now).await;
        pool.close().await;
        out
    }

    async fn open_ro(&self) -> anyhow::Result<SqlitePool> {
        let uri = format!("sqlite://{}?mode=ro", self.vscdb_path.display());
        let opts = SqliteConnectOptions::from_str(&uri)?.read_only(true);
        Ok(sqlx::pool::PoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?)
    }
}

/// The store walk, separated from pool setup so tests can drive a synthetic
/// vscdb. Returns (composer uuid, records) pairs, oldest-changed first.
pub(crate) async fn collect_from_pool(
    pool: &SqlitePool,
    endpoints: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> Vec<(String, Vec<NormRecord>, Option<String>)> {
    // Self-ingest protection (cursor-agent persisting its own summary runs)
    // lives in `sources::sweep()` via SUMMARY_PROMPT_MARKER — not here. An env
    // check would be dead code: MERIDIAN_SUMMARISER is set on summariser
    // CHILD processes, never on the daemon process this runs in.
    tracing::debug!("cursor collect_from_pool: scanning composerData entries");
    let rows: Vec<(String, String)> =
        match sqlx::query_as("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'")
            .fetch_all(pool)
            .await
        {
            Ok(r) => {
                tracing::debug!(count = r.len(), "cursor found composerData rows");
                r
            }
            Err(e) => {
                tracing::warn!(error = %e, "cursor composerData scan failed");
                return Vec::new();
            }
        };

    // Filter to changed composers first (cheap — metadata only).
    let mut changed: Vec<(f64, String, Value)> = Vec::new();
    for (key, value) in rows {
        let uuid = match key.strip_prefix("composerData:") {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => {
                tracing::debug!("cursor: skipping row with invalid key");
                continue;
            }
        };
        let data: Value = match serde_json::from_str(&value) {
            Ok(v) => v,
            Err(_) => {
                tracing::debug!(uuid = %uuid, "cursor: skipping row with invalid JSON");
                continue;
            }
        };
        // lastUpdatedAt is None until the first reply lands; fall back to
        // createdAt so a brand-new conversation is still picked up.
        let updated_ms = data
            .get("lastUpdatedAt")
            .and_then(Value::as_f64)
            .or_else(|| data.get("createdAt").and_then(Value::as_f64));
        let updated_epoch = match updated_ms {
            Some(ms) => ms / 1000.0,
            None => {
                tracing::debug!(uuid = %uuid, "cursor: skipping row with no timestamp");
                continue;
            }
        };
        let stored = endpoints.get(&uuid).map(String::as_str);
        if !epoch_is_candidate(updated_epoch, stored, now) {
            tracing::debug!(uuid = %uuid, updated_epoch = updated_epoch, "cursor: not a candidate (already processed or too old)");
            continue;
        }
        // Draft composers carry no conversation yet.
        let has_headers = data
            .get("fullConversationHeadersOnly")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if !has_headers {
            tracing::debug!(uuid = %uuid, "cursor: skipping draft composer (no headers)");
            continue;
        }
        tracing::debug!(uuid = %uuid, "cursor: composer is a candidate");
        changed.push((updated_epoch, uuid, data));
    }
    changed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    tracing::debug!(candidates = changed.len(), "cursor: filtered to candidates");

    let mut out: Vec<(String, Vec<NormRecord>, Option<String>)> = Vec::new();
    for (_, uuid, data) in changed {
        let records = load_composer(pool, &uuid, &data).await;
        // Cursor auto-names conversations after the first exchange
        // (composerData.name); absent on drafts and some IDE-agent runs.
        let title = data
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from);
        tracing::debug!(uuid = %uuid, record_count = records.len(), "cursor: loaded composer");
        out.push((uuid, records, title));
    }
    tracing::debug!(loaded = out.len(), "cursor collect_from_pool complete");
    out
}

/// Walk fullConversationHeadersOnly (the authoritative order) and normalise
/// each bubble. A missing bubble row is skipped, not fatal.
async fn load_composer(pool: &SqlitePool, composer_id: &str, data: &Value) -> Vec<NormRecord> {
    let headers = match data
        .get("fullConversationHeadersOnly")
        .and_then(Value::as_array)
    {
        Some(h) => h,
        None => return Vec::new(),
    };

    let mut records = Vec::with_capacity(headers.len());
    for h in headers {
        let bubble_id = match h.get("bubbleId").and_then(Value::as_str) {
            Some(b) => b,
            None => continue,
        };
        let key = format!("bubbleId:{}:{}", composer_id, bubble_id);
        let raw: Option<String> =
            match sqlx::query_scalar("SELECT value FROM cursorDiskKV WHERE key = ?")
                .bind(&key)
                .fetch_optional(pool)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, key = %key, "cursor bubble fetch failed");
                    continue;
                }
            };
        let bubble: Value = match raw.and_then(|s| serde_json::from_str(&s).ok()) {
            Some(v) => v,
            None => continue,
        };
        records.push(norm_bubble(&bubble));
    }
    records
}

/// Normalise one bubble. type 1 = user, 2 = assistant. The body collects the
/// message text plus thinking / tool activity in the Claude renderer's
/// `[thinking] …` / `[tool_use: …]` / `[tool_result: …]` spelling, so the
/// summariser sees one consistent transcript dialect across agents. A bubble
/// with no body at all (UI scaffolding) anchors timing only.
fn norm_bubble(raw: &Value) -> NormRecord {
    let ts = raw
        .get("createdAt")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let is_user = raw.get("type").and_then(Value::as_i64) == Some(1);

    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = raw.get("text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            parts.push(text.to_string());
        }
    }
    if let Some(th) = raw
        .get("thinking")
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
    {
        if !th.trim().is_empty() {
            parts.push(format!("[thinking] {}", th));
        }
    }
    if let Some(tfd) = raw.get("toolFormerData").and_then(Value::as_object) {
        let name = tfd.get("name").and_then(Value::as_str).unwrap_or("?");
        // rawArgs / result are JSON-encoded STRINGS in the store.
        let args = tfd
            .get("rawArgs")
            .and_then(Value::as_str)
            .map(|s| take_chars(s, TOOL_ARGS_CAP))
            .unwrap_or_default();
        parts.push(
            format!("[tool_use: {} {}]", name, args)
                .trim_end()
                .to_string(),
        );
        if let Some(res) = tfd.get("result").and_then(Value::as_str) {
            let trimmed = res.trim();
            if !trimmed.is_empty() {
                let capped = if trimmed.chars().count() > TOOL_RESULT_CAP {
                    format!("{}…[truncated]", take_chars(trimmed, TOOL_RESULT_CAP))
                } else {
                    trimmed.to_string()
                };
                parts.push(format!("[tool_result: {}]", capped));
            }
        }
    }

    let body = parts.join("\n");
    let has_body = !body.is_empty();
    let is_user_prompt = is_user
        && raw
            .get("text")
            .and_then(Value::as_str)
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
    NormRecord {
        timestamp: ts,
        cwd: None, // not stored on rows; Cursor keeps no per-session cwd field
        is_turn: has_body,
        is_user,
        is_user_prompt,
        role_label: if has_body {
            Some(if is_user {
                "user".to_string()
            } else {
                ASSISTANT_LABEL.to_string()
            })
        } else {
            None
        },
        body,
        is_session_end: false,
    }
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent_session_ingest::segment::{segment_records, SegmentParams};
    use chrono::TimeZone;

    /// In-memory stand-in for Cursor's cursorDiskKV table.
    async fn fake_vscdb() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn put(pool: &SqlitePool, key: &str, value: &Value) {
        sqlx::query("INSERT OR REPLACE INTO cursorDiskKV (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value.to_string())
            .execute(pool)
            .await
            .unwrap();
    }

    fn bubble(btype: i64, ts: &str, text: &str) -> Value {
        serde_json::json!({"type": btype, "createdAt": ts, "text": text})
    }

    /// Base instant for the synthetic store; "now" derives from it so the
    /// backfill-today rule sees the data as fresh.
    fn base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 5, 8, 0, 0).unwrap()
    }

    async fn seed_session(pool: &SqlitePool, cid: &str) {
        let t0 = base().timestamp_millis();
        put(
            pool,
            &format!("composerData:{cid}"),
            &serde_json::json!({
                "composerId": cid,
                "name": "fix the bug",
                "createdAt": t0,
                "lastUpdatedAt": t0 + 60_000,
                "fullConversationHeadersOnly": [
                    {"bubbleId": "b1", "type": 1},
                    {"bubbleId": "b2", "type": 2},
                    {"bubbleId": "b3", "type": 2},
                    {"bubbleId": "b4", "type": 2},
                ],
            }),
        )
        .await;
        put(
            pool,
            &format!("bubbleId:{cid}:b1"),
            &bubble(1, "2026-06-05T08:00:00.000Z", "fix the login bug"),
        )
        .await;
        // Tool bubble: no text, toolFormerData only (rawArgs/result are JSON strings).
        put(
            pool,
            &format!("bubbleId:{cid}:b2"),
            &serde_json::json!({
                "type": 2, "createdAt": "2026-06-05T08:00:10.000Z", "text": "",
                "toolFormerData": {
                    "name": "read_file_v2", "status": "completed",
                    "rawArgs": "{\"path\":\"/repo/auth.ts\"}",
                    "result": "{\"contents\":\"…\",\"totalLinesInFile\":42}"
                }
            }),
        )
        .await;
        // Thinking-only bubble.
        put(
            pool,
            &format!("bubbleId:{cid}:b3"),
            &serde_json::json!({
                "type": 2, "createdAt": "2026-06-05T08:00:20.000Z", "text": "",
                "thinking": {"text": "the null check is missing"}
            }),
        )
        .await;
        put(
            pool,
            &format!("bubbleId:{cid}:b4"),
            &bubble(
                2,
                "2026-06-05T08:00:30.000Z",
                "Added the null check in auth.ts.",
            ),
        )
        .await;
    }

    #[tokio::test]
    async fn collects_session_in_header_order_with_tool_rendering() {
        let pool = fake_vscdb().await;
        seed_session(&pool, "c1").await;
        // A draft composer (no headers) must be skipped.
        put(
            &pool,
            "composerData:draft",
            &serde_json::json!({"composerId": "draft", "createdAt": base().timestamp_millis(),
                "lastUpdatedAt": null, "fullConversationHeadersOnly": []}),
        )
        .await;

        let now = base() + chrono::Duration::hours(1);
        let got = collect_from_pool(&pool, &HashMap::new(), now).await;
        assert_eq!(got.len(), 1, "draft skipped, real session collected");
        let (uuid, records, title) = &got[0];
        assert_eq!(uuid, "c1");
        assert_eq!(
            title.as_deref(),
            Some("fix the bug"),
            "composerData.name becomes the title"
        );
        assert_eq!(records.len(), 4);

        // Header order preserved; roles + prompt flags right.
        assert!(records[0].is_user && records[0].is_user_prompt);
        assert_eq!(records[0].body, "fix the login bug");
        assert!(!records[1].is_user && records[1].is_turn);
        assert!(records[1]
            .body
            .contains("[tool_use: read_file_v2 {\"path\":\"/repo/auth.ts\"}]"));
        assert!(records[1].body.contains("[tool_result: {\"contents\""));
        assert!(records[2]
            .body
            .contains("[thinking] the null check is missing"));
        assert_eq!(records[3].body, "Added the null check in auth.ts.");

        // End-to-end through the shared segmenter.
        let (_meta, segs) =
            segment_records(records.clone(), uuid, AGENT, 0, &SegmentParams::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].user_turns, 1);
        assert_eq!(segs[0].assistant_turns, 3);
        assert!(segs[0].transcript.contains("[user] fix the login bug"));
        assert!(segs[0].transcript.contains("[cursor] Added the null check"));
    }

    #[tokio::test]
    async fn endpoint_ahead_of_last_update_skips_session() {
        let pool = fake_vscdb().await;
        seed_session(&pool, "c2").await;
        let mut eps = HashMap::new();
        eps.insert(
            "c2".to_string(),
            "2099-01-01T00:00:00.000000+00:00".to_string(),
        );
        let now = base() + chrono::Duration::hours(1);
        assert!(collect_from_pool(&pool, &eps, now).await.is_empty());
    }

    #[tokio::test]
    async fn stale_never_seen_session_skipped_by_backfill_today_rule() {
        let pool = fake_vscdb().await;
        seed_session(&pool, "c3").await;
        // "now" is 10 days after the session's lastUpdatedAt → not today → skip.
        let now = base() + chrono::Duration::days(10);
        assert!(collect_from_pool(&pool, &HashMap::new(), now)
            .await
            .is_empty());
    }
}
