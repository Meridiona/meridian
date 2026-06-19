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
fn gh_bin() -> String {
    for candidate in [
        "/opt/homebrew/bin/gh",              // Apple Silicon Homebrew
        "/usr/local/bin/gh",                 // Intel Homebrew / manual install
        "/home/linuxbrew/.linuxbrew/bin/gh", // Linux Homebrew
    ] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "gh".to_string() // fallback to PATH
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

    // Fail fast if gh is not available (tray runs with a minimal PATH — probe
    // Homebrew locations via gh_bin() before falling back to bare "gh").
    //
    // GITHUB_TOKEN is unset from every gh child process: if the daemon loaded
    // it from .env, gh sees it as an ambient credential and refuses to run
    // `auth refresh` / `auth token` with "clear the value from the environment".
    let bin = gh_bin();
    let gh_found = tokio::process::Command::new(&bin)
        .arg("--version")
        .env_remove("GITHUB_TOKEN")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gh_found {
        bail!(
            "gh CLI not found — install it from https://cli.github.com then try again \
             (looked for: {bin})"
        );
    }

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
            let updated = contents
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
            std::fs::write(p, updated).context("write .env")?;
            return Ok(());
        }
    }

    // Key not present — append.
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .context("open .env for append")?;
    writeln!(f, "{new_line}").context("append to .env")?;
    Ok(())
}
