// meridian — normalises screenpipe activity into structured app sessions
//
// `meridian coding-agent-hook` — the Claude Code SessionEnd hook entry point.
// Claude Code invokes it with a JSON payload on stdin describing the event;
// for SessionEnd that payload carries the transcript path. We register that one
// session immediately (sealing it), then exit 0 — a SessionEnd hook must never
// block Claude, so every failure path still returns cleanly.
//
// Port of the former Python indexer/hook.py. One-shot: opens its own
// short-lived pool against MERIDIAN_DB (the daemon already created + migrated
// it), registers, closes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::Utc;
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;

use super::indexer::register_session;

/// Run the hook. Always returns (never errors out): a SessionEnd hook that
/// returned non-zero or panicked could disrupt Claude, so we swallow every
/// failure to stderr and let the caller exit 0.
pub async fn run_hook() {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);

    let jsonl_path = match extract_jsonl_path(&payload) {
        Some(p) => p,
        None => {
            eprintln!("coding-agent-hook: could not determine JSONL path from payload");
            return;
        }
    };
    // Defense-in-depth: the payload arrives on stdin (untrusted), so a crafted
    // `transcript_path` must never let the hook open arbitrary files. Resolve
    // the path (collapsing `..`/symlinks) and require it to live under one of
    // the accepted coding-agent transcript roots before touching it.
    let jsonl_path = match jsonl_path.canonicalize() {
        Ok(resolved) if within_accepted_roots(&resolved) => resolved,
        Ok(resolved) => {
            eprintln!(
                "coding-agent-hook: transcript path outside accepted roots: {}",
                resolved.display()
            );
            return;
        }
        Err(_) => {
            eprintln!(
                "coding-agent-hook: transcript not found: {}",
                jsonl_path.display()
            );
            return;
        }
    };

    let db_path = meridian_db_path();
    let uri = format!("sqlite://{}", db_path.display());
    let opts = match SqliteConnectOptions::from_str(&uri) {
        Ok(o) => o.create_if_missing(false),
        Err(e) => {
            eprintln!("coding-agent-hook: bad db uri {}: {}", uri, e);
            return;
        }
    };
    let pool = match SqlitePool::connect_with(opts).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "coding-agent-hook: open {} failed: {}",
                db_path.display(),
                e
            );
            return;
        }
    };

    let outcome = register_session(&pool, &jsonl_path, true, Utc::now()).await;
    pool.close().await;
    eprintln!(
        "coding-agent-hook: {:?} for {}",
        outcome,
        jsonl_path.display()
    );
}

/// Tease the JSONL path out of the payload (mirrors `_extract_jsonl_path`):
/// `transcript_path` / `jsonl_path` directly, else construct from
/// `session_id` + `cwd` via Claude's project-dir convention.
fn extract_jsonl_path(payload: &Value) -> Option<PathBuf> {
    let direct = payload
        .get("jsonl_path")
        .or_else(|| payload.get("transcript_path"))
        .and_then(|v| v.as_str());
    if let Some(p) = direct {
        return Some(expand(p));
    }

    let session_id = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(|v| v.as_str());
    let cwd = payload
        .get("cwd")
        .or_else(|| payload.get("project_cwd"))
        .and_then(|v| v.as_str());
    if let (Some(sid), Some(cwd)) = (session_id, cwd) {
        let sanitized = format!("-{}", cwd.replace('/', "-"));
        let base = expand("~/.claude/projects");
        return Some(base.join(sanitized).join(format!("{}.jsonl", sid)));
    }
    None
}

/// The only directories the hook is allowed to read transcripts from. Anything
/// outside these is rejected (path-traversal / arbitrary-file-read defense).
fn accepted_roots() -> [PathBuf; 2] {
    [expand("~/.claude/projects"), expand("~/.codex/sessions")]
}

/// True iff `resolved` (an already-canonicalised path) lives under an accepted
/// root. Roots are canonicalised too so symlinked homes compare correctly; a
/// non-existent root simply never matches.
fn within_accepted_roots(resolved: &Path) -> bool {
    accepted_roots().iter().any(|root| {
        root.canonicalize()
            .map(|r| resolved.starts_with(r))
            .unwrap_or(false)
    })
}

fn meridian_db_path() -> PathBuf {
    let raw =
        std::env::var("MERIDIAN_DB").unwrap_or_else(|_| "~/.meridian/meridian.db".to_string());
    expand(&raw)
}

fn expand(p: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(p).into_owned())
}
