//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use meridian::config::Config;
use meridian::db::meridian::{cleanup_incomplete_runs, setup_db};
use meridian::db::screenpipe::open_screenpipe;
use meridian::etl::run_etl;
use meridian::intelligence::{
    check_classification_ready, mark_session_subprocess_error, run_coding_agent_classification,
    run_pm_force_sync, run_pm_sync, run_task_linking, TaskLinkOutcome,
};
use meridian::observability;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;

/// After this many consecutive subprocess failures for the same session,
/// write a `subprocess_error` sentinel and advance the cursor past it.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Load the repo-local .env — the single source of config, shared by this
    //    daemon and the Python services. Nothing is read from outside the repo.
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

    // `meridian coding-agent-classify` — one-shot: classify every summarised
    // coding-agent row (the pending_classifier queue) via the MLX server. Manual
    // backfill of the last link in seal→summarise→classify.
    if std::env::args().nth(1).as_deref() == Some("coding-agent-classify") {
        let cfg = Config::from_env();
        match meridian::coding_agent_session_ingest::open_meridian_pool().await {
            Ok(pool) => {
                let mut total = 0usize;
                loop {
                    match run_coding_agent_classification(&pool, &cfg, None).await {
                        Ok(0) => break,
                        Ok(n) => {
                            total += n;
                            println!("classified {n} (total {total})");
                        }
                        Err(e) => {
                            eprintln!("coding-agent-classify: {e}");
                            break;
                        }
                    }
                }
                println!("coding-agent-classify: {total} classified");
                pool.close().await;
            }
            Err(e) => eprintln!("coding-agent-classify: open db: {e}"),
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
        // Keep in sync with services/skills/coding-agent/session-summary/SKILL.md
        // (install.sh and install-from-bundle.sh copy that file directly; this
        // path is the fallback for `meridian doctor --fix` and `meridian coding-agent-install-skill`)
        let content = concat!(
            "---\n",
            "description: Summarise a coding-agent session transcript for a Jira work-log.\n",
            "---\n\n",
            "You summarise ONE work-burst of a developer's coding-agent session for a Jira ",
            "work-log. The transcript is timestamped as `[<ISO ts>] [role] <message>`. Write ",
            "a factual prose summary of 10-40 sentences: name the files edited, commands run, ",
            "errors hit, decisions made, tests/validations performed, and any rework or ",
            "blockers (an approach abandoned, a failed build/test, something deleted and ",
            "rebuilt). State ONLY what is in the transcript — never invent files, tickets, ",
            "commands, or outcomes. No preamble, no markdown headings, no bullet lists — just ",
            "clear paragraphs. If an 'EARLIER IN THIS SESSION' section is present, do not ",
            "repeat it; summarise only this burst.\n\n",
            "Return JSON with `summary` (the prose) and `blockers` (a list of distinct ",
            "blockers / failures / rework, possibly empty).\n"
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
                match meridian::intelligence::oauth::jira::login(&client_id, port).await {
                    Ok(site) => println!(
                        "\n✓ Jira connected: {site}\n  Tokens saved to ~/.meridian/oauth/jira.json — run `meridian restart` to pick them up."
                    ),
                    Err(e) => {
                        eprintln!("oauth-login jira failed: {e:#}");
                        eprintln!(
                            "\nIf your Atlassian org blocks third-party OAuth apps (a \"site admin \
                             must authorize\" message, or app installs disabled), use the API-token \
                             fallback instead: set JIRA_BASE_URL / JIRA_EMAIL / JIRA_API_TOKEN via \
                             `meridian config edit`."
                        );
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
                eprintln!("oauth-login: unknown provider {other:?} (supported: jira, trello)");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // `meridian pm-worklog [--day YYYY-MM-DD]` — one-shot Stage 4: walk the day's
    // hours and DRAFT one worklog per task per ready hour (never posts — posting
    // is approval-gated). Opens via setup_db so migrations (incl. the pm_worklog
    // tables) are applied even when run standalone.
    if std::env::args().nth(1).as_deref() == Some("pm-worklog") {
        let args: Vec<String> = std::env::args().collect();
        let day = args
            .iter()
            .position(|a| a == "--day")
            .and_then(|i| args.get(i + 1).cloned());
        let cfg = Config::from_env();
        // Initialise observability so this one-shot emits the SAME worklog_draft
        // trace the daemon does (lineage child spans + the active span whose
        // traceparent propagates to the MLX synth, nesting worklog_input/output
        // inside the worklog trace). Without this the CLI emitted no spans and the
        // synth landed as an orphan root. Flushed explicitly before exit so the
        // batch processor's final spans reach the telemetry spool.
        let obs_guard = observability::init("meridian-rust").ok();
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                meridian::pm_worklog::cli_run(&pool, day.as_deref()).await;
                pool.close().await;
            }
            Err(e) => eprintln!("pm-worklog: open db: {e}"),
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

    // `meridian telemetry <status|export|import>` — telemetry spool management.
    // Read-only for status/export; import POSTs to OO. No daemon init needed.
    if std::env::args().nth(1).as_deref() == Some("telemetry") {
        let args: Vec<String> = std::env::args().collect();
        meridian::telemetry_spool::cli::run(&args).await;
        return Ok(());
    }

    // 2. Tracing — layered subscriber (stdout + JSONL file + OTLP to OpenObserve).
    //    Guard must outlive the program; we shut it down explicitly at the end
    //    so OTel's blocking flush doesn't run inside tokio's drop path.
    let obs_guard = observability::init("meridian-rust")?;

    // 3. Load initial config — DB paths and startup parameters come from here.
    //    DB pool paths and observability are fixed at startup and do not change.
    let initial_cfg = Config::from_env();
    tracing::info!(stage = "config_loaded", "configuration ready");

    // 4. Log startup parameters
    tracing::info!(
        screenpipe_db = %initial_cfg.screenpipe_db,
        meridian_db   = %initial_cfg.meridian_db,
        poll_interval_secs = initial_cfg.poll_interval_secs,
        "meridian daemon starting"
    );

    // 4b. Open / create meridian pool and run migrations FIRST — before any
    //     preflight that can block or fail. The UI and MCP server read this DB
    //     directly, so it must exist even when an optional component (MLX
    //     server, screenpipe) is down; ordering it after the MLX preflight left
    //     machines with a broken MLX install running a daemon that never
    //     created its own database.
    let meridian = setup_db(&initial_cfg.meridian_db_uri()).await?;

    // 4c. Preflight: verify the classification stack. NON-FATAL — classification
    //     is an enhancement layer; ETL must keep recording sessions while the
    //     MLX server is down. Unclassified sessions stay pending and the task
    //     linker's 5-minute fallback drains them once the server is reachable.
    //     (Exiting here put launchd in a 120s-wait → exit → respawn loop on any
    //     machine where the MLX server could not start.)
    if let Err(e) = check_classification_ready(&initial_cfg) {
        tracing::error!(
            error = %e,
            "classification preflight failed — continuing with classification degraded"
        );
        eprintln!(
            "\nWARNING: {e}\n\
             Sessions will be recorded but not classified until the MLX server is reachable.\n"
        );
    }

    // 4d. Open screenpipe pool (read-only)
    let screenpipe = open_screenpipe(&initial_cfg.screenpipe_db_uri()).await?;

    // 4e. Capture-layer (L1) preflight: surface degraded screen capture (revoked
    //     Screen Recording / Accessibility permission, dead screenpipe, stale
    //     frames) before the poll loop. Non-fatal — the daemon still runs; we log
    //     the fault so misclassifications aren't blamed on the model.
    meridian::health::Report::new(
        meridian::health::capture::checks(&initial_cfg, Some(&screenpipe)).await,
    )
    .log("startup");

    // 5b. Unix domain socket — health endpoint for the tray / UI.
    //     ~/.meridian/daemon.sock: connecting succeeds = daemon is running.
    //     Stale socket from a previous crash is removed before binding.
    let sock_path = {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
        std::path::PathBuf::from(format!("{}/.meridian/daemon.sock", home))
    };
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

    // 7b. A background task drains the classification queue without blocking the
    //     poll loop (each session can take ~16 s). The poll loop notifies it after
    //     every ETL pass; it calls the persistent MLX classifier server.
    let etl_notify: Arc<Notify> = Arc::new(Notify::new());
    // Shared slot: main task clones the current tick span here so the linker task
    // can parent its run_task_linking spans under poll_tick / startup_tick.
    let etl_tick_span: Arc<std::sync::Mutex<Option<tracing::Span>>> =
        Arc::new(std::sync::Mutex::new(None));

    // Wakes the PM-worklog driver the moment the classifier settles a session: an
    // hour becomes draftable exactly when its last in-flight session is classified,
    // so the driver reacts in seconds instead of waiting up to a full interval. The
    // driver's interval timer is kept as a fallback (aging escape + day rollover).
    let worklog_notify: Arc<Notify> = Arc::new(Notify::new());

    // 7c. Run ETL once immediately before entering the loop.
    //     Re-read config so that any settings.json present at startup takes effect.
    {
        let cfg = Config::from_env();
        let startup_tick = tracing::info_span!("startup_tick");
        *etl_tick_span.lock().unwrap() = Some(startup_tick.clone());
        let _guard = startup_tick.enter();
        tracing::info!("running initial ETL pass");
        if let Err(e) = run_etl(&screenpipe, &meridian).await {
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

    // 8a. MLX only: spawn the task linker loop.
    //     Wakes immediately when ETL signals new sessions; drains oldest-first
    //     (preserving the 5-session context window) until caught up, then waits
    //     for the next ETL notification. A 5-min fallback ensures recovery if a
    //     notify was missed (e.g. daemon restart with existing backlog).
    //
    //     Failure handling:
    //       - Transient failure  → cursor stays, retry on next notify
    //       - Permanent failure  → sentinel written after MAX_CONSECUTIVE_FAILURES,
    //                              cursor advances, drain continues
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

    // 7d. PM-worklog driver (Stage 4): the hour-driven loop that DRAFTS one Jira
    //     worklog per task per settled hour. Never posts — drafted worklogs wait
    //     for a human to approve them in the dashboard. Independent of the ETL tick.
    {
        let pool_pm = meridian.clone();
        let rx_pm = shutdown_rx.clone();
        let notify_pm = worklog_notify.clone();
        tokio::spawn(async move {
            meridian::pm_worklog::run_loop(pool_pm, rx_pm, notify_pm).await;
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

    {
        let mut shutdown_rx = shutdown_rx;
        let meridian_linker = meridian.clone();
        let notify_linker = etl_notify.clone();
        let tick_span_linker = etl_tick_span.clone();
        let notify_worklog = worklog_notify.clone();
        tokio::spawn(async move {
            // Tracks consecutive subprocess failures per session_id.
            // Reset to zero whenever any session is successfully classified.
            // Persists across drain cycles within this daemon run (lost on restart,
            // which is fine — transient failures before restart won't be double-counted).
            let mut failure_counts: HashMap<i64, u32> = HashMap::new();

            loop {
                // Take the tick span written by the main task so run_task_linking spans
                // appear as children of the triggering poll_tick / startup_tick.
                let parent_span: tracing::Span = tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = notify_linker.notified() => {
                        tick_span_linker.lock().unwrap().take()
                            .unwrap_or_else(tracing::Span::none)
                    }
                    _ = tokio::time::sleep(Duration::from_secs(300)) => tracing::Span::none(),
                };

                // Each classification below runs as its OWN root trace (not a child
                // of this tick), so a backlog drain produces one self-contained trace
                // per session instead of N siblings collapsed into one tick trace.
                // We still want the daemon→session relationship, so capture the tick's
                // span context here and pass it down as a span LINK. `Span::none()`
                // (the 5-min fallback wake) yields no context → no link, which is fine.
                let tick_link = parent_span
                    .in_scope(meridian::observability::current_traceparent)
                    .as_deref()
                    .and_then(meridian::observability::span_context_from_traceparent);

                // Whether this wake settled at least one session — if so, wake the
                // PM-worklog driver afterwards so a now-complete hour drafts at once.
                let mut classified_any = false;

                // Drain: classify oldest-first until nothing is left or a failure stops us.
                loop {
                    let cfg = Config::from_env();
                    // No `.instrument(parent_span)` here on purpose: run_task_linking's
                    // own #[tracing::instrument] span is created with no ambient parent,
                    // so it starts a fresh root trace. `tick_link` (the tick's span
                    // context) is passed as a LINK instead — daemon→session stays
                    // navigable without merging every drained session into one trace.
                    match run_task_linking(&meridian_linker, &cfg, tick_link.clone())
                        .await
                    {
                        Ok(TaskLinkOutcome::Classified) => {
                            failure_counts.clear();
                            classified_any = true;
                            let _ = meridian::notices::clear(&meridian_linker, "mlx.down").await;
                            // Loop immediately — more sessions may be waiting.
                        }
                        Ok(TaskLinkOutcome::NoPendingWork) => {
                            // Cursor work is caught up — now drain the coding-agent
                            // classify queue (summarised rows → task linking), the
                            // last link of seal→summarise→classify. Repeat until empty.
                            loop {
                                match run_coding_agent_classification(
                                    &meridian_linker,
                                    &cfg,
                                    tick_link.clone(),
                                )
                                .await
                                {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        classified_any = true;
                                        tracing::info!(
                                            classified = n,
                                            "coding-agent rows classified"
                                        )
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "coding-agent classification failed");
                                        break;
                                    }
                                }
                            }
                            break; // Caught up — go back to waiting for next notify.
                        }
                        Ok(TaskLinkOutcome::SubprocessFailed {
                            session_id,
                            pending,
                        }) => {
                            let count = failure_counts.entry(session_id).or_insert(0);
                            *count += 1;

                            if *count >= MAX_CONSECUTIVE_FAILURES {
                                tracing::warn!(
                                    session_id,
                                    failures = *count,
                                    pending,
                                    "max consecutive failures — writing subprocess_error sentinel \
                                 and advancing cursor"
                                );
                                let _ = meridian::notices::raise(
                                    &meridian_linker,
                                    "mlx.down",
                                    "warning",
                                    "MLX classifier is not responding",
                                    &format!("Failed to classify session {session_id} after {count} attempts — classification is paused"),
                                    Some("Start MLX server: cd services && .venv313/bin/meridian-server --backend mlx"),
                                ).await;
                                if let Err(e) =
                                    mark_session_subprocess_error(&meridian_linker, session_id)
                                        .await
                                {
                                    tracing::error!(
                                        session_id,
                                        error = %e,
                                        "failed to write error sentinel — will retry next tick"
                                    );
                                    break;
                                }
                                failure_counts.remove(&session_id);
                                // Loop again — cursor advanced, try the next session.
                            } else {
                                tracing::warn!(
                                    session_id,
                                    failures = *count,
                                    max = MAX_CONSECUTIVE_FAILURES,
                                    pending,
                                    "subprocess failed — cursor held, will retry on next ETL tick"
                                );
                                break; // Stop drain, wait for next notify / 5-min fallback.
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "classification run error");
                            break;
                        }
                    }
                }

                // This wake settled at least one session — nudge the PM-worklog
                // driver so an hour that just became complete drafts immediately
                // instead of waiting for the next interval tick.
                if classified_any {
                    notify_worklog.notify_one();
                }
            }
            tracing::info!("task linker loop stopped");
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
                if let Err(e) = run_etl(&screenpipe, &meridian).await {
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

                // Proactive classifier health probe. Detect a down/wedged MLX
                // server every tick via a fast /health check (NOT reactively, only
                // when a classify happens to fail) so the fault surfaces promptly
                // on the dashboard banner AND — via the notices→outbox bridge — as
                // a desktop toast + in-app banner. Auto-clears when it recovers.
                if meridian::intelligence::mlx_ready(&cfg).await {
                    let _ = meridian::notices::clear(&meridian, "mlx.down").await;
                } else {
                    let _ = meridian::notices::raise(
                        &meridian,
                        "mlx.down",
                        "warning",
                        "Classifier offline",
                        "The MLX classifier server isn't responding — new sessions are recorded but won't be tagged until it's back.",
                        Some("Restart it: cd services && .venv313/bin/meridian-server --backend mlx"),
                    )
                    .await;
                }
                // pm_tasks is refreshed on demand at its read boundaries
                // (classification in run_task_linking, drafting in the worklog
                // driver), so no timer-driven refresh is needed here.
            }
        }
    }

    // Signal the task linker loops to stop. The shipper has its OWN channel and
    // is intentionally left running for now (see below).
    let _ = shutdown_tx.send(true);

    // 9. Shutdown
    tracing::info!("shutting down");
    let _ = std::fs::remove_file(&sock_path_cleanup);
    screenpipe.close().await;
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
