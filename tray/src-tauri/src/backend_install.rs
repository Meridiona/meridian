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
//! writes to — unifying tray and daemon on one credential file. A bundle→DMG
//! migrant's pre-existing `~/.meridian/app/.env` is copied across once
//! ([`migrate_legacy_bundle_env`]) and stale bundle launchd agents
//! (screenpipe / MLX / UI server) are booted out during [`install`].
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

    // Purge leftovers from a pre-fold / pre-cutover **bundle** install before
    // staging the in-process backend. A user migrating from the old npm/curl
    // bundle to this DMG carries launchd agents the new topology either replaced
    // (the in-process capturer supersedes screenpipe; the tray supervises MLX
    // itself) or retired (the embedded dashboard replaced the standalone UI
    // server). Left running they race the tray, contend for :7823, or burn RAM.
    // `install-from-bundle.sh` boots these out for the bundle path; the DMG path
    // needs the same. All best-effort + non-fatal.
    cleanup_legacy_screenpipe(home).await;
    cleanup_legacy_mlx_server(home).await;
    cleanup_legacy_ui(home).await;
    // Recover tracker credentials the bundle wrote to ~/.meridian/app/.env so the
    // DMG daemon (which reads the canonical ~/.meridian/.env) doesn't lose them.
    migrate_legacy_bundle_env(home).await;

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

/// Purge a leftover **pre-cutover screenpipe install**. Before the in-process
/// cutover, capture ran as a separate `screenpipe` binary under a
/// `com.meridiona.screenpipe` launchd agent (staged by `install-from-bundle.sh`).
/// The in-process build doesn't use screenpipe, but an *update* over such an
/// install leaves that agent running — it respawns `screenpipe record`, which
/// requests Screen Recording (a duplicate prompt) and races the tray's in-process
/// capture. Boot it out, remove its plist + binary, and kill any live process.
/// Entirely best-effort and non-fatal — a launchctl hiccup must not abort install.
async fn cleanup_legacy_screenpipe(home: &Path) {
    let label = "com.meridiona.screenpipe";
    let target = format!("gui/{}/{label}", crate::sys::uid_str());
    if launchctl(&["print", &target]).await.is_ok_and(|s| s) {
        let _ = launchctl(&["bootout", &target]).await;
        tracing::info!(label, "backend_install: removed leftover screenpipe agent");
    }
    let _ =
        tokio::fs::remove_file(home.join("Library/LaunchAgents/com.meridiona.screenpipe.plist"))
            .await;
    let _ = tokio::fs::remove_file(home.join(".meridian/bin/screenpipe")).await;
    // Kill any still-running screenpipe the agent had spawned (best-effort).
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "screenpipe record"])
        .output()
        .await;
}

/// Purge a leftover **bundle MLX launchd agent**. The npm/curl bundle registers
/// the MLX inference server as `com.meridiona.mlx-server` (via
/// `install-mlx-server-daemon.sh`) on port 7823. The DMG instead supervises MLX
/// **in-process** through [`crate::mlx_server::MlxManager`] on that *same* port,
/// so a leftover launchd agent contends for :7823 — the tray's spawn hits
/// `EADDRINUSE` and the agent's `KeepAlive` keeps respawning, producing retry
/// churn + log spam. Boot it out and remove its plist so the tray owns the port.
/// We do **not** `pkill` by name — the tray's own MLX child also listens on 7823,
/// and `bootout` already stops only the launchd-spawned one. Best-effort.
async fn cleanup_legacy_mlx_server(home: &Path) {
    let label = "com.meridiona.mlx-server";
    let target = format!("gui/{}/{label}", crate::sys::uid_str());
    if launchctl(&["print", &target]).await.is_ok_and(|s| s) {
        let _ = launchctl(&["bootout", &target]).await;
        tracing::info!(label, "backend_install: removed leftover MLX launchd agent");
    }
    let _ =
        tokio::fs::remove_file(home.join("Library/LaunchAgents/com.meridiona.mlx-server.plist"))
            .await;
}

/// Purge a leftover **standalone UI server agent**. Pre-fold, the Next.js
/// dashboard ran as `com.meridiona.ui` (a `KeepAlive` Node server on
/// localhost:3939). The fold embeds the dashboard in the tray webview, so a
/// leftover agent is a zombie Node process burning ~150 MB indefinitely (no port
/// clash — 3939 is dev-only). Boot it out and remove its plist. Mirrors the
/// `install-from-bundle.sh` cleanup for the bundle path. Best-effort.
async fn cleanup_legacy_ui(home: &Path) {
    let label = "com.meridiona.ui";
    let target = format!("gui/{}/{label}", crate::sys::uid_str());
    if launchctl(&["print", &target]).await.is_ok_and(|s| s) {
        let _ = launchctl(&["bootout", &target]).await;
        tracing::info!(label, "backend_install: removed leftover UI server agent");
    }
    let _ = tokio::fs::remove_file(home.join("Library/LaunchAgents/com.meridiona.ui.plist")).await;
}

/// Recover tracker credentials when migrating from a **bundle** install. The
/// npm/curl bundle writes its `.env` to `~/.meridian/app/.env` (its daemon's
/// WorkingDirectory); the DMG daemon's WorkingDirectory is `~/.meridian`, so it
/// reads the **canonical** `~/.meridian/.env`. Without a copy, a bundle→DMG
/// migrant's Jira/GitHub/Linear tokens would silently vanish and need re-entering
/// via the setup wizard. Copy the bundle file across **only when the canonical
/// one doesn't already exist** — never clobber creds the tray already wrote.
/// Best-effort + non-fatal.
async fn migrate_legacy_bundle_env(home: &Path) {
    let canonical = home.join(".meridian/.env");
    let bundle = home.join(".meridian/app/.env");
    if tokio::fs::metadata(&canonical).await.is_ok() {
        return; // canonical creds already present — leave them untouched
    }
    if tokio::fs::metadata(&bundle).await.is_err() {
        return; // no bundle .env to migrate (fresh install / source run)
    }
    match tokio::fs::copy(&bundle, &canonical).await {
        Ok(_) => tracing::info!(
            src = %bundle.display(),
            dest = %canonical.display(),
            "backend_install: migrated bundle .env to the canonical path"
        ),
        Err(e) => tracing::warn!(error = %e, "backend_install: could not migrate bundle .env"),
    }
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

    /// `migrate_legacy_bundle_env` must: copy the bundle `.env` to the canonical
    /// path when only the bundle exists; **never clobber** an existing canonical
    /// file; and no-op when there's nothing to migrate. The clobber guard is the
    /// load-bearing case — overwriting would wipe creds the tray already wrote.
    #[tokio::test]
    async fn migrate_bundle_env_copies_but_never_clobbers() {
        let base = std::env::temp_dir().join(format!("meridian-bundle-env-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&base).await; // clean slate if a prior run died
        let mk = |name: &str| base.join(name);

        // Case 1: bundle present, canonical absent → copies content across.
        let h1 = mk("copy");
        tokio::fs::create_dir_all(h1.join(".meridian/app"))
            .await
            .unwrap();
        tokio::fs::write(h1.join(".meridian/app/.env"), "JIRA_API_TOKEN=abc")
            .await
            .unwrap();
        migrate_legacy_bundle_env(&h1).await;
        assert_eq!(
            tokio::fs::read_to_string(h1.join(".meridian/.env"))
                .await
                .unwrap(),
            "JIRA_API_TOKEN=abc"
        );

        // Case 2: both present → canonical is left untouched (no clobber).
        let h2 = mk("noclobber");
        tokio::fs::create_dir_all(h2.join(".meridian/app"))
            .await
            .unwrap();
        tokio::fs::write(h2.join(".meridian/app/.env"), "FROM=bundle")
            .await
            .unwrap();
        tokio::fs::write(h2.join(".meridian/.env"), "FROM=canonical")
            .await
            .unwrap();
        migrate_legacy_bundle_env(&h2).await;
        assert_eq!(
            tokio::fs::read_to_string(h2.join(".meridian/.env"))
                .await
                .unwrap(),
            "FROM=canonical"
        );

        // Case 3: nothing to migrate → canonical stays absent, no error.
        let h3 = mk("noop");
        tokio::fs::create_dir_all(h3.join(".meridian"))
            .await
            .unwrap();
        migrate_legacy_bundle_env(&h3).await;
        assert!(tokio::fs::metadata(h3.join(".meridian/.env"))
            .await
            .is_err());

        let _ = tokio::fs::remove_dir_all(&base).await;
    }

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
