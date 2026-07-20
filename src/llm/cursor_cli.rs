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
//! | `--allowed-tools ""` | no tools at all; -11,000 input tokens |
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
#[cfg(unix)]
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
    let root = std::env::temp_dir().join("meridian-cursor-home");

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
/// `--setting-sources ""` suppresses. `std::env::temp_dir()` sits outside the home tree on macOS
/// (`$TMPDIR` -> `/var/folders/…`) and Linux (`/tmp`), so no such ancestor can exist. An empty
/// workspace also leaves the agent nothing to read - every input we need is already in the prompt.
pub fn neutral_workspace() -> PathBuf {
    let dir = std::env::temp_dir().join("meridian-llm-workspace");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, path = %dir.display(), "cursor: neutral workspace mkdir failed");
    }
    dir
}

/// The safety flags every Meridian `cursor-agent` call carries, in argv order.
///
/// `no_tools` adds `--allowed-tools ""` (an EMPTY allowlist = no tools at all). That flag is
/// UNDOCUMENTED - hidden from `--help`, marked "internal only" in the CLI bundle - so callers
/// must be able to retry without it; see [`looks_unsupported_flag`].
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
        args.push("--allowed-tools".into());
        args.push(String::new());
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
        env.push(("HOME", home.to_string_lossy().into_owned()));
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The sandbox must never be the user's real home - that is the whole safety property.
    #[test]
    fn sandbox_is_outside_the_real_home() {
        let real = meridian_core::paths::home_dir_or_cwd();
        let sandbox = std::env::temp_dir().join("meridian-cursor-home");
        assert!(
            !sandbox.starts_with(&real),
            "sandbox must not live under $HOME"
        );
        assert!(
            !neutral_workspace().starts_with(&real),
            "workspace must not live under $HOME"
        );
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
