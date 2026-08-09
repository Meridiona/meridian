//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::etl::run_etl;
use meridian::intelligence::{run_pm_force_sync, run_pm_sync};
use meridian::observability;
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

    // `meridian tasks-sync` — force an immediate sync of all configured PM
    // providers (Jira, Linear, GitHub), bypassing the 5-minute staleness gate.
    // Exits 0 on success, non-zero if the DB cannot be opened.
    if std::env::args().nth(1).as_deref() == Some("tasks-sync") {
        let cfg = Config::from_env();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                if let Err(e) = run_pm_force_sync(&pool, &cfg).await {
                    eprintln!("tasks-sync: {e}");
                }
                pool.close().await;
            }
            Err(e) => {
                // `{e:#}` prints the full anyhow source chain (e.g. the sqlx
                // "migration N was previously applied / missing" cause) instead
                // of just the top-level "failed to run migrations" context.
                eprintln!("tasks-sync: open db: {e:#}");
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
                        let _ = run_pm_force_sync(&pool, &cfg).await;
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
                        let _ = run_pm_force_sync(&pool, &cfg).await;
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

    // 4. Log startup parameters
    tracing::info!(
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
                error = %e,
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

    // 5b. Unix domain socket — health endpoint for the tray / UI, AND the
    //     single-instance guard. ~/.meridian/daemon.sock: a successful connect that
    //     gets a greeting means ANOTHER daemon already owns this data dir. That
    //     happens routinely — a leftover packaged install's launchd agent
    //     (KeepAlive=true) respawning next to a dev build, two `meridian` invocations
    //     racing — and two daemons on one meridian.db double every ETL pass and fire
    //     the worklog trigger twice (near-duplicate day_tasks, clobbering folds). So
    //     if one is already answering, exit cleanly here rather than delete its socket
    //     and become a second writer. Only a stale socket (no listener) is removed
    //     before we bind our own. Whoever starts second bows out; dev-start.sh stops
    //     the installed daemon first so the dev build wins, and any KeepAlive respawn
    //     self-terminates on the next line.
    //     The endpoint itself is OS-specific (a socket file on Unix, a named
    //     pipe on Windows) — see `meridian::platform`.
    if meridian::platform::daemon_already_running().await {
        tracing::warn!(
            endpoint = %meridian::platform::endpoint_display(),
            "another meridian daemon already owns this data dir — exiting (single-instance guard)"
        );
        return Ok(());
    }
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
        Err(e) => tracing::error!("cleanup_incomplete_runs failed: {}", e),
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
        Err(e) => tracing::error!("reset_stuck_generating_hours failed: {}", e),
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
                                deep_link: Some("/logs"),
                            },
                        )
                        .await;
                        db_corrupt.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    // The check itself failing is not evidence either way — carry on and
                    // let the ETL's own error path classify it.
                    Err(e) => {
                        tracing::warn!(error = %e, "startup integrity check could not run");
                    }
                }
            }
            .instrument(tracing::info_span!("startup_db_integrity_check")),
        );
    }

    // 7c. Run ETL once immediately before entering the loop.
    //     Re-read config so that any settings.json present at startup takes effect.
    {
        let cfg = Config::from_env();
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
                tracing::warn!(error = %e, "capture retention sweep failed");
            }
        }
        if let Err(e) = run_pm_sync(&meridian, &cfg).await {
            tracing::error!("intelligence run failed: {}", e);
        }
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
                tracing::warn!(error = %e, "embedder: weight provisioning failed — distiller stays on lexical-only until this succeeds");
            }
        });
    }

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
                            tracing::warn!(error = %e, "capture retention sweep failed");
                        }
                    }
                }

                // Morning plan nudge — idempotent per day, gated to working hours.
                if let Err(e) = meridian::daily_plan::maybe_nudge(&meridian).await {
                    tracing::debug!(error = %e, "plan nudge check skipped");
                }

                // Coding-agent summariser dead-letter digest — idempotent per day.
                if let Err(e) =
                    meridian::coding_agent_session_ingest::summariser::maybe_notify_dead_letters(
                        &meridian,
                    )
                    .await
                {
                    tracing::debug!(error = %e, "summariser dead-letter digest check skipped");
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

                // Interactive-notification responses — act on the user's answers
                // (snooze re-enqueues an hour out). Idempotent end-to-end, so the
                // same cadence as the nudge above is safe.
                if let Err(e) =
                    meridian::notification_responses::consume_responses(&meridian).await
                {
                    tracing::debug!(error = %e, "notification response consume skipped");
                }

                // Refresh the PM task cache (pm_tasks) every tick — interval-gated
                // per provider (~5 min), so this is a cheap no-op most ticks. The
                // legacy drafting driver that used to trigger this before every
                // pass was retired when the worklog pipeline moved to the
                // clock-aligned Python trigger, which never calls this itself —
                // leaving pm_tasks (and hence a ticket's title on the timeline)
                // stuck at whatever it was at the last daemon restart. This is
                // the only thing that keeps it live during normal operation.
                if let Err(e) = run_pm_sync(&meridian, &cfg).await {
                    tracing::warn!(error = %e, "pm_tasks refresh failed — using cached tasks");
                }
            }
        }
    }

    // Signal the task linker loops to stop. The shipper has its OWN channel and
    // is intentionally left running for now (see below).
    let _ = shutdown_tx.send(true);

    // 9. Shutdown
    tracing::info!("shutting down");
    meridian::platform::release_endpoint();
    meridian.close().await;

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
                error = %e,
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
                    deep_link: Some("/logs"),
                },
            )
            .await;
            true
        }
        Err(e) => {
            tracing::error!(error = %e, "ETL run failed");
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
