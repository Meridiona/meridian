// meridian — normalises screenpipe activity into structured app sessions
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
    run_pm_sync, run_task_linking, TaskLinkOutcome,
};
use meridian::observability;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;
use tracing::Instrument as _;

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
                    match run_coding_agent_classification(&pool, &cfg).await {
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
    // browser OAuth (Authorization Code + PKCE) for a PM provider. Opens the
    // system browser, captures the loopback redirect, and persists tokens to
    // ~/.meridian/oauth/<provider>.json. Currently supports: jira.
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
            other => {
                eprintln!("oauth-login: unknown provider {other:?} (supported: jira)");
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
        match setup_db(&cfg.meridian_db_uri()).await {
            Ok(pool) => {
                meridian::pm_worklog::cli_run(&pool, day.as_deref()).await;
                pool.close().await;
            }
            Err(e) => eprintln!("pm-worklog: open db: {e}"),
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

    // 4b. Preflight: verify classification stack is ready before starting the daemon.
    //     Fails fast with a clear message rather than silently erroring every tick.
    if let Err(e) = check_classification_ready(&initial_cfg) {
        tracing::error!("{}", e);
        eprintln!("\nERROR: {}\n", e);
        std::process::exit(1);
    }

    // 4. Open screenpipe pool (read-only)
    let screenpipe = open_screenpipe(&initial_cfg.screenpipe_db_uri()).await?;

    // 4c. Capture-layer (L1) preflight: surface degraded screen capture (revoked
    //     Screen Recording / Accessibility permission, dead screenpipe, stale
    //     frames) before the poll loop. Non-fatal — the daemon still runs; we log
    //     the fault so misclassifications aren't blamed on the model.
    meridian::health::Report::new(
        meridian::health::capture::checks(&initial_cfg, Some(&screenpipe)).await,
    )
    .log("startup");

    // 5. Open / create meridian pool and run migrations
    let meridian = setup_db(&initial_cfg.meridian_db_uri()).await?;

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

    // 6. Graceful shutdown: listen for SIGINT and SIGTERM
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    // Combines both signals into a single future that resolves on whichever fires first.
    async fn wait_for_shutdown(
        sigint: &mut tokio::signal::unix::Signal,
        sigterm: &mut tokio::signal::unix::Signal,
    ) {
        tokio::select! {
            _ = sigint.recv()  => {},
            _ = sigterm.recv() => {},
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
            tracing::error!("ETL run failed: {}", e);
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

                // Whether this wake settled at least one session — if so, wake the
                // PM-worklog driver afterwards so a now-complete hour drafts at once.
                let mut classified_any = false;

                // Drain: classify oldest-first until nothing is left or a failure stops us.
                loop {
                    let cfg = Config::from_env();
                    // .instrument() enters parent_span on each poll; #[tracing::instrument]
                    // on run_task_linking creates its span inside the async block at first
                    // poll (tracing-attributes >= 0.1.24), so it sees parent_span as current
                    // and becomes its child.
                    match run_task_linking(&meridian_linker, &cfg)
                        .instrument(parent_span.clone())
                        .await
                    {
                        Ok(TaskLinkOutcome::Classified) => {
                            failure_counts.clear();
                            classified_any = true;
                            // Loop immediately — more sessions may be waiting.
                        }
                        Ok(TaskLinkOutcome::NoPendingWork) => {
                            // Cursor work is caught up — now drain the coding-agent
                            // classify queue (summarised rows → task linking), the
                            // last link of seal→summarise→classify. Repeat until empty.
                            loop {
                                match run_coding_agent_classification(&meridian_linker, &cfg)
                                    .instrument(parent_span.clone())
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
    loop {
        // Determine the sleep duration from the current settings.json before sleeping.
        let poll_interval = {
            let cfg = Config::from_env();
            Duration::from_secs(cfg.runtime.poll_interval_secs)
        };

        tokio::select! {
            _ = wait_for_shutdown(&mut sigint, &mut sigterm) => {
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Re-read config to pick up any settings.json changes made while sleeping.
                let cfg = Config::from_env();
                let poll_tick = tracing::info_span!(
                    "poll_tick",
                    poll_interval_secs = cfg.runtime.poll_interval_secs
                );
                *etl_tick_span.lock().unwrap() = Some(poll_tick.clone());
                let _guard = poll_tick.enter();
                tracing::debug!("starting ETL tick");
                if let Err(e) = run_etl(&screenpipe, &meridian).await {
                    tracing::error!("ETL run failed: {}", e);
                }
                // Wake the background task linker to drain newly-created sessions.
                etl_notify.notify_one();
                // pm_tasks is refreshed on demand at its read boundaries
                // (classification in run_task_linking, drafting in the worklog
                // driver), so no timer-driven refresh is needed here.
            }
        }
    }

    // Signal the task linker loop to stop.
    let _ = shutdown_tx.send(true);

    // 9. Shutdown
    tracing::info!("shutting down");
    let _ = std::fs::remove_file(&sock_path_cleanup);
    screenpipe.close().await;
    meridian.close().await;

    // Flush OTel exporters while the tokio runtime is still alive.
    obs_guard.shutdown().await;

    Ok(())
}
