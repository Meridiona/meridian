//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Bundled-backend first-run install orchestration (Gap-2 Bucket 1, slice 1b).
//!
//! On the self-contained `.app` DMG path the non-capture backend (the Rust
//! daemon + the a11y-helper) ships inside `Meridian.app/Contents/Resources/backend/`
//! (wired by `tauri.conf.json`'s `bundle.resources`). This module is the tray
//! side of that: on startup it stages those binaries to the **stable**
//! `~/.meridian/bin/` path and registers their launchd agents — the same
//! daemon + a11y-helper steps `scripts/install-from-bundle.sh` does for the npm
//! bundle, ported into the tray so the DMG needs no shell installer.
//!
//! It is a **faithful port of the launchctl flow, not SMAppService**: the
//! a11y-helper's Accessibility grant is keyed to its binary's code hash and
//! survives updates only because it runs from a stable path *outside* the
//! re-signed `.app`; SMAppService's managed-Login-Items payoff only matters once
//! the app is Developer-ID-signed (Gap-2 Bucket 3), so it's deferred to then.
//!
//! Unlike the npm bundle (WorkingDirectory `~/.meridian/app`, daemon reads
//! `~/.meridian/app/.env`), the DMG daemon's WorkingDirectory is `~/.meridian`,
//! so `dotenvy` self-loads the **canonical** `~/.meridian/.env` the tray already
//! writes to — unifying tray and daemon on one credential file.
//!
//! # Who calls this
//! [`crate::run`]'s Tauri `setup` hook spawns [`ensure_backend_installed`] once,
//! off the main thread (the launchd bootout-wait can take seconds).
//!
//! # Related
//! - [`crate::install`] — resolves where data lives; [`crate::install::meridian_bin`]
//!   prefers the `~/.meridian/bin/meridian` this module stages.
//! - [`crate::mlx_server`] — the *other* backend service (provisioned via Approach C,
//!   not bundled); [`crate::mlx_server::sha256_hex_of`] is reused here for the
//!   update-detection marker.
//! - `scripts/install-from-bundle.sh`, `scripts/install-daemon.sh`,
//!   `scripts/install-a11y-helper-daemon.sh` — the shell flow this ports.

use std::path::{Path, PathBuf};

use tauri::Manager;

use crate::mlx_server;

/// launchd agents this stages, paired with their bundled plist template.
const AGENTS: &[(&str, &str)] = &[
    ("com.meridiona.daemon", "com.meridiona.daemon.plist"),
    (
        "com.meridiona.a11y-helper",
        "com.meridiona.a11y-helper.plist",
    ),
];

/// Stage the bundled backend and register its launchd agents — idempotent and
/// non-fatal.
///
/// No-op unless **all** hold: running from a packaged `.app` whose
/// `Resources/backend/` exists (absent under `tauri dev` and source checkouts —
/// those keep using the shell scripts), and the bundled daemon binary's SHA-256
/// differs from the last successful install (first run, or a post-update where
/// the shipped binary changed). Any staging/launchctl failure is logged and
/// swallowed so a backend hiccup never crashes the tray; the marker is persisted
/// **only after** both agents bootstrap, so a partial failure retries next launch.
#[tracing::instrument(skip(app))]
pub async fn ensure_backend_installed(app: &tauri::AppHandle) {
    let backend = match bundled_backend_dir(app) {
        Some(d) => d,
        None => {
            tracing::debug!("backend_install: no bundled backend (dev/source run) — skipping");
            return;
        }
    };
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            tracing::warn!("backend_install: HOME unset — cannot stage backend");
            return;
        }
    };

    let daemon_src = backend.join("meridian");
    let bundled_hash = match mlx_server::sha256_hex_of(&daemon_src) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, src = %daemon_src.display(), "backend_install: cannot hash bundled daemon");
            return;
        }
    };
    let marker = home.join(".meridian/backend-version");
    if tokio::fs::read_to_string(&marker).await.ok().as_deref() == Some(bundled_hash.as_str()) {
        tracing::debug!(hash = %bundled_hash, "backend_install: backend up to date — skipping");
        return;
    }

    tracing::info!(hash = %bundled_hash, "backend_install: installing bundled backend");
    if let Err(e) = install(&backend, &home).await {
        tracing::warn!(error = %e, "backend_install: install failed — will retry next launch");
        return;
    }

    // Persist the marker only on full success so a partial install retries.
    if let Err(e) = tokio::fs::write(&marker, &bundled_hash).await {
        tracing::warn!(error = %e, "backend_install: could not write version marker");
    }
    tracing::info!("backend_install: backend installed");
}

/// `Meridian.app/Contents/Resources/backend/` when it exists, else `None`.
fn bundled_backend_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?.join("backend");
    dir.is_dir().then_some(dir)
}

/// Stage both binaries, render + lint both plists, register both agents.
/// Returns `Err` if any step fails so the caller skips the success marker.
async fn install(backend: &Path, home: &Path) -> Result<(), String> {
    for dir in [".meridian/bin", ".meridian/logs"] {
        let p = home.join(dir);
        tokio::fs::create_dir_all(&p)
            .await
            .map_err(|e| format!("mkdir {}: {e}", p.display()))?;
    }
    let launch_agents = home.join("Library/LaunchAgents");
    tokio::fs::create_dir_all(&launch_agents)
        .await
        .map_err(|e| format!("mkdir {}: {e}", launch_agents.display()))?;

    let daemon_bin = home.join(".meridian/bin/meridian");
    let helper_bin = home.join(".meridian/bin/meridian-a11y-helper");
    stage_binary(&backend.join("meridian"), &daemon_bin).await?;
    stage_binary(&backend.join("meridian-a11y-helper"), &helper_bin).await?;

    // Render the two plists. The bundled templates carry {{…}} placeholders the
    // npm installer substitutes too; here REPO_ROOT (the daemon's WorkingDirectory)
    // is ~/.meridian so dotenvy self-loads ~/.meridian/.env, and OTLP is left
    // empty for the daemon to self-load (a baked value would go stale).
    let home_str = home.to_string_lossy();
    render_plist(
        &backend.join("com.meridiona.daemon.plist"),
        &launch_agents.join("com.meridiona.daemon.plist"),
        &[
            ("{{HOME}}", home_str.as_ref()),
            ("{{REPO_ROOT}}", &home.join(".meridian").to_string_lossy()),
            ("{{DAEMON_BIN}}", &daemon_bin.to_string_lossy()),
            ("{{MERIDIAN_OTLP_ENDPOINT}}", ""),
        ],
    )
    .await?;
    render_plist(
        &backend.join("com.meridiona.a11y-helper.plist"),
        &launch_agents.join("com.meridiona.a11y-helper.plist"),
        &[
            ("{{HOME}}", home_str.as_ref()),
            ("{{HELPER_BIN}}", &helper_bin.to_string_lossy()),
        ],
    )
    .await?;

    for (label, plist) in AGENTS {
        register_agent(label, &launch_agents.join(plist)).await?;
    }
    Ok(())
}

/// Copy `src` → `dest` only when the bytes differ, then `chmod 0755`.
/// Skipping an identical copy keeps the code hash (and any TCC grant) stable.
async fn stage_binary(src: &Path, dest: &Path) -> Result<(), String> {
    let same = match (tokio::fs::read(src).await, tokio::fs::read(dest).await) {
        (Ok(a), Ok(b)) => a == b,
        (Err(e), _) => return Err(format!("read {}: {e}", src.display())),
        _ => false,
    };
    if same {
        tracing::debug!(dest = %dest.display(), "backend_install: binary unchanged");
        return Ok(());
    }
    tokio::fs::copy(src, dest)
        .await
        .map_err(|e| format!("copy {} → {}: {e}", src.display(), dest.display()))?;
    set_executable(dest).await?;
    tracing::info!(dest = %dest.display(), "backend_install: staged binary");
    Ok(())
}

/// `chmod u+rwx,go+rx` (0755) on a freshly staged binary.
#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let perm = std::fs::Permissions::from_mode(0o755);
    tokio::fs::set_permissions(path, perm)
        .await
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Replace each `{{KEY}}` in `template` with its value. Pure — the testable core
/// of [`render_plist`].
fn apply_subs(template: &str, subs: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (key, val) in subs {
        text = text.replace(key, val);
    }
    text
}

/// Read a bundled plist template, replace each `{{KEY}}`, write it to
/// `~/Library/LaunchAgents/`, and `plutil -lint` the result.
async fn render_plist(template: &Path, dest: &Path, subs: &[(&str, &str)]) -> Result<(), String> {
    let raw = tokio::fs::read_to_string(template)
        .await
        .map_err(|e| format!("read {}: {e}", template.display()))?;
    let text = apply_subs(&raw, subs);
    tokio::fs::write(dest, text)
        .await
        .map_err(|e| format!("write {}: {e}", dest.display()))?;

    let out = tokio::process::Command::new("plutil")
        .arg("-lint")
        .arg(dest)
        .output()
        .await
        .map_err(|e| format!("run plutil: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "plutil -lint {} failed: {}",
            dest.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    tracing::debug!(plist = %dest.display(), "backend_install: rendered plist");
    Ok(())
}

/// Bootout (and wait for the domain entry to clear), then bootstrap + enable +
/// kickstart the agent under `gui/<uid>` — the same dance the shell installers
/// run, ported. `bootout` is async, so we poll `launchctl print` until the label
/// clears (≤15 s) before bootstrapping, else `bootstrap` can fail with EIO.
async fn register_agent(label: &str, plist: &Path) -> Result<(), String> {
    let gui = format!("gui/{}", crate::sys::uid_str());
    let target = format!("{gui}/{label}");

    let _ = launchctl(&["bootout", &target]).await; // ok if not loaded
    for _ in 0..15 {
        if !launchctl(&["print", &target]).await.is_ok_and(|s| s) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let _ = launchctl(&["enable", &target]).await;
    let plist_s = plist.to_string_lossy();
    if !launchctl(&["bootstrap", &gui, &plist_s])
        .await
        .unwrap_or(false)
    {
        return Err(format!("launchctl bootstrap {label} failed"));
    }
    let _ = launchctl(&["enable", &target]).await;
    let _ = launchctl(&["kickstart", "-k", &target]).await;
    tracing::info!(label, "backend_install: launchd agent registered");
    Ok(())
}

/// Run `launchctl <args>`, returning `Ok(true)` on exit 0. Errors only on spawn
/// failure; a non-zero exit is `Ok(false)` so callers decide what's fatal.
async fn launchctl(args: &[&str]) -> Result<bool, String> {
    tokio::process::Command::new("launchctl")
        .args(args)
        .output()
        .await
        .map(|o| o.status.success())
        .map_err(|e| format!("run launchctl: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_subs_replaces_every_occurrence() {
        let out = apply_subs(
            "a={{HOME}} b={{HOME}} c={{X}}",
            &[("{{HOME}}", "/Users/me"), ("{{X}}", "v")],
        );
        assert_eq!(out, "a=/Users/me b=/Users/me c=v");
    }

    /// Remove `<!-- … -->` blocks so the placeholder check sees only the live
    /// plist body — the templates document their placeholder names (incl. the
    /// deprecated `{{MERIDIAN_OO_AUTH}}`) inside an XML comment, which is not a
    /// value the daemon ever reads.
    fn strip_xml_comments(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(start) = rest.find("<!--") {
            out.push_str(&rest[..start]);
            match rest[start..].find("-->") {
                Some(end) => rest = &rest[start + end + 3..],
                None => return out, // unterminated comment — drop the tail
            }
        }
        out.push_str(rest);
        out
    }

    /// The bundled templates must have NO `{{…}}` left **in the live body** after
    /// the exact sub sets `install()` applies — a new body placeholder added
    /// upstream without a matching sub would otherwise ship a broken plist. Reads
    /// the real committed templates so the two can't drift apart silently.
    #[test]
    fn bundled_templates_fully_substituted() {
        let scripts = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts");

        let daemon = std::fs::read_to_string(format!("{scripts}/com.meridiona.daemon.plist"))
            .expect("read daemon plist template");
        let rendered = strip_xml_comments(&apply_subs(
            &daemon,
            &[
                ("{{HOME}}", "/Users/me"),
                ("{{REPO_ROOT}}", "/Users/me/.meridian"),
                ("{{DAEMON_BIN}}", "/Users/me/.meridian/bin/meridian"),
                ("{{MERIDIAN_OTLP_ENDPOINT}}", ""),
            ],
        ));
        assert!(
            !rendered.contains("{{"),
            "daemon plist body still has an unsubstituted placeholder: {rendered}"
        );

        let helper = std::fs::read_to_string(format!("{scripts}/com.meridiona.a11y-helper.plist"))
            .expect("read a11y-helper plist template");
        let rendered = strip_xml_comments(&apply_subs(
            &helper,
            &[
                ("{{HOME}}", "/Users/me"),
                (
                    "{{HELPER_BIN}}",
                    "/Users/me/.meridian/bin/meridian-a11y-helper",
                ),
            ],
        ));
        assert!(
            !rendered.contains("{{"),
            "a11y-helper plist body still has an unsubstituted placeholder: {rendered}"
        );
    }
}
