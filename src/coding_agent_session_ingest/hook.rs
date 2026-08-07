//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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

use chrono::Utc;
use serde_json::Value;

use super::indexer::register_session;

/// Run the hook. Always returns (never errors out): a SessionEnd hook that
/// returned non-zero or panicked could disrupt Claude, so we swallow every
/// failure to stderr and let the caller exit 0.
///
/// # Observability
///
/// Because every path exits 0 and there is no terminal to read, the emitted
/// span IS the only record that this ran at all - so it carries the outcome
/// rather than leaving it to be reconstructed from log lines. Both fields are
/// declared here on purpose: `Span::record` on a field the macro did not
/// declare is a silent no-op, which would drop exactly the error status the
/// failure paths below record.
///
/// `skip_all` because nothing in scope is safe or useful to attach - the
/// payload is untrusted stdin and the resolved path names the user's project.
#[tracing::instrument(
    name = "coding_agent.hook",
    skip_all,
    fields(
        outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    )
)]
pub async fn run_hook() {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);

    let jsonl_path = match extract_jsonl_path(&payload) {
        Some(p) => p,
        None => {
            tracing::Span::current().record("outcome", "no_transcript_path");
            tracing::warn!("coding-agent-hook: could not determine JSONL path from payload");
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
            tracing::warn!(
                jsonl_path = %resolved.display(),
                "coding-agent-hook: transcript path outside accepted roots"
            );
            tracing::Span::current().record("outcome", "path_rejected");
            return;
        }
        Err(_) => {
            tracing::warn!(
                jsonl_path = %jsonl_path.display(),
                "coding-agent-hook: transcript not found"
            );
            tracing::Span::current().record("outcome", "transcript_missing");
            return;
        }
    };

    // The shared keyed opener, NOT a hand-built pool. This used to be a raw
    // `SqlitePool::connect_with`, which never applies the SQLCipher key, so on a
    // packaged (encrypted) install every query failed with "file is not a
    // database". The hook exits 0 on every path by design (it must never block
    // Claude), so that failure was completely silent: SessionEnd sealed nothing
    // and sessions fell through to the indexer's 1 h idle backstop instead.
    //
    // Bounded, because "never block Claude" is a latency contract and not just
    // an exit-code one. `open_pool_with_key` fails a keyless open of an
    // encrypted file by STALLING to its ~10 s acquire timeout rather than
    // erroring - so the exact misconfiguration this hook is most likely to
    // meet (a key that did not reach the CLI's environment) would hang the
    // user's editor for ten seconds at the end of every session. The hook has
    // nothing useful to do after that budget anyway: the indexer's idle
    // backstop seals the session regardless, so a timeout costs a delayed
    // seal, never a lost one.
    let pool = match tokio::time::timeout(DB_OPEN_BUDGET, super::open_meridian_pool()).await {
        Err(_) => {
            let span = tracing::Span::current();
            span.record("outcome", "db_open_timeout");
            span.record("otel.status_code", "ERROR");
            tracing::error!(
                budget_ms = DB_OPEN_BUDGET.as_millis() as u64,
                "coding-agent-hook: opening the database exceeded its budget - leaving this session to the indexer's idle backstop"
            );
            return;
        }
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            // `{e:#}` (the full anyhow context chain), not `%e` — plain Display
            // renders only the outermost context ("failed to open SQLite at
            // …"), dropping the cause that actually identifies the fault
            // (`code: 26, file is not a database`). This hook has no terminal
            // and exits 0 regardless, so the shipped record is the ONLY evidence
            // it ever ran; stripping the cause from it defeats the purpose.
            // `error` is on both SAFE_STRING_KEYS and FREE_TEXT_KEYS, so the
            // chain is path/URL/token-scrubbed on the ship leg.
            let detail = format!("{e:#}");
            let span = tracing::Span::current();
            span.record("outcome", "db_open_failed");
            span.record("otel.status_code", "ERROR");
            tracing::error!(error = %detail, "coding-agent-hook: failed to open db");
            return;
        }
    };

    let outcome = register_session(&pool, &jsonl_path, true, Utc::now()).await;
    pool.close().await;
    tracing::Span::current().record("outcome", "registered");
    tracing::info!(
        outcome = ?outcome,
        jsonl_path = %jsonl_path.display(),
        "coding-agent-hook: session registered"
    );
}

/// Ceiling on opening `meridian.db` inside the SessionEnd hook.
///
/// Sized against what it is protecting: this runs synchronously in Claude's
/// hook path, so the number the user feels is this one. Comfortably above a
/// healthy open (milliseconds - the daemon has already created and migrated
/// the file) and well below `open_pool_with_key`'s own ~10 s acquire timeout,
/// which is the stall this exists to cut short.
const DB_OPEN_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

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

fn expand(p: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(p).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget must stay comfortably under the acquire timeout it exists to
    /// pre-empt. If it ever drifts above it the wrapper still compiles, still
    /// looks correct, and does nothing at all - the inner stall wins and the
    /// hook blocks Claude for the full ~10 s again.
    #[test]
    fn the_db_open_budget_stays_under_the_acquire_timeout() {
        // `open_pool_with_key`'s acquire timeout, the stall being cut short.
        const ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
        assert!(
            DB_OPEN_BUDGET < ACQUIRE_TIMEOUT,
            "a budget at or above the acquire timeout never fires, and the hook \
             is back to hanging the editor for the full stall"
        );
        // And not so tight that a cold cache on a loaded machine trips it: a
        // spurious timeout costs a delayed seal on every single session.
        assert!(
            DB_OPEN_BUDGET >= std::time::Duration::from_secs(2),
            "too tight - healthy opens would start timing out"
        );
    }

    /// The open must actually be wrapped. Scanned from source because
    /// `run_hook` reads stdin and opens the real `MERIDIAN_DB`, so no unit test
    /// can drive it - the same tactic `daemon_lifecycle` and `backend_install`
    /// use for their own unreachable invariants.
    ///
    /// Bounded to the production half so the reference in this test module
    /// cannot vouch for the call site, and matched on the call itself rather
    /// than the constant name, which also appears in prose above it.
    #[test]
    fn the_hook_bounds_its_database_open() {
        const SRC: &str = include_str!("hook.rs");
        let production = SRC.split("#[cfg(test)]").next().expect("split yields one");
        assert!(
            production.contains("timeout(DB_OPEN_BUDGET, super::open_meridian_pool())"),
            "run_hook must bound the database open - an unbounded one stalls to \
             the acquire timeout on a key misconfiguration and blocks Claude at \
             the end of every session"
        );
    }
}
