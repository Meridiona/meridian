//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `meridian oauth-login github` — authenticate via the local `gh` CLI.
//!
//! Uses `gh auth login --web` to open a browser-based GitHub OAuth flow, ensures
//! the `repo,read:org,read:project` scopes are present, extracts the token with
//! `gh auth token`, and writes `GITHUB_TOKEN=<token>` to `~/.meridian/.env` so
//! the daemon picks it up on the next poll / restart.
//!
//! Requires the GitHub CLI (`gh`) to be installed — <https://cli.github.com>.
//!
//! # Who calls this
//! `src/main.rs` → `oauth-login github` subcommand dispatch.

use anyhow::{bail, Context, Result};
use std::io::Write as _;

const HOSTNAME: &str = "github.com";
/// Scopes needed for issue/PR reads and GitHub Projects v2 node-ID listing.
const REQUIRED_SCOPES: &str = "repo,read:org,read:project";

/// Resolve the `gh` CLI binary. Probes Homebrew paths first so the tray app
/// (which runs with a minimal launchd PATH, not the user's shell PATH) finds
/// it on Apple Silicon and Intel Macs without requiring it on `/usr/bin`.
fn gh_bin() -> Option<String> {
    for candidate in [
        "/opt/homebrew/bin/gh",
        "/usr/local/bin/gh",
        "/home/linuxbrew/.linuxbrew/bin/gh",
    ] {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Resolve the `brew` binary (same PATH problem as `gh_bin`).
fn brew_bin() -> Option<String> {
    for candidate in [
        "/opt/homebrew/bin/brew",
        "/usr/local/bin/brew",
        "/home/linuxbrew/.linuxbrew/bin/brew",
    ] {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Install `gh` via Homebrew if Homebrew is available; returns the installed binary path.
async fn brew_install_gh() -> Result<String> {
    let brew = brew_bin()
        .context("Homebrew not found — install gh from https://cli.github.com then try again")?;
    eprintln!("Installing gh CLI via Homebrew (brew install gh)…");
    let st = tokio::process::Command::new(&brew)
        .args(["install", "gh"])
        .status()
        .await
        .context("brew install gh failed")?;
    if !st.success() {
        bail!("brew install gh failed — install gh manually from https://cli.github.com");
    }
    // Re-probe after install.
    gh_bin().context("gh not found even after brew install — check your Homebrew setup")
}

/// The interactive `meridian oauth-login github` flow.
///
/// Fails fast with a user-friendly message if `gh` is not installed. Opens
/// the browser via `gh auth login --web` when not yet authenticated; otherwise
/// refreshes the existing token to add any missing scopes. Writes
/// `GITHUB_TOKEN=<token>` to `~/.meridian/.env` on success.
pub async fn login() -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let env_path = format!("{home}/.meridian/.env");

    // Resolve gh binary (tray has a minimal launchd PATH — no Homebrew).
    // If gh is missing, auto-install via Homebrew; bail only if Homebrew is
    // also absent (rare for a developer on macOS).
    //
    // GITHUB_TOKEN is unset from every gh child: if the daemon loaded it from
    // .env, gh treats it as an ambient credential and refuses auth operations.
    let bin = match gh_bin() {
        Some(b) => b,
        None => brew_install_gh().await?,
    };

    let already_authed = tokio::process::Command::new(&bin)
        .args(["auth", "status", "--hostname", HOSTNAME])
        .env_remove("GITHUB_TOKEN")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !already_authed {
        // Not yet authenticated — open the browser-based login flow.
        // --web opens the browser directly (no Enter prompt on modern gh).
        let st = tokio::process::Command::new(&bin)
            .args([
                "auth",
                "login",
                "--hostname",
                HOSTNAME,
                "--git-protocol",
                "https",
                "--web",
                "--scopes",
                REQUIRED_SCOPES,
            ])
            .env_remove("GITHUB_TOKEN")
            .status()
            .await
            .context("failed to run gh auth login")?;
        if !st.success() {
            bail!("GitHub authorization failed — check your browser and try again");
        }
    }

    // Extract the stored token.
    let out = tokio::process::Command::new(&bin)
        .args(["auth", "token", "--hostname", HOSTNAME])
        .env_remove("GITHUB_TOKEN")
        .output()
        .await
        .context("gh auth token failed")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("gh auth token: {err}");
    }
    let token = std::str::from_utf8(&out.stdout)
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        bail!("gh auth token returned empty — run `gh auth login` in your terminal");
    }

    upsert_env_key(&env_path, "GITHUB_TOKEN", &token)
        .with_context(|| format!("could not write GITHUB_TOKEN to {env_path}"))?;

    eprintln!("✓ GitHub connected — GITHUB_TOKEN written to {env_path}");
    eprintln!("  Run `meridian restart` so the daemon picks it up.");
    eprintln!("  Optional: set GITHUB_PROJECT_IDS in {env_path} to sync GitHub Projects v2 tasks.");
    Ok(())
}

/// Upsert `KEY=value` in a `.env` file: replaces the existing `KEY=…` line if
/// present, otherwise appends. Does not touch any other lines.
fn upsert_env_key(path: &str, key: &str, value: &str) -> Result<()> {
    let new_line = format!("{key}={value}");
    let p = std::path::Path::new(path);

    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).context("create parent dir")?;
    }

    if p.exists() {
        let contents = std::fs::read_to_string(p).context("read .env")?;
        let prefix = format!("{key}=");
        if contents
            .lines()
            .any(|l| l.trim_start().starts_with(prefix.as_str()))
        {
            // Replace in-place. Restore trailing newline that .lines() strips.
            let mut updated = contents
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with(prefix.as_str()) {
                        new_line.as_str()
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            if contents.ends_with('\n') {
                updated.push('\n');
            }
            std::fs::write(p, updated).context("write .env")?;
            return Ok(());
        }
        // Key not present — append. Ensure we start on a new line so we don't
        // run onto the tail of a file that has no trailing newline.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(p)
            .context("open .env for append")?;
        let sep = if contents.ends_with('\n') { "" } else { "\n" };
        writeln!(f, "{sep}{new_line}").context("append to .env")?;
        return Ok(());
    }

    // File doesn't exist — create and write.
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(p)
        .context("open .env for create")?;
    writeln!(f, "{new_line}").context("write to new .env")?;
    Ok(())
}
