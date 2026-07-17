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
//! # Authentication is deliberately NOT probed
//!
//! There is no cheap non-interactive auth check for these CLIs, and `cursor-agent login`
//! was observed to hang forever when already signed in. So [`ProviderStatus`] reports
//! *installed*, not *usable*, and the UI says so plainly: Meridian uses your existing
//! login, and if it isn't signed in the hour falls back to on-device. Better an honest
//! unknown than a check that hangs the wizard.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use meridian_core::LlmProvider;
use serde::Serialize;
use tokio::process::Command;

/// How long a probe may take before we call it absent. A login shell sources the user's
/// profile, which can be slow (nvm, rbenv, …), but not this slow.
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

/// Where these CLIs actually land, for when the login shell is unavailable or too slow.
fn candidate_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        format!("{home}/.bun/bin"),
        format!("{home}/.volta/bin"),
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
}

/// Probe one provider. The on-device model is always "installed" — it is an HTTP call to
/// a server we manage, not a binary the user has to have.
pub async fn detect(provider: LlmProvider) -> ProviderStatus {
    let id = provider.as_str().to_string();
    let Some(bin) = provider.cli_name() else {
        return ProviderStatus {
            id,
            installed: true,
            path: None,
            authenticated: None,
        };
    };

    let found = probe_login_shell(bin)
        .await
        .or_else(|| probe_candidates(bin));
    ProviderStatus {
        id,
        installed: found.is_some(),
        path: found.map(|p| p.display().to_string()),
        authenticated: None,
    }
}

/// Probe every provider at once. The shell probes are I/O-bound and independent.
pub async fn detect_all() -> Vec<ProviderStatus> {
    let futures = LlmProvider::all().map(detect);
    futures::future::join_all(futures).await
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

    #[tokio::test]
    async fn the_on_device_model_is_always_available() {
        let s = detect(LlmProvider::Local).await;
        assert!(s.installed, "the local model needs no CLI");
        assert_eq!(s.path, None);
    }

    #[tokio::test]
    async fn detect_all_covers_every_provider_exactly_once() {
        let all = detect_all().await;
        assert_eq!(all.len(), LlmProvider::all().len());
        for p in LlmProvider::all() {
            assert_eq!(all.iter().filter(|s| s.id == p.as_str()).count(), 1);
        }
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
}
