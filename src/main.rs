//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::etl::run_etl;
use meridian::intelligence::sync_delegate::Delegation;
use meridian::observability;
use meridian_core::pm_sync_requests::SyncMode;
use tokio::sync::Notify;
use tracing::Instrument;

#[tokio::main]
async fn main() -> Result<()> {
    // 0. Raise the file-descriptor soft limit BEFORE any database or socket
    //    work. The daemon shares meridian.db with the tray, and each pooled
    //    connection holds three descriptors (db/-wal/-shm); crossing macOS's
    //    default 256 fails a -wal/-shm open mid-write with SQLITE_IOERR (522)
    //    and desyncs the shared WAL index into "database disk image is
    //    malformed" (11). The tray cannot do this on the daemon's behalf — it
    //    never runs this `main` — so both entry points call it. See
    //    `meridian_core::fd_limit`.
    meridian_core::fd_limit::raise_fd_limit();

    // 1. Load the working-directory .env. `dotenv_override` walks UP from the
    //    CWD and stops at the first `.env`, so a source/dev run picks up
    //    <repo>/.env, and on macOS — where the launchd plist sets
    //    WorkingDirectory — a packaged install picks up ~/.meridian/.env. Its
    //    values beat any empty defaults injected by the plist. (CLI subcommands
    //    invoked from elsewhere fall back to built-in defaults, e.g.
    //    MERIDIAN_DB → ~/.meridian/meridian.db.)
    let _ = dotenvy::dotenv_override();

    // 1a. …but that walk is CWD-dependent, and on Windows NOTHING sets a
    //     working directory for the daemon. Neither launcher the tray installs
    //     has one: `schtasks /Create` (see `backend_install.rs`) has no "Start
    //     in" field, and the Startup-folder fallback calls `WScript.Shell.Run`
    //     without setting `CurrentDirectory`. Both therefore start the daemon in
    //     system32, where the walk finds no `.env` at all — so it came up with
    //     no MERIDIAN_DB_KEY.
    //
    //     That stayed invisible for as long as meridian.db was plaintext:
    //     `key_unless_plaintext` drops the key for a plaintext file, so the open
    //     succeeded without one. It turns fatal the moment the tray's
    //     encrypt-in-place completes (which it does as soon as it runs while the
    //     daemon is not holding the file open) — from then on every connection
    //     fails in `after_connect`, permanently, with the key sitting in a file
    //     this process never read.
    //
    //     So also load the canonical ~/.meridian/.env, the file the tray writes
    //     the key and tracker credentials into, regardless of where we were
    //     started from. `from_path` does NOT override, so anything already set —
    //     by the real environment or by the repo .env above — still wins: dev
    //     and macOS behaviour are unchanged, and this only fills the gap left
    //     when the walk came up empty.
    if let Some(home) = meridian_core::paths::home_dir() {
        let _ = dotenvy::from_path(home.join(".meridian").join(".env"));
    }

    // 1b. Subcommand dispatch. `meridian coding-agent-hook` is the Claude Code
    //     SessionEnd hook entry point: one-shot, reads a JSON payload on stdin,
    //     seals that session, exits 0. It must stay light (no daemon init, no
    //     network) and must never block Claude, so it always exits 0.
    //     `observability::init` is still cheap here: capture is a local-disk
    //     write only (see `telemetry_spool::spool_client::SpoolClient`), never
    //     a network call, so it doesn't violate the "must never block" contract
    //     — without it, hook failures were only visible on Claude Code's own
    //     stderr, never in `meridian logs` or an exported diagnostics bundle.
    if std::env::args().nth(1).as_deref() == Some("coding-agent-hook") {
        let obs_guard = observability::init("meridian-rust").ok();
        meridian::coding_agent_session_ingest::hook::run_hook().await;
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian coding-agent-summarise [--dry-run] [--day YYYY-MM-DD] [--limit N]`
    // — one-shot manual backfill / eval of the summariser queue for one day.
    if std::env::args().nth(1).as_deref() == Some("coding-agent-summarise") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let dry_run = args.iter().any(|a| a == "--dry-run");
        let day = flag("--day");
        let limit: i64 = flag("--limit").and_then(|v| v.parse().ok()).unwrap_or(8);
        let obs_guard = observability::init("meridian-rust").ok();
        let mut open_failed = false;
        match meridian::coding_agent_session_ingest::open_meridian_pool().await {
            Ok(pool) => {
                meridian::coding_agent_session_ingest::summariser::cli_summarise(
                    &pool,
                    dry_run,
                    day.as_deref(),
                    limit,
                )
                .await;
                pool.close().await;
            }
            Err(e) => {
                // `eprintln!`, not just `tracing`. Both `fmt` layers in
                // `observability::init` are `cfg!(debug_assertions)`-gated and
                // the spool is the only persisted sink, so in a RELEASE build —
                // the only place the unkeyed pool ever failed — the tracing line
                // below reaches no terminal at all, and this arm then returned
                // `Ok(())`. An operator running the documented backlog-drain
                // command against a bad or missing key saw an empty screen and
                // exit 0: the same silent-failure class this commit fixes, one
                // frame further out, and the frame the operator stands in.
                //
                // `{e:#}` is anyhow's alternate form (the full context chain);
                // plain `%e` renders only the outermost context and would drop
                // the actual SQLite cause from the shipped record too.
                let detail = format!("{e:#}");
                eprintln!("summarise: open db: {detail}");
                tracing::error!(error = %detail, "coding-agent-summarise: failed to open db");
                open_failed = true;
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        // Non-zero so `meridian doctor --fix` — which runs this as a guided fix
        // and checks `.success()` — reports the failure instead of a false pass.
        if open_failed {
            std::process::exit(1);
        }
        return Ok(());
    }

    // `meridian db check` / `meridian db repair` — corruption diagnosis and
    // recovery. Both are one-shot and exit; `repair` refuses to run while the
    // daemon or the tray is alive, because rebuilding a file underneath a live
    // writer is how you turn one corrupt database into two.
    if std::env::args().nth(1).as_deref() == Some("db") {
        let sub = std::env::args().nth(2).unwrap_or_default();
        let obs_guard = observability::init("meridian-rust").ok();
        let code = meridian::db::cli::run_db_command(&sub).await;
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        std::process::exit(code);
    }

    // (`meridian coding-agent-install-skill` used to live here. It wrote
    // ~/.claude/commands/session-summary.md so `claude -p /session-summary`
    // would resolve — an invocation the summariser no longer makes: the Claude
    // engine embeds SUMMARY_RULES inline (see
    // `coding_agent_session_ingest::summariser::claude`), so nothing has read
    // that file since. It also kept a hand-copied duplicate of
    // assets/skills/coding-agent/session-summary/SKILL.md in sync by comment
    // only — while `summariser::prompts` `include_str!`s the real file. The
    // asset stays; only the dead writer is gone.)

    // `meridian oauth-login <provider> [--client-id ID] [--port N]` — interactive
    // browser OAuth flow for a PM provider. Opens the system browser, captures
    // the loopback redirect (or JS relay for fragment-based flows), and persists
    // tokens to ~/.meridian/oauth/<provider>.json. Supports: jira, trello.
    if std::env::args().nth(1).as_deref() == Some("oauth-login") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let provider = std::env::args().nth(2).unwrap_or_default();
        match provider.as_str() {
            "jira" => {
                // --client-id flag > JIRA_OAUTH_CLIENT_ID env > baked-in default.
                let client_id = flag("--client-id")
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(meridian::intelligence::oauth::jira::client_id);
                let port = flag("--port")
                    .and_then(|v| v.parse::<u16>().ok())
                    .unwrap_or_else(meridian::intelligence::oauth::jira::redirect_port);
                println!(
                    "Starting Jira browser authorization (redirect http://127.0.0.1:{port}/callback)…"
                );
                let client_secret = meridian::intelligence::oauth::jira::client_secret();
                match meridian::intelligence::oauth::jira::login(&client_id, &client_secret, port).await {
                    Ok(site) => println!(
                        "\n✓ Jira connected: {site}\n  Tokens saved to ~/.meridian/oauth/jira.json — run `meridian restart` to pick them up."
                    ),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        eprintln!("oauth-login jira failed: {msg}");
                        // Only show the admin-block hint when the failure is a
                        // consent-phase denial (Atlassian redirects with
                        // error=access_denied when the org policy blocks the app).
                        // Token-exchange errors (invalid_client, missing secret,
                        // network issues) have their own clear messages above.
                        if msg.contains("access_denied")
                            || msg.contains("provider returned OAuth error")
                        {
                            eprintln!(
                                "\nIf your Atlassian org blocks third-party OAuth apps (a \
                                 \"site admin must authorize\" message, or app installs \
                                 disabled), use the API-token fallback instead: set \
                                 JIRA_BASE_URL / JIRA_EMAIL / JIRA_API_TOKEN via \
                                 `meridian config edit`."
                            );
                        }
                        std::process::exit(1);
                    }
                }
            }
            "trello" => {
                let app_key = flag("--app-key")
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(meridian::intelligence::oauth::trello::app_key);
                let port = flag("--port")
                    .and_then(|v| v.parse::<u16>().ok())
                    .unwrap_or_else(meridian::intelligence::oauth::trello::redirect_port);
                println!(
                    "Starting Trello browser authorization (redirect http://127.0.0.1:{port}/callback)…"
                );
                match meridian::intelligence::oauth::trello::login(&app_key, port).await {
                    Ok(()) => println!(
                        "\n✓ Trello connected.\n  Token saved to ~/.meridian/oauth/trello.json — run `meridian restart` to pick it up."
                    ),
                    Err(e) => {
                        eprintln!("oauth-login trello failed: {e:#}");
                        std::process::exit(1);
                    }
                }
            }
            other => {
                // GitHub connects via the in-app browser device flow (the tray's
                // start_oauth), not a CLI subcommand — see meridian-oauth::github.
                eprintln!("oauth-login: unknown provider {other:?} (supported: jira, trello)");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian worklog-hour <YYYY-MM-DDTHH>` — force-run the hour-level worklog
    // pipeline for one explicit local hour (bypasses the activity gate + done-check,
    // still waits for coding summarisation). Manual runs / testing the clock driver.
    if std::env::args().nth(1).as_deref() == Some("worklog-hour") {
        let label = std::env::args().nth(2);
        let Some(label) = label else {
            eprintln!("worklog-hour: usage: meridian worklog-hour <YYYY-MM-DDTHH>");
            return Ok(());
        };
        let cfg = Config::from_env();
        // Initialise observability so this one-shot emits the same worklog.hour trace
        // the daemon does; flush before exit so the batch processor's spans ship.
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                meridian::worklog_pipeline::cli_run_hour(&pool, &cfg.meridian_db, &label).await;
                pool.close().await;
            }
            Err(e) => eprintln!("worklog-hour: open db: {e}"),
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian llm-experiment run|create|exec|list|get …` — the dev-only LLM Lab
    // harness: replay one prose stage (hour report / workstream fold / worklog
    // generate) across several provider/model variants and record every outcome in
    // the llm_experiment* tables. Never writes production tables.
    if std::env::args().nth(1).as_deref() == Some("llm-experiment") {
        let cfg = Config::from_env();
        // Init observability so each variant emits its llm.experiment.variant span.
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                let result = meridian::llm_experiment::cli::run(&pool).await;
                pool.close().await;
                if let Err(e) = result {
                    if let Some(g) = obs_guard {
                        g.shutdown().await;
                    }
                    eprintln!("llm-experiment: {e:#}");
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("llm-experiment: open db: {e:#}");
                std::process::exit(1);
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian worklog-post-approved` — post every worklog the user approved in
    // the dashboard to Jira now (the same sweep the daemon runs every ~60s). This
    // is the only path that writes to real Jira.
    if std::env::args().nth(1).as_deref() == Some("worklog-post-approved") {
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                meridian::pm_worklog::cli_post_approved(&pool).await;
                pool.close().await;
            }
            Err(e) => eprintln!("worklog-post-approved: open db: {e}"),
        }
        return Ok(());
    }

    // `meridian db-export-plaintext --out <path>` — write a plaintext snapshot
    // of meridian.db to `<path>`. The ONLY consumer today is
    // `packages/meridian-mcp`'s `sql.js` reader (a pure-WASM SQLite build with
    // no SQLCipher support), which shells out to this instead of reading an
    // encrypted meridian.db directly. If the db is not encrypted (no
    // MERIDIAN_DB_KEY set, e.g. dev/source installs), this is a plain file
    // copy — no SQLCipher involvement at all.
    if std::env::args().nth(1).as_deref() == Some("db-export-plaintext") {
        let out_path = std::env::args()
            .skip(2)
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--out")
            .map(|w| w[1].clone());
        let Some(out_path) = out_path else {
            eprintln!("db-export-plaintext: usage: meridian db-export-plaintext --out <path>");
            std::process::exit(1);
        };
        let cfg = Config::from_env();
        let db_path = std::path::Path::new(&cfg.meridian_db);
        let out_path = std::path::Path::new(&out_path);
        let key = std::env::var("MERIDIAN_DB_KEY").ok();
        let result = match key {
            Some(k) => meridian_core::db_crypto::export_plaintext(db_path, &k, out_path).await,
            None => std::fs::copy(db_path, out_path)
                .map(|_| ())
                .context("db-export-plaintext: plain file copy failed"),
        };
        match result {
            Ok(()) => println!("exported plaintext snapshot to {}", out_path.display()),
            Err(e) => {
                eprintln!("db-export-plaintext: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian tasks-sync` (force) / `meridian pm-sync` (gated) — the two on-demand
    // CLI sync entry points. There is no background poller anymore; see
    // `src/intelligence/mod.rs`'s doc comments for the full trigger list.
    //
    // `tasks-sync` bypasses the per-provider staleness gate (the user asked for fresh
    // data now); `pm-sync` honours it, so it is a cheap no-op on an already-fresh
    // board. Both DELEGATE to a running daemon rather than syncing here — see
    // `cli_sync` for why the rotating credential has exactly one safe writer.
    //
    // One arm, because the two differ only in mode and label. They were separate
    // copies of the same fourteen lines, which is how `pm-sync` ended up documented as
    // the command the tray used "on opening the dashboard" — a trigger that no longer
    // exists.
    //
    // Exit 1 if the DB cannot be opened, or if the sync itself failed. Reporting the
    // sync failure in the exit code matters: with no daemon running the tray's "Sync
    // now" shells out to `tasks-sync` and reads a non-zero exit as the failure, so
    // exiting 0 here would show the user a successful sync that did not happen. A
    // timeout is NOT a failure (`cli_sync` returns true) — the daemon is still working.
    let cli_sync_mode = match std::env::args().nth(1).as_deref() {
        Some("tasks-sync") => Some((SyncMode::Force, "tasks-sync")),
        Some("pm-sync") => Some((SyncMode::Gated, "pm-sync")),
        _ => None,
    };
    if let Some((mode, label)) = cli_sync_mode {
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                let ok = cli_sync(&pool, &cfg, mode, label).await;
                pool.close().await;
                if !ok {
                    std::process::exit(1);
                }
            }
            Err(e) => {
                // `{e:#}` prints the full anyhow source chain (e.g. the sqlx
                // "migration N was previously applied / missing" cause) instead
                // of just the top-level "failed to run migrations" context.
                eprintln!("{label}: open db: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian ticket-update --provider P --key K --field F --value V` — apply
    // ONE board-hygiene fix to the real tracker (due date, assignee, label, …).
    // Prints a JSON result the UI reads: {"status":"applied"} or
    // {"status":"redirected","browse_url":...}. On a successful write it triggers
    // a force sync so the local mirror + hygiene verdicts reflect the change.
    if std::env::args().nth(1).as_deref() == Some("ticket-update") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let provider = flag("--provider").unwrap_or_default();
        let key = flag("--key").unwrap_or_default();
        let field = flag("--field").unwrap_or_default();
        let value = flag("--value").unwrap_or_default();
        if provider.is_empty() || key.is_empty() || field.is_empty() {
            eprintln!("ticket-update: --provider, --key and --field are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match meridian::intelligence::ticket_update::apply(&cfg, &provider, &key, &field, &value)
            .await
        {
            Ok(result) => {
                // Reflect an applied write back into our mirror + hygiene verdicts.
                if matches!(
                    result.status,
                    meridian::intelligence::ticket_update::ApplyStatus::Applied
                ) {
                    if let Ok(pool) = setup_db(&cfg.meridian_db_uri()).await {
                        cli_sync_after_write(&pool, &cfg, "ticket-update").await;
                        pool.close().await;
                    }
                }
                println!("{}", result.to_json());
            }
            Err(e) => {
                eprintln!("ticket-update: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian ticket-parents --provider P --key K` — list valid parents for a
    // ticket (Epic / parent task / parent work item, per the tracker's hierarchy)
    // + a create-parent deep link, for the "link to a parent" hygiene fix. Prints
    // JSON {"parents":[{key,title}],"parent_label":...,"create_url":...}. Read-only.
    if std::env::args().nth(1).as_deref() == Some("ticket-parents") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let provider = flag("--provider").unwrap_or_default();
        let key = flag("--key").unwrap_or_default();
        if provider.is_empty() || key.is_empty() {
            eprintln!("ticket-parents: --provider and --key are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match meridian::intelligence::ticket_update::parents::list(&cfg, &provider, &key).await {
            Ok(result) => println!("{}", result.to_json()),
            Err(e) => {
                eprintln!("ticket-parents: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian ticket-statuses --provider P --key K` — list the statuses a
    // ticket can move to (each normalised to the canonical lifecycle taxonomy)
    // plus its current status, for the dashboard's status control. Prints ONE
    // JSON line {"statuses":[{id,name,category}],"current_id":..,"current_name":..}.
    // Read-only.
    if std::env::args().nth(1).as_deref() == Some("ticket-statuses") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let provider = flag("--provider").unwrap_or_default();
        let key = flag("--key").unwrap_or_default();
        if provider.is_empty() || key.is_empty() {
            eprintln!("ticket-statuses: --provider and --key are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match meridian::intelligence::ticket_update::statuses::list_statuses(&cfg, &provider, &key)
            .await
        {
            Ok(result) => println!("{}", result.to_json()),
            Err(e) => {
                eprintln!("ticket-statuses: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian ticket-set-status --provider P --key K --status ID_OR_NAME` —
    // move a ticket to a status (id, or name case-insensitively — the UI's Undo
    // passes the previous status NAME). Prints ONE JSON line
    // {"result":{"status":"applied"|"redirected","browse_url":..,"reason":..},
    //  "new_status":{id,name,category}|null}. On an applied move it triggers a
    // force sync so the local mirror's status_raw/is_terminal reflect the change.
    if std::env::args().nth(1).as_deref() == Some("ticket-set-status") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let provider = flag("--provider").unwrap_or_default();
        let key = flag("--key").unwrap_or_default();
        let status = flag("--status").unwrap_or_default();
        if provider.is_empty() || key.is_empty() || status.is_empty() {
            eprintln!("ticket-set-status: --provider, --key and --status are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match meridian::intelligence::ticket_update::statuses::set_status(
            &cfg, &provider, &key, &status,
        )
        .await
        {
            Ok(result) => {
                // Reflect an applied move back into our mirror + hygiene verdicts.
                if matches!(
                    result.status,
                    meridian::intelligence::ticket_update::ApplyStatus::Applied
                ) {
                    if let Ok(pool) = setup_db(&cfg.meridian_db_uri()).await {
                        cli_sync_after_write(&pool, &cfg, "ticket-set-status").await;
                        pool.close().await;
                    }
                }
                println!("{}", result.to_json());
            }
            Err(e) => {
                eprintln!("ticket-set-status: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian plan-task-{draft,create,edit}` — the daily plan's task composer:
    // shape a rough note into a task with the user's LLM, create it (personal or
    // filed on a tracker) into today's plan, and edit it afterwards. Each prints ONE
    // JSON line. The blocks live in `plan_tasks::cli` rather than here — they share a
    // preamble, and this file is long enough already.
    if meridian::plan_tasks::cli::try_handle().await? {
        return Ok(());
    }

    // `meridian day-summary --day YYYY-MM-DD` — compose the day's summary: ONE
    // provider-agnostic LLM call that reads the day's evidence and answers with a
    // headline and two-to-three free-text insight cards. The plan ledger (which
    // tickets got done, the ring, the counts) is computed deterministically in Rust,
    // NOT asked of the model. Prints ONE JSON line: the summary object. Regenerate =
    // the same command (UPSERT overwrites). Never fails on a bad answer — the
    // deterministic ledger still renders, so a non-zero exit here means the DB or
    // the day's data, not the model.
    if std::env::args().nth(1).as_deref() == Some("day-summary") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
        // `--now`: draft the day's worklogs first (the on-demand "Generate now"
        // path), so the deterministic plan ledger binds to fresh matches instead of
        // only what happened to be drafted already. Without it, day-summary just
        // composes over the current state (the quiet staleness recompose).
        let run_worklogs = args.iter().any(|a| a == "--now");
        if day.is_empty() {
            eprintln!("day-summary: --day is required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        // Init observability so the LLM call emits its llm.request/infer/response
        // subspans under day_summary.generate; flush before exit.
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                if run_worklogs {
                    meridian::pm_worklog::auto_generate::generate_now(&pool, &cfg, &day).await;
                }
                match meridian::day_summary::generate::generate(&pool, &day).await {
                    Ok(summary) => {
                        println!("{}", serde_json::to_string(&summary).unwrap_or_default())
                    }
                    Err(e) => {
                        pool.close().await;
                        if let Some(g) = obs_guard {
                            g.shutdown().await;
                        }
                        eprintln!("day-summary: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("day-summary: open db: {e:#}");
                std::process::exit(1);
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian day-summary-get --day YYYY-MM-DD` — read the stored summary for a
    // day, or print JSON `null` if none exists. Read-only, no LLM.
    if std::env::args().nth(1).as_deref() == Some("day-summary-get") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
        if day.is_empty() {
            eprintln!("day-summary-get: --day is required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                match meridian_core::day_summaries::get_day_summary(&pool, &day).await {
                    Ok(s) => println!("{}", serde_json::to_string(&s).unwrap_or_default()),
                    Err(e) => {
                        pool.close().await;
                        eprintln!("day-summary-get: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("day-summary-get: open db: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian day-summary-data --day YYYY-MM-DD` — the deterministic half of the
    // summary screen (the day's aggregate datasets, its headline scalars, and the
    // evidence stamp) as one JSON object. Read-only, no LLM. The tray reads these
    // straight from meridian-core rather than spawning this; it exists for checking
    // what the screen was actually given
    // (`meridian day-summary-data --day X | jq .scalars`).
    if std::env::args().nth(1).as_deref() == Some("day-summary-data") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
        if day.is_empty() {
            eprintln!("day-summary-data: --day is required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                // Through `panel_data`, NOT `collect` directly: this command's only
                // job is to show what the screen is given, and it is worth nothing
                // if it shapes the answer itself and shows something else.
                match meridian::day_summary::generate::panel_data(&pool, &day).await {
                    Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
                    Err(e) => {
                        pool.close().await;
                        eprintln!("day-summary-data: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("day-summary-data: open db: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian worklog-generate --day YYYY-MM-DD --task-id T1` — the day-task
    // "Generate worklog" action: ONE provider-agnostic LLM call that matches the
    // day-task's workstream to an existing ticket (or proposes a new one) and
    // drafts a high-level status update. Prints ONE JSON line: the draft object.
    // Regenerate = the same command (UPSERT overwrites the drafted row).
    if std::env::args().nth(1).as_deref() == Some("worklog-generate") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
        let task_id = flag("--task-id").unwrap_or_default();
        if day.is_empty() || task_id.is_empty() {
            eprintln!("worklog-generate: --day and --task-id are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        // Init observability so the LLM call emits its llm.request/infer/response
        // subspans under a worklog.generate span; flush before exit.
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                // Matching needs current ticket state — a stale board could silently
                // bind this draft to a closed/renamed ticket. Best-effort: a sync
                // failure must not block drafting against whatever's cached.
                //
                // Delegated to the daemon, the sole owner of the rotating Jira OAuth
                // token (see `intelligence::sync_delegate`). The wait budget is short
                // because an LLM call follows inside the tray's 150 s timeout for this
                // command: better to draft against a slightly stale board than to blow
                // the timeout and show the user nothing at all.
                match meridian::intelligence::sync_delegate::sync_and_wait(
                    &pool,
                    &cfg,
                    SyncMode::Gated,
                    "worklog-generate",
                    std::time::Duration::from_secs(30),
                )
                .await
                {
                    Delegation::Synced { .. } => {}
                    Delegation::Failed { error } => {
                        tracing::warn!(%error, "worklog-generate: pm sync failed — matching against cached tasks");
                    }
                    Delegation::Pending => {
                        tracing::warn!("worklog-generate: pm sync still running — matching against cached tasks");
                    }
                }
                match meridian::pm_worklog::generate(&pool, &cfg, &day, &task_id).await {
                    Ok(mut draft) => {
                        // A matched draft carries a target_key we can link even
                        // before posting; fill browse_url deterministically.
                        meridian::pm_worklog::generate::hydrate_browse_url(&cfg, &mut draft);
                        println!("{}", serde_json::to_string(&draft).unwrap_or_default())
                    }
                    Err(e) => {
                        pool.close().await;
                        if let Some(g) = obs_guard {
                            g.shutdown().await;
                        }
                        eprintln!("worklog-generate: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("worklog-generate: open db: {e:#}");
                std::process::exit(1);
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian worklog-generate-get --day YYYY-MM-DD --task-id T1` — read the
    // current draft for a day-task, or print JSON `null` if none exists. Read-only.
    if std::env::args().nth(1).as_deref() == Some("worklog-generate-get") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
        let task_id = flag("--task-id").unwrap_or_default();
        if day.is_empty() || task_id.is_empty() {
            eprintln!("worklog-generate-get: --day and --task-id are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                match meridian_core::day_task_worklogs::get_day_task_worklog(&pool, &day, &task_id)
                    .await
                {
                    Ok(mut draft) => {
                        // Repair a stored-empty browse_url (e.g. an OAuth-Jira row
                        // posted before URL resolution read the site URL from the
                        // token store) so the "Linked to …" chip is a live link.
                        if let Some(d) = draft.as_mut() {
                            meridian::pm_worklog::generate::hydrate_browse_url(&cfg, d);
                        }
                        println!("{}", serde_json::to_string(&draft).unwrap_or_default())
                    }
                    Err(e) => {
                        pool.close().await;
                        eprintln!("worklog-generate-get: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("worklog-generate-get: open db: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian worklog-generate-approve --day YYYY-MM-DD --task-id T1` — approve
    // the current draft: create the proposed ticket if any, post the status update
    // as a comment, link the day-task. Idempotent. Prints ONE JSON line
    // {"posted":..,"target_key":..,"created_task_key":..,"created":..,"browse_url":..,"error":..}.
    if std::env::args().nth(1).as_deref() == Some("worklog-generate-approve") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
        let task_id = flag("--task-id").unwrap_or_default();
        if day.is_empty() || task_id.is_empty() {
            eprintln!("worklog-generate-approve: --day and --task-id are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                match meridian::pm_worklog::approve(&pool, &cfg, &day, &task_id).await {
                    Ok(res) => println!("{}", serde_json::to_string(&res).unwrap_or_default()),
                    Err(e) => {
                        pool.close().await;
                        if let Some(g) = obs_guard {
                            g.shutdown().await;
                        }
                        eprintln!("worklog-generate-approve: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("worklog-generate-approve: open db: {e:#}");
                std::process::exit(1);
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian worklog-escalate-create --task LOCAL-7` — escalate a personal
    // task onto a real tracker: create a new ticket seeded from the task's
    // title/description and post its logged update there, then keep-and-link the
    // personal task. Prints ONE JSON line: the EscalateResult.
    if std::env::args().nth(1).as_deref() == Some("worklog-escalate-create") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let task = flag("--task").unwrap_or_default();
        if task.is_empty() {
            eprintln!("worklog-escalate-create: --task is required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                match meridian::pm_worklog::escalate::escalate_create(&pool, &cfg, &task).await {
                    Ok(res) => println!("{}", serde_json::to_string(&res).unwrap_or_default()),
                    Err(e) => {
                        pool.close().await;
                        if let Some(g) = obs_guard {
                            g.shutdown().await;
                        }
                        eprintln!("worklog-escalate-create: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("worklog-escalate-create: open db: {e:#}");
                std::process::exit(1);
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian worklog-escalate-match --task LOCAL-7 --target KAN-12` — post a
    // personal task's logged update onto an EXISTING real ticket, then
    // keep-and-link the personal task. Prints ONE JSON line: the EscalateResult.
    if std::env::args().nth(1).as_deref() == Some("worklog-escalate-match") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let task = flag("--task").unwrap_or_default();
        let target = flag("--target").unwrap_or_default();
        if task.is_empty() || target.is_empty() {
            eprintln!("worklog-escalate-match: --task and --target are required");
            std::process::exit(2);
        }
        let cfg = Config::from_env();
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                match meridian::pm_worklog::escalate::escalate_match(&pool, &cfg, &task, &target)
                    .await
                {
                    Ok(res) => println!("{}", serde_json::to_string(&res).unwrap_or_default()),
                    Err(e) => {
                        pool.close().await;
                        if let Some(g) = obs_guard {
                            g.shutdown().await;
                        }
                        eprintln!("worklog-escalate-match: {e:#}");
                        std::process::exit(1);
                    }
                }
                pool.close().await;
            }
            Err(e) => {
                eprintln!("worklog-escalate-match: open db: {e:#}");
                std::process::exit(1);
            }
        }
        if let Some(g) = obs_guard {
            g.shutdown().await;
        }
        return Ok(());
    }

    // `meridian worklog-status [--day YYYY-MM-DD]` — a human-readable report of
    // the day's worklogs (hours done/pending/stuck, rows by state, per-ticket
    // comments + flagged ones). Read-only; no daemon init.
    if std::env::args().nth(1).as_deref() == Some("worklog-status") {
        let args: Vec<String> = std::env::args().collect();
        let day = args
            .iter()
            .position(|a| a == "--day")
            .and_then(|i| args.get(i + 1).cloned());
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                meridian::pm_worklog::cli_status(&pool, day.as_deref()).await;
                pool.close().await;
            }
            Err(e) => eprintln!("worklog-status: open db: {e}"),
        }
        return Ok(());
    }

    // `meridian doctor` — content-free system-health sweep. Read-only, no daemon
    // init. Surfaces broken capture/config so a misclassification isn't blamed on
    // the model. Currently covers L1 screenpipe capture; more layers TBD. Exits
    // non-zero if any check is critical.
    if std::env::args().nth(1).as_deref() == Some("doctor") {
        // `--porcelain` emits TSV rows for machine ingestion; otherwise a rich,
        // colour-when-a-tty, by-daemon table. Read-only comprehensive sweep.
        let porcelain = std::env::args().any(|a| a == "--porcelain");
        let fix = std::env::args().any(|a| a == "--fix");
        let dry_run = std::env::args().any(|a| a == "--dry-run");
        let cfg = Config::from_env();
        let report = meridian::health::run_all(&cfg).await;
        if fix {
            // Attempt repair (auto silently, guided with confirm); exit non-zero
            // if anything still needs a human.
            let residual = meridian::health::fix::run(&cfg, &report, dry_run);
            std::process::exit(if residual { 1 } else { 0 });
        }
        if porcelain {
            print!("{}", report.render_porcelain());
        } else {
            use std::io::IsTerminal;
            let color = std::io::stdout().is_terminal();
            print!("{}", report.render(color));
            // Diagnose + escalate: chain the warnings into root causes, then
            // point at `--fix` / support / claude.
            let dx = meridian::health::diagnose::root_causes(&report);
            print!("{}", meridian::health::diagnose::render(&dx, color));
            if report.worst() >= meridian::health::Severity::Warn {
                print!("{}", meridian::health::diagnose::escalation_hint(color));
            }
        }
        let critical = report.worst() == meridian::health::Severity::Critical;
        std::process::exit(if critical { 1 } else { 0 });
    }

    // `meridian uninstall [--purge] [--dry-run] [--yes]` — stop + remove the
    // launchd agents and staged binaries (inverse of the tray's first-run
    // install). One-shot, no daemon init; survives the .app being trashed because
    // this binary lives at ~/.meridian/bin/meridian.
    if std::env::args().nth(1).as_deref() == Some("uninstall") {
        let args: Vec<String> = std::env::args().collect();
        meridian::uninstall::run(&args);
        return Ok(());
    }

    // `meridian telemetry <status|export|import>` — telemetry spool management.
    // Read-only for status/export; import POSTs to OO. No daemon init needed.
    if std::env::args().nth(1).as_deref() == Some("telemetry") {
        let args: Vec<String> = std::env::args().collect();
        meridian::telemetry_spool::cli::run(&args).await;
        return Ok(());
    }

    // `meridian logs [--service <name>] [-n N] [-f]` — decode the local OTel
    // spool into human-readable lines. The direct replacement for the old
    // bash `meridian logs` (which tailed launchd-redirected stdout/stderr
    // text) now that the OTel spool is the sole log/trace sink — see
    // observability.rs's module doc. No daemon init needed.
    if std::env::args().nth(1).as_deref() == Some("logs") {
        let args: Vec<String> = std::env::args().collect();
        meridian::telemetry_spool::render::run(&args).await;
        return Ok(());
    }

    // 1b. Anything left in argv[1] that isn't a flag is a subcommand NO block above
    //     claimed — a typo, or (the real case) a caller newer than this binary. Falling
    //     through to the daemon is a trap: `meridian plan-task-draft` on a stale binary
    //     silently booted a SECOND daemon, which the single-instance guard then killed
    //     with a warning on stdout, and the tray reported "could not parse result" —
    //     a message that names neither the stale binary nor the unknown subcommand.
    //     Exit 2 (the CLI's bad-args code) and say the word we didn't recognise, so the
    //     next person sees the cause instead of a mystery. Bare `meridian` (no args) and
    //     flags like `--version` still start the daemon, which is the documented entry.
    if let Some(arg) = std::env::args().nth(1) {
        if !arg.starts_with('-') {
            eprintln!(
                "meridian: unknown subcommand {arg:?}\n\
                 If you expected this to exist, this binary is older than whatever called it \
                 ({}). Reinstall or rebuild it.",
                std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "the meridian binary".to_string()),
            );
            std::process::exit(2);
        }
    }

    // 2. Tracing — layered subscriber (OTLP-only: spool traces + logs; see
    //    observability.rs's module doc for why there's no stdout/file mirror).
    //    Guard must outlive the program; we shut it down explicitly at the end
    //    so OTel's blocking flush doesn't run inside tokio's drop path.
    let obs_guard = observability::init("meridian-rust")?;

    // 3. Load initial config — DB paths and startup parameters come from here.
    //    DB pool paths and observability are fixed at startup and do not change.
    let initial_cfg = Config::from_env();
    tracing::info!(stage = "config_loaded", "configuration ready");

    // 4. Log startup parameters.
    //
    //    `pid` is here so a generation can be TOLD APART from the next one.
    //    Without it, a machine that started three daemons in 35 seconds — which
    //    is what a quit-then-relaunch during an update looks like — produces
    //    three identical "meridian daemon starting" lines and a set of signal
    //    lines that cannot be attributed to any of them. Every corruption
    //    investigation so far has run aground on exactly that: the events are
    //    all in the spool, and nothing says which process each belongs to.
    //
    //    Logged as `i64`, not the `u32` `std::process::id` returns, and this is
    //    load-bearing rather than cosmetic. `tracing-opentelemetry` 0.28 has no
    //    `record_u64`, so a `u32`/`u64` field falls through to `record_debug`
    //    and is emitted as a STRING — which then has to survive the attribute
    //    allowlist as a string key to egress at all. An `i64` becomes a real
    //    `IntValue`, and `redact::keep_attribute`'s first arm keeps every
    //    `IntValue` unconditionally. See CLAUDE.md's third "coupling that
    //    silently deletes error coverage".
    tracing::info!(
        pid = std::process::id() as i64,
        meridian_db = %initial_cfg.meridian_db,
        poll_interval_secs = initial_cfg.poll_interval_secs,
        "meridian daemon starting"
    );

    // 4a-bis. Stand down if a repair is claimed on this database.
    //
    //     MUST come before `setup_db` below: the whole point is not to open a
    //     pool on a file the tray is about to rebuild and swap out from under
    //     us. Migrations would run against the doomed file, and any write we
    //     made would land in the copy that gets moved aside.
    //
    //     Exiting cleanly (not erroring) is deliberate. launchd's KeepAlive
    //     relaunches us after ThrottleInterval — 30 s on the shipped plist —
    //     so this is a stand-down that repeats every half minute until the
    //     marker clears, not a crash loop. `meridian::db::repair::marker`
    //     explains why the alternative (having the tray hold the daemon down
    //     via launchctl) is a trap this repo has already been bitten by, and
    //     why the marker expires rather than trusting the tray to live.
    if meridian::db::repair::marker::pending(std::path::Path::new(&initial_cfg.meridian_db)) {
        tracing::info!(
            "a database repair is in progress — standing down without opening the database"
        );
        obs_guard.shutdown().await;
        meridian::telemetry_spool::shipper::drain_once().await;
        return Ok(());
    }

    // 4a-ter. Single-instance guard, part 1 of 2: the endpoint PROBE. Cheap,
    //     informative, and not authoritative — 4a-quater below is the acquire.
    //     Checked here, before `setup_db`, even though the listener isn't
    //     bound until 5b.
    //
    //     ~/.meridian/daemon.sock: a successful connect that gets a greeting
    //     means ANOTHER daemon already owns this data dir. That happens
    //     routinely — a leftover packaged install's launchd agent
    //     (KeepAlive=true) respawning next to a dev build, two `meridian`
    //     invocations racing — and, the case that motivated moving this check
    //     ahead of `setup_db`, a version-update restart where
    //     `backend_install::register_agent` bootstraps the new daemon after a
    //     15s best-effort wait for the old launchd entry to clear, and
    //     proceeds anyway (logged at WARN) if it doesn't. Previously this
    //     check ran AFTER `setup_db` (4b) and the capture preflight (4c), so a
    //     daemon that was always going to lose this race still opened a pool
    //     and ran migrations — including a live `ALTER TABLE` — against a
    //     file the winning daemon could simultaneously be writing to or
    //     checkpointing. Checking before `setup_db` means a losing daemon
    //     never touches meridian.db at all — but only for a race this probe
    //     can SEE, which is why the lock at 4a-quater exists.
    //
    //     Only a stale socket (no listener) is removed, and only right before
    //     THIS process binds its own — see the bind site below. Whoever starts
    //     second bows out; dev-start.sh stops the installed daemon first so
    //     the dev build wins, and any KeepAlive respawn self-terminates here,
    //     repeating every ~30s (ThrottleInterval) until the winner's listener
    //     goes away — the same non-crash stand-down cadence as the repair-
    //     marker check above, not a crash loop (`KeepAlive` is unconditional
    //     in the shipped plist, so both an `exit(1)` and this `return Ok(())`
    //     relaunch regardless).
    //
    //     The endpoint itself is OS-specific (a socket file on Unix, a named
    //     pipe on Windows) — see `meridian::platform`.
    //
    //     `pid` on this and the two stand-down logs below is not decoration.
    //     These WARNs are the ONLY record of a stand-down that reaches central
    //     telemetry: the redaction ship leg is WARN+ only, and `meridian daemon
    //     starting` — the line that would otherwise carry the pid — is INFO and
    //     never egresses. Without it a stand-down arrives as an anonymous event
    //     that cannot be tied to a process or correlated with anything around
    //     it, which is the same gap that made the 2026-08-25 investigation
    //     unresolvable.
    if meridian::platform::daemon_already_running().await {
        tracing::warn!(
            pid = std::process::id() as i64,
            endpoint = %meridian::platform::endpoint_display(),
            "another meridian daemon already owns this data dir — exiting (single-instance guard)"
        );
        return Ok(());
    }

    // 4a-quater. ACQUIRE the single-instance lock. The check above is a probe;
    //     this is the acquire, and the difference is the whole point.
    //
    //     `daemon_already_running` asks "is anyone listening?" — and the winner
    //     of a two-daemon race has NOT bound its listener at that moment,
    //     because the bind is deliberately deferred to 5b so a daemon about to
    //     `exit(1)` on a corrupt database never advertises `{"running":true}`.
    //     So two daemons starting together both see nothing, both fall through,
    //     and both run `setup_db` — migrations included, `ALTER TABLE`
    //     included — against one file. That is check-then-act, and this repo
    //     has a documented history of `database disk image is malformed`
    //     attributed to exactly two writers.
    //
    //     `flock`/`LockFileEx` is atomic: exactly one caller wins however the
    //     two interleave. The guard is held for the rest of the process's life
    //     and released by the OS when the process dies, however it dies — so
    //     there is no stale lock to reason about and nothing to clean up.
    //
    //     The probe stays. It is the cheap, informative check, it produces the
    //     better log line, and it is the ONLY thing that sees a daemon from a
    //     build predating this lock — which is every daemon during the rollout
    //     window. Neither is redundant.
    //
    //     `None` below is "running unlocked", not "no lock needed" — see the
    //     Unavailable arm.
    let _single_instance_lock = match meridian::platform::acquire_single_instance_lock() {
        meridian::platform::LockOutcome::Acquired(guard) => Some(guard),
        // This WARN is a MEASUREMENT as much as a stand-down. It fires exactly
        // when two daemons raced and the probe above did not see it — the case
        // that was previously invisible AND unguarded. Because it egresses to
        // central telemetry, its rate across the fleet is the first direct
        // evidence of how often this actually happens, rather than how often it
        // could happen in principle. Rewording it is fine; dropping it or
        // demoting it below WARN removes the only signal we have.
        meridian::platform::LockOutcome::HeldByAnother => {
            tracing::warn!(
                pid = std::process::id() as i64,
                "another meridian daemon holds the single-instance lock for this data dir — exiting (lock)"
            );
            return Ok(());
        }
        // Could not find out. Proceed UNLOCKED rather than refuse to start:
        // before this lock existed there was no lock at all, so running on is
        // exactly the previous behaviour and gives up nothing that was ever
        // guaranteed. Standing down here would instead be a brand-new way for
        // the daemon to be permanently dead on a machine where nothing is
        // wrong (a read-only home, an odd errno, an antivirus holding the
        // file). A guard against a rare race must not be able to cause a
        // common outage.
        meridian::platform::LockOutcome::Unavailable(e) => {
            tracing::warn!(
                pid = std::process::id() as i64,
                error = %e, // not-anyhow: a String the acquire already formatted with its full cause; there is no chain to walk
                "could not take the single-instance lock — continuing without it; \
                 the endpoint probe above remains the only guard this start has"
            );
            None
        }
    };

    // 4b. Open / create meridian pool and run migrations FIRST — before any
    //     preflight that can block or fail. The UI and MCP server read this DB
    //     directly, so it must exist even when an optional component (capture,
    //     an agent CLI) is degraded; ordering it after a preflight that could
    //     block once left machines running a daemon that never created its own
    //     database.
    let meridian = match setup_db(&initial_cfg.meridian_db_uri()).await {
        Ok(pool) => pool,
        Err(e) => {
            // The daemon cannot run without its database, and a bare `?` here
            // would unwind to `main`'s stderr Debug print — invisible to central
            // OO, because this path dies *before* the telemetry shipper (7f
            // below) ever starts, so nothing ever drains the spool. Report it
            // through `tracing` and then flush + one-shot ship (mirroring the
            // shutdown path at the end of `main`) so a daemon that can't open its
            // DB — a wrong/absent encryption key, a file another process holds
            // locked, corruption — is finally diagnosable in central telemetry
            // instead of crash-looping silently. `shutdown` consumes the guard,
            // but we exit immediately after, so the success path still owns it.
            tracing::error!(
                error = %meridian::errors::chain(&e),
                "daemon startup: failed to open the database — the daemon cannot start (wrong/absent encryption key, a locked file, or corruption)"
            );
            obs_guard.shutdown().await;
            meridian::telemetry_spool::shipper::drain_once().await;
            std::process::exit(1);
        }
    };

    // 4c. Capture-layer (L1) preflight: surface degraded in-process capture
    //     (revoked Screen Recording / Accessibility permission, the tray not
    //     running, stale frames) before the poll loop. Reads meridian's own
    //     capture tables — no screenpipe pool. Non-fatal — the daemon still runs;
    //     we log the fault so misclassifications aren't blamed on the model.
    meridian::health::Report::new(meridian::health::capture::checks(&meridian).await)
        .log("startup");

    // 5b. Bind the health-endpoint socket now that the pool is open and
    //     migrations have run. The single-instance CHECK already happened
    //     above (4a-ter), before `setup_db` — deliberately not moved down
    //     here with the bind: binding only now means the greeting
    //     (`{"running":true}`) is never advertised until the database is
    //     actually usable. Binding it back at the earlier check site instead
    //     would let a daemon that's about to `exit(1)` on a locked or corrupt
    //     database falsely tell the tray's watchdog it's healthy for the
    //     brief window before that failure surfaces. Safe to bind
    //     unconditionally here: we hold the single-instance lock taken at
    //     4a-quater, so no other daemon process reached this line at all. (The
    //     older justification — "the check above established nothing else is
    //     listening, and we're single-threaded up to the poll loop" — was only
    //     ever about THIS process's threads and said nothing about a second
    //     process. The lock is what actually makes this claim true.)
    //
    //     NOTE: `spawn_health_listener` itself unlinks a stale socket and then
    //     binds, which is its own check-then-act across processes. The lock
    //     makes that unreachable for two daemons, but the sequence is left
    //     untouched here on purpose — #862 owns that boundary.
    meridian::platform::spawn_health_listener()?;
    tracing::info!(endpoint = %meridian::platform::endpoint_display(), "daemon health endpoint ready");

    // 6. Graceful shutdown, on whichever signal set this OS provides
    //    (SIGINT/SIGTERM/SIGHUP, or the Windows console-control events).
    use meridian::platform::wait_for_shutdown;

    // 7a. Clean up any runs left in 'running' state from a previous crash.
    match cleanup_incomplete_runs(&meridian).await {
        Ok(0) => {
            tracing::info!("no incomplete runs found");
        }
        Ok(n) => tracing::warn!(
            deleted_partial_sessions = n,
            "cleaned up incomplete ETL run"
        ),
        Err(e) => tracing::error!(
            error = %meridian::errors::chain(&e),
            "cleanup_incomplete_runs failed"
        ),
    }

    // 7a-bis. Same recovery, for the worklog pipeline's own ledger: an hour left
    // `generating` by a crash mid-`/worklog_hour` call would otherwise never
    // retry and would silently block every hour after it from ever getting a
    // ledger row (see reset_stuck_generating_hours's doc comment).
    match meridian::pm_worklog::ledger::reset_stuck_generating_hours(&meridian).await {
        Ok(0) => {
            tracing::info!("no stuck-generating worklog hours found");
        }
        Ok(n) => tracing::warn!(
            reset_count = n,
            "reset worklog hour(s) stuck in generating from a previous crash"
        ),
        Err(e) => tracing::error!(
            error = %meridian::errors::chain(&e),
            "reset_stuck_generating_hours failed"
        ),
    }

    // 7b. Shared handles the poll loop uses to signal ETL ticks to observers.
    let etl_notify: Arc<Notify> = Arc::new(Notify::new());
    let etl_tick_span: Arc<std::sync::Mutex<Option<tracing::Span>>> =
        Arc::new(std::sync::Mutex::new(None));

    // 7b-bis. Structural screen of meridian.db, run in the background rather
    //     than blocking the first ETL pass.
    //
    //     `quick_check` catches damage the ETL would otherwise only discover
    //     when a query happened to touch a bad page — which, depending on
    //     where the corruption sits, can be hours later or (if it is confined
    //     to a table the ETL never reads) never, while the tray silently fails
    //     every frame write.
    //
    //     It reads every page in the file, which is multi-second on a
    //     multi-GB database and would otherwise tax every daemon start —
    //     including every launchd `KeepAlive` restart. Spawning it loses no
    //     coverage: `etl_tick`'s own error path independently classifies
    //     corruption the moment a query touches it, so the worst case here is
    //     detecting a table the ETL never reads a few seconds later than an
    //     inline check would, not missing it. Deliberately NOT inside
    //     `setup_db` either: that has ~20 call sites, almost all short-lived
    //     CLI hops.
    //
    //     Non-fatal. A corrupt database still serves the dashboard from the
    //     tables that survived, and `meridian db repair` needs the user to
    //     stop the daemon anyway — exiting here would just crash-loop under
    //     launchd's KeepAlive.
    let db_corrupt = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let meridian = meridian.clone();
        let db_corrupt = db_corrupt.clone();
        // Wrapped in its own span rather than relying on the startup_tick span below: this
        // task is detached (spawned, not awaited) and can still be running once that span
        // has closed, so nesting under it would attribute the check's duration/failure to a
        // span that may already have ended.
        tokio::spawn(
            async move {
                match meridian::db::integrity::quick_check(&meridian, 20).await {
                    Ok(problems) if problems.is_empty() => {}
                    Ok(problems) => {
                        tracing::error!(
                            problem_count = problems.len(),
                            first = %problems.first().map(String::as_str).unwrap_or_default(),
                            "meridian.db failed its integrity check at startup — the ETL will not run until it is repaired"
                        );
                        let _ = meridian::notices::raise_typed(
                            &meridian,
                            meridian::notices::Notice {
                                id: DB_CORRUPT_NOTICE,
                                severity: "error",
                                title: "Meridian's database is damaged",
                                detail: &problems.join("; "),
                                remedy: Some(
                                    "Quit Meridian, then run 'meridian db repair' in a terminal",
                                ),
                                event_key: DB_CORRUPT_NOTICE,
                                deep_link: Some(meridian_core::notifications::deep_links::LOGS),
                            },
                        )
                        .await;
                        db_corrupt.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    // The check itself failing is not evidence either way — carry on and
                    // let the ETL's own error path classify it.
                    Err(e) => {
                        tracing::warn!(error = %meridian::errors::chain(&e), "startup integrity check could not run");
                    }
                }
            }
            .instrument(tracing::info_span!("startup_db_integrity_check")),
        );
    }

    // 7c. Run ETL once immediately before entering the loop.
    {
        let startup_tick = tracing::info_span!("startup_tick");
        *etl_tick_span.lock().unwrap_or_else(|e| e.into_inner()) = Some(startup_tick.clone());
        let _guard = startup_tick.enter();
        // Skipped outright when 7b-bis already found the database damaged —
        // the notice is raised and the pass could only fail. The background
        // check may still be in flight; if so this pass runs anyway and
        // `etl_tick`'s own error path classifies corruption just the same.
        if !db_corrupt.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("running initial ETL pass");
            if etl_tick(&meridian).await {
                db_corrupt.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        etl_notify.notify_one();
        // Retention reads capture_frames too, so on a corrupt database it can
        // only fail the same way — skip it rather than log a second, more
        // confusing error for the same root cause.
        if !db_corrupt.load(std::sync::atomic::Ordering::Relaxed) {
            if let Err(e) = meridian::etl::capture_retention::prune_capture_tables(&meridian).await
            {
                tracing::warn!(error = %meridian::errors::chain(&e), "capture retention sweep failed");
            }
        }
        // No PM sync here anymore — there's no background poller. Syncing is
        // on-demand now (the daily plan, the match-to-ticket picker, connecting a
        // tracker, a board write, worklog drafting, `meridian tasks-sync`/`pm-sync`);
        // see `src/intelligence/mod.rs`'s doc comments for the full set of triggers.
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // 8a-bis. Coding-agent tasks (both gated — dormant if neither agent is
    //         present). The indexer turns Claude Code / Codex JSONLs into
    //         app_sessions segment rows; the summariser turns sealed segments
    //         into prose summaries. They share a Notify so the summariser wakes
    //         near-instantly on the indexer's own seals (plus its own sweep for
    //         hook-sealed rows). Decoupled from the ETL tick.
    {
        let ca_notify: Arc<Notify> = Arc::new(Notify::new());
        let pool_idx = meridian.clone();
        let notify_idx = ca_notify.clone();
        let rx_idx = shutdown_rx.clone();
        tokio::spawn(async move {
            meridian::coding_agent_session_ingest::indexer::run_loop(pool_idx, notify_idx, rx_idx)
                .await;
        });
        let pool_sum = meridian.clone();
        let rx_sum = shutdown_rx.clone();
        tokio::spawn(async move {
            meridian::coding_agent_session_ingest::summariser::run_loop(
                pool_sum, ca_notify, rx_sum,
            )
            .await;
        });
    }

    // 7d. PM-worklog driver: hour-level pipeline — clock-aligned (HH:03 local), runs
    //     once per completed hour fully in-process (distil → report → workstream fold
    //     → draft). Drafts only; posting is run_post_loop's job.
    {
        let pool_pm = meridian.clone();
        let db_path_pm = initial_cfg.meridian_db.clone();
        let rx_pm = shutdown_rx.clone();
        tokio::spawn(async move {
            meridian::worklog_pipeline::run_loop(pool_pm, db_path_pm, rx_pm).await;
        });
    }

    // 7e. PM-worklog approved-poster: the ~60s sweep that posts worklogs the user
    //     approved in the dashboard to Jira. This is the SOLE path to real Jira
    //     (there is no unattended auto-post). Gated on the global LLM gate's
    //     siblings only — posting itself is a plain HTTP call, not an LLM hop.
    {
        let pool_post = meridian.clone();
        let rx_post = shutdown_rx.clone();
        tokio::spawn(async move {
            meridian::pm_worklog::run_post_loop(pool_post, rx_post).await;
        });
    }

    // 7f. Telemetry spool shipper: drains ~/.meridian/telemetry/pending/ to
    //     OpenObserve every MERIDIAN_TELEMETRY_SHIP_INTERVAL_S (default 30s).
    //     Active only when otlp_enabled is true; noop ticks if no OO target is
    //     configured (e.g. credentials not yet set).
    //
    //     The shipper gets its OWN shutdown channel (not the shared one) so the
    //     shutdown sequence can flush the OTel exporters — which write the
    //     daemon's final spans/logs INTO the spool — and drain them BEFORE the
    //     shipper stops. Stopping it on the shared signal would race the flush and
    //     strand the shutdown telemetry until the next daemon start.
    let (shipper_shutdown_tx, shipper_shutdown_rx) = tokio::sync::watch::channel(false);
    {
        tokio::spawn(async move {
            meridian::telemetry_spool::shipper::run_shipper(shipper_shutdown_rx).await;
        });
    }

    // 7g. Embedder weight provisioning: first-run download of the session distiller's
    //     on-device embedding model (~130 MB). Fire-and-forget — `embedder::is_ready()`
    //     gates every caller, so a slow or failed download just means the distiller keeps
    //     taking its lexical-only degrade path until this succeeds (retried next start).
    //     Idempotent (`ensure_weights` skips files already on disk), so this is safe to run
    //     unconditionally on every boot rather than only once at setup time.
    {
        tokio::spawn(async move {
            if let Err(e) = meridian::embedder::ensure_weights().await {
                tracing::warn!(error = %meridian::errors::chain(&e), "embedder: weight provisioning failed — distiller stays on lexical-only until this succeeds");
            }
        });
    }

    // 7h. PM sync request watcher — the daemon is the SOLE holder of the rotating
    //     Jira OAuth refresh token, so every other process (the tray, the CLIs) asks
    //     for a sync by writing a `pm_sync_requests` row instead of refreshing
    //     itself. This drains those requests.
    //
    //     Why single-ownership matters: the refresh is a single-use exchange whose
    //     lost response kills the grant outside a 10-minute window. Serialising N
    //     processes with an advisory file lock did not work — a 10 s lock timeout
    //     guarding a ~26 s operation, and on timeout the code proceeded WITHOUT the
    //     lock — so two processes could spend the same token and only Atlassian's
    //     grace window prevented corruption.
    //
    //     Deliberately its own task on a short cadence rather than folded into the
    //     60 s poll loop below: a user pressing "Sync now" must not wait up to a
    //     minute for the sync to begin. It never syncs on its own initiative — only
    //     ever on a row a producer wrote, so every refresh still traces to a human
    //     action, which is what keeps a refresh POST from being in flight when a
    //     laptop lid closes.
    //
    //     Its `JoinHandle` is KEPT, unlike the loops above, and awaited in the
    //     shutdown sequence before the WAL checkpoint. This task is the daemon's
    //     only writer outside the poll loop, and `service_once` deliberately runs a
    //     whole sync (network + `pm_tasks` writes, tens of seconds) without
    //     re-checking the shutdown flag, so dropping the handle meant checkpointing
    //     and closing the pool underneath live writes on every restart. See the
    //     shutdown site for the full reasoning.
    let sync_watcher = {
        let pool_sync = meridian.clone();
        let rx_sync = shutdown_rx.clone();
        tokio::spawn(async move {
            meridian::intelligence::sync_requests::run_watcher(pool_sync, rx_sync).await;
        })
    };

    // 8b. Poll loop — ETL, PM sync, and FM categorization on the configured interval.
    // Track the last-applied log level so we can detect changes and hot-reload
    // the EnvFilter without restarting the daemon.
    let mut last_log_level = initial_cfg.runtime.log_level.clone();

    loop {
        // Determine the sleep duration from the current settings.json before sleeping.
        let poll_interval = {
            let cfg = Config::from_env();
            Duration::from_secs(cfg.runtime.poll_interval_secs)
        };

        tokio::select! {
            _ = wait_for_shutdown() => {
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Re-read config to pick up any settings.json changes made while sleeping.
                let cfg = Config::from_env();

                // Hot-reload the log level if it changed in settings.json.
                if cfg.runtime.log_level != last_log_level
                    && observability::reload_log_level(&cfg.runtime.log_level)
                {
                    tracing::info!(
                        old_level = %last_log_level,
                        new_level = %cfg.runtime.log_level,
                        "log level hot-reloaded"
                    );
                    last_log_level = cfg.runtime.log_level.clone();
                }
                let poll_tick = tracing::info_span!(
                    "poll_tick",
                    poll_interval_secs = cfg.runtime.poll_interval_secs
                );
                *etl_tick_span.lock().unwrap_or_else(|e| e.into_inner()) = Some(poll_tick.clone());
                let _guard = poll_tick.enter();
                tracing::debug!("starting ETL tick");
                // Latched, never re-tried — see DB_CORRUPT_NOTICE. Everything
                // else in the tick (PM sync, notifications, plan nudge) reads
                // tables corruption may not have touched, so the daemon stays
                // useful instead of exiting.
                if !db_corrupt.load(std::sync::atomic::Ordering::Relaxed) {
                    if etl_tick(&meridian).await {
                        db_corrupt.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    // Wake the background task linker to drain newly-created sessions.
                    etl_notify.notify_one();

                    // Capture-table retention — age + processed-based prune of
                    // capture_frames/capture_ui_events/capture_secondary_screens,
                    // which otherwise grow unbounded. Runs every tick; internally
                    // paces its own incremental_vacuum to a coarser cadence.
                    if !db_corrupt.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Err(e) = meridian::etl::capture_retention::prune_capture_tables(&meridian).await {
                            tracing::warn!(error = %meridian::errors::chain(&e), "capture retention sweep failed");
                        }
                    }
                }

                // Morning plan nudge — idempotent per day, gated to working hours.
                if let Err(e) = meridian::daily_plan::maybe_nudge(&meridian).await {
                    tracing::debug!(error = %meridian::errors::chain(&e), "plan nudge check skipped");
                }

                // Coding-agent summariser dead-letter digest — idempotent per day.
                if let Err(e) =
                    meridian::coding_agent_session_ingest::summariser::maybe_notify_dead_letters(
                        &meridian,
                    )
                    .await
                {
                    tracing::debug!(error = %meridian::errors::chain(&e), "summariser dead-letter digest check skipped");
                }

                // Disk space on ~/.meridian's volume — raise/clear every tick,
                // same idempotent pattern as etl.failed above.
                match meridian::health::platform::meridian_data_low_gb() {
                    Some(gb) => {
                        let _ = meridian::notices::raise_typed(
                            &meridian,
                            meridian::notices::Notice {
                                id: "system.disk_low",
                                severity: "warning",
                                title: "Disk space is low",
                                detail: &format!(
                                    "Only {gb:.1} GB free — Meridian's database may stop writing."
                                ),
                                remedy: Some("Free up disk space to keep tracking running."),
                                event_key: "system.disk_low",
                                deep_link: None,
                            },
                        )
                        .await;
                    }
                    None => {
                        let _ = meridian::notices::clear_typed(
                            &meridian,
                            "system.disk_low",
                            "system.disk_low",
                        )
                        .await;
                    }
                }

                // Groq is discontinued and actively BLOCKED (see the hard refusal in
                // `llm::resolver::complete_inner` — every call is refused unconditionally
                // while Groq is the active provider, so hourly summaries stop, not just
                // "may fail"). Its free tier's token-rate limits are too tight for this
                // pipeline's hourly calls — real, observed production failures, not a
                // hypothetical. The row itself is left untouched (still visible, still
                // manageable, still selectable) so the user isn't locked out of their own
                // settings, but the moment Groq is active this banner says plainly that
                // nothing is being sent to it. Raise/clear every tick, same idempotent
                // pattern as disk-space above. This is the steady-state driver; the tray's
                // `update_settings` calls the SAME function the instant the provider changes,
                // so switching away from Groq clears the banner immediately rather than
                // leaving it up for up to a minute after the fix already took effect — see
                // `sync_groq_deprecated_notice`'s doc for why both call it.
                meridian::notices::sync_groq_deprecated_notice(
                    &meridian,
                    cfg.runtime.active_custom_provider().map(|p| p.vendor.as_str()),
                )
                .await;

                // Interactive-notification responses — act on the user's answers
                // (snooze re-enqueues an hour out). Idempotent end-to-end, so the
                // same cadence as the nudge above is safe.
                if let Err(e) =
                    meridian::notification_responses::consume_responses(&meridian).await
                {
                    tracing::debug!(error = %meridian::errors::chain(&e), "notification response consume skipped");
                }

                // No PM sync on this tick anymore — there's no background poller.
                // `pm_tasks` is refreshed on-demand instead: the daily plan and the
                // match-to-ticket picker (the two screens that decide something from
                // the whole board), connecting a tracker, any board write, the worklog
                // drafting sweep (`pm_worklog::auto_generate`), and the manual
                // `meridian tasks-sync`/`pm-sync` CLI paths. See
                // `src/intelligence/mod.rs`'s doc comments for the full trigger list
                // and the reasoning (fewer standing background refreshes also shrinks
                // how often a Jira OAuth token refresh can straddle a laptop
                // sleep/wake).
            }
        }
    }

    // Signal the task linker loops to stop. The shipper has its OWN channel and
    // is intentionally left running for now (see below).
    let _ = shutdown_tx.send(true);

    // 9. Shutdown
    tracing::info!(pid = std::process::id() as i64, "shutting down");

    // 9a. Wait for the PM sync watcher to actually STOP before touching the WAL.
    //
    //     `shutdown_tx.send(true)` above only sets a flag, and the watcher checks it
    //     on its sleep - not around `service_once`, which is deliberate (a claimed
    //     request finishes and records an outcome rather than being cut off with the
    //     row left claimed). The consequence is that after the signal this task can
    //     still be inside a full provider sync for tens of seconds, writing to
    //     `pm_tasks`. Without this await, `checkpoint_wal` and `close` below ran
    //     underneath those writes on EVERY restart - and a reconnect flow restarts
    //     the daemon, which is exactly when a burst of sync requests exists to
    //     service. A TRUNCATE checkpoint racing live writes is the one thing that
    //     function exists to prevent (see its doc: it hands the next generation a
    //     half-written WAL while the tray keeps a stale view of the file).
    //
    //     Bounded, because the whole point of the no-interrupt design is that
    //     `service_once` may be mid-network-call: on timeout we proceed anyway and
    //     say so, which is strictly better than the old unconditional race, and no
    //     worse than a hard kill.
    {
        const WATCHER_DRAIN_TIMEOUT: Duration = Duration::from_secs(45);
        match tokio::time::timeout(WATCHER_DRAIN_TIMEOUT, sync_watcher).await {
            Ok(Ok(())) => tracing::debug!("PM sync watcher stopped cleanly before checkpoint"),
            // `JoinError`'s Display ignores `f.alternate()`, so `chain()` would
            // render byte-identically - it carries a panic payload or a
            // cancellation, never a `.context()` chain.
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "sync watcher ended abnormally"); // not-anyhow: JoinError
            }
            Err(_elapsed) => tracing::warn!(
                timeout_s = WATCHER_DRAIN_TIMEOUT.as_secs() as i64,
                "PM sync watcher did not stop in time - checkpointing anyway, which may leave a non-empty WAL"
            ),
        }
    }

    // (`release_endpoint` used to be here. It now runs at 9b, after the pool is
    // closed - see there for why the exiting side's ordering matters too.)

    // See `db::meridian::checkpoint_wal`'s doc for why this runs before every
    // close, not just a plain shutdown. Best-effort: a failed checkpoint must
    // not block shutdown.
    //
    // BOTH outcomes are logged, and the success line is the point. Previously a
    // failed checkpoint logged a WARN and a successful one logged nothing, so
    // "no line after `shutting down`" meant either "it worked" or "the process
    // was killed part-way through" — indistinguishable, and the difference is
    // the entire question when a `meridian.db` is later found malformed. A
    // corruption investigation on 2026-08-25 stalled on exactly this ambiguity
    // and had to withdraw its conclusion. Now silence here means killed.
    match meridian::db::meridian::checkpoint_wal(&meridian).await {
        Ok(()) => tracing::info!(
            pid = std::process::id() as i64,
            "WAL checkpoint on shutdown complete"
        ),
        Err(e) => {
            tracing::warn!(error = %meridian::errors::chain(&e), "WAL checkpoint on shutdown failed - continuing anyway")
        }
    }
    meridian.close().await;

    // 9b. Release the single-instance endpoint LAST, after the pool is closed.
    //
    //     This used to run first, before the checkpoint. `daemon_already_running` is
    //     how a starting daemon decides whether to bow out (4a-ter), so releasing it
    //     early opens the guard while THIS process is still checkpointing and closing
    //     - the window `single_instance_check_precedes_setup_db_and_bind_follows_it`
    //     exists to close, reopened from the exiting side. It only pinned the
    //     STARTING daemon's ordering. Under launchd the relaunch is immediate, and a
    //     new generation running migrations against a file the old one is mid-
    //     checkpoint on is the double-writer profile the fleet-correlated corruption
    //     was traced to.
    meridian::platform::release_endpoint();

    // Flush OTel exporters FIRST, while the runtime is alive — this writes the
    // daemon's final shutdown spans/logs into the spool's pending/ dir...
    obs_guard.shutdown().await;
    // ...then run one last ship so that final batch reaches OO now rather than on
    // the next daemon start (the spool would persist it either way; this just
    // delivers it promptly, and matters when the daemon is being uninstalled)...
    meridian::telemetry_spool::shipper::drain_once().await;
    // ...and only now stop the shipper task.
    let _ = shipper_shutdown_tx.send(true);

    Ok(())
}

/// Body of the `tasks-sync` / `pm-sync` CLIs: ask the daemon and report what it did.
///
/// The delegation itself (and why a CLI must not spend the rotating Jira OAuth token
/// itself) lives in [`meridian::intelligence::sync_delegate`]; this only maps the
/// outcome onto stdout/stderr and an exit code, so the user-facing wording stays in
/// one place next to the other CLI output.
async fn cli_sync(
    pool: &meridian::db::SqlitePool,
    cfg: &Config,
    mode: SyncMode,
    label: &str,
) -> bool {
    // A generous budget: a cold sync across five providers with a token refresh can
    // legitimately take a while, and a CLI that gives up early looks like a failure
    // when the sync is still going. Syncing IS the point of this command, so unlike
    // the post-write callers it waits.
    match meridian::intelligence::sync_delegate::sync_and_wait(
        pool,
        cfg,
        mode,
        label,
        std::time::Duration::from_secs(120),
    )
    .await
    {
        Delegation::Synced { count: Some(n) } => {
            println!("{label}: synced {n} task(s)");
            true
        }
        Delegation::Synced { count: None } => {
            println!("{label}: synced");
            true
        }
        Delegation::Failed { error } => {
            eprintln!("{label}: {error}");
            false
        }
        // NOT an error exit: the request is queued and the daemon will service it.
        // Exiting non-zero here would fail scripts over a slow sync that ultimately
        // succeeds.
        Delegation::Pending => {
            println!("{label}: still running - it will finish in the background");
            true
        }
    }
}

/// Refresh the board after a CLI write applied to the tracker (`ticket-update`,
/// `ticket-set-status`).
///
/// Waits for the outcome (see `sync_delegate::POST_WRITE_SYNC_BUDGET`): the frontend
/// re-reads the board as soon as this process exits, so returning before the mirror
/// caught up would briefly show the pre-write value and read as a lost edit. Failures
/// and timeouts are logged, never printed - the tracker write already succeeded, so a
/// sync hiccup must not make the command look like it failed.
async fn cli_sync_after_write(pool: &meridian::db::SqlitePool, cfg: &Config, label: &str) {
    match meridian::intelligence::sync_delegate::sync_after_write(pool, cfg, label).await {
        Delegation::Synced { .. } => {}
        Delegation::Failed { error } => {
            tracing::warn!(label, %error, "post-write pm sync failed - the tracker write still landed");
        }
        Delegation::Pending => {
            tracing::warn!(
                label,
                "post-write pm sync still running - the tracker write still landed"
            );
        }
    }
}

/// Runs one ETL pass and maps the outcome onto the notice bus.
///
/// Returns `true` when the failure was database corruption, which the caller
/// MUST treat as terminal for the ETL: see [`DB_CORRUPT_NOTICE`].
///
/// Shared by the startup pass and the poll loop so the two cannot drift - they
/// previously carried copy-pasted `raise`/`clear` blocks, and a corruption
/// branch added to one of them only would leave whichever path the user hit
/// first still spinning.
async fn etl_tick(meridian: &meridian::db::SqlitePool) -> bool {
    match run_etl(meridian).await {
        Ok(_) => {
            let _ = meridian::notices::clear(meridian, "etl.failed").await;
            false
        }
        Err(e) if meridian::db::integrity::is_corrupt_error(&e) => {
            tracing::error!(
                error = %meridian::errors::chain(&e),
                "ETL run failed: meridian.db is corrupt - stopping the ETL until it is repaired"
            );
            // Distinct from `etl.failed` on purpose. The generic notice says
            // "Open /logs", which for corruption is a dead end: nothing in the
            // logs tells the user what to do, and the condition cannot clear
            // by itself.
            let _ = meridian::notices::raise_typed(
                meridian,
                meridian::notices::Notice {
                    id: DB_CORRUPT_NOTICE,
                    severity: "error",
                    title: "Meridian's database is damaged",
                    detail: &e.to_string(),
                    remedy: Some("Quit Meridian, then run 'meridian db repair' in a terminal"),
                    event_key: DB_CORRUPT_NOTICE,
                    deep_link: Some(meridian_core::notifications::deep_links::LOGS),
                },
            )
            .await;
            true
        }
        Err(e) => {
            tracing::error!(error = %meridian::errors::chain(&e), "ETL run failed");
            let _ = meridian::notices::raise(
                meridian,
                "etl.failed",
                "error",
                "Activity capture pipeline failed",
                &e.to_string(),
                Some("Open /logs in the dashboard to see details"),
            )
            .await;
            false
        }
    }
}

/// Notice id raised when `meridian.db` is structurally damaged.
///
/// Unlike every other fault on the bus this one is **latched, not polled**:
/// once raised, the daemon stops calling `run_etl` for the rest of the
/// process's life. Corruption cannot resolve without an operator running
/// `meridian db repair` (which requires the daemon to be stopped anyway), so
/// retrying every 60 s only re-reads damaged pages behind a banner that is
/// already correct. That retry loop is what this whole feature replaces: the
/// motivating incident spun a failing `get_frames_since` once a minute for
/// over a day.
///
/// The id itself lives in the lib ([`meridian::notices::DB_CORRUPT`]) because
/// `db::repair` must clear the very same id from a rebuilt database.
const DB_CORRUPT_NOTICE: &str = meridian::notices::DB_CORRUPT;

#[cfg(test)]
mod startup_order_tests {
    /// The single-instance guard's CHECK must run before `setup_db` opens the
    /// pool and runs migrations, and the health-endpoint BIND must run after.
    ///
    /// This is the whole fix: a daemon that is about to lose the
    /// single-instance race must never touch `meridian.db` — see the comment
    /// at the check's call site (4a-ter) for the fleet-correlated corruption
    /// this closes. Regressing either half reopens a real hazard:
    /// - check moved back after `setup_db` → a losing daemon runs migrations
    ///   (including a live `ALTER TABLE`) against a file the winning daemon
    ///   may be concurrently writing to or checkpointing — the double-writer
    ///   window this change exists to close.
    /// - bind moved up alongside the check → the health socket can advertise
    ///   `{"running":true}` for a daemon that is about to `exit(1)` on a
    ///   locked/corrupt database, feeding the tray's watchdog a false-healthy
    ///   signal.
    ///
    /// `main()` shells out to `launchctl`/binds real OS sockets and can't be
    /// unit-tested directly, so — matching the established idiom in
    /// `backend_install.rs` (`a_stuck_bootout_is_reported_at_warn`,
    /// `every_early_return_still_restores_the_daemon`) — this scans the
    /// source for the three call sites and asserts their relative order.
    #[test]
    fn single_instance_lock_precedes_setup_db_and_bind_follows_it() {
        const SRC: &str = include_str!("main.rs");
        // Truncate at THIS test module first — the file scans itself, and the
        // needles below (`daemon_already_running`, `setup_db(&initial_cfg`)
        // both appear again in this doc comment / this very module, so an
        // untruncated scan could match its own source and never fail. Same
        // trap noted in `backend_install.rs`'s self-scanning tests.
        let prod = SRC
            .split_once("\n#[cfg(test)]")
            .map_or(SRC, |(before, _)| before);

        let check_pos = prod
            .find("meridian::platform::daemon_already_running().await")
            .expect("the single-instance guard's check call must exist in main()");
        let lock_pos = prod
            .find("meridian::platform::acquire_single_instance_lock()")
            .expect("the single-instance LOCK acquire must exist in main()");
        let setup_db_pos = prod
            .find("setup_db(&initial_cfg.meridian_db_uri()).await")
            .expect("the setup_db() call must exist in main()");
        let bind_pos = prod
            .find("meridian::platform::spawn_health_listener()?;")
            .expect("the health-listener bind call must exist in main()");

        assert!(
            check_pos < lock_pos,
            "the cheap endpoint probe should run before the lock acquire, so \
             the common case (a daemon that is plainly already up) produces \
             the informative log line. Found check at byte {check_pos}, lock \
             at byte {lock_pos}."
        );
        assert!(
            lock_pos < setup_db_pos,
            "the single-instance lock must be ACQUIRED before setup_db() opens \
             the pool and runs migrations — a daemon that will lose that race \
             must never touch meridian.db. This assertion used to name the \
             PROBE instead, which is why it passed for two releases while the \
             hazard was wide open: the probe is check-then-act, so both racers \
             pass it. Only the lock is an acquire. Found lock at byte \
             {lock_pos}, setup_db at byte {setup_db_pos}."
        );
        assert!(
            setup_db_pos < bind_pos,
            "the health-endpoint listener must be BOUND only after setup_db() \
             succeeds — binding earlier would let a daemon that is about to \
             exit(1) on a locked/corrupt database advertise {{\"running\":true}} \
             to the tray's watchdog. Found setup_db at byte {setup_db_pos}, \
             bind at byte {bind_pos}."
        );
    }

    /// The EXIT-side ordering, which the test above does not cover and which was
    /// wrong until 1.91.0-staging.2's write wedge was traced.
    ///
    /// Three things must happen in this order on shutdown:
    /// 1. await the PM sync watcher — it is the daemon's only writer outside the
    ///    poll loop, and `service_once` deliberately ignores the shutdown flag
    ///    once it has claimed a row (so it can finish and record an outcome), so
    ///    it can still be writing for tens of seconds after the signal;
    /// 2. `checkpoint_wal` — a TRUNCATE checkpoint racing those live writes is
    ///    the exact thing that function exists to prevent, and it is what hands
    ///    the next daemon generation a half-written WAL while the tray keeps a
    ///    stale view of the file;
    /// 3. `release_endpoint` LAST — it is what `daemon_already_running` answers,
    ///    so releasing it before the checkpoint and close lets a relaunching
    ///    daemon pass the single-instance guard and start migrating against a
    ///    file this process is still checkpointing. Under launchd the relaunch
    ///    is immediate, so that window is real, not theoretical.
    ///
    /// Same self-scanning idiom (and the same truncate-at-the-test-module trap)
    /// as the test above.
    #[test]
    fn shutdown_awaits_the_sync_watcher_then_checkpoints_then_releases_the_endpoint() {
        const SRC: &str = include_str!("main.rs");
        let prod = SRC
            .split_once("\n#[cfg(test)]")
            .map_or(SRC, |(before, _)| before);

        let await_pos = prod
            .find("WATCHER_DRAIN_TIMEOUT, sync_watcher)")
            .expect("shutdown must await the PM sync watcher's JoinHandle");
        let checkpoint_pos = prod
            .find("meridian::db::meridian::checkpoint_wal(&meridian).await")
            .expect("shutdown must checkpoint the WAL");
        let release_pos = prod
            .find("meridian::platform::release_endpoint();")
            .expect("shutdown must release the single-instance endpoint");

        assert!(
            await_pos < checkpoint_pos,
            "the PM sync watcher must be awaited BEFORE the WAL checkpoint — \
             checkpointing underneath its live writes is what corrupted the \
             shared WAL index. Found await at byte {await_pos}, checkpoint at \
             byte {checkpoint_pos}."
        );
        assert!(
            checkpoint_pos < release_pos,
            "release_endpoint() must run AFTER the checkpoint and pool close — \
             releasing it earlier opens the single-instance guard while this \
             process is still writing, letting a relaunching daemon migrate \
             against the same file. Found checkpoint at byte {checkpoint_pos}, \
             release at byte {release_pos}."
        );
    }
}
