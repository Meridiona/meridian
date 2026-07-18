//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! DMG auto-update via `tauri-plugin-updater`.
//!
//! # What this is
//! The in-app side of the public-distribution plan: the running app checks the
//! GitHub `latest.json` (endpoint + minisign pubkey are in `tauri.conf.json`),
//! and when a newer version is published, downloads the signed `.app.tar.gz`,
//! verifies it against the embedded pubkey, swaps it over the installed bundle,
//! and relaunches into it.
//!
//! This is the **DMG** path. An npm/CLI install updates instead via `meridian
//! update` in a Terminal ([`crate::commands::run_update`]); the two mechanisms
//! are independent and never both fire — a DMG install has no `~/.meridian/app`
//! npm bundle, and the npm install isn't the `.app` the updater rewrites.
//!
//! # Surfaces
//! - [`check_status`] → the dashboard sidebar + the tray popover banner (via the
//!   `check_update` command) — a structured, never-throwing status so the UI can
//!   render "available / up to date / unsupported / error" instead of a silent
//!   toast. **This is the primary surface** (the tray notification path proved
//!   invisible when notifications aren't granted).
//! - [`download_and_apply`] → both banners' "Restart & Update" button (via the
//!   `install_update` command); emits `update-progress` events for a progress bar.
//! - [`check_for_updates`] → the tray menu's "Check for Updates…" (notification
//!   fallback, reusing the same core).
//! - [`enforce_minimum_version`] → spawned once at launch. The single exception
//!   to the consent rule: when the manifest notes carry a `Minimum-Version:` line
//!   and the running version is below it, the update installs + relaunches
//!   automatically (a kill-switch for releases old versions must not keep
//!   running — broken update paths, data-corrupting bugs).
//!
//! # Related
//! - [`notify_update`] — routes every tray-menu toast through the outbox
//!   (event_key `system.update`) instead of a direct bypass, so quiet-hours,
//!   the master switch, and the `notify_system_update` toggle all apply —
//!   previously these six call sites skipped that policy entirely.
//! - `scripts/package-updater.sh` — the producer of the `Minimum-Version:` notes
//!   line (reads the optional `tray/minimum-version` file at release time).
//! - Plan: Obsidian `Decisions/Public distribution + auto-update for the DMG`.

use meridian_core::SqlitePool;
use semver::Version;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Enqueue a discrete update-related toast through the outbox instead of the
/// direct `sys::notify` bypass. Each of these six call sites represents a
/// one-shot event (a check outcome, a download starting), not an ongoing
/// condition to raise/clear like [`meridian::notices`] models — so this goes
/// straight to [`meridian::notifications::enqueue`], gated by `system.update`
/// (master switch + quiet hours + the `notify_system_update` toggle) the same
/// way every other outbox producer is. `dedup_suffix` only needs to be unique
/// per call (the current timestamp is fine — these are user-triggered or
/// rare background events, never a tight loop that would spam distinct keys).
async fn notify_update(app: &AppHandle, dedup_suffix: &str, title: &str, body: &str) {
    let Some(pool) = app
        .try_state::<Option<SqlitePool>>()
        .and_then(|s| s.inner().clone())
    else {
        tracing::debug!(title, "notify_update: DB not open yet — toast dropped");
        return;
    };
    let dedup = format!(
        "system.update:{dedup_suffix}:{}",
        chrono::Utc::now().timestamp_millis()
    );
    let n = meridian::notifications::NewNotification::event(&dedup, "system.update", title, body)
        .via(meridian::notifications::CHANNEL_NATIVE);
    if let Err(e) = meridian::notifications::enqueue(&pool, n).await {
        tracing::warn!(error = %e, title, "update notification enqueue failed");
    }
}

/// Guards the tray-menu path against re-entry (a second click mid-download).
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Guards the non-idempotent install across *all* UI surfaces (tray menu + both
/// banners). [`CHECKING`] only covers the tray-menu path; the `install_update`
/// command can fire from the sidebar and popover too, so the single-flight guard
/// has to wrap the install itself or parallel `download_and_install` + restart
/// attempts would race.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Structured result of an update check, for the in-app banners. Deliberately
/// NOT a `Result` — a failed check is data (`state = "error"` + `error`) the UI
/// renders, not a thrown command that the banner would have to swallow. `state`
/// is one of: `available` · `uptodate` · `unsupported` · `error`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// `available` | `uptodate` | `unsupported` | `error`.
    pub state: String,
    /// The running app version (the baked `tauri.conf.json` version).
    pub current_version: String,
    /// The newer version, when `state == "available"`.
    pub version: Option<String>,
    /// Release notes (the manifest `notes` with the `Minimum-Version:` marker
    /// line stripped — it's transport, not prose), when present.
    pub notes: Option<String>,
    /// The manifest's minimum supported version, when it declared one.
    pub minimum_version: Option<String>,
    /// True when the running version is below `minimum_version` — the update
    /// installs automatically ([`enforce_minimum_version`]) instead of waiting
    /// for the banner click.
    pub mandatory: bool,
    /// Human-readable error text, when `state == "error"`.
    pub error: Option<String>,
}

impl UpdateStatus {
    /// A status with no version/notes/error — for the plain `uptodate` /
    /// `unsupported` cases.
    fn simple(state: &str, current: String) -> Self {
        Self {
            state: state.to_string(),
            current_version: current,
            version: None,
            notes: None,
            minimum_version: None,
            mandatory: false,
            error: None,
        }
    }

    /// An `error` status carrying `msg`.
    fn errored(current: String, msg: String) -> Self {
        Self {
            state: "error".to_string(),
            current_version: current,
            version: None,
            notes: None,
            minimum_version: None,
            mandatory: false,
            error: Some(msg),
        }
    }
}

/// Split the manifest `notes` into the minimum supported version (from a
/// `Minimum-Version: <semver>` line, matched case-insensitively) and the
/// remaining human-readable notes (`None` when nothing else is left).
///
/// The notes body is the transport because `tauri-plugin-updater` drops unknown
/// manifest fields — a custom top-level `minimumVersion` key in `latest.json`
/// would never reach the app. An unparsable version is treated as absent (and
/// logged), never as mandatory: a malformed manifest must not force-restart
/// every install in the field.
fn split_minimum_version(notes: &str) -> (Option<Version>, Option<String>) {
    let mut minimum = None;
    let mut kept: Vec<&str> = Vec::new();
    for line in notes.lines() {
        let trimmed = line.trim();
        let key = "minimum-version:";
        // Checked slice: a multibyte char straddling the boundary must not panic.
        let is_marker = trimmed
            .get(..key.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(key));
        if is_marker {
            let value = trimmed[key.len()..].trim();
            match Version::parse(value) {
                Ok(v) => minimum = Some(v),
                Err(e) => {
                    tracing::warn!(value, error = %e, "update: unparsable Minimum-Version in manifest notes — ignoring");
                }
            }
        } else {
            kept.push(line);
        }
    }
    let rest = kept.join("\n");
    let rest = rest.trim();
    let rest = (!rest.is_empty()).then(|| rest.to_string());
    (minimum, rest)
}

/// True only for a packaged `.app` run. A source/dev run can't be swapped by the
/// updater (the running binary lives under `target/…`, not inside a bundle), and
/// the GitHub manifest may not be published yet — so we report `unsupported` and
/// keep the banner hidden rather than surfacing a 404 as a scary error.
/// True when running from a packaged `.app` bundle. Delegates to the single
/// shared detector [`crate::sys::is_bundled`] so the updater's "is this a real
/// bundle?" gate and the notification-plugin registration gate can never
/// disagree — they previously used two separately-implemented path checks.
fn is_packaged() -> bool {
    crate::sys::is_bundled()
}

/// Check the configured endpoint for a newer version. A pure status read — does
/// NOT download — so it's safe on every popover open / sidebar mount.
#[tracing::instrument(skip(app))]
pub async fn check_status(app: &AppHandle) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    if !is_packaged() {
        tracing::info!("update: not a packaged .app — reporting unsupported");
        return UpdateStatus::simple("unsupported", current);
    }
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => return UpdateStatus::errored(current, e.to_string()),
    };
    match updater.check().await {
        Ok(Some(u)) => {
            let (minimum, notes) = split_minimum_version(u.body.as_deref().unwrap_or(""));
            let mandatory = minimum
                .as_ref()
                .is_some_and(|m| app.package_info().version < *m);
            tracing::info!(version = %u.version, mandatory, "update: newer version available");
            UpdateStatus {
                state: "available".to_string(),
                current_version: current,
                version: Some(u.version.clone()),
                notes,
                minimum_version: minimum.map(|m| m.to_string()),
                mandatory,
                error: None,
            }
        }
        Ok(None) => {
            tracing::info!("update: already up to date");
            UpdateStatus::simple("uptodate", current)
        }
        Err(e) => {
            tracing::warn!(error = %e, "update: check failed");
            UpdateStatus::errored(current, e.to_string())
        }
    }
}

/// Download → verify → install the available update, emitting `update-progress`
/// `{ downloaded, contentLength }` events so the banners can show a bar, then
/// relaunch into the new version. Returns `Err` only on a pre-relaunch failure —
/// the success path never returns (`restart()` re-execs the app).
#[tracing::instrument(skip(app))]
pub async fn download_and_apply(app: &AppHandle) -> Result<(), String> {
    // Single-flight: refuse a concurrent install. On the happy path `restart()`
    // re-execs the process so the guard's reset never matters; on every error
    // path the drop guard clears it so a failed install can be retried.
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("An update is already being installed.".to_string());
    }
    struct ResetOnDrop;
    impl Drop for ResetOnDrop {
        fn drop(&mut self) {
            INSTALLING.store(false, Ordering::SeqCst);
        }
    }
    let _reset = ResetOnDrop;

    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available".to_string())?;

    let emitter = app.clone();
    let mut downloaded: usize = 0;
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk;
                let _ = emitter.emit(
                    "update-progress",
                    serde_json::json!({ "downloaded": downloaded, "contentLength": total }),
                );
            },
            || tracing::info!("update: download finished, installing"),
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("update: installed — restarting");
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

/// Delay before the first mandatory-update check after launch.
const ENFORCE_FIRST_DELAY: Duration = Duration::from_secs(30);
/// Steady-state cadence for the mandatory-update check. One network round trip
/// per hour per install — cheap enough to be unnoticeable, and it bounds how
/// long a machine can keep running a version the manifest has retired.
const ENFORCE_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// First retry delay after a *failed* forced install, doubling up to
/// [`ENFORCE_INTERVAL`]. A transient network blip must not cost a full cycle on
/// a machine we are actively trying to evict.
const ENFORCE_RETRY_BASE: Duration = Duration::from_secs(2 * 60);
/// Timer granularity. The loop wakes this often and compares a **wall-clock**
/// deadline rather than sleeping one long stretch, so a machine that suspends
/// mid-interval re-checks promptly on wake instead of resuming a timer that
/// stood still. No network happens on a tick that isn't due.
const ENFORCE_TICK: Duration = Duration::from_secs(60);

/// The next retry delay after a failed forced install: double, capped at the
/// steady-state interval so a persistently failing install settles into the
/// normal cadence instead of retrying forever at speed.
fn next_retry(current: Duration) -> Duration {
    (current * 2).min(ENFORCE_INTERVAL)
}

/// Mandatory-update enforcement — the one path that installs without consent.
/// Spawned once at launch; checks [`ENFORCE_FIRST_DELAY`] after startup and
/// then every [`ENFORCE_INTERVAL`] (the tray runs for weeks between relaunches,
/// so a launch-only check would never catch a mandatory release published
/// mid-run). When the manifest marks the running version as below its minimum,
/// it notifies and installs; on success [`download_and_apply`] never returns.
///
/// Two properties the naive "sleep the whole interval" loop did not have:
///
/// - **Survives suspend.** The deadline is wall-clock ([`SystemTime`]), and the
///   loop wakes every [`ENFORCE_TICK`] to compare against it. tokio's timer runs
///   on a monotonic clock that does not advance while macOS is asleep, so a
///   single long sleep silently stretched by however long the lid was shut — the
///   common case for a laptop we are trying to evict.
/// - **Retries a failed install quickly.** A failure previously waited the full
///   cycle; it now retries from [`ENFORCE_RETRY_BASE`], backing off via
///   [`next_retry`] to the steady interval. The consent banner remains the
///   fallback throughout.
///
/// The "no longer supported" toast fires once per mandatory episode, not once
/// per retry — notifying on every attempt would be a notification loop on a
/// machine that is simply offline.
pub fn enforce_minimum_version(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut due = SystemTime::now() + ENFORCE_FIRST_DELAY;
        let mut retry = ENFORCE_RETRY_BASE;
        let mut notified = false;
        loop {
            // Wake at most a tick at a time; `due` is the real gate below.
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO);
            tokio::time::sleep(wait.min(ENFORCE_TICK)).await;
            if SystemTime::now() < due {
                continue;
            }

            let status = check_status(&app).await;
            // `unsupported` (dev run) never flips to packaged mid-run, so the
            // loop is pure idle there; keep it anyway — it costs one no-op
            // check per cycle and avoids a second is_packaged() code path.
            if status.state != "available" || !status.mandatory {
                due = SystemTime::now() + ENFORCE_INTERVAL;
                retry = ENFORCE_RETRY_BASE;
                notified = false;
                continue;
            }
            let v = status.version.clone().unwrap_or_default();
            tracing::info!(
                version = %v,
                minimum = status.minimum_version.as_deref().unwrap_or(""),
                "update: running version is below the minimum supported - forcing install"
            );
            if !notified {
                // Warm and factual. The user did not ask for this install and
                // is about to have the app restart under them, so the toast
                // says what is happening and that it will restart — but it does
                // not scold them for the version they were on. Matches the tone
                // of the other update toasts ("You're on the latest version.").
                notify_update(
                    &app,
                    "mandatory",
                    "Updating Meridian",
                    &format!("Installing v{v} - Meridian will restart in a moment."),
                )
                .await;
                notified = true;
            }
            if let Err(e) = download_and_apply(&app).await {
                due = SystemTime::now() + retry;
                tracing::warn!(
                    error = %e,
                    retry_s = retry.as_secs(),
                    "update: forced install failed - retrying"
                );
                retry = next_retry(retry);
            } else {
                due = SystemTime::now() + ENFORCE_INTERVAL;
            }
        }
    });
}

/// Tray-menu entry ("Check for Updates…"). Notification-driven (the menu has no
/// window for a banner), reusing the same core as the in-app surfaces. Always
/// user-initiated, so it installs + relaunches on success and toasts every
/// outcome. The in-app banners are the primary path; this is the fallback.
pub fn check_for_updates(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if CHECKING.swap(true, Ordering::SeqCst) {
            tracing::info!("update: check already in progress — skipping");
            return;
        }
        let status = check_status(&app).await;
        match status.state.as_str() {
            "available" => {
                let v = status.version.clone().unwrap_or_default();
                notify_update(
                    &app,
                    "downloading",
                    "Updating Meridian",
                    &format!("Downloading v{v}…"),
                )
                .await;
                if let Err(e) = download_and_apply(&app).await {
                    tracing::warn!(error = %e, "update: install failed");
                    notify_update(
                        &app,
                        "failed",
                        "Update failed",
                        "The update couldn't be installed.",
                    )
                    .await;
                }
            }
            "uptodate" => {
                notify_update(
                    &app,
                    "uptodate",
                    "Meridian is up to date",
                    "You're on the latest version.",
                )
                .await;
            }
            "unsupported" => {
                notify_update(
                    &app,
                    "unsupported",
                    "Updates via npm",
                    "This build updates with `meridian update`.",
                )
                .await;
            }
            _ => {
                let msg = status
                    .error
                    .as_deref()
                    .unwrap_or("Couldn't reach the update server.");
                notify_update(&app, "check_failed", "Update check failed", msg).await;
            }
        }
        CHECKING.store(false, Ordering::SeqCst);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_doubles_then_caps_at_the_steady_interval() {
        // Doubles while below the cap …
        assert_eq!(next_retry(ENFORCE_RETRY_BASE), ENFORCE_RETRY_BASE * 2);
        assert_eq!(next_retry(ENFORCE_RETRY_BASE * 2), ENFORCE_RETRY_BASE * 4);
        // … and never exceeds the steady-state cadence, so a persistently
        // failing install settles into the normal rhythm instead of hammering.
        assert_eq!(next_retry(ENFORCE_INTERVAL), ENFORCE_INTERVAL);
        assert_eq!(next_retry(ENFORCE_INTERVAL * 4), ENFORCE_INTERVAL);
    }

    #[test]
    fn retry_backoff_reaches_the_cap_from_the_base() {
        // The base must actually climb to the cap in a bounded number of
        // failures — a base that divided the interval unevenly would otherwise
        // sit just under it forever.
        let mut d = ENFORCE_RETRY_BASE;
        for _ in 0..16 {
            d = next_retry(d);
        }
        assert_eq!(d, ENFORCE_INTERVAL);
    }

    #[test]
    fn no_marker_keeps_notes_verbatim() {
        let (min, notes) = split_minimum_version("Meridian v1.70.0");
        assert_eq!(min, None);
        assert_eq!(notes.as_deref(), Some("Meridian v1.70.0"));
    }

    #[test]
    fn marker_is_extracted_and_stripped() {
        let (min, notes) = split_minimum_version("Meridian v1.70.0\nMinimum-Version: 1.68.0");
        assert_eq!(min, Some(Version::new(1, 68, 0)));
        assert_eq!(notes.as_deref(), Some("Meridian v1.70.0"));
    }

    #[test]
    fn marker_matches_case_insensitively_and_tolerates_whitespace() {
        let (min, notes) = split_minimum_version("  minimum-version:   1.2.3  \nnotes");
        assert_eq!(min, Some(Version::new(1, 2, 3)));
        assert_eq!(notes.as_deref(), Some("notes"));
    }

    #[test]
    fn marker_only_notes_become_none() {
        let (min, notes) = split_minimum_version("Minimum-Version: 2.0.0");
        assert_eq!(min, Some(Version::new(2, 0, 0)));
        assert_eq!(notes, None);
    }

    #[test]
    fn unparsable_version_is_ignored_not_mandatory() {
        let (min, notes) = split_minimum_version("Meridian v1.70.0\nMinimum-Version: not-semver");
        assert_eq!(min, None);
        assert_eq!(notes.as_deref(), Some("Meridian v1.70.0"));
    }

    #[test]
    fn empty_notes_are_none() {
        let (min, notes) = split_minimum_version("");
        assert_eq!(min, None);
        assert_eq!(notes, None);
    }

    /// The comparison `check_status` performs: strictly-below is mandatory,
    /// equal or above is not.
    #[test]
    fn mandatory_is_strictly_below_minimum() {
        let min = Version::new(1, 68, 0);
        assert!(Version::new(1, 67, 9) < min);
        assert!(Version::new(1, 68, 0) >= min);
        assert!(Version::new(1, 69, 0) >= min);
    }
}
