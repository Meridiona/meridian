// meridian — normalises screenpipe activity into structured app sessions
//
// Cursor Agent CLI source: `cursor-agent` (the headless CLI, both interactive
// TUI and `-p` print mode) does NOT write the IDE's vscdb — each chat gets its
// own content-addressed SQLite store:
//
//   ~/.cursor/chats/<workspace-md5>/<chat-uuid>/store.db
//     meta  (key TEXT, value TEXT)  — key '0' holds hex-encoded JSON:
//                                     {agentId, latestRootBlobId, name,
//                                      createdAt (epoch ms), …}
//     blobs (id TEXT, data BLOB)    — sha-addressed blobs; the root blob is a
//                                     protobuf whose repeated field 1 lists the
//                                     conversation blob ids IN ORDER; each
//                                     conversation blob is plain JSON
//                                     {role: system|user|assistant, content:
//                                      string | [{type:"text", text}]}
//
// Pinned against a real store (cursor-agent 2026.06.04): conversation blobs
// were all JSON; user scaffolding (<user_info>…) rides in a separate user
// blob from the real prompt (<user_query>…</user_query>); non-conversation
// blobs (timing envelopes, checkpoints) are binary and NOT referenced by the
// root's field-1 prefix.
//
// Timestamps: the store keeps none per message — only meta.createdAt and the
// file mtime. Every record is stamped createdAt except the last, which gets
// the mtime: one segment, ended_at tracks the store's change signal (so a
// resumed chat re-registers — same rule as every other source).
//
// CIRCULAR-DEPENDENCY NOTE: the summariser's own cursor-agent runs persist
// here (probed live 2026-06-06 — they do NOT touch the vscdb). The
// SUMMARY_PROMPT_MARKER guard in sources/mod.rs::sweep() is what cuts that
// loop for this source — do not remove it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;

use super::file_is_candidate;
use crate::coding_agent_session_ingest::jsonl::NormRecord;

pub const AGENT: &str = "cursor_cli";
const ASSISTANT_LABEL: &str = "cursor";

pub struct CursorCliSource {
    pub chats_dir: PathBuf,
}

impl CursorCliSource {
    pub fn from_env() -> Self {
        let raw =
            std::env::var("CURSOR_CLI_CHATS_DIR").unwrap_or_else(|_| "~/.cursor/chats".to_string());
        Self {
            chats_dir: PathBuf::from(shellexpand::tilde(&raw).into_owned()),
        }
    }

    pub fn present(&self) -> bool {
        self.chats_dir.is_dir()
    }

    /// Chats whose store.db mtime moved past the stored endpoint, as
    /// (chat uuid, store.db path, mtime epoch) triples, oldest-changed first.
    fn changed_stores(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<(String, PathBuf, f64)> {
        let mut out: Vec<(f64, String, PathBuf)> = Vec::new();
        let workspaces = match std::fs::read_dir(&self.chats_dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        for ws in workspaces.flatten() {
            let chats = match std::fs::read_dir(ws.path()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for chat in chats.flatten() {
                let uuid = match chat.file_name().to_str() {
                    Some(u) => u.to_string(),
                    None => continue,
                };
                let store = chat.path().join("store.db");
                let mtime = match store.metadata().and_then(|m| m.modified()) {
                    Ok(t) => t,
                    Err(_) => continue, // no store.db → nothing to ingest
                };
                let stored = endpoints.get(&uuid).map(String::as_str);
                if !file_is_candidate(mtime, stored, now) {
                    continue;
                }
                let epoch = mtime
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                out.push((epoch, uuid, store));
            }
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.into_iter().map(|(e, u, p)| (u, p, e)).collect()
    }

    /// Discover + load changed chats (async sqlx, one short-lived read-only
    /// pool per store — they're tiny single-chat DBs).
    pub async fn collect_changed(
        &self,
        endpoints: &HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> Vec<(String, Vec<NormRecord>, Option<String>)> {
        let mut out = Vec::new();
        for (uuid, store, mtime_epoch) in self.changed_stores(endpoints, now) {
            let (records, title) = match load_store(&store, mtime_epoch).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, store = %store.display(), "cursor_cli store load failed");
                    continue;
                }
            };
            out.push((uuid, records, title));
        }
        out
    }
}

/// Load one store.db into normalised records.
async fn load_store(
    store: &std::path::Path,
    mtime_epoch: f64,
) -> anyhow::Result<(Vec<NormRecord>, Option<String>)> {
    let uri = format!("sqlite://{}?mode=ro", store.display());
    let opts = SqliteConnectOptions::from_str(&uri)?.read_only(true);
    let pool = sqlx::pool::PoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    let result = load_from_pool(&pool, mtime_epoch).await;
    pool.close().await;
    result
}

pub(crate) async fn load_from_pool(
    pool: &SqlitePool,
    mtime_epoch: f64,
) -> anyhow::Result<(Vec<NormRecord>, Option<String>)> {
    // meta '0' → {createdAt, latestRootBlobId}. The value is hex-encoded JSON
    // (observed); tolerate plain JSON too in case a future version drops the
    // hex wrapping.
    let raw_meta: String = sqlx::query_scalar("SELECT value FROM meta WHERE key = '0'")
        .fetch_one(pool)
        .await?;
    let meta: Value = match serde_json::from_str(&raw_meta) {
        Ok(v) => v,
        Err(_) => {
            let bytes = hex_decode(raw_meta.trim())
                .ok_or_else(|| anyhow::anyhow!("meta '0' is neither JSON nor hex"))?;
            serde_json::from_slice(&bytes)?
        }
    };
    let created_ms = meta
        .get("createdAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("meta missing createdAt"))?;
    let root_id = meta
        .get("latestRootBlobId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("meta missing latestRootBlobId"))?;

    let root: Vec<u8> = sqlx::query_scalar("SELECT data FROM blobs WHERE id = ?")
        .bind(root_id)
        .fetch_one(pool)
        .await?;
    let conversation_ids = root_conversation_ids(&root);

    let created_iso = epoch_ms_to_iso(created_ms);
    let mtime_iso = epoch_ms_to_iso((mtime_epoch * 1000.0) as i64);

    let mut records = Vec::with_capacity(conversation_ids.len());
    for id in &conversation_ids {
        let blob: Option<Vec<u8>> = sqlx::query_scalar("SELECT data FROM blobs WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        let blob = match blob {
            Some(b) => b,
            None => continue,
        };
        let msg: Value = match serde_json::from_slice(&blob) {
            Ok(v) => v,
            Err(_) => continue, // non-JSON blob in the list → skip, stay tolerant
        };
        if let Some(rec) = norm_message(&msg, created_iso.clone()) {
            records.push(rec);
        }
    }
    // The store keeps no per-message clocks; the last record carries the file
    // mtime so ended_at tracks the store's change signal (resume detection).
    if let Some(last) = records.last_mut() {
        last.timestamp = mtime_iso;
    }
    // meta.name is the chat's title; 'New Agent' is the unnamed placeholder.
    let title = meta
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "New Agent")
        .map(String::from);
    Ok((records, title))
}

/// The root blob's leading repeated field 1 (32-byte sha256 ids) is the
/// conversation order. Minimal protobuf wire walk: stop at the first
/// non-field-1 tag — everything after is section metadata we don't need.
fn root_conversation_ids(root: &[u8]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut i = 0usize;
    while i < root.len() {
        let (tag, n) = match read_varint(root, i) {
            Some(t) => t,
            None => break,
        };
        if tag != 0x0A {
            break; // field 1, wire type 2 only
        }
        i += n;
        let (len, n) = match read_varint(root, i) {
            Some(t) => t,
            None => break,
        };
        i += n;
        let end = i + len as usize;
        if len != 32 || end > root.len() {
            break;
        }
        ids.push(hex_encode(&root[i..end]));
        i = end;
    }
    ids
}

fn read_varint(d: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    let mut shift = 0u32;
    let mut n = 0usize;
    loop {
        let b = *d.get(i)?;
        i += 1;
        n += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((v, n));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// One conversation blob → one record. System prompts and scaffolding-only
/// user blobs (<user_info> with no <user_query>) are dropped entirely — they
/// are injected framing, not conversation.
fn norm_message(msg: &Value, ts: Option<String>) -> Option<NormRecord> {
    let role = msg.get("role").and_then(Value::as_str)?;
    if role == "system" {
        return None;
    }
    let content = msg.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let is_user = role == "user";
    let body = if is_user {
        // The real prompt is inside <user_query>…</user_query>; a user blob
        // without one is environment scaffolding (<user_info>, git status, …).
        extract_tag(&text, "user_query")?
    } else {
        text.trim().to_string()
    };
    if body.is_empty() {
        return None;
    }
    Some(NormRecord {
        timestamp: ts,
        cwd: None, // workspace dir hash is not reversible to a path
        is_turn: true,
        is_user,
        is_user_prompt: is_user,
        role_label: Some(if is_user {
            "user".to_string()
        } else {
            ASSISTANT_LABEL.to_string()
        }),
        body,
        is_session_end: false,
    })
}

fn extract_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close).map(|e| start + e)?;
    Some(text[start..end].trim().to_string())
}

fn epoch_ms_to_iso(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent_session_ingest::segment::{segment_records, SegmentParams};

    /// 2026-06-06T08:50:00Z in epoch ms.
    const CREATED_MS: i64 = 1_780_735_800_000;
    /// store.db mtime ~5 minutes later.
    const MTIME_EPOCH: f64 = 1_780_736_100.0;

    async fn fake_store() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn put_blob(pool: &SqlitePool, id: &str, data: &[u8]) {
        sqlx::query("INSERT INTO blobs (id, data) VALUES (?, ?)")
            .bind(id)
            .bind(data)
            .execute(pool)
            .await
            .unwrap();
    }

    /// Encode a root blob: repeated field 1 of 32-byte ids, then a trailing
    /// non-field-1 section (which the walker must stop at, not choke on).
    fn encode_root(ids: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for id in ids {
            out.push(0x0A); // field 1, wire type 2
            out.push(32);
            out.extend(hex_decode(id).unwrap());
        }
        // field 6 (0x32), some metadata section the parser must ignore.
        out.extend([0x32, 0x03, 0x01, 0x02, 0x03]);
        out
    }

    fn id(n: u8) -> String {
        hex_encode(&[n; 32])
    }

    async fn seed_conversation(pool: &SqlitePool) {
        let root_id = id(0xAA);
        let meta = serde_json::json!({
            "agentId": "chat-1",
            "latestRootBlobId": root_id,
            "name": "New Agent",
            "createdAt": CREATED_MS,
        })
        .to_string();
        // meta value is hex-encoded JSON (as observed in the real store).
        sqlx::query("INSERT INTO meta (key, value) VALUES ('0', ?)")
            .bind(hex_encode(meta.as_bytes()))
            .execute(pool)
            .await
            .unwrap();

        put_blob(
            pool,
            &id(1),
            br#"{"role":"system","content":"You are an AI coding assistant."}"#,
        )
        .await;
        put_blob(
            pool,
            &id(2),
            br#"{"role":"user","content":"<user_info>OS: darwin</user_info>"}"#,
        )
        .await;
        put_blob(
            pool,
            &id(3),
            br#"{"role":"user","content":[{"type":"text","text":"<user_query>\nwhat is a WAL file?\n</user_query>"}]}"#,
        )
        .await;
        put_blob(
            pool,
            &id(4),
            br#"{"role":"assistant","content":" A WAL file is a write-ahead log."}"#,
        )
        .await;
        let root = encode_root(&[&id(1), &id(2), &id(3), &id(4)]);
        put_blob(pool, &id(0xAA), &root).await;
    }

    #[tokio::test]
    async fn loads_conversation_skipping_system_and_scaffolding() {
        let pool = fake_store().await;
        seed_conversation(&pool).await;

        let (recs, title) = load_from_pool(&pool, MTIME_EPOCH).await.unwrap();
        assert!(title.is_none(), "'New Agent' placeholder is not a title");
        assert_eq!(recs.len(), 2, "system + scaffolding dropped");

        assert!(recs[0].is_user && recs[0].is_user_prompt);
        assert_eq!(recs[0].body, "what is a WAL file?");
        assert_eq!(
            recs[0].timestamp.as_deref(),
            Some("2026-06-06T08:50:00.000Z"),
            "user turn carries meta.createdAt"
        );

        assert!(!recs[1].is_user && recs[1].is_turn);
        assert_eq!(recs[1].body, "A WAL file is a write-ahead log.");
        assert_eq!(
            recs[1].timestamp.as_deref(),
            Some("2026-06-06T08:55:00.000Z"),
            "last record carries the store mtime"
        );

        // End-to-end through the shared segmenter.
        let (meta, segs) = segment_records(recs, "chat-1", AGENT, 0, &SegmentParams::default());
        assert_eq!(meta.agent, AGENT);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].user_turns, 1);
        assert_eq!(segs[0].assistant_turns, 1);
        assert!(segs[0].transcript.contains("[user] what is a WAL file?"));
        assert!(segs[0].transcript.contains("[cursor] A WAL file"));
    }

    #[tokio::test]
    async fn missing_blob_and_binary_blob_in_list_are_tolerated() {
        let pool = fake_store().await;
        let root_id = id(0xBB);
        let meta = serde_json::json!({
            "latestRootBlobId": root_id, "createdAt": CREATED_MS,
        })
        .to_string();
        sqlx::query("INSERT INTO meta (key, value) VALUES ('0', ?)")
            .bind(hex_encode(meta.as_bytes()))
            .execute(&pool)
            .await
            .unwrap();
        // List references a missing id, a binary blob, then a real message.
        put_blob(&pool, &id(7), &[0xFF, 0x00, 0x01]).await;
        put_blob(
            &pool,
            &id(8),
            br#"{"role":"user","content":"<user_query>hi</user_query>"}"#,
        )
        .await;
        let root = encode_root(&[&id(6), &id(7), &id(8)]);
        put_blob(&pool, &id(0xBB), &root).await;

        let (recs, _) = load_from_pool(&pool, MTIME_EPOCH).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].body, "hi");
    }

    #[test]
    fn root_walker_stops_at_first_non_conversation_field() {
        let root = encode_root(&[&id(1), &id(2)]);
        let ids = root_conversation_ids(&root);
        assert_eq!(ids, vec![id(1), id(2)]);

        // Truncated root → no panic, returns what was parseable.
        let ids = root_conversation_ids(&root[..10]);
        assert!(ids.is_empty());
    }

    #[test]
    fn changed_stores_walks_workspace_dirs() {
        use std::collections::HashMap;
        let mut dir = std::env::temp_dir();
        dir.push(format!("meridian_cursor_cli_test_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let chat = dir.join("ws-hash-1").join("chat-uuid-1");
        std::fs::create_dir_all(&chat).unwrap();
        std::fs::write(chat.join("store.db"), b"placeholder").unwrap();
        // A chat dir without store.db must be ignored.
        std::fs::create_dir_all(dir.join("ws-hash-1").join("empty-chat")).unwrap();

        let src = CursorCliSource {
            chats_dir: dir.clone(),
        };
        assert!(src.present());
        let got = src.changed_stores(&HashMap::new(), Utc::now());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "chat-uuid-1");

        let mut eps = HashMap::new();
        eps.insert(
            "chat-uuid-1".to_string(),
            "2099-01-01T00:00:00.000000+00:00".to_string(),
        );
        assert!(src.changed_stores(&eps, Utc::now()).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
