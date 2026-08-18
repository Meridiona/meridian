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
/// [`resolve_cli`]), so it never reaches this tier for the common case anyway. The
/// Windows locations worth listing are npm's global bin, `%APPDATA%\npm` (claude/codex/
/// copilot), and cursor-agent's own installer root, `%LOCALAPPDATA%\cursor-agent` (Cursor
/// does not go through npm on Windows — see [`meridian_core::CURSOR_INSTALL_CMD`]'s
/// Windows doc) — for the rare case where a fresh shell's `PATH` hasn't picked either up
/// yet even though `PATH` is otherwise trustworthy on this platform. This is not
/// hypothetical for cursor-agent specifically: [`install_provider`] confirms the install
/// by immediately re-probing in the SAME process, whose own `PATH` cannot yet see a user
/// `PATH` write the installer just made (that only takes effect for new processes) —
/// without this fallback, a successful Windows Cursor install would report itself as
/// failed.
fn candidate_dirs() -> Vec<PathBuf> {
    let home = meridian_core::paths::home_dir_or_cwd();
    #[cfg(windows)]
    let platform: Vec<String> = {
        let mut dirs = Vec::new();
        if let Ok(appdata) = std::env::var("APPDATA") {
            dirs.push(
                PathBuf::from(appdata)
                    .join("npm")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(&local_appdata)
                    .join("cursor-agent")
                    .to_string_lossy()
                    .into_owned(),
            );
            // Where OpenAI's native installer puts `codex.exe`
            // ([`meridian_core::CODEX_INSTALL_CMD`]'s Windows form). It writes the USER
            // PATH, which an already-running tray cannot see, so without this entry a
            // SUCCESSFUL install reports itself as failed - the same trap `cursor-agent`
            // above exists for. Keep the two in lockstep with those install commands.
            dirs.push(
                PathBuf::from(&local_appdata)
                    .join("Programs")
                    .join("OpenAI")
                    .join("Codex")
                    .join("bin")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        dirs
    };
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
    /// The installer this platform will actually run
    /// ([`meridian_core::LlmProvider::install_command`]), or `None` for a provider with
    /// nothing to install.
    ///
    /// Reported to the frontend because the UI's own `installHint`
    /// (`ui/lib/llm-providers.ts`) is a single static string compiled into a dashboard that
    /// runs on BOTH platforms, so it cannot be right on both: it names the npm command,
    /// while Windows installs natively. That mattered beyond cosmetics — the hint doubles as
    /// the "run this yourself" fallback shown when an install fails, so a Windows user was
    /// told to run the exact command that cannot work there. Sourcing it from here makes the
    /// displayed command the one that ran, on every platform, by construction.
    pub install_command: Option<String>,
}

/// Probe one provider. A provider with no CLI binary (only `Custom`, a cloud endpoint) has
/// nothing to probe on disk — `detect_all` never enumerates it (its cards come from the
/// registry), so this early-return is not reached in practice.
pub async fn detect(provider: LlmProvider) -> ProviderStatus {
    let id = provider.as_str().to_string();
    let install_command = provider.install_command().map(str::to_string);
    let Some(bin) = provider.cli_name() else {
        return ProviderStatus {
            id,
            installed: true,
            path: None,
            authenticated: None,
            last_test: None,
            install_command,
        };
    };

    let found = resolve_cli(bin).await;
    ProviderStatus {
        id,
        installed: found.is_some(),
        path: found.map(|p| p.display().to_string()),
        install_command,
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

// ── In-use provider health (dashboard banner) ────────────────────────────────────────

/// Whether the user's CURRENTLY-CHOSEN provider looks usable, for the dashboard's
/// "provider unavailable" banner. Deliberately CHEAP: an install probe (memoised by
/// [`resolve_cli`]) plus the on-disk last-test result — never a fresh metered call, so it's
/// safe to compute on every health poll.
#[derive(Debug, Clone)]
pub struct InUseProviderHealth {
    /// `true` when the chosen provider looks usable: a CLI provider is installed and its last
    /// connectivity test (if any) didn't fail; a cloud endpoint's last test didn't fail. `false`
    /// means the dashboard should warn — summaries are paused or degraded until it's fixed.
    pub ok: bool,
    /// `true` when the provider is usable (`ok` stays `true`) but its last test was RATE-LIMITED —
    /// signed in and working, just throttled. Drives a distinct, softer "catching up" notice
    /// rather than the "unavailable" alarm, because it clears on its own. Mutually exclusive with
    /// `!ok` (a missing/failed provider is never also "rate-limited").
    pub rate_limited: bool,
    /// Human name for the banner copy, e.g. "Codex".
    pub name: String,
    /// The reason to show: the failure/"not installed" text when `!ok`, or the rate-limit message
    /// when `rate_limited`. `None` when the provider is simply fine.
    pub detail: Option<String>,
    /// This `!ok` verdict rests on a MEASURED failure, so only a real call can overturn it.
    ///
    /// Exists for [`crate::llm::resolver`]'s probe exemption, which is the only thing that
    /// reads it. The gate in `complete_inner` refuses every call while `!ok`, and the
    /// evidence that would clear the verdict can only come FROM a call — so without a
    /// periodic exemption the refusal is self-latching. Three states have to be told apart,
    /// and only the first is worth spending a call on:
    ///
    /// - **a recorded `Failed` outcome** → `true`. Nothing in the system re-measures this on
    ///   its own: a runtime observation ages out after six hours, and a manual test result
    ///   never expires at all, so a single bad Test Connection would otherwise black out
    ///   every AI feature until the user happened to press the button again.
    /// - **not installed** → `false`. Already self-healing: `resolve_cli` caches only
    ///   successes, so the next health poll re-probes the filesystem and clears this by
    ///   itself the moment the CLI appears. A call would spend a spawn to learn nothing.
    /// - **an ASSERTED (`sticky`) verdict** → `false`. Someone is deliberately holding this
    ///   state (today the dev-only Disconnect button) and a measurement must not overturn an
    ///   assertion — that is the entire point of the flag.
    pub probeable: bool,
}

/// A human display name for the banner — mirrors the chooser tiles in `ui/lib/llm-providers.ts`.
fn provider_display_name(p: LlmProvider) -> &'static str {
    match p {
        LlmProvider::Claude => "Claude Code",
        LlmProvider::Codex => "Codex",
        LlmProvider::Cursor => "Cursor",
        LlmProvider::Copilot => "Copilot",
        LlmProvider::Custom => "your AI provider",
    }
}

/// How long an [`in_use_provider_health`] result is reused before recomputing. The recurring
/// health poll (every 60 s) AND every on-demand `get_health` would otherwise re-run the install
/// probe each tick — and for an UNINSTALLED provider (the exact state the "provider unavailable"
/// banner exists to surface) that probe can't hit `resolve_cli`'s hit-only cache, so it falls
/// through to a login-shell spawn on a packaged app every time. A few-minute TTL bounds that to
/// one probe per window while still clearing/raising the banner promptly relative to the hourly
/// summary cadence it guards.
const IN_USE_HEALTH_TTL: Duration = Duration::from_secs(300);

/// Cache of the last computed provider health: `(provider wire id, computed_at, result)`. Keyed
/// by the provider id so switching the selected provider invalidates it immediately rather than
/// serving the previous provider's verdict for up to a TTL.
#[allow(clippy::type_complexity)]
static IN_USE_HEALTH_CACHE: std::sync::OnceLock<
    std::sync::Mutex<Option<(String, Instant, InUseProviderHealth)>>,
> = std::sync::OnceLock::new();

/// Whether the user's CURRENTLY-CHOSEN provider looks usable — [`InUseProviderHealth`] for the
/// provider named in `settings.llm_provider`, served from a short-TTL cache
/// ([`IN_USE_HEALTH_TTL`]) so the recurring health poll doesn't re-run the (possibly
/// login-shell-spawning) install probe on every tick. See [`classify_provider_health`] for the
/// unavailable → rate-limited → fine decision.
#[tracing::instrument(skip(settings), fields(provider = %settings.llm_provider))]
pub async fn in_use_provider_health(settings: &RuntimeSettings) -> InUseProviderHealth {
    // Serve a fresh-enough cached result for the SAME provider.
    {
        let guard = IN_USE_HEALTH_CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((provider, at, health)) = guard.as_ref() {
            if provider == &settings.llm_provider && at.elapsed() < IN_USE_HEALTH_TTL {
                tracing::debug!(cache = "hit", age_ms = at.elapsed().as_millis() as u64);
                return health.clone();
            }
        }
    }

    // Cache miss: this is the path that does real work - an install probe that can fall
    // through to spawning a login shell (see `IN_USE_HEALTH_TTL`). Timed so a hang here is
    // attributable from a shipped report rather than invisible.
    tracing::debug!(cache = "miss");
    let started = Instant::now();
    let health = compute_in_use_provider_health(settings).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if !health.ok {
        // The ladder flipping to unavailable is what pauses hourly summaries, so it is a
        // warning, not a debug line. `detail` is not on `redact::SAFE_STRING_KEYS`, so it
        // stays local rather than egressing.
        tracing::warn!(
            elapsed_ms,
            rate_limited = health.rate_limited,
            detail = health.detail.as_deref().unwrap_or(""),
            "in-use LLM provider is unavailable"
        );
    } else {
        tracing::debug!(
            elapsed_ms,
            rate_limited = health.rate_limited,
            "provider health probed"
        );
    }

    let mut guard = IN_USE_HEALTH_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *guard = Some((
        settings.llm_provider.clone(),
        Instant::now(),
        health.clone(),
    ));
    health
}

/// The uncached computation behind [`in_use_provider_health`]: resolve install state (one
/// [`resolve_cli`] probe for a CLI provider; a cloud `Custom` endpoint has no binary) plus the
/// last on-disk test result, then hand both to [`classify_provider_health`].
#[tracing::instrument(skip(settings), fields(provider = %settings.llm_provider))]
async fn compute_in_use_provider_health(settings: &RuntimeSettings) -> InUseProviderHealth {
    let Some(provider) = LlmProvider::from_wire(&settings.llm_provider) else {
        // An unrecognised provider string (a downgrade, a hand-edited settings file): don't
        // alarm on something we can't reason about.
        return InUseProviderHealth {
            ok: true,
            rate_limited: false,
            name: settings.llm_provider.clone(),
            detail: None,
            probeable: false,
        };
    };
    let name = provider_display_name(provider).to_string();
    // Two independent signals: the last MANUAL Settings → Test, and what the pipeline last
    // observed on a real call. The manual test alone can't see a provider that passed an hour
    // ago and is failing every call since — the state that stalls the worklog pipeline — so
    // the fresher of the two wins (see `runtime_health::most_recent_outcome`).
    let cache = load_test_cache();
    let last_test = cache.get(provider.as_str());
    let last_runtime = super::runtime_health::latest_for(provider);
    let last_outcome = super::runtime_health::most_recent_outcome(last_test, last_runtime.as_ref());
    // A cloud endpoint (`Custom`, `cli_name() == None`) has no binary to probe, so it's treated
    // as "installed" here — only its last test can tell us anything.
    let installed = match provider.cli_name() {
        Some(bin) => resolve_cli(bin).await.is_some(),
        None => true,
    };
    tracing::debug!(
        installed,
        // The variant name only - the `message` payload can carry a provider's raw
        // stderr, which has no business in a log field.
        last_outcome = last_outcome.as_ref().map_or("none", |o| match o {
            ProviderTestOutcome::Ok => "ok",
            ProviderTestOutcome::RateLimited { .. } => "rate_limited",
            ProviderTestOutcome::Failed { .. } => "failed",
        }),
        "resolved provider install state"
    );
    // `sticky` is read from the RAW test row rather than from `last_outcome`, which has
    // already collapsed the two signals into one verdict and cannot say which side won.
    let asserted = last_test.is_some_and(|t| t.sticky);
    classify_provider_health(name, installed, last_outcome.as_ref(), asserted)
}

/// The pure unavailable → rate-limited → fine decision, split out so the ladder can be unit-tested
/// without a filesystem probe.
///
/// - not installed → **unavailable** (`ok: false`).
/// - installed + last test `Failed` → **unavailable** (signed out, spawn error, …).
/// - installed + last test `RateLimited` → **rate-limited** (`ok: true, rate_limited: true`): it
///   clears on its own, so a softer flag rather than the alarm — alarming would cry wolf.
/// - installed + `Ok`/no test on record → **fine** (don't alarm before we've had a reason to).
///
/// `asserted` marks `last_outcome` as coming from a `sticky` test row; it only ever
/// suppresses [`InUseProviderHealth::probeable`] and never changes the verdict itself.
fn classify_provider_health(
    name: String,
    installed: bool,
    last_outcome: Option<&ProviderTestOutcome>,
    asserted: bool,
) -> InUseProviderHealth {
    if !installed {
        return InUseProviderHealth {
            ok: false,
            rate_limited: false,
            detail: Some(format!("{name} isn't installed")),
            name,
            // Re-probed from the filesystem on every health poll, so it clears itself.
            probeable: false,
        };
    }
    match last_outcome {
        Some(ProviderTestOutcome::Failed { message }) => InUseProviderHealth {
            ok: false,
            rate_limited: false,
            name,
            detail: Some(message.clone()),
            probeable: !asserted,
        },
        Some(ProviderTestOutcome::RateLimited { message }) => InUseProviderHealth {
            ok: true,
            rate_limited: true,
            name,
            detail: Some(message.clone()),
            probeable: false,
        },
        _ => InUseProviderHealth {
            ok: true,
            rate_limited: false,
            name,
            detail: None,
            probeable: false,
        },
    }
}

// ── Real connectivity test ───────────────────────────────────────────────────────────

/// A test call is capped far tighter than a real hourly summary (which can legitimately
/// take minutes on a big input) — this is one word, so a slow answer already means trouble
/// and the user is watching a spinner.
const PROBE_TIMEOUT_S: u64 = 20;

/// Deliberately MULTI-LINE, not a single sentence. A single-line probe cannot catch a real,
/// previously-shipped bug: on Windows, `claude`/`codex`/`cursor-agent` resolve to `.cmd`/
/// `.bat` shims, and Rust's std library refuses to spawn one when an argument contains a
/// newline (the CVE-2024-24576 "BatBadBut" fix) - which every real prompt does (Markdown
/// rules files, multi-paragraph instructions). A one-line probe passed every Test Connection
/// click while every real hourly summarisation call failed outright, so the bug went
/// undetected until it showed up in production. This shape is the minimum needed to exercise
/// the same spawn path a real call takes.
const PROBE_SYSTEM: &str =
    "You are being connectivity-tested by Meridian.\n\nReply with exactly: OK. No other text, no punctuation, nothing else.";

/// Completion budget for a connectivity test.
///
/// The answer this asks for is one word, and for a CLI provider 16 tokens was plenty. It is
/// NOT plenty for a REASONING model on an OpenAI-compatible endpoint, because reasoning
/// tokens are charged against the same completion budget as the visible answer - so a small
/// cap is spent thinking and the response comes back with `finish_reason: "length"` and an
/// EMPTY `content`.
///
/// That is not a hypothetical. Measured against Groq's `openai/gpt-oss-120b` - the model
/// Meridian itself picks for the free path - on this exact prompt:
///
/// | max_tokens | reasoning_tokens | finish_reason | content |
/// |------------|------------------|---------------|---------|
/// | 16         | 14               | length        | `""`    |
/// | 64         | 62               | length        | `""`    |
/// | 256        | 53               | stop          | `"OK"`  |
///
/// `openai_compat` correctly reports empty content as a failure, so a perfectly good key
/// failed Test Connection with "custom provider returned an empty answer" - the worst kind
/// of wrong answer, because it accuses the user's key of being broken when nothing about it
/// is. 512 leaves roughly 6x the observed reasoning trace as headroom, and it is a CEILING,
/// not a target: a non-reasoning model still stops after "OK" and is billed for two tokens.
const PROBE_MAX_TOKENS: u32 = 512;

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
    /// This verdict was ASSERTED, not measured, and must not be second-guessed by
    /// anything except an explicit test.
    ///
    /// Exactly one thing sets it: the dev-only Disconnect button, whose entire job
    /// is to hold the app in the signed-out state so that state can be worked on.
    /// Without this flag an automatic correction undoes it almost immediately -
    /// on a machine being used, a background fold or coding-agent summary succeeds
    /// within minutes - so pressing Disconnect appeared to do nothing at all.
    ///
    /// It is honoured in [`crate::llm::runtime_health::most_recent_outcome`], which
    /// is now the single place the health verdict is decided (this module's older
    /// `note_live_call_success` + failure-TTL pair was superseded by that module and
    /// removed). A measurement must never overturn an assertion; only a
    /// `Test Connection` or a Rescan replaces the whole row and therefore clears
    /// this, which is right - the user asked for a real answer.
    ///
    /// `#[serde(default)]` so caches written before this field existed still load.
    #[serde(default)]
    pub sticky: bool,
}

/// Run one real, trivial call against `provider` and report what happened. Does NOT touch
/// the cache — callers that want the result remembered call [`persist_test_result`]
/// themselves, so a throwaway/preview test is possible without disturbing what's on disk.
///
/// `settings` supplies the model override — but only when `provider` is the user's
/// currently CHOSEN one: `llm_provider_model` is scoped to "within the chosen provider"
/// (see [`LlmConfig`]), so applying it while testing a provider the user has NOT selected
/// would pass one provider's model string to a different CLI's `--model` flag.
///
/// **Calls the backend DIRECTLY, and must keep doing so.** [`crate::llm::complete`] now
/// refuses outright when this provider's last recorded verdict is unavailable — which is
/// the correct behaviour for real work, and fatal here: this function's whole job is to
/// find out whether that verdict is still true. Routed through the funnel, a provider that
/// failed once could never be re-tested, so the failure that closed the gate would also be
/// the thing preventing it from ever reopening. It would look like a provider that can
/// never be reconnected, with a Test button that fails instantly for no visible reason.
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
        max_tokens: PROBE_MAX_TOKENS,
        label: format!("provider-test {id}"),
        interactive: false,
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
        // A measured result, so it is open to correction like any other.
        sticky: false,
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

/// Install `provider`'s CLI by running its official installer through the platform's shell,
/// then confirm the binary is now resolvable.
///
/// # Why a shell at all
///
/// On macOS/Linux, the tray is a Finder-launched `.app` with the stripped launchd `PATH`
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), which has no `npm`, `node`, or Homebrew. `npm i -g …`
/// would fail with "command not found" spawned directly. `$SHELL -l` sources the user's
/// profile, so it sees the same `npm`/PATH they do in a terminal — the same reason
/// [`resolve_cli`] probes through a login shell.
///
/// Windows has no equivalent stripping (see [`resolve_cli`]'s doc), so [`installer_command`]
/// doesn't need a login shell for that reason — but it still needs a REAL shell for Cursor's
/// installer specifically: [`meridian_core::CURSOR_INSTALL_CMD`]'s Windows form uses `irm`/
/// `iex`, PowerShell aliases with no `cmd.exe` equivalent, so [`installer_command`] spawns
/// `powershell.exe` directly there (not nested inside `cmd /C`, which would need a second,
/// fragile layer of quote-escaping for the embedded `-Command` payload).
///
/// # Why the leading binary is resolved to an absolute path first
///
/// `$SHELL -l` only gets the installer as far as whatever `~/.zprofile`/`~/.zshenv` put on
/// `PATH` (see [`installer_command`]'s doc on why `-i` is avoided). In practice almost every
/// PATH customization — Homebrew's `eval "$(brew shellenv)"`, nvm, Volta, a hand-added
/// `export PATH=…` — ends up in `~/.zshrc` instead, purely because that is the file every
/// popular tutorial and `oh-my-zsh` tell you to edit; `~/.zprofile` is rarely touched. Chasing
/// each of those tools' own rc-sourcing convention one at a time (nvm's `nvm.sh` + `nvm use
/// default` is one earlier example) does not scale. So before building the shell command, the
/// leading binary name (`npm`, `curl`, …) is resolved through [`resolve_cli`] — the same
/// multi-tier probe (current `PATH`, login shell, and fixed install directories like
/// `/opt/homebrew/bin`) already used everywhere else in this module — and swapped in as an
/// absolute path. That sidesteps the shell's own `PATH` resolution for the installer binary
/// entirely, rather than adding another rc file to the sourcing allowlist. Falls back to the
/// bare command unchanged when resolution fails, so this can only ever help, never break an
/// install that already worked.
///
/// # Safety
///
/// The command comes from [`LlmProvider::install_command`], a fixed literal per provider with
/// no user input, so passing it to the shell cannot inject anything. This is the ONE place the
/// daemon runs a vendor installer, and only ever on an explicit user click (the tray's
/// `install_llm_provider` command) — never automatically.
///
/// Replaces `cmd`'s leading binary name with its resolved absolute path, when [`resolve_cli`]
/// can find one — see [`install_provider`]'s "leading binary" doc section for why. Returns
/// `cmd` unchanged (not an `Option`) so every caller has a command to run either way; the
/// resolution is best-effort by design.
///
/// # Windows: deliberately a no-op
///
/// Every reason above is a macOS/Linux reason, and the rewrite it produces is POSIX shell
/// text - `export PATH="…:$PATH"; <abs path> …` - which [`installer_command`] feeds to
/// **PowerShell**, where it is not merely suboptimal but unrunnable:
///
/// - `export` is not a PowerShell command (`$env:PATH` is, and entries are `;`-separated,
///   not `:`), and `$PATH` is an undefined variable there.
/// - The substituted absolute path is interpolated unquoted and without the `&` call
///   operator, so a perfectly ordinary `C:\Program Files\nodejs\npm.cmd` is parsed as the
///   command `C:\Program`.
///
/// Both fail with `CommandNotFoundException` before the vendor's installer runs at all. The
/// premise doesn't hold here either: Windows has no Finder/launchd PATH stripping (see
/// [`resolve_cli`]'s doc), so the current process's `PATH` already sees what a terminal
/// does, and PowerShell resolves `npm` → `npm.cmd` itself via `PATHEXT`. Passing `cmd`
/// through untouched is therefore both correct and sufficient.
///
/// This was a live bug: it made the Install button fail for every npm-based provider on
/// Windows, and - because the leading token is what triggers the rewrite - Cursor was
/// unaffected purely by accident, its command starting with `$s`.
#[cfg(windows)]
async fn resolve_installer_binary(cmd: &str) -> String {
    cmd.to_string()
}

#[cfg(not(windows))]
async fn resolve_installer_binary(cmd: &str) -> String {
    match cmd.split_once(' ') {
        Some((installer_bin, rest)) => match resolve_cli(installer_bin).await {
            // `npm` (and most CLIs installed by the same tooling) is itself a
            // `#!/usr/bin/env node` script — the OS execs `env`, and `env` does its OWN
            // independent PATH search for `node` using the shell's PATH, completely
            // unaffected by how npm itself was invoked. Substituting only npm's absolute
            // path fixed the FIRST lookup ("command not found: npm") but not this second,
            // nested one ("env: node: No such file or directory") — every real npm
            // install keeps `node` in the exact same directory as `npm` (Homebrew, nvm's
            // per-version bin/, Volta's shim dir, the official installer), so prepending
            // that directory onto PATH fixes the shebang lookup the same way resolving
            // npm itself fixed the first one.
            Some(resolved) => match resolved.parent() {
                Some(dir) => format!(
                    "export PATH=\"{}:$PATH\"; {} {rest}",
                    dir.display(),
                    resolved.display()
                ),
                None => format!("{} {rest}", resolved.display()),
            },
            None => cmd.to_string(),
        },
        None => cmd.to_string(),
    }
}

/// Build a `Command` for `path`, a CLI already located via [`resolve_cli`], with `path`'s own
/// directory prepended onto the child's `PATH` — same fix as [`resolve_installer_binary`],
/// applied to the sign-in flows ([`cursor_sign_in`]/[`codex_sign_in`]/[`claude_sign_in`]),
/// which spawn the resolved binary directly rather than through a shell.
///
/// Those CLIs are still spawned by ABSOLUTE PATH, so the OS doesn't need `PATH` to find `path`
/// itself — but `codex`/`claude` (unlike `cursor-agent`, a native binary) are `#!/usr/bin/env
/// node` scripts, and `env` does its own independent `PATH` search for `node` at exec time,
/// using whatever environment the child inherits. `Command::new` inherits the CURRENT
/// process's `PATH` by default — the tray's own stripped launchd one on macOS, with no
/// `/opt/homebrew/bin` — so that inner lookup fails with "env: node: No such file or
/// directory" exactly like [`resolve_installer_binary`]'s install-time case, even though the
/// CLI itself was already found and resolved correctly.
fn command_for_resolved_cli(path: &std::path::Path) -> Command {
    let mut cmd = Command::new(path);
    if let Some(dir) = path.parent() {
        let existing = std::env::var_os("PATH").unwrap_or_default();
        // std::env::join_paths/split_paths, not a hardcoded `:` — this is called
        // on every platform (cursor_sign_in/codex_sign_in/claude_sign_in all use
        // it directly), and Windows PATH entries are `;`-separated, not `:`.
        let dirs = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&existing));
        let new_path = std::env::join_paths(dirs).unwrap_or(existing);
        cmd.env("PATH", new_path);
    }
    cmd
}

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

    let cmd = resolve_installer_binary(cmd).await;
    let cmd = cmd.as_str();

    tracing::info!(provider = provider.as_str(), %cmd, "llm: running provider installer");
    let mut command = installer_command(cmd);
    command
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

/// Build the (unspawned) command that runs `cmd` through the platform's shell.
///
/// `pub(crate)`: also reused by
/// [`crate::coding_agent_session_ingest::cursor_agent_init::try_auto_install`], the daemon's
/// own (opt-in) unattended installer — so both the tray's "Install" button and the daemon's
/// background install go through the exact same platform dispatch instead of growing two
/// copies that can drift (the daemon's used to hardcode `bash -c`, which never worked on
/// Windows in the first place).
///
/// Unix: the user's login shell (`$SHELL -l -c`) — see the module docs on why a login shell
/// specifically. Also sources `~/.nvm/nvm.sh` and runs `nvm use default` first if present, a
/// no-op otherwise: `-l` without `-i` (see [`resolve_cli`]'s doc on why `-i` is avoided)
/// sources `~/.zprofile`/`~/.zshenv` but NOT `~/.zshrc` — zsh only reads `~/.zshrc` for an
/// interactive shell. nvm's own official install instructions add its init block to
/// `~/.zshrc` (or it lands there via oh-my-zsh's nvm plugin), so on any machine where nvm
/// was set up that conventional way, an `npm i -g …` install command would fail with
/// "command not found: npm" even though a normal Terminal resolves it fine.
///
/// Sourcing `nvm.sh` alone is NOT sufficient, though — it only defines the `nvm` shell
/// function; it does not itself put any installed Node's `bin/` on `PATH`. That only
/// happens after an explicit `nvm use <version>`, which is why a first fix that stopped at
/// sourcing `nvm.sh` still hit the exact same "command not found: npm" on a machine that
/// never runs `nvm use` from a *second*, separately-added line in its shell rc (nvm does not
/// add one itself — auto-switching on shell start is something the user or a plugin like
/// oh-my-zsh's opts into on top of the base install). `nvm use default` mirrors what that
/// second line does — activating whatever `nvm alias default` points at — and is itself a
/// no-op (silently, via the trailing `>/dev/null 2>&1`) when no default alias is set, so it
/// cannot make a working machine worse. Both nvm calls are `;`-separated from `{cmd}`, not
/// `&&`-chained into it, so neither one failing (nvm absent, no default alias) skips running
/// the actual install command.
///
/// Windows: `powershell.exe` directly, NOT `cmd.exe`. `cmd /C` cannot run Cursor's Windows
/// installer (`irm`/`iex` are PowerShell aliases, no `cmd` equivalent — see
/// [`meridian_core::CURSOR_INSTALL_CMD`]'s Windows doc), and nesting `cmd /C "powershell
/// ... -Command \"...\""` to work around that would need a second, mismatched layer of
/// quote-escaping (`cmd.exe`'s quote handling and a C-runtime-style argv escape do not agree
/// on what `\"` means) on top of the one Rust already does to hand `cmd` a single argv
/// element — spawning `powershell.exe` directly keeps it to that one layer. Plain commands
/// (`npm i -g …`) run identically well under `-Command` as they did under `cmd /C`.
/// `-ExecutionPolicy Bypass` is defensive: inline `-Command` text isn't normally subject to
/// the script-file execution policy, but a locked-down machine's policy could still be
/// stricter than default, and there is no `.ps1` file here to sign.
///
/// # Why the exit-code epilogue is more than `; exit $LASTEXITCODE`
///
/// Propagating the exit code explicitly is necessary — relying on PowerShell's implicit
/// propagation for the LAST statement in a `-Command` script is not documented behaviour.
/// But `; exit $LASTEXITCODE` ALONE is not sufficient, and its insufficiency is silent:
///
/// **`$LASTEXITCODE` is only ever set by a NATIVE executable.** If the command dies before
/// reaching one — a `CommandNotFoundException` from a malformed command, a parse error — the
/// variable is still `$null`, and `exit $null` exits **0**. [`install_provider`] then reads
/// `status.success()` as true, skips the branch that surfaces stderr, and reports the
/// generic "installer finished but the CLI still isn't on your PATH". That message sends the
/// user to debug their PATH over what is really a broken install command, and the actual
/// error — sitting in stderr — is discarded unread.
///
/// So the epilogue distinguishes three cases:
///
/// 1. A native exe ran → trust its code verbatim, exactly as before.
/// 2. No native exe ran AND a command name failed to resolve → exit 127 (the conventional
///    "command not found"), so the real stderr reaches the user.
/// 3. Anything else → exit 0.
///
/// Case 2 keys on `CommandNotFoundException` specifically rather than "`$Error` is
/// non-empty" on purpose: a vendor installer that raises and HANDLES a non-terminating error
/// still leaves it in `$Error`, and failing on that would break installs that work today
/// (Cursor's `iex`-ed script is pure PowerShell and may never set `$LASTEXITCODE` at all).
/// An unresolvable command name, by contrast, is never a recoverable condition.
#[cfg(windows)]
pub(crate) fn installer_command(cmd: &str) -> Command {
    let mut command = Command::new("powershell");
    command
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(format!("$Error.Clear(); {cmd}; {EXIT_EPILOGUE}"));
    command
}

/// The exit-code epilogue appended to every Windows installer command — see
/// [`installer_command`]'s "Why the exit-code epilogue is more than `; exit $LASTEXITCODE`".
///
/// Split out as a named constant rather than inlined into the `format!` so the three-case
/// logic is readable on its own, and so there is exactly one definition of it to change.
/// Its BEHAVIOUR is pinned by `installer_command_reports_an_unrunnable_command_as_failed`
/// and `installer_command_tolerates_a_handled_powershell_error`, which run the real shell
/// rather than string-matching this text - a match on the text would pass just as happily
/// for an epilogue that PowerShell rejects.
#[cfg(windows)]
pub(crate) const EXIT_EPILOGUE: &str = concat!(
    "if ($null -ne $LASTEXITCODE) { exit $LASTEXITCODE }; ",
    "if ($Error | Where-Object { $_.FullyQualifiedErrorId -like 'CommandNotFound*' }) ",
    "{ exit 127 }; ",
    "exit 0"
);

#[cfg(not(windows))]
pub(crate) fn installer_command(cmd: &str) -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut command = Command::new(shell);
    let script = format!(
        "export NVM_DIR=\"$HOME/.nvm\"; [ -s \"$NVM_DIR/nvm.sh\" ] && \\. \"$NVM_DIR/nvm.sh\" && nvm use default >/dev/null 2>&1; {cmd}"
    );
    command.arg("-l").arg("-c").arg(script);
    command
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

/// How long an interactive provider sign-in (Cursor, Codex or Claude) may take - a human
/// finishing an OAuth flow in their browser, so generous, but not unbounded.
const INTERACTIVE_LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

/// How long to let a freshly-spawned login CLI run before the first authoritative status
/// poll (see [`interactive_login`]) - the local OAuth round-trip legitimately takes a few
/// seconds even on the happy path, so polling from time zero would just be a wasted extra
/// process spawn on every sign-in.
const VERIFY_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// How often to re-poll status once the grace period has elapsed. Short enough that a
/// browser callback landing early doesn't cost the user the rest of the 180s wait if the
/// child process itself hangs; long enough that it isn't a second CLI process running
/// nearly back-to-back with the login itself.
const VERIFY_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Spawn a background task that appends each line from `pipe` into a shared buffer, tagged
/// by `stream` ("stdout"/"stderr") so both streams are distinguishable in traces. Returns
/// the buffer immediately; empty forever if `pipe` is `None` (nothing to drain).
///
/// Draining BOTH streams live (not just via a final `wait_with_output`) is what lets
/// [`interactive_login`] poll `child.try_wait()` without deadlocking a login CLI that fills
/// its pipe buffer while we are busy waiting on something else.
fn drain_lines(
    pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    stream: &'static str,
) -> std::sync::Arc<std::sync::Mutex<String>> {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(out) = pipe {
        let seen = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(line = %line, stream, "llm: interactive login output");
                let mut buf = seen.lock().unwrap_or_else(|e| e.into_inner());
                buf.push_str(&line);
                buf.push('\n');
            }
        });
    }
    seen
}

/// The last `n` characters buffered by [`drain_lines`], char-safe - see [`tail`].
fn buffered_tail(buf: &std::sync::Mutex<String>, n: usize) -> String {
    let s = buf.lock().unwrap_or_else(|e| e.into_inner());
    tail(&s, n)
}

/// [`interactive_login`]'s three durations, bundled so a real sign-in and a test can each
/// supply their own without growing the function's argument count. Production always uses
/// [`Self::PRODUCTION`]; tests use tiny values so the same race logic exercises in
/// milliseconds instead of minutes.
#[derive(Clone, Copy)]
struct InteractiveLoginTiming {
    deadline: Duration,
    grace: Duration,
    poll_interval: Duration,
}

impl InteractiveLoginTiming {
    const PRODUCTION: Self = Self {
        deadline: INTERACTIVE_LOGIN_TIMEOUT,
        grace: VERIFY_GRACE_PERIOD,
        poll_interval: VERIFY_POLL_INTERVAL,
    };
}

/// Drive one interactive `<bin> <args…>` login to completion.
///
/// # Sign-in detection hardening
///
/// Before this existed, [`cursor_sign_in`]/[`codex_sign_in`]/[`claude_sign_in`] trusted
/// ONLY the spawned CLI child's own exit code (or a bare 180s timeout). That is provably
/// not the whole story: a browser OAuth round-trip can genuinely complete - the vendor's
/// own local callback server renders its own "signed in" page - while the CLI process that
/// owns that server fails to reflect it back to us within the wait window (a hang, a
/// secondary post-login step exiting non-zero after auth already landed, or our own
/// `kill_on_drop` racing a credential write that was about to finish). The visible symptom
/// was exactly what shipped as a bug report: the browser shows success, the app keeps
/// showing "not signed in".
///
/// This function trusts a clean `exit 0` directly (no added latency on the happy path,
/// which is the overwhelming common case), but treats every OTHER outcome - a non-zero
/// exit, a spawn/wait error, or the timeout firing - as advisory rather than final: it
/// re-checks against `verify`, a cheap LOCAL-only ground-truth probe (a cached credential
/// file / a fast status endpoint - see `codex::login_status_signed_in`,
/// `cursor::login_status_signed_in`, `claude::login_status_signed_in`), and reports success
/// anyway if THAT says the user is actually signed in. Both signals are always logged, so a
/// divergence between "the process said no" and "but you actually are" is visible in
/// traces rather than silently resolved.
///
/// It also polls `verify` periodically WHILE the child is still running (after an initial
/// grace period), so a browser callback that lands early doesn't force the user to wait out
/// the rest of a 180s timeout just because the underlying CLI process happens to hang.
///
/// `display_name` is the vendor name used in user-facing copy ("Cursor"/"Codex"/"Claude").
async fn interactive_login<V, VFut>(
    label: &'static str,
    bin: &str,
    args: &[&str],
    display_name: &str,
    success_message: String,
    verify: V,
    timing: InteractiveLoginTiming,
) -> InstallOutcome
where
    V: Fn() -> VFut,
    VFut: std::future::Future<Output = Option<bool>>,
{
    let Some(path) = resolve_cli(bin).await else {
        return install_failed(
            label,
            format!("{bin} isn't installed yet - install it first"),
        );
    };

    tracing::info!(bin, "llm: launching interactive login");
    let mut cmd = command_for_resolved_cli(&path);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .no_window();

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return install_failed(label, format!("couldn't start {bin}: {e}")),
    };

    // Both streams are drained live, not just via a final `wait_with_output` - see
    // `drain_lines`'s doc. stdout carries the verification URL / device code a stuck login
    // prints while it waits, which is exactly what the user needs to finish it by hand.
    let stdout_buf = drain_lines(child.stdout.take(), "stdout");
    let stderr_buf = drain_lines(child.stderr.take(), "stderr");

    enum Ended {
        Exited(std::process::ExitStatus),
        WaitError(std::io::Error),
        TimedOut,
    }

    /// `None` = still running. Centralised so the loop can re-check right after waking from
    /// a sleep without duplicating the match - see the loop body for why that second check
    /// matters (closes a race where the child exits DURING the sleep).
    fn poll_exit(child: &mut tokio::process::Child) -> Option<Ended> {
        match child.try_wait() {
            Ok(Some(status)) => Some(Ended::Exited(status)),
            Ok(None) => None,
            Err(e) => Some(Ended::WaitError(e)),
        }
    }

    let deadline = tokio::time::Instant::now() + timing.deadline;
    let mut next_verify = tokio::time::Instant::now() + timing.grace;

    let ended = loop {
        if let Some(ended) = poll_exit(&mut child) {
            break ended;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break Ended::TimedOut;
        }
        tokio::time::sleep_until(next_verify.min(deadline)).await;
        // The child may have exited WHILE we slept - a clean exit must never pay for a
        // verify round-trip it doesn't need, which is exactly what skipping this check
        // would risk on a fast-exiting CLI and a short grace period.
        if let Some(ended) = poll_exit(&mut child) {
            break ended;
        }
        if tokio::time::Instant::now() >= next_verify {
            if verify().await == Some(true) {
                tracing::info!(
                    bin,
                    "llm: sign-in confirmed by status check while the CLI was still running"
                );
                return InstallOutcome {
                    ok: true,
                    message: success_message,
                    path: Some(path.display().to_string()),
                    command: label.to_string(),
                };
            }
            next_verify = tokio::time::Instant::now() + timing.poll_interval;
        }
    };

    match ended {
        Ended::Exited(status) if status.success() => {
            let span = tracing::Span::current();
            span.record("ok", true);
            span.record("cli_path", tracing::field::display(path.display()));
            tracing::info!(bin, "llm: interactive login succeeded");
            InstallOutcome {
                ok: true,
                message: success_message,
                path: Some(path.display().to_string()),
                command: label.to_string(),
            }
        }
        Ended::Exited(status) => {
            // stderr first (that is where the reason goes), but fall back to what the CLI
            // printed on stdout - on some failures the URL/device code is the only useful
            // thing said.
            let stderr_tail = buffered_tail(&stderr_buf, 400);
            let reason = if stderr_tail.is_empty() {
                buffered_tail(&stdout_buf, 300)
            } else {
                stderr_tail
            };
            let process_message = format!(
                "{bin} exited {:?}: {}",
                status.code(),
                if reason.is_empty() {
                    "no output".to_string()
                } else {
                    reason
                }
            );
            confirm_or_report(label, bin, &path, success_message, &verify, process_message).await
        }
        Ended::WaitError(e) => {
            confirm_or_report(
                label,
                bin,
                &path,
                success_message,
                &verify,
                format!("couldn't run {bin}: {e}"),
            )
            .await
        }
        Ended::TimedOut => {
            let printed = buffered_tail(&stdout_buf, 300);
            let timeout_message = if printed.is_empty() {
                format!(
                    "the sign-in wasn't finished in time - click Sign in to {display_name} again"
                )
            } else {
                format!(
                    "the sign-in wasn't finished in time. Finish it by hand with what \
                     {bin} printed:\n{printed}"
                )
            };
            confirm_or_report(label, bin, &path, success_message, &verify, timeout_message).await
        }
    }
}

/// The shared tail of every non-happy-path outcome in [`interactive_login`]: give the
/// process's own report one last chance to be overridden by ground truth before it becomes
/// what the user sees.
async fn confirm_or_report<V, VFut>(
    label: &str,
    bin: &str,
    path: &std::path::Path,
    success_message: String,
    verify: &V,
    process_message: String,
) -> InstallOutcome
where
    V: Fn() -> VFut,
    VFut: std::future::Future<Output = Option<bool>>,
{
    if verify().await == Some(true) {
        // The two signals disagree: the CLI process reported failure/timeout, but a direct
        // read of the vendor's own auth state says otherwise. Ground truth wins - logged at
        // WARN (not swallowed as INFO) precisely because a real divergence like this is
        // worth seeing on a packaged install, not just locally.
        tracing::warn!(
            bin,
            process_outcome = %process_message,
            "llm: sign-in process reported failure but the status check confirms the user IS \
             signed in - trusting the status check"
        );
        let span = tracing::Span::current();
        span.record("ok", true);
        span.record("cli_path", tracing::field::display(path.display()));
        return InstallOutcome {
            ok: true,
            message: success_message,
            path: Some(path.display().to_string()),
            command: label.to_string(),
        };
    }
    install_failed(label, process_message)
}

/// Run the interactive `cursor-agent login` on the user's behalf — the "Sign in to Cursor"
/// button in the provider detail view.
///
/// This signs into the user's own Cursor account, so the coding-agent summariser then runs on
/// their **Cursor subscription** — there is no API key and nothing metered. The browser is
/// deliberately ENABLED here (the daemon's unattended `cursor_agent_init::ensure_ready` sets
/// `NO_OPEN_BROWSER` because it can't ask a human anything; this path is an explicit click, so
/// opening the browser to finish the sign-in is exactly what's wanted). Once it completes,
/// cursor-agent persists the auth and every later daemon run just adopts it.
///
/// See [`interactive_login`] for the shared race/verify logic this and its two siblings run
/// through, and `cursor::login_status_signed_in` for Cursor's ground-truth check.
#[tracing::instrument(skip_all, fields(ok = tracing::field::Empty, cli_path = tracing::field::Empty))]
pub async fn cursor_sign_in() -> InstallOutcome {
    let meridian_home = default_meridian_home();
    interactive_login(
        "cursor-agent login",
        "cursor-agent",
        &["login"],
        "Cursor",
        "Signed in to Cursor.".to_string(),
        move || {
            let home = meridian_home.clone();
            async move { super::cursor::login_status_signed_in(&home).await }
        },
        InteractiveLoginTiming::PRODUCTION,
    )
    .await
}

/// Run the interactive `codex login` on the user's behalf — the "Sign in to Codex" button in
/// the provider detail view.
///
/// `codex login` (no subcommand) opens the user's browser to sign into their ChatGPT account
/// and runs a localhost callback server to receive the OAuth redirect; on success it writes
/// `~/.codex/auth.json` and exits 0, so the summariser then runs on their **ChatGPT
/// subscription** — no API key, nothing metered. A deliberate mirror of [`cursor_sign_in`].
///
/// See [`interactive_login`] for the shared race/verify logic and `codex::login_status_signed_in`
/// for Codex's ground-truth check (the same `codex login status` call `codex::signed_out`
/// already uses as a pre-flight for a real completion call).
#[tracing::instrument(skip_all, fields(ok = tracing::field::Empty, cli_path = tracing::field::Empty))]
pub async fn codex_sign_in() -> InstallOutcome {
    let meridian_home = default_meridian_home();
    interactive_login(
        "codex login",
        "codex",
        &["login"],
        "Codex",
        "Signed in to Codex.".to_string(),
        move || {
            let home = meridian_home.clone();
            async move { super::codex::login_status_signed_in(&home).await }
        },
        InteractiveLoginTiming::PRODUCTION,
    )
    .await
}

/// Run the interactive `claude auth login` on the user's behalf — the "Sign in to Claude"
/// button in the provider detail view.
///
/// `claude auth login` (default `--claudeai`) opens the user's browser to sign into their
/// Anthropic account on their **Claude subscription** — no API key, nothing metered. A
/// deliberate mirror of [`cursor_sign_in`]/[`codex_sign_in`].
///
/// See [`interactive_login`] for the shared race/verify logic and `claude::login_status_signed_in`
/// for Claude's ground-truth check.
#[tracing::instrument(skip_all, fields(ok = tracing::field::Empty, cli_path = tracing::field::Empty))]
pub async fn claude_sign_in() -> InstallOutcome {
    let meridian_home = default_meridian_home();
    interactive_login(
        "claude auth login",
        "claude",
        &["auth", "login", "--claudeai"],
        "Claude",
        "Signed in to Claude.".to_string(),
        move || {
            let home = meridian_home.clone();
            async move { super::claude::login_status_signed_in(&home).await }
        },
        InteractiveLoginTiming::PRODUCTION,
    )
    .await
}

/// `~/.meridian` (or `$MERIDIAN_HOME`), resolved the same way [`LlmConfig::from_settings`]
/// does - reused here (rather than re-deriving it) so a status-check verifier and a real
/// backend call never disagree about which credentials directory they're reading.
fn default_meridian_home() -> PathBuf {
    LlmConfig::from_settings(&meridian_core::settings::load_runtime_settings()).meridian_home
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

/// The last recorded connectivity test for one provider wire id, or `None` if it has never
/// been tested.
///
/// Exists for the providers [`detect_all`] does not enumerate — i.e. `custom`, which has no
/// CLI to probe but does accumulate test results (`test_llm_provider`, and the dev Disconnect
/// button). Without this the cached verdict for a cloud endpoint was written and then never
/// read by anything that renders it.
pub fn cached_test_result(id: &str) -> Option<ProviderTestResult> {
    load_test_cache().get(id).cloned()
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
    {
        let _guard = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = load_test_cache();
        cache.insert(result.id.clone(), result.clone());
        if let Err(e) = meridian_core::fs_utils::atomic_write_json(&test_cache_path(), &cache) {
            tracing::warn!(error = %e, provider = %result.id, "failed to persist provider test result");
        }
    }
    // A recorded outcome is the ONLY thing that changes the health verdict, and
    // that verdict is memoised for IN_USE_HEALTH_TTL (5 min). Without this the
    // banner contradicts the panel for up to five minutes in both directions: a
    // user who fixes a signed-out provider and watches its card go green still
    // sees "unavailable" across the top, and concludes the fix did not work.
    // Every path that records an outcome comes through here, so this is the one
    // place it can be done without a caller being able to forget.
    invalidate_in_use_health();
}

/// Drop the memoised in-use provider verdict, so the next
/// [`in_use_provider_health`] recomputes instead of serving a stale answer.
pub fn invalidate_in_use_health() {
    if let Some(cell) = IN_USE_HEALTH_CACHE.get() {
        *cell.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Successful `bin name → absolute path` resolutions, memoised for the process lifetime.
///
/// Only SUCCESSES are cached. A negative result must stay retryable: a user who installs
/// `claude` while the tray is running would otherwise be told it is missing until they
/// restart the app. The cost of not caching misses is one login-shell probe per call on a
/// genuinely absent CLI, bounded by [`PROBE_TIMEOUT`].
///
/// A cache HIT is still re-validated with a cheap [`Path::exists`] check in [`resolve_cli`]
/// before being trusted: a CLI uninstalled (or moved) while this process is running would
/// otherwise be reported "installed" forever, and the next real invocation would fail with
/// a raw, unfriendly OS/shell error instead of "not installed" — that was the exact symptom
/// hit uninstalling `codex`/`cursor-agent` out from under a running tray. `exists()` is a
/// single stat syscall, not a shell spawn, so this costs nothing on the hot path.
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
    // Read the cache under a SCOPED lock and drop the guard on this `let` before the block
    // below. Holding it across an `if let` body would keep the guard alive through the body
    // (temporary lifetime), and the re-lock to evict a stale entry would then deadlock on this
    // same thread — a `std::sync::Mutex` is not re-entrant.
    let cached = RESOLVED_BINS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(bin)
        .cloned();
    if let Some(hit) = cached {
        if hit.exists() {
            return Some(hit);
        }
        // Stale: the CLI was removed (or moved) since we last resolved it. Drop the
        // entry and fall through to a fresh probe rather than trusting it forever.
        tracing::debug!(bin, path = %hit.display(), "cached CLI path no longer exists - re-probing");
        RESOLVED_BINS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(bin);
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
///
/// Resolves through [`path_candidate_names`] the same way [`probe_current_path`] does, NOT a
/// bare `d.join(bin)`: on Windows an install dir can hold the same bare-shebang-plus-`.cmd`
/// layout `probe_current_path`'s doc describes (npm writes all three), and this tier is
/// reached precisely when the current process's `PATH` doesn't have the dir yet (a fresh
/// install's `PATH` write not being visible to an already-running process) — the exact
/// situation [`install_provider`] hits when it re-probes right after running an installer.
/// Joining the bare name first would resolve to the extensionless POSIX script and fail to
/// spawn with `os error 193`, reproducing the same bug `probe_current_path` was fixed for, one
/// tier down.
fn probe_candidates(bin: &str) -> Option<PathBuf> {
    let names = path_candidate_names(bin);
    for dir in candidate_dirs() {
        for name in &names {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
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

    // ── interactive_login: sign-in race/verify logic ─────────────────────────────────────
    //
    // Exercised against a REAL spawned `sh -c "…"` (unix-only - POSIX guarantees `sh` at an
    // absolute path, same reasoning as `resolve_cli_returns_an_absolute_existing_path` above)
    // rather than a mock Child, so the actual `try_wait`/`kill_on_drop`/pipe-draining
    // machinery is what's under test, not a stand-in for it. `InteractiveLoginTiming` is
    // always overridden to millisecond scale here - the real constants exist for a human
    // finishing a browser OAuth flow, and using them in a unit test would make the suite
    // take minutes for no extra coverage.

    fn instant_timing() -> InteractiveLoginTiming {
        InteractiveLoginTiming {
            deadline: Duration::from_millis(500),
            grace: Duration::from_millis(5),
            poll_interval: Duration::from_millis(15),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clean_exit_is_trusted_without_ever_calling_verify() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called2 = called.clone();
        let out = interactive_login(
            "test login",
            "sh",
            &["-c", "exit 0"],
            "Test",
            "Signed in to Test.".to_string(),
            move || {
                called2.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Some(true) }
            },
            instant_timing(),
        )
        .await;
        assert!(out.ok);
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "the happy path must not pay the extra verify round-trip"
        );
    }

    /// THE regression this whole refactor exists for: a process that reports failure is
    /// overridden by a ground-truth check that says the user really is signed in.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_process_is_overridden_by_a_confirming_verify() {
        let out = interactive_login(
            "test login",
            "sh",
            &["-c", "exit 1"],
            "Test",
            "Signed in to Test.".to_string(),
            || async { Some(true) },
            instant_timing(),
        )
        .await;
        assert!(
            out.ok,
            "verify's confirmation must win over the process exit code"
        );
        assert_eq!(out.message, "Signed in to Test.");
    }

    /// The mirror case: a failed process AND an inconclusive (or negative) verify must still
    /// report the original failure - ground truth can only ever promote a result to success,
    /// never invent a more specific failure than what actually happened.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_process_with_an_inconclusive_verify_reports_the_process_failure() {
        let out = interactive_login(
            "test login",
            "sh",
            &["-c", "echo boom 1>&2; exit 1"],
            "Test",
            "Signed in to Test.".to_string(),
            || async { None },
            instant_timing(),
        )
        .await;
        assert!(!out.ok);
        assert!(out.message.contains("boom"), "got: {}", out.message);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_confirmed_negative_verify_does_not_override_a_failed_process() {
        let out = interactive_login(
            "test login",
            "sh",
            &["-c", "exit 1"],
            "Test",
            "Signed in to Test.".to_string(),
            || async { Some(false) },
            instant_timing(),
        )
        .await;
        assert!(!out.ok);
    }

    /// A browser callback landing early must not force the user to wait out the whole
    /// process: once `verify` confirms sign-in WHILE the child is still running, this
    /// returns immediately rather than waiting for the child to exit or the deadline to
    /// fire. Proven by a child that sleeps far longer than `instant_timing`'s deadline -
    /// the test only completes because the early return actually fires.
    #[cfg(unix)]
    #[tokio::test]
    async fn verify_confirming_mid_wait_returns_before_the_child_exits_or_times_out() {
        let out = interactive_login(
            "test login",
            "sh",
            &["-c", "sleep 30"],
            "Test",
            "Signed in to Test.".to_string(),
            || async { Some(true) },
            instant_timing(),
        )
        .await;
        assert!(out.ok);
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

    /// The regression this exists for: uninstalling `codex`/`cursor-agent` out from under a
    /// running tray left `RESOLVED_BINS` pointing at a path that no longer existed, and the
    /// stale hit was trusted forever - reporting "installed" (then failing with a raw OS
    /// error on the next real invocation) instead of "not installed". A cache hit must be
    /// re-validated with `exists()` and dropped if it no longer holds.
    #[tokio::test]
    async fn resolve_cli_drops_a_stale_cache_entry_whose_path_no_longer_exists() {
        let bin = "meridian-uninstalled-while-running-xyz";
        let stale = std::env::temp_dir().join("meridian-definitely-does-not-exist-anymore");
        assert!(
            !stale.exists(),
            "canary path must not exist: {}",
            stale.display()
        );

        RESOLVED_BINS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(bin.to_string(), stale.clone());

        assert_eq!(
            resolve_cli(bin).await,
            None,
            "a stale cached path was trusted instead of being re-probed"
        );
        assert!(
            !RESOLVED_BINS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .contains_key(bin),
            "the stale entry was not evicted from the cache"
        );
    }

    /// The regression this exists for: a single-line probe passed Test Connection on every
    /// provider while every REAL summarisation call (multi-line prompts) failed to even spawn
    /// on Windows. See `PROBE_SYSTEM`'s doc — this must stay multi-line or the probe silently
    /// stops exercising the spawn path that actually broke.
    #[test]
    fn probe_system_is_multiline_to_catch_the_windows_batch_spawn_bug() {
        assert!(
            PROBE_SYSTEM.contains('\n'),
            "a single-line probe passes even when a real prompt would fail to spawn on Windows"
        );
    }

    /// A reasoning model spends the completion budget on hidden reasoning tokens BEFORE it
    /// writes a visible character, so a probe budget sized for "one word" comes back with
    /// `finish_reason: "length"` and empty content — and `openai_compat` correctly calls that
    /// a failure. Measured against Groq's `openai/gpt-oss-120b` (the model Meridian picks for
    /// its own free path): 16 tokens → 14 reasoning tokens, no content; 256 → answered "OK".
    ///
    /// So a perfectly good key failed Test Connection while the SCHEMA probe on the very same
    /// endpoint passed — because `probe::PROBE_MAX_TOKENS` was already 512 and this one was
    /// not. Both budgets must stay large enough to hold a reasoning trace.
    #[test]
    fn the_connectivity_probe_leaves_room_for_a_reasoning_trace() {
        // Compared through a binding rather than asserted on the constant directly, which
        // clippy rejects as always-true - the point is to fail the build if the CONSTANT is
        // ever lowered, so the floor is what the comparison names.
        let floor = 256;
        assert!(
            PROBE_MAX_TOKENS >= floor,
            "a budget this small is spent on reasoning tokens before the model answers, and \
             the empty response reads to the user as a broken key"
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

    /// The dashboard banner's whole meaning rides on this ladder, so pin every rung: not
    /// installed and an outright `Failed` are UNAVAILABLE (the alarm); a rate limit stays
    /// available with the softer flag (it self-recovers - alarming would cry wolf); an `Ok` or
    /// no-test-yet is fine. Pure, so no probe/filesystem needed.
    #[test]
    fn classify_provider_health_ranks_unavailable_over_rate_limited_over_fine() {
        // not installed → unavailable, regardless of any prior test result.
        let h = classify_provider_health("Codex".into(), false, None, false);
        assert!(!h.ok && !h.rate_limited);
        assert_eq!(h.detail.as_deref(), Some("Codex isn't installed"));

        // installed but last test FAILED → unavailable, surfacing that reason.
        let failed = ProviderTestOutcome::Failed {
            message: "not signed in".into(),
        };
        let h = classify_provider_health("Codex".into(), true, Some(&failed), false);
        assert!(!h.ok && !h.rate_limited);
        assert_eq!(h.detail.as_deref(), Some("not signed in"));

        // installed + RATE-LIMITED → still available (ok), softer flag set, message surfaced.
        let rl = ProviderTestOutcome::RateLimited {
            message: "usage limit".into(),
        };
        let h = classify_provider_health("Codex".into(), true, Some(&rl), false);
        assert!(h.ok && h.rate_limited);
        assert_eq!(h.detail.as_deref(), Some("usage limit"));

        // installed + Ok, and installed + no test on record → fine, no banner, no detail.
        let ok = ProviderTestOutcome::Ok;
        for last in [Some(&ok), None] {
            let h = classify_provider_health("Codex".into(), true, last, false);
            assert!(h.ok && !h.rate_limited && h.detail.is_none());
        }
    }

    /// Which `!ok` verdicts are worth spending a real call to re-check.
    ///
    /// `probeable` exists because the resolver's health gate refuses on a recorded
    /// verdict while only a successful call can replace one — so a verdict nothing
    /// re-measures latches shut. The distinction it draws is the whole value: a
    /// MEASURED failure is worth a periodic call because nothing else will ever
    /// overturn it, while the other two `!ok` states must not spend one.
    #[test]
    fn only_a_measured_failure_is_worth_probing() {
        let failed = ProviderTestOutcome::Failed {
            message: "not signed in".into(),
        };

        // A measured failure: nothing re-measures this on its own. A runtime observation
        // ages out after six hours and a manual test result never expires at all, so
        // without a probe a single bad Test Connection is a permanent blackout.
        assert!(
            classify_provider_health("Codex".into(), true, Some(&failed), false).probeable,
            "a measured failure is the case the exemption exists for"
        );

        // ASSERTED (`sticky`): someone is deliberately holding this state, and a
        // measurement must never overturn an assertion.
        assert!(
            !classify_provider_health("Codex".into(), true, Some(&failed), true).probeable,
            "an asserted verdict must not be second-guessed by a probe"
        );

        // Not installed: already self-healing. `resolve_cli` caches only successes, so the
        // next health poll re-probes the filesystem and clears this for free — a call would
        // spend a CLI spawn to learn what a `Path::exists` already knows.
        assert!(
            !classify_provider_health("Codex".into(), false, None, false).probeable,
            "a missing install re-checks itself; a call would buy nothing"
        );

        // Rate-limited is not a refusal at all, so there is nothing to probe past.
        let rl = ProviderTestOutcome::RateLimited {
            message: "usage limit".into(),
        };
        assert!(!classify_provider_health("Codex".into(), true, Some(&rl), false).probeable);
    }

    /// The health verdict is memoised for 5 minutes, and recording a test result is
    /// the only thing that can change it. When the two were not wired together, the
    /// banner disagreed with the Intelligence panel for up to that long IN BOTH
    /// DIRECTIONS — a user who fixed a signed-out provider watched its card go green
    /// while "provider unavailable" stayed across the top, and reasonably concluded
    /// the fix had not worked. Nothing about that failure is visible in a type or a
    /// log; it just looks like the app ignoring you.
    #[test]
    fn recording_a_test_result_drops_the_memoised_health_verdict() {
        // Seed the cache with a verdict, as a health poll would.
        *IN_USE_HEALTH_CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap() = Some((
            "claude".to_string(),
            Instant::now(),
            InUseProviderHealth {
                ok: true,
                rate_limited: false,
                name: "Claude Code".into(),
                detail: None,
                probeable: false,
            },
        ));

        invalidate_in_use_health();

        assert!(
            IN_USE_HEALTH_CACHE.get().unwrap().lock().unwrap().is_none(),
            "a recorded outcome must invalidate the memoised verdict, or the banner \
             serves a stale answer for up to IN_USE_HEALTH_TTL"
        );
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
        let _guard = ENV_LOCK.lock().await;
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

    /// The regression this exists for: `install_provider` used to build its command with
    /// `Command::new(SHELL-or-"/bin/zsh")` unconditionally, which on Windows fails to even
    /// spawn (`os error 3`, no such path) before the installer — even a plain `npm i -g …`
    /// — gets any chance to run. `installer_command` must produce something that actually
    /// starts and executes the given command on every platform.
    #[tokio::test]
    async fn installer_command_actually_runs_on_this_platform() {
        let mut cmd = installer_command("echo hello-from-installer");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.output().await.expect("installer_command must spawn");
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("hello-from-installer"),
            "{output:?}"
        );
    }

    /// A failing installer must be reported as failed. On Windows this is the whole reason
    /// for the `; exit $LASTEXITCODE` suffix in `installer_command`: PowerShell's own exit
    /// code for a `-Command` script is not guaranteed to mirror a native command's exit
    /// code, so without it a failing `npm i -g …` could be reported to the user as a
    /// successful install.
    #[tokio::test]
    async fn installer_command_propagates_a_failure_exit_code() {
        let mut cmd = installer_command("exit 7");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.output().await.expect("installer_command must spawn");
        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(7));
    }

    /// The measured Windows regression, pinned end to end: a command that dies before
    /// reaching any native executable must be reported as FAILED, with its real error on
    /// stderr.
    ///
    /// This is the exact shape `resolve_installer_binary` used to hand PowerShell — POSIX
    /// `export` plus an unquoted space-bearing path — and every part of the old epilogue's
    /// failure is reproduced here: PowerShell raises `CommandNotFoundException` twice, no
    /// native exe ever runs, so `$LASTEXITCODE` stays `$null` and the old `exit $LASTEXITCODE`
    /// exited **0**. `install_provider` read that as success, never looked at stderr, and told
    /// the user their PATH was wrong.
    #[cfg(windows)]
    #[tokio::test]
    async fn installer_command_reports_an_unrunnable_command_as_failed() {
        let mut cmd = installer_command(
            r#"export PATH="C:\Some Dir\bin:$PATH"; C:\Some Dir\bin\npm.cmd i -g some-package"#,
        );
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.output().await.expect("installer_command must spawn");
        assert!(
            !output.status.success(),
            "a command that never reached a native exe must not report success: {output:?}"
        );
        assert_eq!(output.status.code(), Some(127), "{output:?}");
        // The cause must survive to stderr — `install_provider` only reads it on !success(),
        // so reporting this as success is what silently discarded it.
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not recognized"),
            "the real error must reach stderr: {output:?}"
        );
    }

    /// The other half of that fix, and the reason it keys on `CommandNotFoundException`
    /// rather than "`$Error` is non-empty": a pure-PowerShell installer that raises and
    /// HANDLES an error still leaves it in `$Error` and may never set `$LASTEXITCODE` at all.
    /// Cursor's `iex`-ed script is exactly that shape, and it installs correctly today —
    /// failing it would be a regression caused by the fix.
    #[cfg(windows)]
    #[tokio::test]
    async fn installer_command_tolerates_a_handled_powershell_error() {
        let mut cmd = installer_command(
            r#"try { Get-Item 'C:\meridian\definitely\missing' -ErrorAction Stop } catch { }"#,
        );
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.output().await.expect("installer_command must spawn");
        assert!(
            output.status.success(),
            "a handled non-terminating error must not fail the install: {output:?}"
        );
    }

    /// `resolve_installer_binary` must leave the command ALONE on Windows. Its rewrite is
    /// POSIX shell text (`export PATH="…:$PATH"; <abs path> …`) which PowerShell cannot run,
    /// and the premise doesn't hold here anyway — see the fn's Windows doc. Uses `cmd`, which
    /// always resolves on Windows (`C:\Windows\System32\cmd.exe`), so this only passes
    /// because the fn is a no-op, not because resolution happened to fail.
    #[cfg(windows)]
    #[tokio::test]
    async fn resolve_installer_binary_is_a_no_op_on_windows() {
        let original = "cmd /c exit 0";
        assert_eq!(
            resolve_installer_binary(original).await,
            original,
            "Windows must pass the installer command through untouched"
        );
    }

    /// OpenAI's native installer writes the USER PATH, which an already-running tray cannot
    /// see, so `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` is the ONLY way the post-install
    /// probe finds `codex.exe`. Dropping it turns a successful install into a reported
    /// failure — the same bug `%LOCALAPPDATA%\cursor-agent` was added to prevent.
    #[cfg(windows)]
    #[test]
    fn candidate_dirs_covers_the_native_codex_install_dir() {
        // Same pattern as `candidate_dirs_includes_the_cursor_agent_install_root`: the lock
        // and a pinned value are load-bearing, because a sibling test removes `LOCALAPPDATA`
        // and these run in parallel.
        let _guard = ENV_LOCK.blocking_lock();
        let original = std::env::var_os("LOCALAPPDATA");
        std::env::set_var("LOCALAPPDATA", r"C:\Users\meridian-test\AppData\Local");

        let dirs = candidate_dirs();

        match original {
            Some(v) => std::env::set_var("LOCALAPPDATA", v),
            None => std::env::remove_var("LOCALAPPDATA"),
        }

        assert!(
            dirs.contains(&PathBuf::from(
                r"C:\Users\meridian-test\AppData\Local\Programs\OpenAI\Codex\bin"
            )),
            "{dirs:?}"
        );
    }

    /// `ProviderStatus` must report the installer THIS build will actually run, for every
    /// provider that has one. The frontend renders this instead of its own static
    /// `installHint`, which is a single string compiled for both platforms and therefore
    /// wrong on one of them — and which doubles as the "run it yourself" fallback after a
    /// failed install.
    ///
    /// Deliberately NOT `#[cfg]`-gated: it asserts the field tracks
    /// `LlmProvider::install_command` rather than any particular command text, so it is
    /// meaningful on macOS and Windows alike and runs in BOTH CI Rust jobs.
    #[tokio::test]
    async fn provider_status_reports_this_platforms_install_command() {
        for provider in LlmProvider::builtins() {
            let status = detect(provider).await;
            assert_eq!(
                status.install_command.as_deref(),
                provider.install_command(),
                "{provider:?} must report the command this platform installs with"
            );
            // Non-empty, not merely present: the UI renders this string as the
            // "how to install" hint, so `Some("")` would pass an `is_some()`
            // check and still show the user a blank instruction.
            assert!(
                status
                    .install_command
                    .as_deref()
                    .is_some_and(|c| !c.trim().is_empty()),
                "{provider:?} is a CLI provider and must have a non-empty installer command"
            );
        }
    }

    /// The wire contract with `ui/lib/api-types.ts`'s `ProviderStatus`: the field must
    /// actually reach the frontend under the name the UI reads. A `#[serde(skip)]` or a
    /// rename would leave `probed?.install_command` permanently `undefined`, silently
    /// reverting the UI to the platform-wrong static hint with no test failing.
    #[tokio::test]
    async fn provider_status_serialises_install_command_for_the_frontend() {
        let status = detect(LlmProvider::Claude).await;
        let json = serde_json::to_value(&status).expect("ProviderStatus must serialise");
        assert_eq!(
            json.get("install_command").and_then(|v| v.as_str()),
            LlmProvider::Claude.install_command(),
            "the frontend reads `install_command` — got {json:?}"
        );
    }

    /// The regression this exists for: an npm-based install (`npm i -g @openai/codex`, etc.)
    /// failed with "command not found: npm" on any Mac where nvm's init block lives only in
    /// `~/.zshrc` — `installer_command`'s `-l` (login, non-interactive) shell never sources
    /// that file. Faking `$HOME/.nvm/nvm.sh` with a script that exports a marker var proves
    /// the fix actually sources it, not just that the file happens to exist on disk.
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_command_sources_nvm_sh_when_present() {
        // The child inherits `HOME` at `spawn()`, which is synchronous — so the guard only
        // needs to span the env mutation, spawn, and restoring `HOME`, not the awaited wait.
        // Holding a std `MutexGuard` across an `.await` is a clippy deny (`await_holding_lock`).
        // The temp dir must outlive the child's own exec (nvm.sh has to still be on disk when
        // the spawned shell opens it), so it isn't removed until after `wait_with_output`.
        let temp_home =
            std::env::temp_dir().join(format!("meridian-detect-nvm-test-{}", std::process::id()));
        let child = {
            let _guard = ENV_LOCK.lock().await;
            let original_home = std::env::var_os("HOME");
            let nvm_dir = temp_home.join(".nvm");
            std::fs::create_dir_all(&nvm_dir).unwrap();
            std::fs::write(
                nvm_dir.join("nvm.sh"),
                "export MERIDIAN_NVM_TEST_MARKER=sourced\n",
            )
            .unwrap();
            std::env::set_var("HOME", &temp_home);

            let mut cmd = installer_command("echo \"$MERIDIAN_NVM_TEST_MARKER\"");
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = cmd.spawn();

            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            child
        };

        let output = child
            .expect("installer_command must spawn")
            .wait_with_output()
            .await
            .expect("installer_command must run to completion");
        let _ = std::fs::remove_dir_all(&temp_home);

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "sourced",
            "{output:?}"
        );
    }

    /// Most machines don't use nvm at all — the `[ -s "$NVM_DIR/nvm.sh" ] &&` guard must be a
    /// true no-op (no error, no hang) when `~/.nvm` doesn't exist, so the fix can't regress the
    /// common case it was already passing before.
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_command_is_a_no_op_without_nvm() {
        // See `installer_command_sources_nvm_sh_when_present`'s comment: the guard must not
        // span the `.await` (clippy's `await_holding_lock`), so it only covers the env
        // mutation + spawn, and `HOME` is restored before the wait.
        let temp_home = std::env::temp_dir().join(format!(
            "meridian-detect-no-nvm-test-{}",
            std::process::id()
        ));
        let child = {
            let _guard = ENV_LOCK.lock().await;
            let original_home = std::env::var_os("HOME");
            std::fs::create_dir_all(&temp_home).unwrap();
            std::env::set_var("HOME", &temp_home);

            let mut cmd = installer_command("echo hello-no-nvm");
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = cmd.spawn();

            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            child
        };

        let output = child
            .expect("installer_command must spawn")
            .wait_with_output()
            .await
            .expect("installer_command must run to completion");
        let _ = std::fs::remove_dir_all(&temp_home);

        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("hello-no-nvm"),
            "{output:?}"
        );
    }

    /// The regression `installer_command_sources_nvm_sh_when_present` did NOT catch: sourcing
    /// `nvm.sh` only defines the `nvm` shell FUNCTION, it does not itself put any Node's
    /// `bin/` on `PATH` — that needs an explicit `nvm use`. A real nvm install run through the
    /// old fix (source-only) still could not find `npm`, because nvm.sh alone never adds it to
    /// PATH. This fakes a minimal `nvm` function that only prepends a bin dir to `PATH` when
    /// called as `nvm use default`, with a fake `npm` script living in that dir — proving
    /// `installer_command` actually resolves npm, not merely that `nvm.sh` got sourced.
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_command_activates_the_default_nvm_version() {
        use std::os::unix::fs::PermissionsExt;

        let temp_home = std::env::temp_dir().join(format!(
            "meridian-detect-nvm-use-default-test-{}",
            std::process::id()
        ));
        let child = {
            let _guard = ENV_LOCK.lock().await;
            let original_home = std::env::var_os("HOME");
            let nvm_dir = temp_home.join(".nvm");
            let node_bin_dir = temp_home.join("fake-node-version").join("bin");
            std::fs::create_dir_all(&nvm_dir).unwrap();
            std::fs::create_dir_all(&node_bin_dir).unwrap();

            let npm_path = node_bin_dir.join("npm");
            std::fs::write(&npm_path, "#!/bin/sh\necho FAKE_NVM_NPM_RAN\n").unwrap();
            let mut perms = std::fs::metadata(&npm_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&npm_path, perms).unwrap();

            std::fs::write(
                nvm_dir.join("nvm.sh"),
                format!(
                    "nvm() {{\n  if [ \"$1\" = use ] && [ \"$2\" = default ]; then\n    export PATH=\"{}:$PATH\"\n  fi\n}}\n",
                    node_bin_dir.display()
                ),
            )
            .unwrap();
            std::env::set_var("HOME", &temp_home);

            let mut cmd = installer_command("npm");
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = cmd.spawn();

            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            child
        };

        let output = child
            .expect("installer_command must spawn")
            .wait_with_output()
            .await
            .expect("installer_command must run to completion");
        let _ = std::fs::remove_dir_all(&temp_home);

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "FAKE_NVM_NPM_RAN",
            "{output:?}"
        );
    }

    /// The Homebrew case that motivated `resolve_installer_binary`: a machine where `npm`
    /// resolves fine in a normal Terminal (Homebrew's `brew shellenv` is set up in `~/.zshrc`,
    /// not `~/.zprofile`) but not through the login-non-interactive shell `installer_command`
    /// uses — `candidate_dirs()` already lists `/opt/homebrew/bin` (for a different reason:
    /// finding an installed CLI afterward), so `resolve_cli` finds it there regardless of which
    /// rc file the real PATH setup lives in. This uses `~/.local/bin` (also a candidate dir)
    /// with a fake binary rather than the real `/opt/homebrew/bin`, so the test doesn't depend
    /// on Homebrew being installed on the machine running it.
    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_installer_binary_finds_a_binary_only_a_candidate_dir_knows_about() {
        let _guard = ENV_LOCK.lock().await;
        let original_home = std::env::var_os("HOME");
        let temp_home = std::env::temp_dir().join(format!(
            "meridian-detect-installer-bin-test-{}",
            std::process::id()
        ));
        let bin_dir = temp_home.join(".local").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_bin = bin_dir.join("meridian-fake-installer-bin");
        std::fs::write(&fake_bin, "#!/bin/sh\necho fake\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perms).unwrap();
        }
        std::env::set_var("HOME", &temp_home);

        let resolved =
            resolve_installer_binary("meridian-fake-installer-bin i -g some-package").await;

        match original_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&temp_home);

        assert_eq!(
            resolved,
            format!(
                "export PATH=\"{}:$PATH\"; {} i -g some-package",
                bin_dir.display(),
                fake_bin.display()
            ),
            "expected the leading binary swapped for its resolved absolute path, with its \
             directory prepended onto PATH"
        );
    }

    /// The Node-shebang case that motivated prepending PATH, not just resolving the leading
    /// binary: `npm` is itself a `#!/usr/bin/env node` script, so the OS execs `env`, and `env`
    /// does its OWN independent PATH search for `node` — resolving npm's own absolute path
    /// does not help THAT lookup. Fakes an `npm` script with a real `env node` shebang plus a
    /// fake `node` living alongside it, so this only passes if PATH was actually prepended
    /// (not just npm's leading token swapped).
    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_installer_binary_fixes_the_env_node_shebang_lookup() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.lock().await;
        let original_home = std::env::var_os("HOME");
        let temp_home = std::env::temp_dir().join(format!(
            "meridian-detect-shebang-test-{}",
            std::process::id()
        ));
        let bin_dir = temp_home.join(".local").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();

        let fake_node = bin_dir.join("node");
        std::fs::write(&fake_node, "#!/bin/sh\necho FAKE_NODE_RAN\n").unwrap();
        let fake_npm = bin_dir.join("meridian-fake-npm");
        std::fs::write(&fake_npm, "#!/usr/bin/env node\n").unwrap();
        for f in [&fake_node, &fake_npm] {
            let mut perms = std::fs::metadata(f).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(f, perms).unwrap();
        }
        std::env::set_var("HOME", &temp_home);

        let resolved_cmd = resolve_installer_binary("meridian-fake-npm i -g some-package").await;

        let child = {
            let mut cmd = installer_command(&resolved_cmd);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            cmd.spawn()
        };

        match original_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }

        let output = child
            .expect("installer_command must spawn")
            .wait_with_output()
            .await
            .expect("installer_command must run to completion");
        let _ = std::fs::remove_dir_all(&temp_home);

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "FAKE_NODE_RAN",
            "{output:?}"
        );
    }

    /// When the leading binary cannot be resolved anywhere (no PATH, no login shell hit, no
    /// candidate dir), `resolve_installer_binary` must return `cmd` unchanged rather than
    /// producing something unspawnable — the caller still needs a command to try.
    #[tokio::test]
    async fn resolve_installer_binary_falls_back_when_unresolvable() {
        let cmd = "meridian-definitely-not-a-real-binary --flag value";
        let resolved = resolve_installer_binary(cmd).await;
        assert_eq!(resolved, cmd);
    }

    /// The sign-in-flow counterpart to `resolve_installer_binary_fixes_the_env_node_shebang_lookup`:
    /// `cursor_sign_in`/`codex_sign_in`/`claude_sign_in` spawn the resolved CLI directly
    /// (`Command::new(&path)`, no shell), so `resolve_installer_binary`'s PATH-prepending fix
    /// never applied to them — `codex`/`claude` are still `#!/usr/bin/env node` scripts, and a
    /// directly-spawned child inherits the CURRENT process's `PATH` (the tray's own stripped
    /// one), not any shell-derived one. Fakes a `#!/usr/bin/env node` CLI with a fake `node`
    /// alongside it; only passes if `command_for_resolved_cli` actually set `PATH` on the child.
    #[cfg(unix)]
    #[tokio::test]
    async fn command_for_resolved_cli_fixes_the_env_node_shebang_lookup() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = std::env::temp_dir().join(format!(
            "meridian-detect-cmd-resolved-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let fake_node = temp_dir.join("node");
        std::fs::write(&fake_node, "#!/bin/sh\necho FAKE_NODE_RAN\n").unwrap();
        let fake_cli = temp_dir.join("meridian-fake-cli");
        std::fs::write(&fake_cli, "#!/usr/bin/env node\n").unwrap();
        for f in [&fake_node, &fake_cli] {
            let mut perms = std::fs::metadata(f).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(f, perms).unwrap();
        }

        let mut cmd = command_for_resolved_cli(&fake_cli);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd
            .output()
            .await
            .expect("command_for_resolved_cli must spawn");
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "FAKE_NODE_RAN",
            "{output:?}"
        );
    }

    /// cursor-agent does not install via npm on Windows (see
    /// `meridian_core::CURSOR_INSTALL_CMD`'s Windows doc) — its own installer root must be a
    /// fallback candidate dir, or a Cursor install's post-install re-probe (same process,
    /// stale `PATH`) reports a successful install as failed.
    #[cfg(windows)]
    #[test]
    fn candidate_dirs_includes_the_cursor_agent_install_root() {
        let _guard = ENV_LOCK.blocking_lock();
        let original = std::env::var_os("LOCALAPPDATA");
        std::env::set_var("LOCALAPPDATA", r"C:\Users\meridian-test\AppData\Local");

        let dirs = candidate_dirs();

        match original {
            Some(v) => std::env::set_var("LOCALAPPDATA", v),
            None => std::env::remove_var("LOCALAPPDATA"),
        }

        assert!(
            dirs.contains(&PathBuf::from(
                r"C:\Users\meridian-test\AppData\Local\cursor-agent"
            )),
            "{dirs:?}"
        );
    }

    /// The `LOCALAPPDATA`-less case must not panic or produce a bogus dir — an unset var is
    /// simply "nothing to add here", same as `APPDATA`'s existing handling.
    #[cfg(windows)]
    #[test]
    fn candidate_dirs_tolerates_a_missing_localappdata() {
        let _guard = ENV_LOCK.blocking_lock();
        let original = std::env::var_os("LOCALAPPDATA");
        std::env::remove_var("LOCALAPPDATA");

        let dirs = candidate_dirs();

        if let Some(v) = original {
            std::env::set_var("LOCALAPPDATA", v);
        }

        assert!(
            dirs.iter().all(|d| !d.ends_with("cursor-agent")),
            "{dirs:?}"
        );
    }

    /// The fallback-directory tier must resolve `PATHEXT` the same way the `PATH` tier does
    /// (`probe_current_path`). A bare `d.join(bin)` would match an extensionless shebang
    /// script before its `.CMD` shim in the same directory and fail to spawn (`os error
    /// 193`) — exactly the bug `probe_current_path` was fixed for, one tier down, and this
    /// tier is reached precisely when a just-installed CLI's directory isn't on the current
    /// process's `PATH` yet (the post-install re-probe in `install_provider`).
    #[cfg(windows)]
    #[test]
    fn probe_candidates_prefers_pathext_over_a_bare_extensionless_shim() {
        let _guard = ENV_LOCK.blocking_lock();
        let original_home = std::env::var_os("HOME");
        let temp_home = std::env::temp_dir().join(format!(
            "meridian-detect-candidates-test-{}",
            std::process::id()
        ));
        let bin_dir = temp_home.join(".local").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bare = bin_dir.join("meridian-fake-cli2");
        let shimmed = bin_dir.join("meridian-fake-cli2.CMD");
        std::fs::write(&bare, "#!/bin/sh\necho fake\n").unwrap();
        std::fs::write(&shimmed, "@echo fake\r\n").unwrap();
        std::env::set_var("HOME", &temp_home);

        let found = probe_candidates("meridian-fake-cli2");

        match original_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&temp_home);

        assert_eq!(
            found.as_deref(),
            Some(shimmed.as_path()),
            "resolved {found:?}, expected the .CMD shim, not the bare shebang script"
        );
    }

    /// `MERIDIAN_HOME`/`HOME` are process-global env vars and cargo runs tests in parallel
    /// threads — every test that points one at a temp dir must hold this lock (same pattern
    /// as `meridian_core::settings`'s `ENV_LOCK`). A `tokio::sync::Mutex`, not `std::sync`:
    /// some `#[tokio::test]` callers (e.g. `resolve_installer_binary_*`) need the lock held
    /// across an `.await` — `resolve_cli`'s async login-shell probe runs before its own
    /// synchronous candidate-dir fallback, so the guard must still be held when that fallback
    /// reads `HOME` — and a `std::sync::MutexGuard` held across `.await` is a clippy deny
    /// (`await_holding_lock`). Sync `#[test]` callers use `.blocking_lock()` instead of
    /// `.lock().await` — safe because plain `#[test]` functions never run inside a Tokio
    /// runtime, which is the only case `blocking_lock` panics in.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        let _guard = ENV_LOCK.blocking_lock();
        with_temp_meridian_home();

        // No cache on disk yet — a fresh install, not a failure.
        assert!(load_test_cache().is_empty());

        let result = ProviderTestResult {
            id: "claude".into(),
            outcome: ProviderTestOutcome::Ok,
            elapsed_ms: 842,
            tested_at: "2026-07-16T10:00:00+00:00".into(),
            sticky: false,
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
            sticky: false,
        };
        persist_test_result(&rate_limited);
        let cache = load_test_cache();
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("claude"), Some(&result));
        assert_eq!(cache.get("cursor"), Some(&rate_limited));
    }

    /// `cached_test_result` is the READ side, and it exists because the write side alone
    /// was not enough: a cloud endpoint's verdict was being persisted and then read by
    /// nothing that renders it, so the picker showed a Groq tile as IN USE for a provider
    /// the app had just been told was not usable. That fix shipped with no test.
    ///
    /// The `custom` id is the case that matters - `detect_all` cannot enumerate it (there
    /// is no CLI to probe), so this lookup is the ONLY way its result reaches the UI.
    #[test]
    fn a_cached_verdict_is_readable_back_for_a_provider_with_no_cli() {
        let _guard = ENV_LOCK.blocking_lock();
        with_temp_meridian_home();

        // Never tested is None, not a default-shaped "passing" result - the distinction
        // the picker relies on to avoid claiming a verdict it does not have.
        assert!(cached_test_result("custom").is_none());

        let failed = ProviderTestResult {
            id: "custom".into(),
            outcome: ProviderTestOutcome::Failed {
                message: "endpoint returned no content".into(),
            },
            elapsed_ms: 340,
            tested_at: "2026-08-07T09:00:00+00:00".into(),
            sticky: false,
        };
        persist_test_result(&failed);

        assert_eq!(cached_test_result("custom"), Some(failed));
        // A different provider's verdict must not answer for this one.
        assert!(cached_test_result("groq").is_none());
    }

    /// The `test_all_installed` scenario: many providers persist their results at once.
    /// Without the read-modify-write lock in `persist_test_result` these interleave and
    /// lose updates; with it, every result survives. Runs real OS threads to make the
    /// contention genuine.
    #[test]
    fn concurrent_persists_do_not_lose_results() {
        let _guard = ENV_LOCK.blocking_lock();
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
                        sticky: false,
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
        let _guard = ENV_LOCK.blocking_lock();
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
