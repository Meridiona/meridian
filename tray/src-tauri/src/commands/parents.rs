//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/triage/parents` GET ported to Rust — a faithful port of
//! `ui/app/api/triage/parents/route.ts`.
//!
//! # What this is
//! For the "link to a parent" hygiene fix: the valid parents for a ticket (the
//! level above it in the tracker hierarchy — Epic / parent task / work item), a
//! label for that level, and a deep link to create a new one. Tracker auth lives
//! in the daemon, so — exactly like the Node route — this **shells out to the
//! `meridian ticket-parents` CLI** (read-only) and relays its JSON. It is NOT a
//! DB read, so it lives tray-side, not in meridian-core.
//!
//! # Who calls this
//! The `get_ticket_parents` Tauri command → the dashboard's `HygieneDialog`
//! (the parent-picker). Mirrors the route's contract: on any failure it returns
//! a 200-equivalent payload with an `error` field (never throws), so the dialog
//! shows the error inline while still rendering an empty list.
//!
//! # Related
//! - [`crate::commands::get_triage`] / [`meridian_core::triage`] — the working
//!   set whose "link a parent" fix opens this picker.
//! - The Node helper `ui/lib/meridian-bin.ts` (`selectMeridianBinary`) — this
//!   module reimplements its "native binary first" resolution ([`meridian_bin`]).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// One candidate parent ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parent {
    pub key: String,
    pub title: String,
}

/// The command's response — mirrors the route's JSON, incl. the optional
/// `error` (set on any failure; the payload is still returned, never thrown).
#[derive(Debug, Clone, Serialize)]
pub struct ParentsResponse {
    pub parents: Vec<Parent>,
    pub parent_label: String,
    pub create_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The shape `meridian ticket-parents` prints (last stdout line, JSON).
#[derive(Deserialize)]
struct ParentsOutput {
    parents: Vec<Parent>,
    parent_label: String,
    create_url: String,
}

impl ParentsResponse {
    /// A failure payload: empty list, default label, the message in `error`.
    /// Status-200-equivalent — the dialog reads `error` and still renders.
    fn failure(error: impl Into<String>) -> Self {
        Self {
            parents: Vec::new(),
            parent_label: "parent".to_string(),
            create_url: String::new(),
            error: Some(error.into()),
        }
    }
}

/// Resolve the `meridian` binary, native build first. Mirrors
/// `selectMeridianBinary(meridianCandidates())`: the native binary
/// (`~/.meridian/app/bin/meridian`) has no runtime deps, so prefer it; fall back
/// to the user-local install, then bare `meridian` on `PATH`.
fn meridian_bin() -> String {
    if let Ok(home) = std::env::var("HOME") {
        for rel in ["/.meridian/app/bin/meridian", "/.local/bin/meridian"] {
            let p = PathBuf::from(format!("{home}{rel}"));
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
    }
    "meridian".to_string()
}

/// List valid parents for `(provider, key)` (the ported /api/triage/parents).
/// Spawns `meridian ticket-parents --provider <p> --key <k>` (args are passed
/// as argv, not a shell string, so they can't inject), with a 30 s timeout, and
/// parses the last JSON line of stdout. Any failure → a `failure(...)` payload.
#[tauri::command]
#[tracing::instrument]
pub async fn get_ticket_parents(provider: String, key: String) -> ParentsResponse {
    if provider.is_empty() || key.is_empty() {
        return ParentsResponse::failure("provider and key are required");
    }

    let bin = meridian_bin();
    let child = tokio::process::Command::new(&bin)
        .args(["ticket-parents", "--provider", &provider, "--key", &key])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match tokio::time::timeout(Duration::from_secs(30), child).await {
        Err(_) => {
            tracing::warn!(%provider, %key, "ticket-parents timed out");
            return ParentsResponse::failure("ticket-parents timed out");
        }
        Ok(Err(e)) => {
            tracing::warn!(%provider, %key, bin = %bin, error = %e, "ticket-parents spawn failed");
            return ParentsResponse::failure(format!("spawn error: {e}"));
        }
        Ok(Ok(o)) => o,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            format!("exited {:?}", output.status.code())
        } else {
            stderr
        };
        tracing::warn!(%provider, %key, "ticket-parents non-zero: {msg}");
        return ParentsResponse::failure(msg);
    }

    // The CLI may print logs before the JSON — take the last non-empty line.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout.lines().rfind(|l| !l.trim().is_empty());
    match last.and_then(|l| serde_json::from_str::<ParentsOutput>(l).ok()) {
        Some(out) => {
            tracing::info!(%provider, %key, parents = out.parents.len(), "ticket-parents served");
            ParentsResponse {
                parents: out.parents,
                parent_label: out.parent_label,
                create_url: out.create_url,
                error: None,
            }
        }
        None => {
            // Last ~200 chars of stdout, for a useful parse-error message.
            let s = stdout.trim();
            let skip = s.chars().count().saturating_sub(200);
            let tail: String = s.chars().skip(skip).collect();
            ParentsResponse::failure(format!("could not parse: {tail}"))
        }
    }
}
