//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The single source of truth for **how Meridian invokes `cursor-agent`**.
//!
//! Two call sites shell out to this CLI - the LLM provider backend ([`crate::llm::cursor`],
//! used by every AI process when Cursor is the selected provider) and the coding-agent
//! summariser ([`crate::coding_agent_session_ingest::summariser::cursor_agent`]). They differ
//! only in output format and parsing, so everything *else* - the safety flags, the environment,
//! and the sandbox - lives here. Adding a hardening flag in one place must not leave the other
//! call site running unprotected, which is exactly what happened before this module existed.
//!
//! # What the hardening is for
//!
//! Every Meridian AI call is pure inference (summarise / classify / draft) over **untrusted**
//! text - coding-agent transcripts and screen OCR that Meridian did not author. `cursor-agent`
//! is a coding agent that defaults to full write + shell access, so the defaults are wrong for
//! us in both directions: too much capability, and a large irrelevant context.
//!
//! | Lever | Effect (measured on cursor-agent 2026.07.16) |
//! |---|---|
//! | `--allowed-tools=` | no tools at all; -11,000 input tokens |
//! | `--mode ask` | read-only, server-enforced; belt-and-braces with the empty allowlist |
//! | `--workspace <empty>` | stops Cursor walking UP into the user's `~/CLAUDE.md` / `AGENTS.md` |
//! | [`sandbox_home`] | stops the user's ~190 skills being injected; -6,500 further tokens |
//! | `CURSOR_API_KEY` stripped | the user's subscription is used, never metered API billing |
//!
//! Net: ~21,000 -> ~3,400 input tokens per call, with summary quality unchanged.
//!
//! # Related
//! - [`crate::llm::cursor`] - JSON-envelope backend for all AI processes
//! - [`crate::coding_agent_session_ingest::summariser::cursor_agent`] - text-output summariser
//! - [`meridian_core::CURSOR_CLI_VERSION`] - the pinned build these flags are verified against

use std::path::{Path, PathBuf};

/// Skill roots `cursor-agent` discovers relative to `$HOME`, as directory names to neutralise.
///
/// `.cursor` is rebuilt selectively (it also holds auth/config we must keep); the rest are
/// simply never created in the sandbox. Sourced from the CLI bundle's own discovery patterns:
/// `.cursor/skills`, `.cursor/skills-cursor`, `.claude/skills`, `.codex/skills`,
/// `.agents/skills`, plus plugin-provided skills under `.cursor/plugins`.
const SKILL_ROOT_DIRS: &[&str] = &[".claude", ".codex", ".agents"];

/// Entries inside `.cursor` that carry skills. Recreated EMPTY and READ-ONLY rather than just
/// omitted: `cursor-agent` self-provisions its built-in skills and plugin cache on startup, so
/// an absent directory is simply recreated and repopulated. A present-but-unwritable one is not.
const CURSOR_SKILL_ENTRIES: &[&str] = &["skills", "skills-cursor", "plugins", "agents"];

/// Directory mode for the neutralised skill dirs: readable and traversable, NOT writable.
///
/// Defined on every platform even though only unix consumes it - [`restrict`] no-ops elsewhere,
/// and gating the const instead of its use broke the Windows build (the call site is
/// unconditional).
#[cfg_attr(not(unix), allow(dead_code))]
const READ_ONLY_DIR_MODE: u32 = 0o555;

/// Build (idempotently) a private `$HOME` for `cursor-agent` that contains the user's real
/// auth/config but NO skills, and return it.
///
/// # Why a sandbox HOME
///
/// Cursor resolves skills from `homedir()`, and there is no flag to disable them
/// (`--exclude-workspace-context` exists but the server refuses it for ordinary accounts).
/// Pointing `HOME` at a directory that mirrors everything EXCEPT the skill roots is the only
/// mechanism that works, and it cuts ~6,500 tokens of skill listings off every call.
///
/// # This never touches the user's real files
///
/// Everything created lives under this sandbox. The user's `~/.cursor/skills-cursor`,
/// `~/.claude/skills` and friends are neither written, moved, nor deleted - they are simply not
/// linked in. The override applies only to the `cursor-agent` subprocess Meridian spawns, so the
/// user's own Cursor and Claude sessions are unaffected.
///
/// # Layout
///
/// - every top-level entry of the real `$HOME` is symlinked, EXCEPT [`SKILL_ROOT_DIRS`] and
///   `.cursor` - mirroring wholesale means auth keeps working without this code needing to know
///   where each platform stores it
/// - `.cursor` is a real directory whose children are symlinked individually, minus
///   [`CURSOR_SKILL_ENTRIES`]
/// - those entries are then created empty and read-only, so the CLI cannot repopulate them
///
/// Returns `None` if the sandbox cannot be built; callers must then fall back to the inherited
/// `HOME` (a fatter call, never a broken one).
pub fn sandbox_home() -> Option<PathBuf> {
    let real = meridian_core::paths::home_dir_or_cwd();
    let root = scratch_root("meridian-cursor-home");

    if let Err(e) = build_sandbox(&real, &root) {
        tracing::warn!(
            error = %e,
            path = %root.display(),
            "cursor: could not build the skills-free sandbox HOME - falling back to the real HOME \
             (the call still works, it just carries the user's skill list)"
        );
        return None;
    }
    tracing::debug!(path = %root.display(), "cursor: using skills-free sandbox HOME");
    Some(root)
}

/// Create or refresh the sandbox. Idempotent: safe to call on every invocation, and it repairs
/// itself if the OS reaped the temp directory between calls.
fn build_sandbox(real: &Path, root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    restrict(root, 0o700)?;

    // Mirror the real HOME, minus the skill roots and .cursor (rebuilt below).
    for entry in std::fs::read_dir(real)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str == ".cursor" || SKILL_ROOT_DIRS.contains(&name_str) {
            continue;
        }
        link(&entry.path(), &root.join(name_str));
    }

    // .cursor: keep auth/config, drop the skill-bearing entries.
    let cursor_dst = root.join(".cursor");
    std::fs::create_dir_all(&cursor_dst)?;
    let cursor_src = real.join(".cursor");
    if cursor_src.is_dir() {
        for entry in std::fs::read_dir(&cursor_src)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if CURSOR_SKILL_ENTRIES.contains(&name_str) {
                continue;
            }
            link(&entry.path(), &cursor_dst.join(name_str));
        }
    }

    // Neutralise the skill entries: present (so they are not recreated) but unwritable (so they
    // cannot be repopulated). Order matters - create while still writable, then restrict.
    for name in CURSOR_SKILL_ENTRIES {
        let dir = cursor_dst.join(name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = restrict(&dir, READ_ONLY_DIR_MODE);
    }
    Ok(())
}

/// Symlink `src` to `dst`, ignoring an existing link (the sandbox is long-lived and rebuilt on
/// every call; a present link is the steady state, not an error).
fn link(src: &Path, dst: &Path) {
    if dst.exists() || dst.is_symlink() {
        return;
    }
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(src, dst);
    #[cfg(windows)]
    let r = if src.is_dir() {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    };
    if let Err(e) = r {
        tracing::trace!(error = %e, src = %src.display(), "cursor: sandbox link skipped");
    }
}

/// Set a directory's permission bits. No-op off unix.
#[allow(unused_variables)]
fn restrict(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// An empty, Meridian-owned directory OUTSIDE `$HOME`, used as `--workspace` for every call.
///
/// Cursor resolves "always applied workspace rules" by walking **UP** from the workspace, so a
/// workspace anywhere under the user's home silently injects their global `~/CLAUDE.md` /
/// `AGENTS.md` / `.cursorrules` - measured: the entire file arrives as
/// `<always_applied_workspace_rules>`. That is coding-assistant instruction text contaminating a
/// pure summarise/classify call, and this is Cursor's equivalent of what Claude's
/// `--setting-sources ""` suppresses. [`scratch_root`] guarantees no such ancestor can exist. An
/// empty workspace also leaves the agent nothing to read - every input we need is already in the
/// prompt.
pub fn neutral_workspace() -> PathBuf {
    let dir = scratch_root("meridian-llm-workspace");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, path = %dir.display(), "cursor: neutral workspace mkdir failed");
    }
    dir
}

/// A Meridian-owned scratch directory that is NOT under the user's home.
///
/// Both callers depend on that property for correctness, for different reasons: the workspace
/// because Cursor walks UP from it looking for rule files, and the sandbox because a `HOME`
/// nested inside the real one mirrors itself.
///
/// `std::env::temp_dir()` satisfies it on macOS (`$TMPDIR` -> `/var/folders/…`) and Linux
/// (`/tmp`) but **not on Windows**, where it resolves to `%USERPROFILE%\AppData\Local\Temp` -
/// i.e. *inside* the home, so `~\CLAUDE.md` and `.cursorrules` would be back in scope. There we
/// prefer `%PUBLIC%` (`C:\Users\Public`), which is user-writable by default and a sibling of the
/// profile rather than a child.
///
/// If neither is outside the home the temp path is still returned - a fatter, rule-contaminated
/// call beats no call - but it is logged, because that silently weakens a guarantee the module
/// docs claim.
fn scratch_root(name: &str) -> PathBuf {
    let home = meridian_core::paths::home_dir_or_cwd();
    let temp = std::env::temp_dir().join(name);
    if !is_under(&temp, &home) {
        return temp;
    }
    if let Some(public) = std::env::var_os("PUBLIC").map(PathBuf::from) {
        let candidate = public.join(name);
        if !is_under(&candidate, &home) {
            return candidate;
        }
    }
    tracing::warn!(
        path = %temp.display(),
        home = %home.display(),
        "cursor: no scratch directory outside the home - workspace rules may be injected"
    );
    temp
}

/// Is `path` inside `ancestor`? Both are canonicalised where they exist, so a symlinked or
/// short-name (`C:\Users\RUNNER~1`) temp dir cannot make an inside path look outside - which is
/// exactly how a naive check passes on CI for the wrong reason.
fn is_under(path: &Path, ancestor: &Path) -> bool {
    let real = |p: &Path| {
        std::fs::canonicalize(p).unwrap_or_else(|_| {
            // Not created yet: canonicalise the nearest existing parent and re-join.
            match p.parent() {
                Some(parent) => std::fs::canonicalize(parent)
                    .map(|c| match p.file_name() {
                        Some(n) => c.join(n),
                        None => c,
                    })
                    .unwrap_or_else(|_| p.to_path_buf()),
                None => p.to_path_buf(),
            }
        })
    };
    real(path).starts_with(real(ancestor))
}

/// The safety flags every Meridian `cursor-agent` call carries, in argv order.
///
/// `no_tools` adds `--allowed-tools=` (an EMPTY allowlist = no tools at all). That flag is
/// UNDOCUMENTED - hidden from `--help`, marked "internal only" in the CLI bundle - so callers
/// must be able to retry without it; see [`looks_unsupported_flag`].
///
/// # Why `--allowed-tools=`, not `--allowed-tools` + `""` as two argv elements
///
/// It used to be two elements, which is correct on macOS/Linux but silently BREAKS on Windows:
/// `cursor-agent.cmd` (Cursor's own installer output, not npm's) forwards argv to node.exe via
/// `cursor-agent.ps1`'s bare `$args` - and Windows PowerShell 5.1's array-to-native-argv
/// flattening DROPS an empty-string element entirely (`$PSNativeCommandArgumentPassing` did not
/// exist in 5.1). The result: `node.exe` sees `--allowed-tools` immediately followed by
/// whatever the NEXT real argument was, so commander.js reports
/// `error: option '--allowed-tools <tool>' argument missing` — a CLI-parsing failure, not an
/// auth one. `CursorCallError`'s ladder doesn't recognise that message ([`looks_auth_failure`]/
/// [`looks_unsupported_flag`] both miss it), so it propagates as an opaque `Failed`, and the
/// tray's Cursor-specific UI (`ui/components/LlmProviderDetail.tsx`) shows EVERY unclassified
/// failure as "not signed in yet" — so a real, signed-in user saw a permanent, un-clearable
/// sign-in prompt. Reproduced live on Windows ARM64 both via a direct `cursor-agent.cmd`
/// invocation and confirmed fixed by switching to a single `--allowed-tools=` token, which
/// survives the flattening because it is never an empty array ELEMENT (the empty string lives
/// after the `=` inside one non-empty token). Standard long-option syntax, so this is not
/// platform-gated - it is correct (and unnecessary-but-harmless) on macOS/Linux too.
///
/// Callers append their own `--output-format` and any prompt/model arguments.
pub fn safety_args(model: &str, no_tools: bool) -> Vec<String> {
    let mut args = vec![
        // Read-only Q&A. Server-enforced: the request carries a system_reminder stating the
        // agent MUST NOT edit or run non-readonly tools, superseding other instructions.
        "--mode".into(),
        "ask".into(),
        // Required, or cursor-agent exits 1 with "Workspace Trust Required" even in print mode.
        // Safe here: the prompt is our own text and, with no tools, nothing can be executed.
        "--trust".into(),
        "--model".into(),
        model.to_string(),
        "--workspace".into(),
        neutral_workspace().to_string_lossy().into_owned(),
    ];
    if no_tools {
        args.push("--allowed-tools=".into());
    }
    args
}

/// Environment overrides for every call: mark the run as ours so the coding-agent indexer can
/// ignore it, opt out of telemetry, and point `HOME` at the skills-free sandbox.
///
/// `sandbox` is threaded in rather than resolved here so a caller can retry WITHOUT it (see
/// [`looks_auth_failure`]) and so tests can exercise both paths.
pub fn env_overrides(sandbox: Option<&Path>) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("MERIDIAN_SUMMARISER", "1".to_string()),
        // Cursor exposes no per-call telemetry flag - the no-training guarantee is its
        // account-level Privacy Mode (a ZDR flag it sends upstream). This is the best-effort
        // standard opt-out we can control; the UI points users at Privacy Mode for the real
        // training/retention guarantee.
        (
            crate::llm::DO_NOT_TRACK.0,
            crate::llm::DO_NOT_TRACK.1.into(),
        ),
    ];
    if let Some(home) = sandbox {
        let home = home.to_string_lossy().into_owned();
        env.push(("HOME", home.clone()));
        // cursor-agent is a Node bundle, and Node's `os.homedir()` reads `USERPROFILE` on
        // Windows - `HOME` alone moves nothing there, so the skills would come straight back.
        // Harmless on unix, where nothing reads it.
        env.push(("USERPROFILE", home));
    }
    env
}

/// Environment variables removed from every call.
///
/// A stray `CURSOR_API_KEY` would silently move the user off their subscription onto metered
/// API billing. Mirrors `claude.rs` stripping `ANTHROPIC_API_KEY`.
pub const ENV_REMOVE: &[&str] = &["CURSOR_API_KEY"];

/// Did this CLI build reject one of our flags outright?
///
/// `--allowed-tools` is an undocumented internal option, so a future cursor-agent may drop it -
/// commander exits with `error: unknown option '--allowed-tools'`. Detecting that lets a caller
/// retry unhardened instead of failing every AI process on a CLI upgrade.
pub fn looks_unsupported_flag(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("unknown option") || m.contains("unknown argument") || m.contains("unrecognized")
}

/// Did the call fail because `cursor-agent` could not see its credentials?
///
/// The sandbox `HOME` mirrors the real one, but auth storage differs per platform and could move
/// between CLI builds. If it ever does, the symptom is an auth error - and the right response is
/// to retry on the inherited `HOME` (a fatter call) rather than leave every AI process broken.
pub fn looks_auth_failure(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("not logged in")
        || m.contains("unauthorized")
        || m.contains("unauthenticated")
        || m.contains("not authenticated")
        || m.contains("please log in")
        || m.contains("login required")
}

/// Did the CLI reject the model as unavailable on this account?
///
/// Cursor plans differ in model access, so a pinned id can simply be absent. Detecting that
/// lets the ladder fall back to [`FALLBACK_MODEL`] rather than failing every call.
pub fn looks_unknown_model(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("model")
        && (m.contains("unknown")
            || m.contains("not available")
            || m.contains("unavailable")
            || m.contains("invalid")
            || m.contains("not found")
            || m.contains("does not exist")
            || m.contains("no access"))
}

/// Did the call fail on a plain network/transport blip rather than anything about the
/// request itself - a connection dropping mid-stream, a reset, a 5xx from Cursor's own
/// backend? Worth exactly one bounded retry: unlike the other levers this isn't reacting to
/// a capability the CLI lacks, it's reacting to noise that a second attempt often doesn't
/// repeat.
///
/// Deliberately does NOT match our own client-side timeout (`"timed out after"`, the exact
/// wording `run_capture` uses - see `SummariserError::Failed`). Retrying that would double
/// the worst-case wait on an already-slow call, which is the opposite of what this exists
/// for; a real timeout should fail promptly and let the caller decide, not silently cost the
/// user a second full timeout window.
pub fn looks_transient(msg: &str) -> bool {
    let m = msg.to_lowercase();
    if m.contains("timed out") {
        return false;
    }
    m.contains("connection reset")
        || m.contains("econnreset")
        || m.contains("stream disconnected")
        || m.contains("socket hang up")
        || m.contains("broken pipe")
        || m.contains("temporarily unavailable")
        || m.contains("502 bad gateway")
        || m.contains("503 service unavailable")
        || m.contains("504 gateway")
}

/// Pinned model for every Meridian `cursor-agent` call: small, fast, cheap and ZDR-eligible
/// (Cursor flags non-ZDR models explicitly in `--list-models`; this is not one) - the right
/// tier for summarise/classify/draft, and deterministic across users unlike the account
/// default `auto`, which is server-routed and varies per call AND per account.
///
/// `-medium` is the effort suffix Cursor displays as plain "GPT-5.4 Mini"; the ladder is
/// `-none/-low/-medium/-high/-xhigh`.
pub const DEFAULT_MODEL: &str = "gpt-5.4-mini-medium";

/// Same pinned model, lower reasoning effort — used only for [`PromptRequest::interactive`]
/// calls that have no explicit user model override (see `CursorBackend::complete`).
///
/// A synchronously-watched call (today, only the task composer's "Draft with AI") pays for
/// `-medium`'s extra reasoning time in perceived latency, not in a better answer: the job is
/// a three-field JSON extraction from a short note, not analysis. Background jobs
/// (summarisation, worklog generation, classification) keep [`DEFAULT_MODEL`] - they run
/// unattended, so slower-but-more-careful is free there and NOT free here.
///
/// [`PromptRequest::interactive`]: super::PromptRequest::interactive
pub const FAST_MODEL: &str = "gpt-5.4-mini-low";

/// Cursor's server-routed default, used only when [`DEFAULT_MODEL`] is not on the account.
pub const FALLBACK_MODEL: &str = "auto";

/// A failure a [`run_hardened`] caller can classify.
///
/// Implemented by both call sites' error types so the ladder is written once. Returning
/// `None` means "never degrade past this" - it is what keeps a rate limit from being
/// mistaken for a broken install and burning a second attempt on an account-level wall.
pub trait CursorCallError {
    /// The failure text to classify, or `None` when this failure must not be retried.
    fn degradable_message(&self) -> Option<&str>;
}

/// Which levers are still engaged. Each is dropped only in response to a DETECTED cause, so
/// a healthy install always takes the fully hardened path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Levers {
    no_tools: bool,
    sandbox: bool,
}

/// Run a `cursor-agent` call, degrading one lever at a time when - and only when - the CLI
/// says so.
///
/// `run` receives the safety argv and the environment for one attempt and performs the actual
/// spawn/parse; everything about *which* flags to use and *when* to back off lives here. That
/// is the whole point: before this existed the provider backend had the recovery ladder and
/// the summariser did not, so the day Cursor drops the undocumented `--allowed-tools` the
/// backend would have degraded gracefully while every coding-agent summary failed.
///
/// The ladder, each step gated on a detected cause:
/// 1. `--allowed-tools` rejected -> retry without it (fatter context, still works)
/// 2. auth invisible from the sandbox `HOME` -> retry on the inherited one (carries skills)
/// 3. model unavailable on this plan -> retry on [`FALLBACK_MODEL`]
/// 4. a plain transient/network blip -> retry once, unchanged (see [`looks_transient`])
///
/// A rate limit degrades nothing: `degradable_message` returns `None` and the error is
/// returned immediately, because Cursor limits are account-level and a second call would
/// spend quota to hit the same wall.
pub async fn run_hardened<T, E, F, Fut>(model: &str, mut run: F) -> Result<T, E>
where
    E: CursorCallError,
    F: FnMut(Vec<String>, Vec<(&'static str, String)>) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut levers = Levers {
        no_tools: true,
        sandbox: true,
    };
    let mut model = model.to_string();
    let mut tried_fallback_model = model == FALLBACK_MODEL;
    // Not a lever - retrying leaves argv/env unchanged, it just asks the same question
    // again in case the first answer was noise. Bounded to one shot so a genuinely broken
    // endpoint still fails promptly rather than doubling every call's worst case.
    let mut tried_transient_retry = false;

    loop {
        let sandbox = if levers.sandbox { sandbox_home() } else { None };
        let err = match run(
            safety_args(&model, levers.no_tools),
            env_overrides(sandbox.as_deref()),
        )
        .await
        {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };

        // `None` = not classifiable / must not be retried (a rate limit). Stop here.
        let Some(msg) = err.degradable_message() else {
            return Err(err);
        };

        if levers.no_tools && looks_unsupported_flag(msg) {
            tracing::warn!(
                "cursor: this CLI build rejects --allowed-tools - retrying without it \
                 (call still works, just carries the full tool context)"
            );
            levers.no_tools = false;
        } else if levers.sandbox && looks_auth_failure(msg) {
            tracing::warn!(
                "cursor: credentials not visible from the sandbox HOME - retrying on the \
                 real HOME (call still works, just carries the user's skill list)"
            );
            levers.sandbox = false;
        } else if !tried_fallback_model && looks_unknown_model(msg) {
            tracing::warn!(
                model = %model,
                fallback = FALLBACK_MODEL,
                "cursor: model unavailable on this account - retrying with auto"
            );
            model = FALLBACK_MODEL.to_string();
            tried_fallback_model = true;
        } else if !tried_transient_retry && looks_transient(msg) {
            tracing::warn!(
                error = %msg,
                "cursor: transient failure - retrying once unchanged"
            );
            tried_transient_retry = true;
        } else {
            // Nothing left to degrade, or a cause no lever addresses.
            return Err(err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the real error types, so the ladder can be exercised without spawning
    /// anything. `None` models the rate-limit case.
    struct TestErr(Option<String>);
    impl CursorCallError for TestErr {
        fn degradable_message(&self) -> Option<&str> {
            self.0.as_deref()
        }
    }

    /// Drive `run_hardened` with a canned failure for the first `fail_n` attempts, recording
    /// the argv/env of every attempt.
    async fn ladder(
        model: &str,
        errs: Vec<Option<&'static str>>,
    ) -> (
        Result<usize, TestErr>,
        Vec<(Vec<String>, Vec<(&'static str, String)>)>,
    ) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let s2 = seen.clone();
        let res = run_hardened(model, move |args, env| {
            let s2 = s2.clone();
            let calls = calls.clone();
            let errs = errs.clone();
            async move {
                let n = {
                    let mut c = calls.lock().unwrap();
                    *c += 1;
                    *c - 1
                };
                s2.lock().unwrap().push((args, env));
                match errs.get(n) {
                    Some(e) => Err(TestErr(e.map(str::to_string))),
                    None => Ok(n),
                }
            }
        })
        .await;
        let seen = seen.lock().unwrap().clone();
        (res, seen)
    }

    #[tokio::test]
    async fn a_healthy_call_takes_the_fully_hardened_path_and_never_retries() {
        let (res, seen) = ladder(DEFAULT_MODEL, vec![]).await;
        assert!(res.is_ok());
        assert_eq!(seen.len(), 1, "no degradation without a detected cause");
        assert!(seen[0].0.contains(&"--allowed-tools=".to_string()));
        assert!(seen[0].1.iter().any(|(k, _)| *k == "HOME"));
    }

    /// THE regression this module exists for: the recovery must be shared, so an unsupported
    /// flag degrades rather than failing every call on a CLI upgrade.
    #[tokio::test]
    async fn an_unknown_option_drops_the_tool_flag_and_retries() {
        let (res, seen) = ladder(
            DEFAULT_MODEL,
            vec![Some("error: unknown option '--allowed-tools'")],
        )
        .await;
        assert!(res.is_ok());
        assert_eq!(seen.len(), 2);
        assert!(!seen[1].0.contains(&"--allowed-tools".to_string()));
    }

    #[tokio::test]
    async fn an_auth_failure_drops_the_sandbox_home() {
        let (res, seen) = ladder(DEFAULT_MODEL, vec![Some("Not logged in")]).await;
        assert!(res.is_ok());
        assert_eq!(seen.len(), 2);
        assert!(
            !seen[1].1.iter().any(|(k, _)| *k == "HOME"),
            "the retry must inherit the real HOME"
        );
    }

    #[tokio::test]
    async fn an_unavailable_model_falls_back_to_auto() {
        let (res, seen) = ladder(DEFAULT_MODEL, vec![Some("Unknown model: gpt-5.4-mini")]).await;
        assert!(res.is_ok());
        assert_eq!(seen.len(), 2);
        assert!(seen[1].0.contains(&FALLBACK_MODEL.to_string()));
    }

    /// A rate limit must NOT walk the ladder - Cursor limits are account-level, so a retry
    /// spends quota to hit the same wall.
    #[tokio::test]
    async fn a_rate_limit_returns_immediately_without_degrading() {
        let (res, seen) = ladder(DEFAULT_MODEL, vec![None, None, None, None]).await;
        assert!(res.is_err());
        assert_eq!(seen.len(), 1, "a rate limit degrades nothing");
    }

    /// An unrelated failure is not a licence to strip hardening.
    #[tokio::test]
    async fn an_unrecognised_failure_does_not_degrade() {
        let (res, seen) = ladder(DEFAULT_MODEL, vec![Some("internal assertion failed")]).await;
        assert!(res.is_err());
        assert_eq!(seen.len(), 1);
    }

    /// THE regression this lever exists for: a plain network blip - noise, not a capability
    /// the CLI lacks - gets one retry with the argv/env completely unchanged.
    #[tokio::test]
    async fn a_transient_failure_retries_once_unchanged() {
        let (res, seen) = ladder(DEFAULT_MODEL, vec![Some("connection reset by peer")]).await;
        assert!(res.is_ok());
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0], seen[1],
            "a transient retry must not change argv/env - it isn't a lever"
        );
    }

    /// Bounded: a second transient failure in a row is not a licence to keep spinning.
    #[tokio::test]
    async fn a_second_transient_failure_is_not_retried_again() {
        let errs = vec![
            Some("connection reset by peer"),
            Some("stream disconnected"),
        ];
        let (res, seen) = ladder(DEFAULT_MODEL, errs).await;
        assert!(res.is_err());
        assert_eq!(seen.len(), 2, "one retry, then give up");
    }

    /// THE regression the exclusion exists for: retrying our own client-side timeout would
    /// double the worst-case wait on an already-slow call, which is what this lever must never
    /// do. `run_capture`'s exact wording (`SummariserError::Failed`) is `"<program> timed out
    /// after <n>s"`.
    #[tokio::test]
    async fn a_timeout_shaped_message_is_not_retried() {
        let (res, seen) = ladder(
            DEFAULT_MODEL,
            vec![Some("cursor-agent timed out after 300s")],
        )
        .await;
        assert!(res.is_err());
        assert_eq!(
            seen.len(),
            1,
            "a timeout must fail promptly, not double the wait"
        );
    }

    /// Each lever drops at most once, so a persistently failing CLI terminates instead of
    /// looping.
    #[tokio::test]
    async fn the_ladder_terminates_after_every_lever_is_spent() {
        let errs = vec![
            Some("unknown option '--allowed-tools'"),
            Some("Not logged in"),
            Some("Unknown model: x"),
            Some("unknown option '--allowed-tools'"),
        ];
        let (res, seen) = ladder(DEFAULT_MODEL, errs).await;
        assert!(res.is_err());
        assert_eq!(seen.len(), 4, "3 degradations, then give up");
    }

    /// Starting on `auto` means there is no model left to fall back to.
    #[tokio::test]
    async fn auto_does_not_fall_back_to_itself() {
        let (res, seen) = ladder(FALLBACK_MODEL, vec![Some("Unknown model: auto")]).await;
        assert!(res.is_err());
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn safety_args_carry_the_read_only_and_no_tool_levers() {
        let a = safety_args("some-model", true).join(" ");
        assert!(a.contains("--mode ask"), "read-only mode is not optional");
        assert!(a.contains("--trust"), "headless runs die without --trust");
        assert!(a.contains("--model some-model"));
        assert!(
            a.contains("--workspace"),
            "workspace pin stops rule injection"
        );
        assert!(a.contains("--allowed-tools"), "empty allowlist = no tools");
    }

    #[test]
    fn safety_args_can_drop_only_the_undocumented_flag() {
        // The retry path: everything else must survive so a CLI that lost --allowed-tools is
        // still read-only and still free of workspace rules.
        let a = safety_args("m", false).join(" ");
        assert!(!a.contains("--allowed-tools"));
        assert!(a.contains("--mode ask"));
        assert!(a.contains("--workspace"));
    }

    /// The regression this exists for: on Windows, `cursor-agent.cmd` -> `cursor-agent.ps1`
    /// forwards argv via PowerShell 5.1's bare `$args`, which silently DROPS a standalone
    /// empty-string array element when flattening it onto node.exe's command line. Two argv
    /// elements (`"--allowed-tools"`, `""`) used to reach `safety_args`' caller; the second
    /// vanished in transit, so cursor-agent saw `--allowed-tools` with nothing after it and
    /// exited `error: option '--allowed-tools <tool>' argument missing` - a CLI-parsing
    /// failure that `CursorCallError`'s ladder does not recognise, so it surfaced to the user
    /// as a permanent "not signed in yet" (the tray's Cursor-specific catch-all for any
    /// unclassified failure). Reproduced live on Windows ARM64 and confirmed fixed by using a
    /// single combined `--allowed-tools=` token instead - this pins that it stays that way.
    #[test]
    fn no_tools_never_emits_a_standalone_empty_argv_element() {
        let args = safety_args("some-model", true);
        assert!(
            !args.iter().any(|a| a.is_empty()),
            "an empty-string element is dropped by Windows PowerShell 5.1's $args \
             flattening in cursor-agent's own Windows wrapper: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--allowed-tools="),
            "expected a single combined '--allowed-tools=' token: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--allowed-tools"),
            "must not also emit the flag as a separate element: {args:?}"
        );
    }

    #[test]
    fn env_overrides_track_the_sandbox_and_always_opt_out_of_telemetry() {
        let without = env_overrides(None);
        assert!(without.iter().all(|(k, _)| *k != "HOME"));
        assert!(without.iter().any(|(k, _)| *k == "DO_NOT_TRACK"));
        assert!(without.iter().any(|(k, _)| *k == "MERIDIAN_SUMMARISER"));

        let with = env_overrides(Some(Path::new("/tmp/x")));
        assert_eq!(
            with.iter()
                .find(|(k, _)| *k == "HOME")
                .map(|(_, v)| v.as_str()),
            Some("/tmp/x")
        );
    }

    #[test]
    fn api_key_is_always_stripped() {
        assert!(ENV_REMOVE.contains(&"CURSOR_API_KEY"));
    }

    #[test]
    fn unsupported_flag_detection_drives_the_unhardened_retry() {
        assert!(looks_unsupported_flag(
            "cursor-agent exited Some(1): error: unknown option '--allowed-tools'"
        ));
        // Ordinary failures must NOT silently drop the hardening.
        assert!(!looks_unsupported_flag("rate limit reached"));
        assert!(!looks_unsupported_flag(
            "cursor-agent returned an empty answer"
        ));
    }

    #[test]
    fn auth_failure_detection_drives_the_real_home_retry() {
        assert!(looks_auth_failure("Not logged in"));
        assert!(looks_auth_failure(
            "cursor-agent exited Some(1): unauthorized"
        ));
        // Must not mistake a quota problem for an auth problem - that retry would waste a call.
        assert!(!looks_auth_failure("rate/usage limit reached"));
        assert!(!looks_auth_failure("workspace trust required"));
    }

    #[test]
    fn transient_failure_detection_drives_the_bounded_retry() {
        assert!(looks_transient("connection reset by peer"));
        assert!(looks_transient("Error: socket hang up"));
        assert!(looks_transient(
            "cursor-agent exited Some(1): 503 Service Unavailable"
        ));
        // Must not mistake our own client-side timeout for noise worth retrying - see the
        // doc comment on `looks_transient` for why that distinction matters.
        assert!(!looks_transient("cursor-agent timed out after 300s"));
        assert!(!looks_transient("Error: request timed out after retries"));
        // Ordinary failures must not spend the one bounded retry either.
        assert!(!looks_transient("rate/usage limit reached"));
        assert!(!looks_transient("Not logged in"));
    }

    /// The sandbox must never be the user's real home - that is the whole safety property.
    #[test]
    fn sandbox_is_outside_the_real_home() {
        let real = meridian_core::paths::home_dir_or_cwd();
        // Assert on what the PRODUCT computes, via the same containment check it uses.
        // Re-deriving the path here (`temp_dir().join(...)`) and comparing with a raw
        // `starts_with` is how this passes for the wrong reason: on Windows CI the temp dir is
        // an 8.3 short name (`C:\Users\RUNNER~1\…`) that never string-matches the home even
        // though it is inside it.
        assert!(
            !is_under(&scratch_root("meridian-cursor-home"), &real),
            "sandbox must not live under $HOME - a nested sandbox mirrors itself"
        );
        assert!(
            !is_under(&neutral_workspace(), &real),
            "workspace must not live under $HOME - Cursor walks UP from it for rule files"
        );
    }

    /// The containment check must see through symlinks and short names, or the guarantee above
    /// is only as good as the string comparison.
    #[test]
    fn containment_resolves_paths_rather_than_comparing_strings() {
        let home = meridian_core::paths::home_dir_or_cwd();
        assert!(is_under(&home.join("some-child"), &home));
        assert!(!is_under(Path::new("/"), &home));
        // The macOS case that motivated canonicalising: /tmp is a symlink to /private/tmp.
        #[cfg(target_os = "macos")]
        assert!(is_under(Path::new("/tmp/x"), Path::new("/private/tmp")));
    }

    /// The sandbox is useless on Windows without this: Node reads `USERPROFILE`, not `HOME`.
    #[test]
    fn the_sandbox_overrides_both_home_variables() {
        let env = env_overrides(Some(Path::new("/tmp/sbx")));
        let home = env.iter().find(|(k, _)| *k == "HOME");
        let profile = env.iter().find(|(k, _)| *k == "USERPROFILE");
        assert_eq!(home.map(|(_, v)| v.as_str()), Some("/tmp/sbx"));
        assert_eq!(profile.map(|(_, v)| v.as_str()), Some("/tmp/sbx"));
        // No sandbox = inherit both, so the fallback path really is the user's real home.
        let plain = env_overrides(None);
        assert!(!plain
            .iter()
            .any(|(k, _)| *k == "HOME" || *k == "USERPROFILE"));
    }

    /// Building it twice must be a no-op, and it must contain neutralised skill dirs.
    #[test]
    fn build_sandbox_is_idempotent_and_neutralises_skill_dirs() {
        let tmp = std::env::temp_dir().join(format!("meridian-sbx-test-{}", std::process::id()));
        let fake_home = tmp.join("home");
        std::fs::create_dir_all(fake_home.join(".cursor").join("skills-cursor")).unwrap();
        std::fs::create_dir_all(fake_home.join(".claude").join("skills")).unwrap();
        std::fs::write(fake_home.join(".cursor").join("cli-config.json"), "{}").unwrap();
        let root = tmp.join("sandbox");

        build_sandbox(&fake_home, &root).unwrap();
        build_sandbox(&fake_home, &root).unwrap(); // idempotent

        // Auth/config carried over, skill roots not linked, skill entries present-but-empty.
        assert!(root.join(".cursor").join("cli-config.json").exists());
        assert!(
            !root.join(".claude").exists(),
            "third-party skill root must not be linked"
        );
        let skills = root.join(".cursor").join("skills-cursor");
        assert!(skills.is_dir(), "must exist so the CLI cannot recreate it");
        assert_eq!(
            std::fs::read_dir(&skills).unwrap().count(),
            0,
            "must be empty"
        );

        // Clean up (restore write bits first - we deliberately made them read-only).
        for name in CURSOR_SKILL_ENTRIES {
            let _ = restrict(&root.join(".cursor").join(name), 0o755);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
