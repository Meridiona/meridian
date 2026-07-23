//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Is the provider's CLI actually installed?
//!
//! # Why this is not just `which`
//!
//! A GUI app launched from Finder (the tray, and therefore the setup wizard) inherits a
//! stripped `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin` — not the one from the user's shell
//! profile. Every one of these CLIs installs somewhere else: `~/.local/bin`,
//! `/opt/homebrew/bin`, the npm global prefix. A bare `which claude` therefore reports
//! "not installed" on a machine where Claude Code works perfectly.
//!
//! This is not hypothetical. The summariser already had a documented outage from exactly
//! this: every row silently fell back to the local model because the daemon's environment
//! had no `PATH` to find `claude` on. Telling a user "Claude is not installed" while they
//! are staring at a terminal with `claude` in it is the worst possible first impression,
//! so we probe through a **login shell**, which sources their profile, and fall back to
//! scanning the usual install locations.
//!
//! # Authentication is not probed by [`detect`]/[`detect_all`]
//!
//! There is no cheap non-interactive AUTH-ONLY check for these CLIs (`cursor-agent login`
//! was observed to hang forever when already signed in), so the fast install probe never
//! claims *usable*, only *installed*.
//!
//! A real connectivity check is still possible, though: every backend already knows how to
//! run one real, trivial completion (`-p "reply with OK"`) non-interactively — that's
//! exactly what [`test_provider`] does, deliberately kept SEPARATE from the free, always-on
//! install probe because it spends one real request against the user's subscription. It
//! only runs when explicitly requested (a card's Test button, or a Rescan that re-tests
//! every already-installed provider) — never silently on mount.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use meridian_core::proc_ext::NoWindow;
use meridian_core::settings::RuntimeSettings;
use meridian_core::LlmProvider;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{resolver::backend_for, LlmConfig, LlmError, PromptRequest};

/// How long a probe may take before we call it absent. A login shell sources the user's
/// profile, which can be slow (nvm, rbenv, …), but not this slow.
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

/// Where these CLIs actually land, for when the login shell is unavailable or too slow.
///
/// Unix-only paths — Windows has no `probe_login_shell` to fall back FROM (see
/// [`resolve_cli`]), so it never reaches this tier for the common case anyway. The one
/// Windows location worth listing is npm's global bin, `%APPDATA%\npm`, for the rare case
/// where a fresh shell's `PATH` hasn't picked it up yet even though `PATH` is otherwise
/// trustworthy on this platform.
fn candidate_dirs() -> Vec<PathBuf> {
    let home = meridian_core::paths::home_dir_or_cwd();
    #[cfg(windows)]
    let platform: Vec<String> = std::env::var("APPDATA")
        .map(|a| vec![PathBuf::from(a).join("npm").to_string_lossy().into_owned()])
        .unwrap_or_default();
    #[cfg(not(windows))]
    let platform: Vec<String> = vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];

    [
        home.join(".local/bin").to_string_lossy().into_owned(),
        home.join(".npm-global/bin").to_string_lossy().into_owned(),
        home.join(".bun/bin").to_string_lossy().into_owned(),
        home.join(".volta/bin").to_string_lossy().into_owned(),
    ]
    .into_iter()
    .chain(platform)
    .map(PathBuf::from)
    .collect()
}

/// Whether one provider's CLI can be found, and where.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    /// The wire form — matches `ui/lib/llm-providers.ts`'s ids.
    pub id: String,
    pub installed: bool,
    /// Resolved absolute path, when we found one.
    pub path: Option<String>,
    /// Whether the user is signed in. Always `None` — see the module docs.
    pub authenticated: Option<bool>,
    /// The last real connectivity test on record for this provider, if any — read from
    /// the on-disk cache, never freshly run by [`detect`]/[`detect_all`] themselves. `None`
    /// means "never tested", not "failed".
    pub last_test: Option<ProviderTestResult>,
}

/// Probe one provider. A provider with no CLI binary (only `Custom`, a cloud endpoint) has
/// nothing to probe on disk — `detect_all` never enumerates it (its cards come from the
/// registry), so this early-return is not reached in practice.
pub async fn detect(provider: LlmProvider) -> ProviderStatus {
    let id = provider.as_str().to_string();
    let Some(bin) = provider.cli_name() else {
        return ProviderStatus {
            id,
            installed: true,
            path: None,
            authenticated: None,
            last_test: None,
        };
    };

    let found = resolve_cli(bin).await;
    ProviderStatus {
        id,
        installed: found.is_some(),
        path: found.map(|p| p.display().to_string()),
        authenticated: None,
        last_test: None,
    }
}

/// [`detect_all`], with each provider's last on-disk connectivity test (if any) merged in.
/// This is what the UI actually wants — install state plus the last time we know for sure
/// whether it worked — without paying for a fresh test on every mount/rescan.
pub async fn detect_all_with_cache() -> Vec<ProviderStatus> {
    let cache = load_test_cache();
    let mut all = detect_all().await;
    for s in &mut all {
        s.last_test = cache.get(&s.id).cloned();
    }
    all
}

/// Probe every built-in provider at once. The shell probes are I/O-bound and independent.
///
/// `builtins()`, not `all()`: a custom endpoint is a registry row, not a variant, so it has
/// no install state to probe — and [`detect`] reads "no CLI name" as "the on-device model,
/// always installed", which for `Custom` would report an endpoint the user has not even
/// configured as ready. Custom cards are built from the registry (each with its measured
/// rung) rather than probed here.
pub async fn detect_all() -> Vec<ProviderStatus> {
    let futures = LlmProvider::builtins().map(detect);
    futures::future::join_all(futures).await
}

// ── Real connectivity test ───────────────────────────────────────────────────────────

/// A test call is capped far tighter than a real hourly summary (which can legitimately
/// take minutes on a big input) — this is one word, so a slow answer already means trouble
/// and the user is watching a spinner.
const PROBE_TIMEOUT_S: u64 = 20;

const PROBE_SYSTEM: &str = "Reply with exactly: OK. No other text, no punctuation, nothing else.";

/// What a real connectivity test found. Mirrors [`super::LlmError`]'s two failure shapes
/// (rate-limited vs everything else) so the UI can tell "this works, just not right now"
/// from "this doesn't work" — a card can be correctly configured and still be temporarily
/// rate-limited, which is a very different fix than "sign in" or "install it".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderTestOutcome {
    Ok,
    RateLimited { message: String },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderTestResult {
    pub id: String,
    pub outcome: ProviderTestOutcome,
    pub elapsed_ms: u64,
    /// RFC3339. When this test was run — the UI reads this back as "Verified 3m ago".
    pub tested_at: String,
}

/// Run one real, trivial call against `provider` and report what happened. Does NOT touch
/// the cache — callers that want the result remembered call [`persist_test_result`]
/// themselves, so a throwaway/preview test is possible without disturbing what's on disk.
///
/// `settings` supplies the model override — but only when `provider` is the user's
/// currently CHOSEN one: `llm_provider_model` is scoped to "within the chosen provider"
/// (see [`LlmConfig`]), so applying it while testing a provider the user has NOT selected
/// would pass one provider's model string to a different CLI's `--model` flag.
pub async fn test_provider(
    provider: LlmProvider,
    settings: &RuntimeSettings,
) -> ProviderTestResult {
    let id = provider.as_str().to_string();
    let t0 = Instant::now();

    let mut cfg = LlmConfig::from_settings(settings);
    cfg.cli_timeout_s = PROBE_TIMEOUT_S;
    let is_selected = LlmProvider::from_wire(&settings.llm_provider) == Some(provider);
    if !is_selected {
        cfg.model.clear();
    }

    let req = PromptRequest {
        system: PROBE_SYSTEM,
        user: String::new(),
        schema: None,
        max_tokens: 16,
        label: format!("provider-test {id}"),
    };

    let outcome = match backend_for(provider, cfg).complete(&req).await {
        Ok(_) => ProviderTestOutcome::Ok,
        Err(LlmError::RateLimited { message: m, .. }) => {
            ProviderTestOutcome::RateLimited { message: m }
        }
        Err(LlmError::Failed(m)) => ProviderTestOutcome::Failed { message: m },
    };
    finish_test(id, outcome, t0)
}

fn finish_test(id: String, outcome: ProviderTestOutcome, t0: Instant) -> ProviderTestResult {
    ProviderTestResult {
        id,
        outcome,
        elapsed_ms: t0.elapsed().as_millis() as u64,
        tested_at: chrono::Utc::now().to_rfc3339(),
    }
}

/// Test every provider [`detect_all`] currently reports installed, concurrently — the
/// "Rescan" action's expensive half. Never spends a request on a provider that isn't even
/// on the machine. Persists each result as it lands, so a slow/hanging CLI can't hold the
/// others' verified state hostage.
pub async fn test_all_installed(settings: &RuntimeSettings) -> Vec<ProviderTestResult> {
    let installed: Vec<LlmProvider> = detect_all()
        .await
        .into_iter()
        .filter(|s| s.installed)
        .filter_map(|s| LlmProvider::from_wire(&s.id))
        .collect();

    let futures = installed.into_iter().map(|p| async move {
        let result = test_provider(p, settings).await;
        persist_test_result(&result);
        result
    });
    futures::future::join_all(futures).await
}

// ── Install: run the provider's own CLI installer on the user's behalf ───────────────────

/// How long the installer may run before we give up. `npm i -g` fetching a fresh toolchain,
/// or `curl … | bash` pulling a signed tarball, can legitimately take a couple of minutes on
/// a slow link — but not five, and the user is watching a spinner.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

/// What running a provider's installer produced. Serialised to the UI, which shows the
/// message verbatim and, on success, moves straight to a connectivity test.
#[derive(Debug, Clone, Serialize)]
pub struct InstallOutcome {
    /// The installer exited 0 AND the CLI is now resolvable.
    pub ok: bool,
    /// Human-readable result — the installer's own tail on failure, a short confirmation on
    /// success. Shown to the user as-is.
    pub message: String,
    /// Where the CLI landed, when we can now find it. `None` on failure.
    pub path: Option<String>,
    /// The exact command that was run, so the UI can offer a "run it yourself" fallback.
    pub command: String,
}

/// Install `provider`'s CLI by running its official installer through the user's LOGIN shell,
/// then confirm the binary is now resolvable.
///
/// # Why a login shell
///
/// The tray is a Finder-launched `.app` with the stripped launchd `PATH`
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), which has no `npm`, `node`, or Homebrew. `npm i -g …`
/// would fail with "command not found" spawned directly. `$SHELL -l` sources the user's
/// profile, so it sees the same `npm`/PATH they do in a terminal — the same reason
/// [`resolve_cli`] probes through a login shell.
///
/// # Safety
///
/// The command comes from [`LlmProvider::install_command`], a fixed literal per provider with
/// no user input, so passing it to `-c` cannot inject anything. This is the ONE place the
/// daemon runs a vendor installer, and only ever on an explicit user click (the tray's
/// `install_llm_provider` command) — never automatically.
#[tracing::instrument(
    skip_all,
    fields(
        provider = provider.as_str(),
        ok = tracing::field::Empty,
        cli_path = tracing::field::Empty,
    )
)]
pub async fn install_provider(provider: LlmProvider) -> InstallOutcome {
    let Some(cmd) = provider.install_command() else {
        return InstallOutcome {
            ok: false,
            message: "This provider is a cloud endpoint - there is nothing to install.".into(),
            path: None,
            command: String::new(),
        };
    };
    let bin = provider.cli_name().unwrap_or("");
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    tracing::info!(provider = provider.as_str(), %cmd, "llm: running provider installer");
    let mut command = Command::new(&shell);
    command
        .arg("-l")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .no_window();

    let output = match command.spawn() {
        Ok(child) => match tokio::time::timeout(INSTALL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return install_failed(cmd, format!("could not run the installer: {e}")),
            Err(_) => {
                return install_failed(
                    cmd,
                    format!(
                        "the installer took longer than {}s and was stopped",
                        INSTALL_TIMEOUT.as_secs()
                    ),
                )
            }
        },
        Err(e) => return install_failed(cmd, format!("could not start a shell to install: {e}")),
    };

    if !output.status.success() {
        let reason = tail(String::from_utf8_lossy(&output.stderr).trim(), 400);
        return install_failed(
            cmd,
            if reason.is_empty() {
                "the installer exited with an error".to_string()
            } else {
                reason
            },
        );
    }

    // Exit 0 is necessary but not sufficient — confirm the binary is actually resolvable now,
    // the same probe the install-state badge uses, so "installed" means the same thing here.
    match resolve_cli(bin).await {
        Some(p) => {
            let span = tracing::Span::current();
            span.record("ok", true);
            span.record("cli_path", tracing::field::display(p.display()));
            tracing::info!(provider = provider.as_str(), path = %p.display(), "llm: provider installed");
            warn_if_version_unpinned(provider, &p).await;
            InstallOutcome {
                ok: true,
                message: "Installed. Checking your sign-in…".into(),
                path: Some(p.display().to_string()),
                command: cmd.to_string(),
            }
        }
        None => install_failed(
            cmd,
            "the installer finished but the CLI still isn't on your PATH - try running it in a terminal".into(),
        ),
    }
}

/// Confirm a pinned CLI actually installed at the pinned version, and warn loudly if not.
///
/// Only Cursor pins today. The pin works by rewriting version strings in the vendor's rolling
/// installer **by pattern**, which means it **fails open**: if Cursor ever changes its version
/// format the `sed` matches nothing, the script installs latest, and the user silently ends up
/// on an unverified build - exactly the outcome the pin exists to prevent, and one that would
/// surface much later as an unexplained flag rejection. This turns that into a log line at the
/// moment it happens.
///
/// Deliberately does NOT fail the install: an unpinned CLI still works (the invocation ladder
/// in [`crate::llm::cursor_cli`] degrades on an unknown flag), so blocking the user over a
/// version mismatch would be worse than telling them.
async fn warn_if_version_unpinned(provider: LlmProvider, path: &std::path::Path) {
    let Some(expected) = provider.pinned_cli_version() else {
        return;
    };
    let out = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .no_window()
        .output()
        .await;
    let Ok(out) = out else {
        tracing::warn!(
            provider = provider.as_str(),
            "llm: could not read the installed CLI version - cannot confirm the pin applied"
        );
        return;
    };
    let reported = String::from_utf8_lossy(&out.stdout);
    let reported = reported.trim();
    if reported.contains(expected) {
        tracing::info!(
            provider = provider.as_str(),
            version = expected,
            "llm: installed CLI matches the pinned version"
        );
    } else {
        tracing::warn!(
            provider = provider.as_str(),
            expected,
            reported = %reported,
            "llm: the version pin did not apply - the vendor installer's version format probably \
             changed, so an UNVERIFIED build was installed. Update CURSOR_CLI_VERSION and the \
             rewrite pattern in meridian-core/src/llm_provider.rs."
        );
    }
}

/// How long the interactive Cursor sign-in may take - a human finishing an OAuth flow in their
/// browser, so generous, but not unbounded.
const CURSOR_LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

/// Run the interactive `cursor-agent login` on the user's behalf — the "Sign in to Cursor"
/// button in the provider detail view.
///
/// This signs into the user's own Cursor account, so the coding-agent summariser then runs on
/// their **Cursor subscription** — there is no API key and nothing metered. The browser is
/// deliberately ENABLED here (the daemon's unattended `cursor_agent_init::ensure_ready` sets
/// `NO_OPEN_BROWSER` because it can't ask a human anything; this path is an explicit click, so
/// opening the browser to finish the sign-in is exactly what's wanted). Once it completes,
/// cursor-agent persists the auth and every later daemon run just adopts it.
#[tracing::instrument(skip_all, fields(ok = tracing::field::Empty, cli_path = tracing::field::Empty))]
pub async fn cursor_sign_in() -> InstallOutcome {
    let label = "cursor-agent login";
    let Some(path) = resolve_cli("cursor-agent").await else {
        return install_failed(
            label,
            "cursor-agent isn't installed yet - install it first".into(),
        );
    };

    tracing::info!("llm: launching interactive cursor-agent login");
    let mut cmd = Command::new(&path);
    cmd.arg("login")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .no_window();

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return install_failed(label, format!("couldn't start cursor-agent: {e}")),
    };

    // Drain stdout as it arrives rather than only via `wait_with_output`. `cursor-agent login`
    // prints its verification URL / device code to STDOUT while it waits, and that is precisely
    // what the user needs when the browser does not open for them (no default browser, wrong
    // profile, a headless session). Waiting for exit would discard it on the timeout path -
    // leaving a 180 s spinner followed by an error with no way to finish the sign-in by hand.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(out) = child.stdout.take() {
        let seen = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(line = %line, "cursor-agent login");
                let mut buf = seen.lock().unwrap_or_else(|e| e.into_inner());
                buf.push_str(&line);
                buf.push('\n');
            }
        });
    }
    let login_output = |seen: &std::sync::Mutex<String>| -> String {
        let buf = seen.lock().unwrap_or_else(|e| e.into_inner());
        tail(buf.trim(), 300)
    };

    let output = match tokio::time::timeout(CURSOR_LOGIN_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return install_failed(label, format!("couldn't run cursor-agent login: {e}"))
        }
        Err(_) => {
            let printed = login_output(&seen);
            return install_failed(
                label,
                if printed.is_empty() {
                    "the sign-in wasn't finished in time - click Sign in to Cursor again".into()
                } else {
                    format!(
                        "the sign-in wasn't finished in time. Finish it by hand with what \
                         cursor-agent printed:\n{printed}"
                    )
                },
            );
        }
    };

    if output.status.success() {
        let span = tracing::Span::current();
        span.record("ok", true);
        span.record("cli_path", tracing::field::display(path.display()));
        tracing::info!("llm: cursor-agent login succeeded");
        InstallOutcome {
            ok: true,
            message: "Signed in to Cursor.".into(),
            path: Some(path.display().to_string()),
            command: label.to_string(),
        }
    } else {
        // stderr first (that is where the reason goes), but fall back to what the CLI printed
        // on stdout - on some failures the URL/device code is the only useful thing said.
        let reason = tail(String::from_utf8_lossy(&output.stderr).trim(), 400);
        let reason = if reason.is_empty() {
            login_output(&seen)
        } else {
            reason
        };
        install_failed(
            label,
            if reason.is_empty() {
                "cursor-agent login failed".to_string()
            } else {
                reason
            },
        )
    }
}

/// The last `n` characters of `s`, char-safe.
///
/// The tail is the useful end of CLI output - npm, curl and cursor-agent all print the real
/// reason last - and bounding it stops a noisy installer flooding a toast.
fn tail(s: &str, n: usize) -> String {
    let rev: String = s.trim().chars().rev().take(n).collect();
    rev.chars().rev().collect()
}

/// Build a failed [`InstallOutcome`], logging the reason.
fn install_failed(cmd: &str, message: String) -> InstallOutcome {
    // Every failure path in install_provider and cursor_sign_in funnels through
    // here, so recording on the CURRENT span marks whichever of them is running
    // as failed without threading a handle through a dozen early returns.
    tracing::Span::current().record("ok", false);
    tracing::warn!(%cmd, %message, "llm: provider install failed");
    InstallOutcome {
        ok: false,
        message,
        path: None,
        command: cmd.to_string(),
    }
}

// ── Cache: last-known test result per provider, survives restarts ───────────────────────

fn test_cache_path() -> PathBuf {
    let home = std::env::var("MERIDIAN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| meridian_core::paths::home_dir_or_cwd().join(".meridian"));
    home.join("provider_test_cache.json")
}

fn load_test_cache() -> HashMap<String, ProviderTestResult> {
    let path = test_cache_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    // A corrupt/foreign-format cache degrades to "never tested" rather than failing the
    // whole panel — it will simply repopulate on the next test.
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Serialises the load→insert→write of the test cache. `test_all_installed` fans the
/// per-provider tests out concurrently and each lands its own result here; without this
/// lock those read-modify-write cycles interleave and silently drop each other's entries
/// (a lost update), exactly the state Rescan produces on any machine with 2+ CLIs. The
/// blocking I/O under the lock is a single tiny JSON file, so it is deliberately left
/// synchronous rather than moved to `spawn_blocking`.
static CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Record one test result to the on-disk cache, crash-safely (temp file + atomic rename,
/// same idiom as `settings.json`). Logged, not propagated — a failed cache write must not
/// fail the test the user just watched succeed or fail in front of them.
///
/// The whole read-modify-write is held under [`CACHE_LOCK`] so concurrent persists (a
/// Rescan testing every installed provider at once) can't lose each other's results.
pub fn persist_test_result(result: &ProviderTestResult) {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cache = load_test_cache();
    cache.insert(result.id.clone(), result.clone());
    if let Err(e) = meridian_core::fs_utils::atomic_write_json(&test_cache_path(), &cache) {
        tracing::warn!(error = %e, provider = %result.id, "failed to persist provider test result");
    }
}

/// Successful `bin name → absolute path` resolutions, memoised for the process lifetime.
///
/// Only SUCCESSES are cached. A negative result must stay retryable: a user who installs
/// `claude` while the tray is running would otherwise be told it is missing until they
/// restart the app. The cost of not caching misses is one login-shell probe per call on a
/// genuinely absent CLI, bounded by [`PROBE_TIMEOUT`].
static RESOLVED_BINS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, PathBuf>>> =
    std::sync::OnceLock::new();

/// Resolve a CLI's absolute path the way [`detect`] does — login shell first, then the
/// usual install locations.
///
/// # Who calls this
///
/// [`detect`], for the install probe; and
/// [`crate::coding_agent_session_ingest::summariser::run_capture`], which spawns the
/// resolved path instead of a bare program name. That second caller is the load-bearing
/// one: `Command::new("claude")` searches only the CALLING process's `PATH`, and the tray
/// is a Finder-launched `.app` whose `PATH` is the stripped launchd default
/// (`/usr/bin:/bin:/usr/sbin:/sbin`). The daemon never hit this because
/// `scripts/com.meridiona.daemon.plist` sets a rich `PATH` for it, which is exactly why
/// the bug only ever showed up in the tray's Test Connection.
///
/// Returns `None` when no probe finds the binary; callers fall back to the bare name so a
/// working `PATH` still behaves as before.
///
/// [`probe_current_path`] runs first and is the ONLY tier that matters on Windows: there is
/// no Finder-style PATH stripping there, so a GUI/daemon process already sees the same
/// `PATH` a terminal does — the login-shell probe below would just fail outright (no
/// `/bin/zsh`, no `$SHELL`) and waste up to [`PROBE_TIMEOUT`] doing it. It also short-circuits
/// the slow shell spawn on macOS whenever the current process's `PATH` is already good (e.g.
/// the daemon's launchd-configured one, per the `resolve_cli` doc above).
pub async fn resolve_cli(bin: &str) -> Option<PathBuf> {
    if let Some(hit) = RESOLVED_BINS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(bin)
        .cloned()
    {
        return Some(hit);
    }

    let found = match probe_current_path(bin) {
        Some(p) => Some(p),
        None => probe_login_shell(bin)
            .await
            .or_else(|| probe_candidates(bin)),
    }?;

    tracing::debug!(bin, path = %found.display(), "resolved CLI path");
    RESOLVED_BINS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(bin.to_string(), found.clone());
    Some(found)
}

/// Ask the user's login shell where the binary is. This is the one that works when the
/// app was launched from Finder — `-l` sources their profile, so we see the same `PATH`
/// they see. `-i` is deliberately omitted: an interactive shell can print banners, run
/// prompts, and block on a tty we do not have.
async fn probe_login_shell(bin: &str) -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = Command::new(&shell);
    // `bin` is passed as a POSITIONAL ARGUMENT, never interpolated into the script. The
    // script text is a fixed literal, so no value of `bin` can be parsed as shell syntax
    // — `format!("command -v {bin}")` would let a name containing `;`, `$()` or a
    // backtick execute arbitrary commands in the user's login shell. Every caller passes
    // a hardcoded literal today, but `resolve_cli` is public and takes an arbitrary
    // `&str`, so the guarantee belongs here rather than in a caller-side convention.
    // For `-c`, the first operand becomes `$0`, hence the placeholder before `bin`.
    cmd.arg("-l")
        .arg("-c")
        .arg("command -v -- \"$1\"")
        .arg("meridian-probe")
        .arg(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .no_window();

    let child = cmd.spawn().ok()?;
    let out = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // `command -v` echoes a shell builtin/alias name unchanged; only an absolute path
    // that exists is proof of an executable.
    let p = PathBuf::from(found);
    (p.is_absolute() && p.exists()).then_some(p)
}

/// Fallback: look where these things actually install. Used when the login shell is
/// unavailable, slow, or exotic.
fn probe_candidates(bin: &str) -> Option<PathBuf> {
    candidate_dirs()
        .into_iter()
        .map(|d| d.join(bin))
        .find(|p| p.exists())
}

/// Check the CURRENT process's own `PATH` for `bin` — the cheap, no-subprocess probe that
/// [`probe_login_shell`]/[`probe_candidates`] exist to work AROUND on macOS (a Finder-launched
/// `.app` gets a stripped `PATH` there, so trusting it isn't safe). Windows has no equivalent
/// stripping, so this tier alone is normally sufficient there.
fn probe_current_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = path_candidate_names(bin);
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|p| p.is_absolute() && p.is_file())
}

/// The file name(s) `bin` could actually be on disk. On Windows, `claude`/`codex`/etc.
/// install as npm-shimmed `.cmd` files, not bare `.exe`s — `CreateProcess` only appends a
/// `PATHEXT` extension automatically when spawning *by name*, not when we hand back (and a
/// caller later spawns) an absolute path, so the extension has to be resolved right here.
///
/// `PATHEXT` extensions are tried BEFORE the bare name, not after. npm's global install
/// writes THREE files per CLI — `claude` (an extensionless POSIX shebang script, for
/// WSL/Git-Bash), `claude.cmd`, and `claude.ps1` — all in the same directory. The bare
/// `claude` file `is_file()` just as truly as `claude.cmd` does, so checking it first
/// resolves to the POSIX script: no `.cmd`/`.bat` extension, so `Command::new`'s own
/// `.bat`/`.cmd` auto-detection doesn't route it through `cmd.exe` either, and it gets
/// handed to `CreateProcess` as a literal shebang text file — `os error 193, %1 is not
/// a valid Win32 application`, on a
/// machine where `claude` undeniably "works". A real `cmd.exe`/PowerShell prompt never
/// makes this mistake because PATHEXT resolution always wins over an extensionless match;
/// the bare name is kept only as a last-resort fallback for a genuinely extensionless
/// native binary.
#[cfg(windows)]
fn path_candidate_names(bin: &str) -> Vec<String> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    exts.split(';')
        .filter(|e| !e.is_empty())
        .map(|ext| format!("{bin}{ext}"))
        .chain(std::iter::once(bin.to_string()))
        .collect()
}

#[cfg(not(windows))]
fn path_candidate_names(bin: &str) -> Vec<String> {
    vec![bin.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing property: whatever `resolve_cli` hands back must be something
    /// `Command::new` can spawn WITHOUT relying on the caller's `PATH` — i.e. an absolute
    /// path that exists. A bare name here would silently reintroduce the tray bug, since
    /// a Finder-launched `.app` has only `/usr/bin:/bin:/usr/sbin:/sbin`.
    ///
    /// `sh` is the probe target because POSIX guarantees it at an absolute path.
    ///
    /// Unix-only, and so is the mechanism under test: the probe shells out to `$SHELL -l`
    /// and falls back to unix install dirs (`~/.local/bin`, `/opt/homebrew/bin`). On the
    /// Windows portability job there is no `/bin/sh` for either path to find, so this
    /// asserted the environment rather than the contract. The bug it guards is macOS-only
    /// (a Finder-launched `.app`), and the other `resolve_cli` tests still run everywhere.
    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_cli_returns_an_absolute_existing_path() {
        let found = resolve_cli("sh").await.expect("sh must resolve");
        assert!(found.is_absolute(), "not absolute: {}", found.display());
        assert!(found.exists(), "does not exist: {}", found.display());
    }

    /// Windows analogue of the unix test above. `cmd` is guaranteed present (`%SystemRoot%\
    /// System32` is always on the default `PATH`) and is reachable ONLY through
    /// `probe_current_path` — there is no login shell or unix candidate dir that could find
    /// it — so a pass here proves that tier actually runs on this platform.
    #[cfg(windows)]
    #[tokio::test]
    async fn resolve_cli_returns_an_absolute_existing_path() {
        let found = resolve_cli("cmd").await.expect("cmd must resolve");
        assert!(found.is_absolute(), "not absolute: {}", found.display());
        assert!(found.exists(), "does not exist: {}", found.display());
    }

    /// A miss must be `None` rather than a bare-name `PathBuf`. `run_capture` treats
    /// `None` as "fall back to the bare name"; a `Some("nope")` would instead be spawned
    /// as a literal relative path and fail with a confusing error.
    #[tokio::test]
    async fn resolve_cli_misses_are_none() {
        assert_eq!(
            resolve_cli("meridian-definitely-not-a-real-binary-xyz").await,
            None
        );
    }

    /// `resolve_cli` is public and takes an arbitrary `&str`, so a name carrying shell
    /// metacharacters must resolve to nothing rather than execute. The probe passes the
    /// name as a positional argument to a fixed script, so there is no way out of it.
    ///
    /// The canary is the side effect: if the `;` were interpreted, the injected `touch`
    /// would run and the file would exist.
    #[tokio::test]
    async fn resolve_cli_does_not_execute_shell_metacharacters() {
        let canary = std::env::temp_dir().join("meridian-injection-canary");
        let _ = std::fs::remove_file(&canary);
        let payload = format!("sh; touch {}", canary.display());

        assert_eq!(resolve_cli(&payload).await, None);
        assert!(
            !canary.exists(),
            "injected command ran - the probe is interpolating into the shell script"
        );
    }

    /// Negative results must NOT be memoised: a user who installs a CLI while the tray is
    /// running has to be able to Test Connection again without restarting the app.
    #[tokio::test]
    async fn resolve_cli_does_not_cache_misses() {
        let bin = "meridian-not-installed-yet-abc";
        assert_eq!(resolve_cli(bin).await, None);
        assert!(
            !RESOLVED_BINS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .contains_key(bin),
            "a miss was cached, so installing the CLI later would not be picked up"
        );
    }

    #[tokio::test]
    async fn detect_all_covers_every_builtin_exactly_once() {
        let all = detect_all().await;
        assert_eq!(all.len(), LlmProvider::builtins().len());
        for p in LlmProvider::builtins() {
            assert_eq!(all.iter().filter(|s| s.id == p.as_str()).count(), 1);
        }
    }

    /// `Custom` must never be probed as if it were one fixed thing: a `Custom` card from
    /// here would claim an endpoint the user has not configured is ready to use. Custom
    /// cards come from the registry, each carrying its own measured rung.
    #[tokio::test]
    async fn detect_all_never_reports_a_custom_card() {
        assert!(detect_all()
            .await
            .iter()
            .all(|s| s.id != LlmProvider::Custom.as_str()));
    }

    #[tokio::test]
    async fn authentication_is_never_claimed() {
        // We report installed, not usable. If this ever starts returning Some(..), the UI
        // copy ("Meridian uses your existing login") is a lie and must change with it.
        for s in detect_all().await {
            assert_eq!(s.authenticated, None, "{}", s.id);
        }
    }

    #[tokio::test]
    async fn a_binary_that_cannot_exist_is_not_found() {
        assert!(probe_login_shell("meridian-definitely-not-a-real-binary")
            .await
            .is_none());
        assert!(probe_candidates("meridian-definitely-not-a-real-binary").is_none());
    }

    /// npm's global install writes an extensionless POSIX shebang script (`claude`)
    /// alongside `claude.cmd` in the SAME directory, for WSL/Git-Bash use. Both
    /// `is_file()`. A resolver that checks the bare name before `PATHEXT` extensions
    /// would match the shebang script instead — which then fails to spawn as a native
    /// binary (`os error 193`) even though `claude` plainly "works" on this machine.
    /// This reproduces exactly that directory layout and asserts the `.CMD` file wins.
    #[cfg(windows)]
    #[tokio::test]
    async fn probe_current_path_prefers_pathext_over_a_bare_extensionless_shim() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "meridian-detect-pathext-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bare = dir.join("meridian-fake-cli");
        let shimmed = dir.join("meridian-fake-cli.CMD");
        std::fs::write(&bare, "#!/bin/sh\necho fake\n").unwrap();
        std::fs::write(&shimmed, "@echo fake\r\n").unwrap();

        let original_path = std::env::var_os("PATH");
        let new_path = match &original_path {
            Some(p) => {
                std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(p)))
                    .unwrap()
            }
            None => dir.clone().into_os_string(),
        };
        std::env::set_var("PATH", &new_path);

        let found = probe_current_path("meridian-fake-cli");

        match original_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            found.as_deref(),
            Some(shimmed.as_path()),
            "resolved {found:?}, expected the .CMD shim, not the bare shebang script"
        );
    }

    /// `MERIDIAN_HOME` is a process-global env var and cargo runs tests in parallel threads
    /// — every test that points the cache at a temp dir must hold this lock (same pattern
    /// as `meridian_core::settings`'s `ENV_LOCK`).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_temp_meridian_home() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "meridian-detect-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("MERIDIAN_HOME", &dir);
        dir
    }

    #[test]
    fn test_cache_round_trips_and_survives_a_missing_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_temp_meridian_home();

        // No cache on disk yet — a fresh install, not a failure.
        assert!(load_test_cache().is_empty());

        let result = ProviderTestResult {
            id: "claude".into(),
            outcome: ProviderTestOutcome::Ok,
            elapsed_ms: 842,
            tested_at: "2026-07-16T10:00:00+00:00".into(),
        };
        persist_test_result(&result);

        let cache = load_test_cache();
        assert_eq!(cache.get("claude"), Some(&result));

        // A second provider's result must not clobber the first.
        let rate_limited = ProviderTestResult {
            id: "cursor".into(),
            outcome: ProviderTestOutcome::RateLimited {
                message: "quota".into(),
            },
            elapsed_ms: 12,
            tested_at: "2026-07-16T10:05:00+00:00".into(),
        };
        persist_test_result(&rate_limited);
        let cache = load_test_cache();
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("claude"), Some(&result));
        assert_eq!(cache.get("cursor"), Some(&rate_limited));
    }

    /// The `test_all_installed` scenario: many providers persist their results at once.
    /// Without the read-modify-write lock in `persist_test_result` these interleave and
    /// lose updates; with it, every result survives. Runs real OS threads to make the
    /// contention genuine.
    #[test]
    fn concurrent_persists_do_not_lose_results() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_temp_meridian_home();

        const N: usize = 12;
        std::thread::scope(|scope| {
            for i in 0..N {
                scope.spawn(move || {
                    persist_test_result(&ProviderTestResult {
                        id: format!("provider-{i}"),
                        outcome: ProviderTestOutcome::Ok,
                        elapsed_ms: i as u64,
                        tested_at: "2026-07-17T10:00:00+00:00".into(),
                    });
                });
            }
        });

        let cache = load_test_cache();
        assert_eq!(cache.len(), N, "a concurrent persist was lost");
        for i in 0..N {
            assert!(cache.contains_key(&format!("provider-{i}")), "missing {i}");
        }
    }

    #[test]
    fn a_corrupt_cache_file_degrades_to_empty_not_a_panic() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = with_temp_meridian_home();
        std::fs::write(dir.join("provider_test_cache.json"), "not json").unwrap();
        assert!(load_test_cache().is_empty());
    }

    #[test]
    fn outcome_serde_uses_a_tagged_status_field() {
        let ok = serde_json::to_value(ProviderTestOutcome::Ok).unwrap();
        assert_eq!(ok["status"], "ok");

        let rl = serde_json::to_value(ProviderTestOutcome::RateLimited {
            message: "usage cap".into(),
        })
        .unwrap();
        assert_eq!(rl["status"], "rate_limited");
        assert_eq!(rl["message"], "usage cap");
    }
}
