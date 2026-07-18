//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::etl::run_etl;
use meridian::intelligence::{run_pm_force_sync, run_pm_sync};
use meridian::observability;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;

/// Single-instance probe for the daemon socket. Returns `true` only when a live
/// daemon answers `~/.meridian/daemon.sock` with its greeting — i.e. one is already
/// running for this data dir. A missing/stale socket with no listener (previous
/// crash) connects-refused or times out → `false`, so this instance may take over.
/// Mirrors the tray's `probe_socket`; kept deliberately short (800 ms) so startup
/// isn't stalled when the socket is genuinely stale.
async fn daemon_already_running(sock_path: &std::path::Path) -> bool {
    use tokio::io::AsyncReadExt as _;
    use tokio::time::timeout;

    let connect = timeout(
        Duration::from_millis(800),
        tokio::net::UnixStream::connect(sock_path),
    )
    .await;
    let Ok(Ok(mut stream)) = connect else {
        return false; // no listener (absent or stale socket) — safe to take over
    };
    // A live daemon writes `{"running":true,"pid":…}` on connect. A non-empty read
    // confirms a real daemon is there, not just a leftover socket file.
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_millis(800), stream.read_to_end(&mut buf)).await;
    !buf.is_empty()
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Load the repo-local .env — the single source of config for the daemon.
    //    Nothing is read from outside the repo.
    //    The launchd plist sets WorkingDirectory to the repo root, so
    //    dotenv_override reads <repo>/.env and its values beat any empty
    //    defaults injected by the plist. (CLI subcommands invoked from elsewhere
    //    fall back to built-in defaults, e.g. MERIDIAN_DB → ~/.meridian/meridian.db.)
    let _ = dotenvy::dotenv_override();

    // 1b. Subcommand dispatch. `meridian coding-agent-hook` is the Claude Code
    //     SessionEnd hook entry point: one-shot, reads a JSON payload on stdin,
    //     seals that session, exits 0. It must stay light (no daemon init, no
    //     OTLP) and must never block Claude, so it always exits 0.
    if std::env::args().nth(1).as_deref() == Some("coding-agent-hook") {
        meridian::coding_agent_session_ingest::hook::run_hook().await;
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
            Err(e) => eprintln!("coding-agent-summarise: open db: {e}"),
        }
        return Ok(());
    }

    // `meridian coding-agent-install-skill` — write the session-summary Claude
    // Code command file so `claude -p /session-summary` works. Idempotent; safe
    // to run any number of times. Also called by `meridian doctor --fix`.
    if std::env::args().nth(1).as_deref() == Some("coding-agent-install-skill") {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let commands_dir = home.join(".claude/commands");
        let skill_path = commands_dir.join("session-summary.md");
        // Keep in sync with assets/skills/coding-agent/session-summary/SKILL.md.
        // install.sh runs this command; it is also the fallback for
        // `meridian doctor --fix` and direct `meridian coding-agent-install-skill`.
        let content = concat!(
            "---\n",
            "description: Summarise a coding-agent session transcript for a Jira work-log.\n",
            "---\n\n",
            "You summarise ONE work-burst of a developer's coding-agent session for a Jira ",
            "work-log. The transcript is timestamped as `[<ISO ts>] [role] <message>`. Write ",
            "a factual prose summary of 10-40 sentences: name the files edited, commands run, ",
            "errors hit, decisions made, tests/validations performed, and any rework. ",
            "State ONLY what is in the transcript — never invent files, tickets, ",
            "commands, or outcomes. No preamble, no markdown headings, no bullet lists — just ",
            "clear paragraphs. If an 'EARLIER IN THIS SESSION' section is present, do not ",
            "repeat it; summarise only this burst.\n\n",
            "Return JSON with `summary` (the prose).\n"
        );
        if let Err(e) = std::fs::create_dir_all(&commands_dir) {
            eprintln!("coding-agent-install-skill: create dir: {e}");
            return Ok(());
        }
        if skill_path.exists() {
            println!(
                "coding-agent-install-skill: already present at {}",
                skill_path.display()
            );
        } else {
            match std::fs::write(&skill_path, content) {
                Ok(()) => println!("coding-agent-install-skill: wrote {}", skill_path.display()),
                Err(e) => eprintln!("coding-agent-install-skill: write: {e}"),
            }
        }
        return Ok(());
    }

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
    // narrative, a few insights, and the Vega-Lite specs it chose. Prints ONE JSON
    // line: the summary object. Regenerate = the same command (UPSERT overwrites).
    // Never fails on a bad answer — it falls back to a deterministic panel set, so
    // a non-zero exit here means the DB or the day's data, not the model.
    if std::env::args().nth(1).as_deref() == Some("day-summary") {
        let args: Vec<String> = std::env::args().collect();
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1).cloned())
        };
        let day = flag("--day").unwrap_or_default();
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

    // `meridian day-summary-data --day YYYY-MM-DD` — the named datasets a
    // summary's panels bind to, as one JSON object. Read-only, no LLM. The tray
    // reads these straight from meridian-core rather than spawning this; it exists
    // for debugging a chart (`meridian day-summary-data --day X | jq .segments`),
    // which is otherwise invisible — a stored spec carries no data at all.
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

    // 4b. Open / create meridian pool and run migrations FIRST — before any
    //     preflight that can block or fail. The UI and MCP server read this DB
    //     directly, so it must exist even when an optional component (capture,
    //     an agent CLI) is degraded; ordering it after a preflight that could
    //     block once left machines running a daemon that never created its own
    //     database.
    let meridian = setup_db(&initial_cfg.meridian_db_uri()).await?;

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
    let sock_path = {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
        std::path::PathBuf::from(format!("{}/.meridian/daemon.sock", home))
    };
    if daemon_already_running(&sock_path).await {
        tracing::warn!(
            path = %sock_path.display(),
            "another meridian daemon already owns this data dir — exiting (single-instance guard)"
        );
        return Ok(());
    }
    let _ = std::fs::remove_file(&sock_path);
    let sock_path_cleanup = sock_path.clone();
    {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::UnixListener::bind(&sock_path)?;
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut stream, _)) => {
                        let pid = std::process::id();
                        tokio::spawn(async move {
                            let msg = format!("{{\"running\":true,\"pid\":{}}}\n", pid);
                            let _ = stream.write_all(msg.as_bytes()).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "daemon.sock accept error");
                        break;
                    }
                }
            }
        });
    }
    tracing::info!(path = %sock_path.display(), "daemon.sock ready");

    // 6. Graceful shutdown: listen for SIGINT, SIGTERM, and SIGHUP.
    //    SIGHUP = "reload config" — same clean shutdown path as SIGTERM so that
    //    launchd auto-restarts the daemon with the new settings.json applied.
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sighup = signal(SignalKind::hangup())?;

    // Combines SIGINT / SIGTERM / SIGHUP into a single future.
    async fn wait_for_shutdown(
        sigint: &mut tokio::signal::unix::Signal,
        sigterm: &mut tokio::signal::unix::Signal,
        sighup: &mut tokio::signal::unix::Signal,
    ) {
        tokio::select! {
            _ = sigint.recv()  => { tracing::info!("SIGINT received") },
            _ = sigterm.recv() => { tracing::info!("SIGTERM received") },
            _ = sighup.recv()  => { tracing::info!("SIGHUP received — reloading (graceful restart)") },
        }
    }

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

    // 7b. Shared handles the poll loop uses to signal ETL ticks to observers.
    let etl_notify: Arc<Notify> = Arc::new(Notify::new());
    let etl_tick_span: Arc<std::sync::Mutex<Option<tracing::Span>>> =
        Arc::new(std::sync::Mutex::new(None));

    // 7c. Run ETL once immediately before entering the loop.
    //     Re-read config so that any settings.json present at startup takes effect.
    {
        let cfg = Config::from_env();
        let startup_tick = tracing::info_span!("startup_tick");
        *etl_tick_span.lock().unwrap() = Some(startup_tick.clone());
        let _guard = startup_tick.enter();
        tracing::info!("running initial ETL pass");
        if let Err(e) = run_etl(&meridian).await {
            tracing::error!(error = %e, "ETL run failed");
            let _ = meridian::notices::raise(
                &meridian,
                "etl.failed",
                "error",
                "Activity capture pipeline failed",
                &e.to_string(),
                Some("Open /logs in the dashboard to see details"),
            )
            .await;
        } else {
            let _ = meridian::notices::clear(&meridian, "etl.failed").await;
        }
        etl_notify.notify_one();
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
            _ = wait_for_shutdown(&mut sigint, &mut sigterm, &mut sighup) => {
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
                *etl_tick_span.lock().unwrap() = Some(poll_tick.clone());
                let _guard = poll_tick.enter();
                tracing::debug!("starting ETL tick");
                if let Err(e) = run_etl(&meridian).await {
                    tracing::error!(error = %e, "ETL run failed");
                    let _ = meridian::notices::raise(
                        &meridian, "etl.failed", "error",
                        "Activity capture pipeline failed",
                        &e.to_string(),
                        Some("Open /logs in the dashboard to see details"),
                    ).await;
                } else {
                    let _ = meridian::notices::clear(&meridian, "etl.failed").await;
                }
                // Wake the background task linker to drain newly-created sessions.
                etl_notify.notify_one();

                // Morning plan nudge — idempotent per day, gated to working hours.
                if let Err(e) = meridian::daily_plan::maybe_nudge(&meridian).await {
                    tracing::debug!(error = %e, "plan nudge check skipped");
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
    let _ = std::fs::remove_file(&sock_path_cleanup);
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
