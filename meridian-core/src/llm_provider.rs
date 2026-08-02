//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Which AI runs the user's pipeline — the single, centralised provider choice.
//!
//! The user picks this once during setup (and can change it in Settings): their own
//! coding-agent CLI subscription (Claude / Codex / Cursor / Copilot) or a
//! [`LlmProvider::Custom`] cloud endpoint they configured themselves. One enum, one
//! setting, one factory — every prose LLM call obeys it. (There is no on-device model:
//! the daemon runs generation only through third-party providers.)
//!
//! # Why `Custom` carries no data
//!
//! A custom provider has an identity (vendor, base URL, API key, model) — but putting it
//! in the variant would cost this enum its `Copy`, its `as_str() -> &'static str`, and its
//! fixed-size [`LlmProvider::all`], across every use site. Worse, there can be SEVERAL
//! configured at once, which one variant cannot express.
//!
//! So the variant is a unit marker meaning "a custom endpoint", and the instances live in
//! a registry in `crate::settings` keyed by id. The stored setting names which one is
//! active; the wire form `custom:<id>` addresses one directly (the LLM Lab runs several
//! side by side). This enum stays the *kind* of provider, never the instance.
//!
//! # Why the setting is a `String`, not this enum
//!
//! [`crate::settings::load_runtime_settings`] falls back to `RuntimeSettings::default()`
//! on *any* deserialise error, so one unparseable field silently resets **every** setting
//! the daemon reads — log level, poll interval, work hours. If `llm_provider` were typed
//! as this enum and a newer build wrote a variant an older daemon didn't know, that
//! daemon wouldn't merely lose the provider: it would lose the user's work hours too.
//!
//! So the field is stored as a `String` and parsed here with [`LlmProvider::from_wire`],
//! which returns `None` for anything unrecognised. The caller falls back to the default.
//! An unknown provider costs you the provider, and nothing else.
//!
//! # Who calls this
//! `crate::settings` (the stored field), the daemon's `llm::resolver` (the one place that
//! turns the choice into a live backend), and the tray's `commands::settings` (validation
//! on write). The UI mirrors the wire forms in `ui/lib/llm-providers.ts`.
//!
//! # Related
//! [`crate::canonical_task::Provider`] — the same shape for PM trackers, and the pattern
//! this follows.

use serde::{Deserialize, Serialize};

/// The AI backend that runs the user's prose LLM calls.
///
/// Wire forms deliberately match the coding-agent summariser's own `Source::as_str()`
/// (`"claude"`, `"codex"`, `"cursor"`, `"copilot"`), so the two can be mapped without a
/// translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    Claude,
    Codex,
    Cursor,
    Copilot,
    /// A user-configured cloud endpoint (OpenAI-compatible), identified by a registry row
    /// rather than by this variant — see the module docs. The only provider that is not a
    /// CLI subprocess: a direct HTTP call on the user's own API key, and the only one that
    /// spends metered money rather than a flat subscription.
    Custom,
}

impl Default for LlmProvider {
    /// Claude Code — the flagship CLI provider, and the friendliest default when one is
    /// installed. If no provider is configured/installed, the LLM-driven steps simply idle
    /// (there is no on-device fallback), which is an accepted, non-fatal state.
    fn default() -> Self {
        Self::Claude
    }
}

impl LlmProvider {
    /// The snake_case string form — equals the serde wire form and the settings value.
    pub fn as_str(self) -> &'static str {
        match self {
            LlmProvider::Claude => "claude",
            LlmProvider::Codex => "codex",
            LlmProvider::Cursor => "cursor",
            LlmProvider::Copilot => "copilot",
            LlmProvider::Custom => "custom",
        }
    }

    /// Parse a stored settings value. `None` for anything unrecognised — never panics,
    /// never errors the whole settings load. See the module docs for why that matters.
    ///
    /// `"custom"` parses to [`LlmProvider::Custom`] — the *kind*. WHICH custom endpoint is
    /// a separate stored id, because this type cannot carry it.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "claude" => Some(LlmProvider::Claude),
            "codex" => Some(LlmProvider::Codex),
            "cursor" => Some(LlmProvider::Cursor),
            "copilot" => Some(LlmProvider::Copilot),
            "custom" => Some(LlmProvider::Custom),
            _ => None,
        }
    }

    /// Every variant — the set a stored `llm_provider` is validated against.
    ///
    /// NOT the set of cards to render: [`LlmProvider::Custom`] is a kind, not an instance,
    /// so it has no card of its own (its cards come one-per-registry-row). Use
    /// [`LlmProvider::builtins`] to enumerate the providers that ARE a single fixed thing.
    pub fn all() -> [LlmProvider; 5] {
        [
            LlmProvider::Claude,
            LlmProvider::Codex,
            LlmProvider::Cursor,
            LlmProvider::Copilot,
            LlmProvider::Custom,
        ]
    }

    /// The providers that are one fixed, self-identifying thing — everything except
    /// [`LlmProvider::Custom`], whose instances live in the settings registry.
    ///
    /// This is what enumerates install probes and picker cards.
    pub fn builtins() -> [LlmProvider; 4] {
        [
            LlmProvider::Claude,
            LlmProvider::Codex,
            LlmProvider::Cursor,
            LlmProvider::Copilot,
        ]
    }

    /// The executable to invoke, or `None` for a provider that is an HTTP call rather than
    /// a subprocess. The single place the binary names live.
    ///
    /// `None` means "not a subprocess" — today only [`LlmProvider::Custom`] (a cloud
    /// endpoint) answers `None`.
    pub fn cli_name(self) -> Option<&'static str> {
        match self {
            LlmProvider::Claude => Some("claude"),
            LlmProvider::Codex => Some("codex"),
            LlmProvider::Cursor => Some("cursor-agent"),
            LlmProvider::Copilot => Some("copilot"),
            LlmProvider::Custom => None,
        }
    }

    /// The shell command that installs this provider's CLI, or `None` when there is nothing
    /// to install ([`LlmProvider::Custom`] is a cloud endpoint, not a binary).
    ///
    /// Cursor's is PINNED - see [`CURSOR_CLI_VERSION`].
    ///
    /// This is the SAME command the UI shows as its `installHint` (`ui/lib/llm-providers.ts`)
    /// — kept here as the authoritative copy because the tray now runs it on the user's behalf
    /// (`crate::llm::detect::install_provider` in the daemon crate). The strings are fixed
    /// literals, never interpolated with user input, so running them through a login shell is
    /// safe. Each is the vendor's own official, non-interactive installer.
    ///
    /// Three of the four are PLATFORM-SPLIT constants rather than inline literals, because
    /// the command that works on macOS is not the command that works on Windows - see
    /// [`CLAUDE_INSTALL_CMD`], [`CODEX_INSTALL_CMD`] and [`CURSOR_INSTALL_CMD`]. Copilot is
    /// still npm-only: it has no native installer, and it is deliberately absent from the
    /// picker (`CHOOSER_PROVIDER_IDS` in `ui/lib/llm-providers.ts`), so nothing in the UI can
    /// reach this arm today.
    pub fn install_command(self) -> Option<&'static str> {
        match self {
            LlmProvider::Claude => Some(CLAUDE_INSTALL_CMD),
            LlmProvider::Codex => Some(CODEX_INSTALL_CMD),
            LlmProvider::Cursor => Some(CURSOR_INSTALL_CMD),
            LlmProvider::Copilot => Some("npm i -g @github/copilot"),
            LlmProvider::Custom => None,
        }
    }

    /// The exact CLI build [`Self::install_command`] is supposed to produce, when this provider
    /// is version-pinned.
    ///
    /// Only Cursor pins today, because its installer is a rolling script with no version flag
    /// and the pin is applied by rewriting it (see [`CURSOR_INSTALL_CMD`]) - a mechanism that
    /// fails OPEN. Returning the expectation here lets the installer verify that the rewrite
    /// actually took effect rather than assuming it did. The other CLIs install a named npm
    /// version range and need no such check.
    pub fn pinned_cli_version(self) -> Option<&'static str> {
        match self {
            LlmProvider::Cursor => Some(CURSOR_CLI_VERSION),
            _ => None,
        }
    }
}

/// The cursor-agent build Meridian installs and supports.
///
/// PINNED DELIBERATELY. `https://cursor.com/install` is a ROLLING script - it hardcodes
/// whatever build is current when you fetch it, with no version flag or env var. Two
/// reasons we don't want that:
///
/// 1. `crate::llm::cursor` depends on `--allowed-tools`, which is **undocumented**
///    ("internal only", hidden from `--help`). An unpinned upgrade could drop it
///    silently. (The backend degrades gracefully if that happens, but a pin means it
///    doesn't happen unattended in the first place.)
/// 2. Reproducibility: every user gets the build this was tested against, not whatever
///    shipped that morning.
///
/// This value is the build all of the flag behaviour was verified on. To move it: bump
/// this constant, re-run the cursor smoke tests, and confirm `--allowed-tools`,
/// `--workspace` and `--mode ask` still behave. Shared by both platform variants of
/// [`CURSOR_INSTALL_CMD`] below - `downloads.cursor.com/lab/<version>/<os>/<arch>/...`
/// keys the same version string across every OS, only the artifact differs.
pub const CURSOR_CLI_VERSION: &str = "2026.07.16-899851b";

/// Cursor's official installer, rewritten to install [`CURSOR_CLI_VERSION`].
///
/// # Unix
/// The `sed` rewrites every version string in their script (all 5 occurrences: the temp
/// dir, the `downloads.cursor.com/lab/<v>/<os>/<arch>/agent-cli-package.tar.gz` URL, the
/// final dir, and both symlinks) to the pinned build. Matching the version by PATTERN
/// rather than by literal is what makes this survive Cursor bumping their script - a
/// literal `s/<old>/<new>/` would silently become a no-op the day they publish a new
/// build, handing the user "latest" again, which is the exact failure being avoided.
///
/// Reusing their script (rather than hand-rolling a download) keeps their OS/arch
/// detection, symlink layout and PATH guidance. Fixed literal, no user input.
///
/// # Windows
/// `https://cursor.com/install` is a **bash** script - `uname -s` only ever answers
/// `Linux`/`Darwin`, so it exits "Unsupported operating system" under `sh`/`bash` on
/// Windows even when one is on PATH (Git-Bash, WSL's `bash.exe` shim), and there is no
/// `sed` either. Cursor publishes a SEPARATE PowerShell installer for Windows at
/// `https://cursor.com/install?win32=true` (same `downloads.cursor.com/lab/<version>/...`
/// backend, `windows/<arch>` instead of `<os>/<arch>`), pinned here the same way: fetch
/// the script text, rewrite the version with a .NET regex `-replace` (PowerShell's `sed`
/// equivalent - same pattern-not-literal reasoning as the Unix rewrite), then `iex`
/// (`Invoke-Expression`) the rewritten text. The daemon's `llm::detect::installer_command`
/// is what actually spawns this — it must run through `powershell.exe` directly, not
/// `cmd.exe`, because `irm`/`iex` are PowerShell aliases with no `cmd` equivalent.
#[cfg(not(windows))]
pub const CURSOR_INSTALL_CMD: &str = concat!(
    "curl -fsSL https://cursor.com/install | ",
    "sed -E 's#[0-9]{4}\\.[0-9]{2}\\.[0-9]{2}-[0-9a-f]{7}#",
    "2026.07.16-899851b",
    "#g' | bash"
);

/// See [`CURSOR_INSTALL_CMD`]'s Windows doc above. Plain PowerShell script text — no
/// enclosing `powershell -Command "..."` wrapper here, because the daemon's
/// `llm::detect::installer_command` supplies that (a single argv element handed straight
/// to `powershell.exe`, so there is exactly one layer of shell-quoting to reason about,
/// not `cmd.exe` re-parsing an already-quoted `-Command` payload).
#[cfg(windows)]
pub const CURSOR_INSTALL_CMD: &str = concat!(
    "$s = (irm 'https://cursor.com/install?win32=true'); ",
    "$s = $s -replace '[0-9]{4}\\.[0-9]{2}\\.[0-9]{2}-[0-9a-f]{7}', '",
    "2026.07.16-899851b",
    "'; iex $s"
);

/// Claude Code's installer.
///
/// # Unix
/// npm, unchanged - the path this has always shipped and been tested on.
///
/// # Windows
/// Anthropic's own native installer, which needs NEITHER node NOR npm: it downloads a
/// self-contained `claude.exe` and delegates placement to the binary's own `claude
/// install`, landing it in `~/.local/bin` (already a [`crate`]-side candidate dir, so the
/// post-install probe finds it). `https://claude.ai/install.ps1` 302s to
/// `downloads.claude.ai/claude-code-releases/bootstrap.ps1`.
///
/// Switching away from npm here is not a preference - it is the only form that WORKS. Three
/// independent reasons, each fatal on its own:
///
/// 1. **`@anthropic-ai/claude-code` requires node >=22.** A Windows box whose PATH node is
///    older installs a package that cannot run. Node lands on the SYSTEM PATH (the
///    `C:\Program Files\nodejs` MSI), which always precedes the User PATH where a newer
///    per-user node (winget, nvm) would sit - so the stale one wins and the user cannot fix
///    it without admin.
/// 2. **The npm form is what trips `llm::detect::resolve_installer_binary`.** A leading token
///    that resolves to a real binary is precisely what triggers the POSIX `export PATH=…`
///    rewrite; `irm` is a PowerShell alias, not a file on PATH, so this form is immune the
///    same way [`CURSOR_INSTALL_CMD`]'s `$s = …` is.
/// 3. **It reports failure honestly.** bootstrap.ps1 `exit`s non-zero on arch, version and
///    manifest errors and propagates the inner installer's code - so a failed install cannot
///    present as a success (see `llm::detect::installer_command`'s exit-code handling).
#[cfg(not(windows))]
pub const CLAUDE_INSTALL_CMD: &str = "npm i -g @anthropic-ai/claude-code";

/// See [`CLAUDE_INSTALL_CMD`]'s Windows doc above. Plain PowerShell script text with no
/// enclosing `powershell -Command "…"` wrapper - `llm::detect::installer_command` supplies
/// that, exactly as it does for [`CURSOR_INSTALL_CMD`].
#[cfg(windows)]
pub const CLAUDE_INSTALL_CMD: &str = "irm https://claude.ai/install.ps1 | iex";

/// Codex's installer.
///
/// # Unix
/// npm, unchanged - as with [`CLAUDE_INSTALL_CMD`].
///
/// # Windows
/// OpenAI's own standalone installer, which needs neither node nor npm (it downloads a
/// pre-compiled binary and will even remove a conflicting npm/bun-managed Codex).
/// `https://chatgpt.com/codex/install.ps1` 302s to `releases.openai.com/codex/install.ps1`.
///
/// **Coupled to `llm::detect::candidate_dirs`:** this installer lands `codex.exe` in
/// `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` and writes the USER PATH, which does not
/// affect the already-running tray process. So the post-install probe can only find it via
/// a candidate dir - that entry MUST stay in `candidate_dirs` or a perfectly successful
/// install reports itself as failed, the identical bug `%LOCALAPPDATA%\cursor-agent` was
/// added there to prevent.
#[cfg(not(windows))]
pub const CODEX_INSTALL_CMD: &str = "npm i -g @openai/codex";

/// See [`CODEX_INSTALL_CMD`]'s Windows doc above.
#[cfg(windows)]
pub const CODEX_INSTALL_CMD: &str = "irm https://chatgpt.com/codex/install.ps1 | iex";

#[cfg(test)]
mod install_pin_tests {
    use super::*;

    /// The pin must actually appear in the command, and the constant must stay the single
    /// source of truth (`concat!` can't interpolate, so the two are pinned together here).
    #[cfg(not(windows))]
    #[test]
    fn cursor_install_command_is_pinned_to_the_supported_version() {
        assert!(CURSOR_INSTALL_CMD.contains(CURSOR_CLI_VERSION));
        // Pattern-based rewrite, not a literal old->new swap: a literal would no-op once
        // Cursor publishes a new build and silently install latest.
        assert!(CURSOR_INSTALL_CMD.contains("[0-9]{4}"));
        assert!(CURSOR_INSTALL_CMD.contains("sed -E"));
        assert_eq!(
            LlmProvider::Cursor.install_command(),
            Some(CURSOR_INSTALL_CMD)
        );
    }

    /// Windows analogue: same pin, same pattern-not-literal guarantee, but via
    /// PowerShell's `-replace` and the win32-flagged installer endpoint rather than
    /// `sed`/bash — see the module doc on why Cursor's Unix script can't run here.
    #[cfg(windows)]
    #[test]
    fn cursor_install_command_is_pinned_to_the_supported_version() {
        assert!(CURSOR_INSTALL_CMD.contains(CURSOR_CLI_VERSION));
        assert!(CURSOR_INSTALL_CMD.contains("[0-9]{4}"));
        assert!(CURSOR_INSTALL_CMD.contains("-replace"));
        assert_eq!(
            LlmProvider::Cursor.install_command(),
            Some(CURSOR_INSTALL_CMD)
        );
    }

    /// The Windows script must go through PowerShell's `irm`/`iex`, not bash's
    /// `curl | sed | bash` — those aliases don't exist under `cmd.exe`, and stock
    /// Windows has no `bash`/`sed` on PATH at all outside WSL/Git-Bash.
    #[cfg(windows)]
    #[test]
    fn windows_install_command_uses_the_win32_powershell_installer() {
        assert!(CURSOR_INSTALL_CMD.contains("cursor.com/install?win32=true"));
        assert!(CURSOR_INSTALL_CMD.contains("irm "));
        assert!(CURSOR_INSTALL_CMD.contains("iex "));
        assert!(!CURSOR_INSTALL_CMD.contains("curl"));
        assert!(!CURSOR_INSTALL_CMD.contains("bash"));
    }

    /// Unix keeps npm for Claude and Codex - the path that has always shipped there. Pins it
    /// so a future Windows-motivated edit can't quietly retarget the platform that works.
    #[cfg(not(windows))]
    #[test]
    fn unix_claude_and_codex_install_via_npm() {
        assert_eq!(
            LlmProvider::Claude.install_command(),
            Some("npm i -g @anthropic-ai/claude-code")
        );
        assert_eq!(
            LlmProvider::Codex.install_command(),
            Some("npm i -g @openai/codex")
        );
    }

    /// The Windows fix, pinned. `npm i -g` cannot be the Windows form for either provider:
    /// Claude's package needs node >=22 (routinely absent, and unfixable without admin when
    /// a stale node sits on the SYSTEM PATH), and an npm leading token is exactly what makes
    /// `llm::detect::resolve_installer_binary` rewrite the command into POSIX `export PATH=…`
    /// syntax that PowerShell cannot parse. Both native installers are node-free.
    #[cfg(windows)]
    #[test]
    fn windows_claude_and_codex_install_natively_without_npm() {
        for cmd in [CLAUDE_INSTALL_CMD, CODEX_INSTALL_CMD] {
            assert!(cmd.contains("irm "), "{cmd}");
            assert!(cmd.contains("iex"), "{cmd}");
            assert!(!cmd.contains("npm"), "{cmd}");
            // The leading token must NOT be a resolvable binary name - that is what keeps
            // `resolve_installer_binary` from rewriting it. `irm` is a PowerShell alias.
            assert!(cmd.starts_with("irm "), "{cmd}");
        }
        assert_eq!(
            LlmProvider::Claude.install_command(),
            Some(CLAUDE_INSTALL_CMD)
        );
        assert_eq!(
            LlmProvider::Codex.install_command(),
            Some(CODEX_INSTALL_CMD)
        );
    }

    /// Every installer Meridian runs on Windows must be PowerShell-executable. `curl`/`bash`/
    /// `sed` are absent from stock Windows, and the POSIX `export` builtin does not exist in
    /// PowerShell at all - a command containing any of them fails with
    /// `CommandNotFoundException` before the vendor's installer is ever reached.
    #[cfg(windows)]
    #[test]
    fn no_windows_install_command_uses_posix_shell_syntax() {
        for p in [LlmProvider::Claude, LlmProvider::Codex, LlmProvider::Cursor] {
            let cmd = p.install_command().expect("built-in CLI has an installer");
            for posix in ["curl ", "bash", "sed ", "export "] {
                assert!(
                    !cmd.contains(posix),
                    "{p:?}'s Windows installer contains POSIX-only `{posix}`: {cmd}"
                );
            }
        }
    }
}

/// Where the "cursor-agent not found" hints in the daemon's `coding_agent_session_ingest`
/// init flow and the coding-agent health checks point the user — the ONE command that
/// actually works on this platform. Kept next to [`CURSOR_INSTALL_CMD`] so the two can't
/// drift: a bare `curl | bash` hint shown on Windows would send the user to run a command
/// that cannot possibly work there.
#[cfg(not(windows))]
pub const CURSOR_INSTALL_HINT: &str = "curl https://cursor.com/install -fsS | bash";

#[cfg(windows)]
pub const CURSOR_INSTALL_HINT: &str =
    "irm 'https://cursor.com/install?win32=true' | iex  (run in PowerShell)";

#[cfg(test)]
mod install_hint_tests {
    use super::*;

    /// The hint must never point at a command that cannot run on this platform: no
    /// `bash`/`curl` in the Windows hint, no PowerShell aliases in the Unix hint.
    #[cfg(windows)]
    #[test]
    fn windows_hint_does_not_suggest_bash() {
        assert!(CURSOR_INSTALL_HINT.contains("irm"));
        assert!(CURSOR_INSTALL_HINT.contains("iex"));
        assert!(!CURSOR_INSTALL_HINT.contains("curl"));
        assert!(!CURSOR_INSTALL_HINT.contains("bash"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_hint_does_not_suggest_powershell() {
        assert!(CURSOR_INSTALL_HINT.contains("curl"));
        assert!(CURSOR_INSTALL_HINT.contains("bash"));
        assert!(!CURSOR_INSTALL_HINT.contains("irm"));
        assert!(!CURSOR_INSTALL_HINT.contains("iex"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips() {
        for p in LlmProvider::all() {
            assert_eq!(LlmProvider::from_wire(p.as_str()), Some(p));
        }
    }

    #[test]
    fn serde_matches_as_str() {
        for p in LlmProvider::all() {
            let json = serde_json::to_string(&p).expect("serialise");
            assert_eq!(json, format!("\"{}\"", p.as_str()));
            let back: LlmProvider = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(back, p);
        }
    }

    #[test]
    fn unknown_wire_form_is_none_not_a_panic() {
        // The whole point of `from_wire`: a provider from a newer build degrades to the
        // default instead of taking every other setting down with it.
        assert_eq!(LlmProvider::from_wire("gemini"), None);
        assert_eq!(LlmProvider::from_wire(""), None);
        assert_eq!(LlmProvider::from_wire("Claude"), None); // case-sensitive by design
                                                            // `local` was the on-device model; it no longer exists.
        assert_eq!(LlmProvider::from_wire("local"), None);
    }

    /// `custom` names the KIND. `custom:<id>` addresses one instance and is deliberately
    /// NOT parsed here — the id has no meaning to this type, and silently accepting the
    /// prefixed form would resolve every custom endpoint to the same thing.
    #[test]
    fn custom_parses_as_the_kind_and_never_as_an_instance() {
        assert_eq!(LlmProvider::from_wire("custom"), Some(LlmProvider::Custom));
        assert_eq!(LlmProvider::from_wire("custom:openrouter-1"), None);
    }

    /// `builtins()` is `all()` minus `Custom` — the split exists so an install probe never
    /// enumerates a kind that has no single binary, endpoint, or identity.
    #[test]
    fn builtins_is_all_without_custom() {
        assert!(!LlmProvider::builtins().contains(&LlmProvider::Custom));
        assert!(LlmProvider::all().contains(&LlmProvider::Custom));
        assert_eq!(LlmProvider::builtins().len() + 1, LlmProvider::all().len());
    }

    #[test]
    fn whitespace_is_tolerated() {
        assert_eq!(
            LlmProvider::from_wire("  claude  "),
            Some(LlmProvider::Claude)
        );
    }

    #[test]
    fn default_is_claude() {
        assert_eq!(LlmProvider::default(), LlmProvider::Claude);
    }

    /// Only `Custom` has no CLI binary now — every built-in provider is a subprocess.
    #[test]
    fn only_custom_has_no_cli() {
        for p in LlmProvider::builtins() {
            assert!(p.cli_name().is_some(), "{p:?}");
        }
        assert!(LlmProvider::Custom.cli_name().is_none());
    }

    /// Every built-in CLI provider has an installer; `Custom` (a cloud endpoint) does not.
    /// The tray runs these on the user's behalf, so a missing one would leave a card with an
    /// Install button that can't work.
    #[test]
    fn every_builtin_has_an_install_command() {
        for p in LlmProvider::builtins() {
            assert!(p.install_command().is_some(), "{p:?}");
        }
        assert!(LlmProvider::Custom.install_command().is_none());
    }
}
