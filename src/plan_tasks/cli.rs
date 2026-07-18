//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The `meridian plan-task-*` subcommands — the tray's only way into this module.
//!
//! # Why these live here and not in `main.rs`
//! `main.rs` dispatches subcommands as flat argv blocks and is already ~1000 lines.
//! Each of these needs the same ~30-line preamble (parse flags, open the DB, init
//! observability, print one JSON line, flush on every exit), so they own that here and
//! `main.rs` keeps a single [`try_handle`] call.
//!
//! # The contract every subcommand honours
//! - stdout is exactly ONE JSON line — the tray parses the last non-empty line.
//! - diagnostics go to stderr; exit 2 = bad arguments, exit 1 = runtime failure.
//! - `observability::init` is held for the whole call and flushed on EVERY exit path,
//!   including the error ones — otherwise the `llm.*` spans are silently dropped.
//!
//! # Who calls this
//! `main.rs` → here → [`super::draft`] / [`super::create`] / [`super::edit`].
//! The tray's `commands/plan_tasks.rs` spawns these.

use anyhow::Result;

use crate::config::Config;
use crate::db::meridian::setup_db;
use crate::observability;

/// Handle a `plan-task-*` subcommand if argv names one. Returns `true` when handled
/// (the caller should return immediately), `false` when this is some other command.
///
/// Never returns `Err` for a user-facing failure — those print to stderr and exit, so
/// the tray sees a non-zero status and a clean message.
pub async fn try_handle() -> Result<bool> {
    let Some(cmd) = std::env::args().nth(1) else {
        return Ok(false);
    };
    match cmd.as_str() {
        "plan-task-draft" => {
            run_draft().await;
            Ok(true)
        }
        "plan-task-create" => {
            run_create().await;
            Ok(true)
        }
        "plan-task-edit" => {
            run_edit().await;
            Ok(true)
        }
        "plan-task-done" => {
            run_done().await;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Read `--name value` out of argv.
fn flag(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Print one JSON line and flush observability. The single exit point for success.
async fn finish<T: serde::Serialize>(value: &T, guard: Option<observability::ObservabilityGuard>) {
    println!("{}", serde_json::to_string(value).unwrap_or_default());
    if let Some(g) = guard {
        g.shutdown().await;
    }
}

/// Print an error and exit 1, flushing observability first.
async fn die(label: &str, msg: String, guard: Option<observability::ObservabilityGuard>) -> ! {
    if let Some(g) = guard {
        g.shutdown().await;
    }
    eprintln!("{label}: {msg}");
    std::process::exit(1);
}

/// `meridian plan-task-draft --note "<text>"` — shape a rough note into
/// `{title, description, issue_type, error}`.
///
/// Exits 0 even when the model failed: the draft carries `error` and empty fields, and
/// the composer falls back to manual entry. See [`super::draft`].
async fn run_draft() {
    let note = flag("--note").unwrap_or_default();
    if note.trim().is_empty() {
        eprintln!("plan-task-draft: --note is required");
        std::process::exit(2);
    }
    // Required for the llm.request/infer/response subspans under plan_task.draft.
    let obs_guard = observability::init("meridian-rust").ok();
    match super::draft::draft(&note).await {
        Ok(d) => finish(&d, obs_guard).await,
        Err(e) => die("plan-task-draft", format!("{e:#}"), obs_guard).await,
    }
}

/// `meridian plan-task-create --title T [--description D] [--issue-type Task|Bug]
/// [--target local|<provider>] [--day YYYY-MM-DD]` — create a task and add it to the
/// day's plan. Prints `{task_key, provider, synced, note}`.
async fn run_create() {
    let title = flag("--title").unwrap_or_default();
    if title.trim().is_empty() {
        eprintln!("plan-task-create: --title is required");
        std::process::exit(2);
    }
    let description = flag("--description").unwrap_or_default();
    let issue_type = flag("--issue-type").unwrap_or_default();
    let target = super::create::Target::parse(&flag("--target").unwrap_or_default());
    let day = flag("--day").unwrap_or_else(meridian_core::date::today_string);

    let cfg = Config::from_env();
    let obs_guard = observability::init("meridian-rust").ok();
    let pool = match setup_db(&cfg.meridian_db_uri()).await {
        Ok(p) => p,
        Err(e) => die("plan-task-create", format!("open db: {e:#}"), obs_guard).await,
    };
    let res = super::create::create(
        &pool,
        &cfg,
        &target,
        &title,
        &description,
        &issue_type,
        &day,
    )
    .await;
    pool.close().await;
    match res {
        Ok(c) => finish(&c, obs_guard).await,
        Err(e) => die("plan-task-create", format!("{e:#}"), obs_guard).await,
    }
}

/// `meridian plan-task-edit --key K [--title T] [--description D]` — rewrite a task's
/// text, routing to our DB or the tracker depending on who owns it. Prints
/// `{task_key, provider, status, browse_url, reason}`.
async fn run_edit() {
    let key = flag("--key").unwrap_or_default();
    if key.trim().is_empty() {
        eprintln!("plan-task-edit: --key is required");
        std::process::exit(2);
    }
    let title = flag("--title");
    let description = flag("--description");
    if title.is_none() && description.is_none() {
        eprintln!("plan-task-edit: --title and/or --description is required");
        std::process::exit(2);
    }

    let cfg = Config::from_env();
    let obs_guard = observability::init("meridian-rust").ok();
    let pool = match setup_db(&cfg.meridian_db_uri()).await {
        Ok(p) => p,
        Err(e) => die("plan-task-edit", format!("open db: {e:#}"), obs_guard).await,
    };
    let res = super::edit::edit(&pool, &cfg, &key, title.as_deref(), description.as_deref()).await;
    pool.close().await;
    match res {
        Ok(r) => finish(&r, obs_guard).await,
        Err(e) => die("plan-task-edit", format!("{e:#}"), obs_guard).await,
    }
}

/// `meridian plan-task-done --key K --done true|false` — mark a task done or not-done,
/// routing to our DB or the tracker depending on who owns it. Prints the same
/// `{task_key, provider, status, browse_url, reason}` shape as `plan-task-edit`.
async fn run_done() {
    let key = flag("--key").unwrap_or_default();
    if key.trim().is_empty() {
        eprintln!("plan-task-done: --key is required");
        std::process::exit(2);
    }
    let done = match flag("--done").unwrap_or_default().as_str() {
        "true" => true,
        "false" => false,
        _ => {
            eprintln!("plan-task-done: --done must be true or false");
            std::process::exit(2);
        }
    };

    let cfg = Config::from_env();
    let obs_guard = observability::init("meridian-rust").ok();
    let pool = match setup_db(&cfg.meridian_db_uri()).await {
        Ok(p) => p,
        Err(e) => die("plan-task-done", format!("open db: {e:#}"), obs_guard).await,
    };
    let res = super::done::set_done(&pool, &cfg, &key, done).await;
    pool.close().await;
    match res {
        Ok(r) => finish(&r, obs_guard).await,
        Err(e) => die("plan-task-done", format!("{e:#}"), obs_guard).await,
    }
}
