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

use meridian_core::settings::RuntimeSettings;
use meridian_core::LlmProvider;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{resolver::backend_for, LlmConfig, LlmError, PromptRequest};

/// How long a probe may take before we call it absent. A login shell sources the user's
/// profile, which can be slow (nvm, rbenv, …), but not this slow.
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

/// Where these CLIs actually land, for when the login shell is unavailable or too slow.
fn candidate_dirs() -> Vec<PathBuf> {
    let home = meridian_core::paths::home_dir_or_cwd();
    [
        home.join(".local/bin").to_string_lossy().into_owned(),
        home.join(".npm-global/bin").to_string_lossy().into_owned(),
        home.join(".bun/bin").to_string_lossy().into_owned(),
        home.join(".volta/bin").to_string_lossy().into_owned(),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ]
    .into_iter()
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
/// Returns `None` when neither probe finds the binary; callers fall back to the bare name
/// so a working `PATH` still behaves as before.
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

    let found = probe_login_shell(bin)
        .await
        .or_else(|| probe_candidates(bin))?;

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
    cmd.arg("-l")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing property: whatever `resolve_cli` hands back must be something
    /// `Command::new` can spawn WITHOUT relying on the caller's `PATH` — i.e. an absolute
    /// path that exists. A bare name here would silently reintroduce the tray bug, since
    /// a Finder-launched `.app` has only `/usr/bin:/bin:/usr/sbin:/sbin`.
    ///
    /// `sh` is the probe target because POSIX guarantees it at an absolute path on every
    /// machine this runs on, so the test asserts the contract rather than the environment.
    #[tokio::test]
    async fn resolve_cli_returns_an_absolute_existing_path() {
        let found = resolve_cli("sh").await.expect("sh must resolve");
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
