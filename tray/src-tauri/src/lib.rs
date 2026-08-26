//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Meridian tray — app bootstrap and wiring.
//!
//! This file is intentionally thin: it builds the Tauri app, opens the shared
//! `meridian.db` pool, installs the tray, registers the command surface, and
//! spawns the poll loop. The substance lives in focused modules:
//!
//! - [`commands`] — the `#[tauri::command]` surface (grouped by domain).
//! - [`tray`]     — tray menu construction + menu-event dispatch.
//! - [`poll`]     — the background health/active/today/worklogs poll loop.
//! - [`state`]    — the shared [`state::AppState`] the poll loop maintains.
//! - [`install`]     — install-mode + db-path resolution.
//! - [`sys`]         — shared uid / notify / dashboard-URL helpers.
//! - [`format`]      — duration formatting for the popover.
//! - [`analytics`]   — PostHog product-analytics capture (DMG installs only).
//! - [`counter_ping`] — public "updates logged" live-counter ping (prod channel only).
//! - [`reopen`]      — where an external re-activation lands, and the onboarding gate.
//! - [`window_panel`] — popover placement + the macOS NSPanel behaviour.

mod analytics;
mod autostart;
mod backend_install;
#[cfg(feature = "capture")]
mod capture;
// Non-gated on purpose: the ignore-list matcher is a plain data type held in
// `AppState` and refreshed by the always-compiled `update_settings` command, so
// it must exist in non-capture builds too (nothing reads it there — harmless).
mod capture_ignore;
mod commands;
mod counter_ping;
mod crash;
mod daemon_lifecycle;
mod db_key;
mod db_pool;
mod deep_link;

/// Lowercase product name as macOS reports it after [`set_process_display_name`].
/// The capture engine uses this to skip self-capture — keep it in sync with the
/// literal passed to that function.
#[cfg(feature = "capture")]
pub(crate) const SELF_PRODUCT_NAME_LOWER: &str = "meridian";

/// Binary name Cargo embeds (dev / ad-hoc builds, before the display name is set).
#[cfg(feature = "capture")]
pub(crate) const SELF_BINARY_NAME: &str = "meridian-tray";
pub(crate) mod format;
mod install;
// Runs a requested database repair during startup, before anything opens
// meridian.db. See its header for why repair cannot happen mid-session.
mod poll;
mod relocate;
mod reopen;
mod repair_boot;
mod state;
mod sys;
// Toast-XML construction for the Windows interactive path. Compiled on every
// platform on purpose so its escaping/ordering/cap logic is unit-tested from a
// macOS dev machine; only `win_toast`'s WinRT calls are Windows-gated.
mod toast_actions;
mod tray;
mod tray_icon;
mod update;
#[cfg(target_os = "windows")]
mod win_toast;
mod window_panel;

// Gated to match the ONLY thing that uses these — the `RunEvent::Reopen` arm at
// the bottom of `run()`, which is `#[cfg(target_os = "macos")]` because Reopen is
// a Dock/Finder activation that no other platform emits. `reopen.rs` already
// carries `cfg_attr(not(target_os = "macos"), allow(dead_code))` on all three
// items, so the definitions were covered; only this import was left unconditional,
// and an unused import is an ERROR under `-D warnings`.
//
// `onboarding_complete` below is deliberately NOT gated: it is called on every
// platform (the `capture`-gated startup path in `run()`).
#[cfg(target_os = "macos")]
use reopen::{is_onboarded, reopen_target, ReopenTarget};
// Re-exported at the crate root so `tray.rs` and `commands::system` keep calling
// `crate::onboarding_complete()` — the exact string `reopen::reopen_tests`'
// source-scan asserts on.
pub(crate) use reopen::onboarding_complete;
#[cfg(target_os = "macos")]
use window_panel::{
    apply_corner_mask, init_as_nspanel, install_click_outside_monitor,
    make_visible_over_fullscreen, set_process_display_name, show_no_focus,
};
use window_panel::{monitor_work_area, tray_anchor_position};

use state::{AppState, HealthStatus};
use std::sync::{Arc, Mutex};
#[cfg(not(target_os = "macos"))]
use tauri::WindowEvent;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

/// Runs `f`, converting a panic into `None` instead of letting it propagate.
///
/// `setup()` runs synchronously inside tao's `did_finish_launching`, itself
/// invoked by Cocoa across an `extern "C"` boundary. An unwinding panic
/// cannot cross that boundary — Rust detects it and calls
/// `panic_cannot_unwind`, aborting the ENTIRE process before the tray icon
/// exists, regardless of this workspace's `panic = "unwind"` profile
/// (`Cargo.toml`'s release-profile guard explains why that's chosen
/// deliberately over `panic = "abort"`: "we ship a data daemon; no"). Stopping
/// the unwind here, inside ordinary Rust frames well short of that boundary,
/// is what makes the profile's guarantee actually hold for `setup()`.
///
/// `be368d4e` fixed one instance of exactly this failure mode (sqlx's lazy
/// pool builder panicking outside a Tokio context) by hardening that one call
/// site. This contains the FAILURE MODE itself, so the next unforeseen panic
/// inside a wrapped span degrades the tray instead of bricking it — and, for
/// spans wrapped before `update::enforce_minimum_version` runs later in
/// `setup()`, keeps that force-update kill switch reachable instead of a
/// fatal abort taking it down along with everything else. This is NOT
/// blanket protection for all of `setup()` — only the spans actually passed
/// through this wrapper (currently the DB key/pool/encryption-notice work;
/// see its call sites) are covered. A panic in unwrapped code — the tray
/// menu build, the tray icon, anything after it — still aborts the process.
///
/// `what` is folded into the log MESSAGE, not passed as a separate `tracing`
/// attribute key: an attribute key not on `redact::SAFE_STRING_KEYS` is
/// stripped from the packaged-build ship leg (see CLAUDE.md's observability
/// section), which would silently gut the one line telling us a startup
/// panic happened.
fn catch_setup_panic<T>(what: &str, f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
    match std::panic::catch_unwind(f) {
        Ok(v) => Some(v),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string());
            tracing::error!(
                "tray setup panicked during {what}: {msg} — degrading to a disabled state instead of crashing the tray"
            );
            None
        }
    }
}

#[cfg(test)]
mod catch_setup_panic_tests {
    use super::catch_setup_panic;

    #[test]
    fn returns_the_value_on_success() {
        assert_eq!(catch_setup_panic("test", || 42), Some(42));
    }

    /// The actual point of the wrapper: a panic must degrade to `None`, not
    /// propagate — Rust's default panic hook still prints to stderr for this
    /// case, which is expected, harmless test noise.
    #[test]
    fn returns_none_instead_of_propagating_a_panic() {
        let result = catch_setup_panic("test", || -> i32 { panic!("boom") });
        assert_eq!(result, None);
    }
}

#[cfg(test)]
mod single_instance_tests {
    /// The second-instance callback must never open or focus a window.
    ///
    /// Source-scanned because the thing being asserted is "nothing appeared",
    /// which a headless test has no window to observe — the same reasoning as
    /// `ui/__tests__/no-native-dialogs.test.ts`. What it protects is specific:
    /// every single-instance example on the internet focuses the main window,
    /// so the natural "improvement" to this callback is precisely the bug. The
    /// common caller is the new-day relaunch trigger on a machine nobody touched.
    #[test]
    fn the_second_instance_callback_opens_nothing() {
        let src = include_str!("lib.rs");
        // Newline-anchored so it matches the DEFINITION and not the string
        // literal on this very line — searching for the bare name found this
        // test first and made it fail against its own forbidden-word list.
        let start = src
            .find("\nfn on_second_instance(")
            .expect("callback renamed - re-check this test's needles");
        let body_end = src[start..]
            .find("\npub fn run()")
            .expect("callback is expected to sit directly above run()");
        let body = &src[start..start + body_end];
        for forbidden in [
            "open_native_dashboard",
            "open_wizard_window",
            "set_focus",
            ".show()",
        ] {
            assert!(
                !body.contains(forbidden),
                "on_second_instance must not call {forbidden} - an unattended \
                 relaunch would put a window on screen"
            );
        }
    }
}

/// What the surviving instance does when a second one was blocked: **nothing
/// but log**.
///
/// This is a deliberate departure from every single-instance example, which
/// focuses or shows the main window. Doing that here would be actively wrong:
/// the overwhelmingly common caller is the new-day relaunch trigger passing
/// `--autostart`, i.e. the OS starting Meridian on a machine nobody touched. A
/// window appearing then is exactly the annoyance that makes people disable
/// autostart, which costs us the capture the tray exists to perform.
///
/// A user-initiated activation does NOT arrive here. Clicking the app in
/// Finder, Spotlight or the Dock goes through LaunchServices, which activates
/// the running instance and fires `RunEvent::Reopen` — handled separately by
/// [`reopen`], which opens the dashboard. This callback only sees a direct
/// re-exec of the binary, which on this app means a scheduler.
///
/// So the correct behaviour is to let the second process die quietly and carry
/// on. Logged at DEBUG rather than INFO because on a machine left running for
/// weeks this fires once a day, forever, and says nothing new each time.
fn on_second_instance(_app: &tauri::AppHandle, args: Vec<String>, _cwd: String) {
    tracing::debug!(
        unattended = autostart::args_indicate_autostart(args.iter().map(String::as_str)),
        "single-instance: a second launch was blocked - staying as we are"
    );
}

pub fn run() {
    // Native-crash capture (Phase 2B). MUST be first: `tauri-plugin-sentry`'s
    // minidump reporter relaunches this exe in a special reporter mode that has
    // to short-circuit before any app work. `crash::init_client()` returns None
    // — and Sentry stays entirely off — unless the user has error reporting
    // enabled AND a DSN was baked into this release build (see crash.rs). The
    // client guard is held for the process lifetime; the minidump reporter and
    // the plugin (below) only wire up when the client actually initialised.
    let sentry_client = crash::init_client();
    #[cfg(not(target_os = "ios"))]
    // Closure (not the bare fn) so `&ClientInitGuard` deref-coerces to the
    // `&Client` the reporter wants.
    let _sentry_minidump = sentry_client
        .as_ref()
        .map(|c| tauri_plugin_sentry::minidump::init(c));

    // Uptime baseline for `analytics::health`. Deliberately AFTER the minidump
    // reporter, not before it: `crash::init_client` above must be the first
    // thing this function does, because the reporter relaunches this exe in a
    // special mode that short-circuits before any app work, and a process that
    // exists only to upload a crash dump has no meaningful "uptime" to record.
    sys::mark_process_start();

    // Tray telemetry. Emit the tray's own spans + logs (service.name =
    // meridian-tray) into the shared OTLP spool — the same one the daemon writes
    // and its shipper drains. In a PACKAGED install this is how tray errors
    // reach the central backend (redacted, error-only, on the shipper's
    // consent-gated central path): the tray is otherwise DARK in release, and it
    // is the one process the user actually clicks. `meridian` is always a
    // dependency (Export Diagnostics), so this only calls the existing init — it
    // pulls in no new deps. In a debug build the daemon's init also installs a
    // stdout mirror, so `cargo run` stays console-observable without a separate
    // subscriber; capture-to-spool honours the MERIDIAN_TELEMETRY_DISABLED kill
    // switch inside init().
    //
    // Must run INSIDE Tauri's Tokio runtime: the OTLP batch exporter spawns a
    // background task and panics ("no reactor running") if called before one
    // exists. `block_on` enters the global runtime so the spawn succeeds; the
    // guard is held for the process lifetime so the exporter keeps flushing.
    let _obs_guard =
        tauri::async_runtime::block_on(async { meridian::observability::init("meridian-tray") })
            .ok();

    let app_state = Arc::new(Mutex::new(AppState::default()));

    let mut builder = tauri::Builder::default()
        // SINGLE INSTANCE MUST BE FIRST. Two reasons, and the second is ours.
        //
        // Upstream requires it so the plugin runs before anything else can
        // interfere. For Meridian it is also what guarantees the doomed second
        // process exits BEFORE it can create a tray icon or, far worse, a
        // capture writer against `meridian.db`.
        //
        // Why a second process happens at all: the new-day relaunch trigger
        // (`autostart`) is started by the OS scheduler, and neither scheduler can
        // see that the app is already running. launchd tracks liveness per job
        // LABEL, so the process macOS's loginwindow started via SMAppService is
        // invisible to our calendar job; Task Scheduler's
        // `MultipleInstancesPolicy: IgnoreNew` only dedupes the task's own
        // instances. So the trigger fires daily into a running app, and without
        // this the result is two processes writing one SQLite file — the
        // double-writer condition behind `database disk image is malformed`.
        //
        // The callback runs in the FIRST (surviving) instance. It deliberately
        // does nothing but log — see `on_second_instance`.
        .plugin(tauri_plugin_single_instance::init(on_second_instance))
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_opener::init())
        // DMG auto-update: reads endpoint + minisign pubkey from tauri.conf.json.
        // Registered unconditionally; the check is a no-op in a source/dev run
        // (the running binary isn't a packaged `.app` for the updater to swap).
        .plugin(tauri_plugin_updater::Builder::new().build());
    // Sentry webview + backend integration (Phase 2B) — only when the crash
    // client initialised (consent + baked DSN). This injects @sentry/browser
    // into the webview and routes its events through the Rust client so JS and
    // native crashes share one project + device context.
    if let Some(client) = &sentry_client {
        builder = builder.plugin(tauri_plugin_sentry::init(client));
    }
    // Clerk email sign-in on the setup wizard (see commands::account and
    // ui/app/setup/signin.tsx). `ClerkPluginBuilder::build()` HARD-FAILS
    // `.setup()` (killing the whole tray, not just sign-in) when the
    // publishable key is empty or malformed — so, like the notifications
    // plugin below, this only registers when a real key is configured. A
    // source build with no `MERIDIAN_CLERK_PUBLISHABLE_KEY` baked in and no
    // `CLERK_PUBLISHABLE_KEY` env override degrades to no Clerk plugin at
    // all; `signin.tsx`'s `SignInGate` catches `initClerk()` failing to find
    // the plugin and shows a message instead of hanging. `tauri_plugin_http`
    // is the transport the Clerk plugin patches `fetch` through;
    // `tauri_plugin_store` gives it persistent session storage so a relaunch
    // doesn't force re-login.
    let clerk_key = commands::account::clerk_publishable_key();
    if !clerk_key.is_empty() {
        builder = builder
            .plugin(tauri_plugin_http::init())
            .plugin(tauri_plugin_store::Builder::new().build())
            .plugin(
                tauri_plugin_clerk::ClerkPluginBuilder::new()
                    .publishable_key(clerk_key)
                    .with_tauri_store()
                    .build(),
            );
    } else {
        tracing::info!(
            "no Clerk publishable key configured — Clerk plugin not registered, setup wizard sign-in unavailable"
        );
    }
    // Interactive notifications (UNUserNotificationCenter via the community
    // notifications plugin). Its init HARD-FAILS outside a `.app` bundle, which
    // would crash `tauri dev` / `cargo run` — so it's registered only when
    // bundled. Unbundled runs drop toasts (sys::notify* degrade to logged
    // no-ops) — the same packaged-only caveat as the popover asset layout.
    if sys::is_bundled() {
        builder = builder.plugin(tauri_plugin_notifications::init());
    } else {
        tracing::info!("unbundled run — notifications plugin not registered, toasts disabled");
    }
    // NOTE: there is no autostart PLUGIN here any more. `tauri-plugin-autostart`
    // could only write `RunAtLoad`, so it had no way to express the morning
    // relaunch, and it named its plist after `productName` rather than
    // `com.meridiona.*`, which is why every uninstall left the login item
    // behind. `autostart.rs` owns the plist / scheduled task outright now; it is
    // called from `setup()` below, still bundled-only.
    builder
        .manage(app_state.clone())
        // Pending dashboard navigation target (e.g. "/plan") — set by tray-side
        // openers, pulled by the dashboard shell on mount. See `deep_link`.
        .manage(deep_link::PendingDeepLink(std::sync::Mutex::new(None)))
        .setup(move |app| {
            // Move-to-Applications self-relocation. Must run before anything
            // else touches the DB pool or spawns the poll loop: a `true`
            // return means a replacement process now exists at
            // `/Applications` and this (transient DMG/translocation)
            // instance must exit immediately, not continue starting up
            // alongside it. Bundled only — an unbundled `cargo run`/`tauri
            // dev` binary under `target/` is never "transient" in the sense
            // this guards against. See `relocate.rs`.
            if sys::is_bundled() && relocate::maybe_relocate_to_applications() {
                std::process::exit(0);
            }

            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            // Dock tooltip, Activity Monitor, and the AX tree all key on the
            // process name. In dev the binary is "meridian-tray"; production
            // bundles show "Meridian" from productName — set it explicitly here
            // so both modes are consistent.
            #[cfg(target_os = "macos")]
            set_process_display_name("Meridian");

            // Request OS notification authorization up front (without it,
            // delivery is a silent no-op until the app has prompted at least
            // once), then register the interactive category set — the fixed
            // UNNotificationCategory ids that give toasts their buttons
            // (meridian-core's `notifications::categories`, one source shared
            // with the daemon producers). Bundled runs only: the plugin isn't
            // registered otherwise (see the builder above).
            if sys::is_bundled() {
                use tauri_plugin_notifications::{NotificationsExt, PermissionState};
                // Register the interactive category set SYNCHRONOUSLY, before
                // setup() returns and the poll loop is spawned. Otherwise the
                // loop's first tick could deliver an interactive toast (a nudge
                // already due at launch) before its `action_type_id` is
                // registered, rendering that one toast button-less. Registration
                // needs no authorization, so it's safe up front; only the
                // permission prompt — which gates whether delivery happens at
                // all, never the buttons — stays async.
                sys::register_notification_categories(app.handle());
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let notifier = handle.notifications();
                    if !matches!(
                        notifier.permission_state().await,
                        Ok(PermissionState::Granted)
                    ) {
                        let _ = notifier.request_permission().await;
                    }
                });
            }

            // Resolve the local SQLCipher encryption key BEFORE opening the
            // pool. Two separate concerns, deliberately gated differently:
            //
            // 1. USING an existing key (any install mode, including Dev): if
            //    `MERIDIAN_DB_KEY` is already sitting in the resolved `.env`,
            //    always read and apply it. `meridian.db` is the one file dev
            //    and installed builds share (`meridian_db_path`'s doc
            //    comment) — if it's ALREADY encrypted (e.g. this same machine
            //    also runs a Canonical install against it), a dev build must
            //    still be able to read it, not silently open unencrypted and
            //    fail on the first real query.
            // 2. GENERATING a new key + migrating an existing plaintext file
            //    (release builds ONLY, via `cfg!(debug_assertions)` — NOT
            //    gated on `detect_install_mode() == Canonical`, deliberately:
            //    that enum only resolves to `Canonical` once `~/.meridian/.env`
            //    already exists, which is NOT yet true on a brand-new
            //    install's very first launch (see `canonical_env_path`'s doc
            //    comment on why the WRITE target must not require the file
            //    to pre-exist) — gating generation on the enum instead of the
            //    compile-time check would mean a new user's first-ever launch
            //    never gets encrypted at all. `cfg!(debug_assertions)` mirrors
            //    `detect_install_mode`'s own compile-time Canonical/Dev split,
            //    so a debug build provably can't reach this branch either
            //    way. Both degrade to `None`/unencrypted on any failure
            //    rather than blocking tray startup — see db_key.rs and
            //    meridian_core::db_crypto::encrypt_in_place's doc comments
            //    for why that's the safe failure mode.
            // The whole span through the encryption notice below runs inside
            // `catch_setup_panic`: it does keychain access, an in-place file
            // migration, and builds sqlx's lazy pool — the exact operation
            // `be368d4e` fixed one instance of (see that helper's doc). An
            // unforeseen panic anywhere in this span now degrades to the SAME
            // `db_pool: None` state every other failure here already
            // produces, instead of taking the whole process down.
            let db_setup_result = catch_setup_panic(
                "meridian.db key resolution + pool build + encryption notice",
                std::panic::AssertUnwindSafe(|| {
                    let db_path = install::meridian_db_path();
                    // Bound once: the same path is re-borrowed as a `Path` several
                    // times below (key resolution, the orphan check, the plaintext
                    // probe, the migration), and `db_path` itself stays a `String`
                    // because the tracing/`open` call sites still want it as one.
                    let db_path_ref = std::path::Path::new(&db_path);
                    let install_mode = install::detect_install_mode();
                    let existing_key = install_mode
                        .env_path()
                        .and_then(|p| install::env_key_from_path(p, "MERIDIAN_DB_KEY"));

                    let db_key_hex = if existing_key.is_some() {
                        existing_key
                    } else if !cfg!(debug_assertions) {
                        match install::canonical_env_path() {
                            Some(env_path) => {
                                match db_key::resolve_or_create_key(&env_path, db_path_ref) {
                                    Ok(key) => Some(key),
                                    Err(e) => {
                                        // `tracing`, not `eprintln!`: observability is
                                        // already initialised above, and on a packaged
                                        // install stderr goes only to the launchd log -
                                        // never `meridian logs`, the diagnostics bundle,
                                        // or central error reporting. A silent fallback to
                                        // an UNENCRYPTED database is exactly the event
                                        // those channels exist to surface.
                                        //
                                        // `would_orphan_existing_db` is a DIFFERENT, worse
                                        // case than the generic fallback below: the DB
                                        // already exists and is NOT plaintext, so there is
                                        // no unencrypted file to "continue" into - opening
                                        // it further down with no key will just fail, the
                                        // same way it's been failing already. The
                                        // DB-backed notices system (used elsewhere in this
                                        // function) can't carry this one: it's the very
                                        // database that's unreadable. A native OS
                                        // notification is the only channel left that
                                        // doesn't itself depend on meridian.db opening.
                                        if db_key::would_orphan_existing_db(db_path_ref) {
                                            tracing::error!(
                                                error = %e,
                                                "refusing to generate a replacement DB encryption key - meridian.db exists and appears already encrypted under a key this install can no longer find"
                                            );
                                            sys::notify(
                                                app.handle(),
                                                "Meridian can't read your local data",
                                                "Your local database appears to be encrypted with a key this install can no longer find. Contact support with your Support ID (Settings -> Account) before removing anything.",
                                            );
                                        } else {
                                            tracing::error!(
                                                error = %e,
                                                "failed to resolve DB encryption key - continuing unencrypted"
                                            );
                                        }
                                        None
                                    }
                                }
                            }
                            None => None,
                        }
                    } else {
                        None
                    };

                    // Whether encryption was intended this launch (a key was resolved).
                    // Re-checked against the on-disk file after the pool opens, to raise
                    // a user-visible notice if the DB is nonetheless still plaintext
                    // (the encrypt-in-place migration didn't complete) — otherwise the
                    // db_crypto plaintext guard keeps operating unencrypted with only a
                    // log line nobody sees. Runtime-gated below (not `#[cfg]`) so it is
                    // always compiled/linted, matching the migration block's own
                    // `!cfg!(debug_assertions)` style.
                    let db_encryption_intended = db_key_hex.is_some();

                    // The daemon must not be writing while `encrypt_in_place` swaps
                    // meridian.db. Stop it first, but ONLY when a migration will
                    // actually attempt (a key was resolved AND the DB is still
                    // plaintext); once encrypted this never runs again, so the stop is a
                    // one-time cost on the migration launch. The daemon is brought back
                    // up by `ensure_backend_installed` later in this same setup hook.
                    //
                    // Both platforms need this, for opposite reasons — on Windows the
                    // rename FAILS while the daemon holds the file (os error 32), on
                    // macOS it SUCCEEDS and corrupts the database instead. See
                    // `backend_install::stop_daemon_for_migration` for the mechanism.
                    //
                    // `None` when no stop was attempted, because nothing is going to
                    // migrate anyway (already encrypted, no key, or a debug build).
                    let stop_outcome = if !cfg!(debug_assertions)
                        && db_encryption_intended
                        && meridian_core::db_crypto::is_plaintext_sqlite(db_path_ref)
                    {
                        Some(tauri::async_runtime::block_on(
                            backend_install::stop_daemon_for_migration(db_path_ref),
                        ))
                    } else {
                        None
                    };
                    if let Some(Err(e)) = &stop_outcome {
                        // ERROR, and it GATES the migration below. Warning and migrating
                        // anyway is what shipped in v1.80.0, and on macOS that is
                        // precisely the data-destroying path: the swap goes ahead under
                        // a live writer. Leaving the DB plaintext for one more launch is
                        // the cheap failure.
                        tracing::error!(
                            error = %e,
                            "could not stop the daemon before encrypting the database - skipping the migration this launch; the database stays plaintext and will be retried next launch"
                        );
                    }
                    // The decision itself lives in `backend_install` so it can be
                    // unit-tested — this closure cannot be.
                    let safe_to_migrate =
                        backend_install::may_swap_database(stop_outcome.as_ref());

                    let db_key_hex = if !cfg!(debug_assertions) {
                        match &db_key_hex {
                            Some(key) if safe_to_migrate => {
                                let migrate =
                                    meridian_core::db_crypto::encrypt_in_place(db_path_ref, key);
                                match tauri::async_runtime::block_on(migrate) {
                                    Ok(()) => Some(key.clone()),
                                    Err(e) => {
                                        tracing::error!(
                                            error = %e,
                                            "failed to migrate meridian.db to encrypted storage - continuing unencrypted this run"
                                        );
                                        None
                                    }
                                }
                            }
                            // Migration skipped (see above). Keep the resolved key:
                            // `db_crypto`'s plaintext guard drops it per-connection
                            // while the file is still in the clear, and it is needed
                            // unchanged the moment a later launch does migrate.
                            Some(key) => Some(key.clone()),
                            None => None,
                        }
                    } else {
                        db_key_hex
                    };

                    // A repair requested last session runs HERE and nowhere else:
                    // the key is resolved (so an encrypted database can be
                    // opened) but no pool exists and capture has not started, so
                    // the tray provably holds nothing open. The daemon is
                    // standing down on the marker meanwhile. See
                    // `repair_boot`'s header for why an in-session repair is not
                    // workable - a rebuilt file leaves every existing connection
                    // bound to the inode that was moved aside, reading stale
                    // data forever without erroring.
                    let repair_outcome =
                        repair_boot::run_if_requested(db_path_ref, db_key_hex.as_deref())
                            // No repair was requested - probe the file anyway
                            // and heal it unattended if it is damaged. This is
                            // the path that recovers a machine whose corrupt
                            // database prevented the repair offer from ever
                            // being shown (see `repair_boot`'s module header).
                            .or_else(|| {
                                repair_boot::run_auto_if_needed(
                                    db_path_ref,
                                    db_key_hex.as_deref(),
                                )
                            });

                    // ALPHA TESTING ONLY — retention half of the raw-email
                    // exception. Consumers already refuse to SHIP the address
                    // past its expiry, and the write is now gated too, but a
                    // tester who signs in before the boundary and stays signed
                    // in never re-runs that write. Without this, their raw
                    // address would sit in `settings.json` indefinitely. Cheap:
                    // one settings read, and a write only on the single launch
                    // that crosses the boundary.
                    commands::account::purge_expired_account_email();

                    // Prepare the meridian.db pool ONCE at startup and share it with
                    // commands via managed state (no migrations — the daemon owns the
                    // schema). `None` only if the path/key is malformed, so reads error
                    // gracefully instead of crashing the tray.
                    //
                    // LAZY on purpose. On a first launch this line runs seconds BEFORE
                    // `ensure_backend_installed` (below) starts the daemon that creates
                    // meridian.db, so an eager open fails — and since the result is
                    // stored as an `Option` that nothing retries, the tray then ran its
                    // entire session against `None`: every DB-backed command silently
                    // returned its empty default and every captured frame was dropped
                    // by the consumers' `else { continue }`, until the user happened to
                    // restart the tray. A lazy pool connects on first use instead, so
                    // the very next command or frame after the daemon creates the file
                    // succeeds on its own.
                    // `block_on`, not a bare call: sqlx spawns the pool's maintenance
                    // task while BUILDING the lazy handle, so constructing it outside a
                    // Tokio runtime panics ("this functionality requires a Tokio
                    // context") and takes the whole tray down before the tray icon even
                    // appears. `setup()` is not itself async, so the runtime has to be
                    // entered explicitly here — exactly as the eager open it replaced
                    // already did. Should this or any sibling third-party call in this
                    // span panic outright instead of returning an `Err`,
                    // `catch_setup_panic` around this whole block is the backstop.
                    let db_pool = tauri::async_runtime::block_on(meridian_core::open_existing_lazy(
                        &db_path,
                        db_key_hex.as_deref(),
                    ))
                    .map_err(
                        |e| tracing::error!(error = %e, db_path = %db_path, "meridian.db pool not prepared"),
                    )
                    .ok();
                    // A lazy pool cannot report reachability at build time, and losing
                    // that startup signal is how the failure above stayed invisible for
                    // a whole session. Probe once, purely to log it — an unreachable DB
                    // here is expected on a first launch and heals by itself, so this
                    // deliberately gates nothing.
                    //
                    // SPAWNED, never blocked on: sqlx retries a failed connection until
                    // the pool's 10s acquire timeout, so a first launch (file not
                    // created yet) would otherwise freeze the whole `setup` closure —
                    // and with it the tray menu — for ten seconds.
                    if let Some(p) = db_pool.clone() {
                        tauri::async_runtime::spawn(async move {
                            match meridian_core::ping(&p).await {
                                Ok(()) => tracing::info!("meridian.db reachable"),
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    "meridian.db not reachable yet — the pool will connect once the daemon creates it"
                                ),
                            }
                        });
                    }
                    // Handed back as this closure's return value below, so the
                    // caller can guarantee `app.manage` runs even when this whole
                    // span panicked before reaching it a few lines down — see
                    // there. Unconditional now (not `#[cfg(feature = "capture")]`
                    // as it was before this span grew a wrapper): the capture
                    // feature is the only later consumer, but the clone itself is
                    // cheap and every build needs a value to return.
                    let capture_pool = db_pool.clone();
                    // The encryption-state notice below needs a pool handle before
                    // db_pool is moved into managed state.
                    let encryption_notice_pool = db_pool.clone();
                    // Wrapped so `commands::daemon::reload_daemon` can close and
                    // reopen it around a daemon restart instead of holding one
                    // connection across two different daemon process
                    // generations — see `db_pool::DbPool`'s header for the
                    // corruption incident this closes off.
                    app.manage(db_pool::DbPool::new(
                        db_pool,
                        db_path.clone(),
                        db_key_hex.clone(),
                    ));

                    // Report the repair now that there is a pool to write a notice
                    // with. Deliberately after `app.manage`: the notice is the
                    // only trace the user sees of an operation that happened
                    // before the window existed, but it must never be able to
                    // cost them the pool itself — so this reads `capture_pool`,
                    // never the `db_pool` that was just moved above, and the
                    // pool is already managed by the time this block runs at all.
                    if let Some(outcome) = repair_outcome {
                        if let Some(p) = capture_pool.clone() {
                            // Own the strings before the task takes them: the
                            // notice borrows its fields, and `outcome` cannot
                            // outlive this scope. `fault_cleared` decides both
                            // halves below: whether the db.corrupt banner is
                            // dropped, and which notice replaces it. A fresh
                            // start clears the fault too - the damaged file is
                            // gone from the canonical path - but reports under
                            // its own id/severity so data loss is never
                            // dressed up as a clean repair.
                            let (fault_cleared, id, severity, title, detail, remedy) = match outcome
                            {
                                repair_boot::Outcome::Repaired { summary } => (
                                    true,
                                    "db.repaired",
                                    "info",
                                    "Database repaired",
                                    summary,
                                    None,
                                ),
                                repair_boot::Outcome::Failed { error } => (
                                    false,
                                    "db.corrupt",
                                    "error",
                                    "Database repair failed",
                                    error,
                                    Some(
                                        "Your data is unchanged. Contact support with your Support ID.",
                                    ),
                                ),
                                repair_boot::Outcome::FreshStart { backup } => (
                                    true,
                                    "db.reset",
                                    "warning",
                                    "Database was reset",
                                    format!(
                                        "Your database could not be opened or repaired, so tracking restarted with a new one. The old file was kept at {backup}."
                                    ),
                                    Some(
                                        "Contact support with your Support ID if you want to attempt recovery of the old file.",
                                    ),
                                ),
                            };
                            tauri::async_runtime::spawn(async move {
                                // RETRIED, not fire-once: this notice can outrun
                                // its own database. After a fresh start the
                                // canonical path is empty until the daemon
                                // relaunches and migrates - which on a machine
                                // leaving the watchdog's "giving up" window can
                                // be most of an hour - so a single write through
                                // the lazy pool fails silently, and the user who
                                // just lost their database is exactly the user
                                // who must see this message. The loop is idle
                                // and bounded; it ends on the first success.
                                // (A repair-failed notice against a still-
                                // unopenable file can never land - the retries
                                // then just run out, and the tracing::error!
                                // at the failure site remains the record.)
                                let deadline = std::time::Instant::now()
                                    + std::time::Duration::from_secs(2 * 60 * 60);
                                loop {
                                    let mut ok = true;
                                    if fault_cleared {
                                        // The fault is gone; drop the banner that
                                        // sent the user here in the first place.
                                        //
                                        // `clear_typed`, NOT the plain `clear` (which
                                        // hardcodes `event_key = "system.fault"`): the
                                        // daemon raised this with `event_key:
                                        // "db.corrupt"` (see `DB_CORRUPT_NOTICE` in
                                        // `src/main.rs`), and `notices::raise_typed`
                                        // dedupes its OS toast on `<event_key>:<id>`.
                                        // Clearing with the wrong event_key deletes the
                                        // banner row (that DELETE matches by id alone)
                                        // but retracts the wrong toast dedup key, so a
                                        // future corruption would silently never toast
                                        // again — deduped against a delivery that was
                                        // never actually retracted.
                                        ok &= meridian::notices::clear_typed(
                                            &p,
                                            "db.corrupt",
                                            "db.corrupt",
                                        )
                                        .await
                                        .is_ok();
                                    }
                                    ok &= meridian::notices::raise_typed(
                                        &p,
                                        meridian::notices::Notice {
                                            id,
                                            severity,
                                            title,
                                            detail: &detail,
                                            remedy,
                                            // event_key mirrors id so each outcome's
                                            // OS toast dedupes independently.
                                            event_key: id,
                                            deep_link: (!fault_cleared).then_some(meridian_core::notifications::deep_links::LOGS),
                                        },
                                    )
                                    .await
                                    .is_ok();
                                    if ok || std::time::Instant::now() > deadline {
                                        if !ok {
                                            tracing::error!(
                                                notice_id = id,
                                                "gave up delivering the repair-outcome notice - the database never became writable"
                                            );
                                        }
                                        break;
                                    }
                                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                                }
                            });
                        }
                    }

                    // Encryption was intended (a key was resolved) but the on-disk DB is
                    // still plaintext ⇒ encrypt_in_place didn't complete (on Windows the
                    // daemon holds the file open, so its final rename fails every
                    // launch). The db_crypto guard keeps the app working by opening
                    // plaintext — a silent security downgrade otherwise — so surface it
                    // to the user, and clear the notice once the DB is actually
                    // encrypted. Skipped in debug builds, where the migration never runs
                    // and a plaintext dev DB is expected. Best-effort: a notice-write
                    // failure must not block startup.
                    //
                    // Contained in its OWN `catch_setup_panic`, separate from the outer
                    // one wrapping this whole span: `app.manage(db_pool)` above already
                    // ran by this point, so a panic here must not cost `capture_pool` —
                    // that's this closure's still-pending return value below, and losing
                    // it would silently drop every captured frame for the rest of the
                    // session (`start_capture`'s caller degrades a missing pool exactly
                    // that quietly) while the DB itself keeps working normally.
                    let _ = catch_setup_panic(
                        "meridian.db encryption notice",
                        std::panic::AssertUnwindSafe(|| {
                            if !cfg!(debug_assertions) {
                                if let Some(pool) = encryption_notice_pool.as_ref() {
                                    let plaintext =
                                        meridian_core::db_crypto::plaintext_state(db_path_ref);
                                    let action = db_encryption_notice_action(
                                        db_encryption_intended,
                                        plaintext,
                                    );
                                    // No `db_path` on the span: a home-dir path is user data.
                                    let span =
                                        tracing::info_span!("tray.db_encryption_notice", ?action);
                                    let _entered = span.enter();
                                    // Deliberately an if/else and NOT an early return: this
                                    // runs inside Tauri's `setup` closure, so returning here
                                    // would skip the tray menu and the whole rest of startup.
                                    if action == DbEncryptionNotice::Leave {
                                        tracing::warn!(
                                            "database encryption state could not be established - leaving any existing notice untouched"
                                        );
                                    } else {
                                        let result = tauri::async_runtime::block_on(async {
                                            if action == DbEncryptionNotice::Raise {
                                                meridian::notices::raise_typed(
                                                    pool,
                                                    meridian::notices::Notice {
                                                        id: "tray.db_encryption_incomplete",
                                                        severity: "warning",
                                                        title: "Your data isn't encrypted at rest yet.",
                                                        detail: "Meridian couldn't finish encrypting its local database this time. Your data is safe but stored unencrypted for now - Meridian will retry automatically the next time it restarts.",
                                                        remedy: None,
                                                        event_key: "system.health",
                                                        deep_link: Some(meridian_core::notifications::deep_links::LOGS),
                                                    },
                                                )
                                                .await
                                            } else {
                                                meridian::notices::clear_typed(
                                                    pool,
                                                    "tray.db_encryption_incomplete",
                                                    "system.health",
                                                )
                                                .await
                                            }
                                        });
                                        if let Err(e) = result {
                                            // ERROR, not WARN: this is the failure boundary for
                                            // the one thing that tells a user their data is not
                                            // encrypted, and WARN-level would be easy to lose.
                                            tracing::error!(
                                                error = %e,
                                                "db-encryption-state notice update failed"
                                            );
                                        }
                                    }
                                }
                            }
                        }),
                    );

                    capture_pool
                }),
            )
            .flatten();
            // `.manage()` no-ops (returns `false`) when the type is already
            // managed — see `tauri::Manager::manage`'s doc — so this only takes
            // effect on the panic path above, where `app.manage(db_pool)` inside
            // the closure never ran. Guarantees `State<Option<SqlitePool>>>` is
            // always resolvable, so a command extracting it doesn't panic a
            // second, unrelated time on missing managed state.
            app.manage(db_setup_result.clone());
            // The SAME guarantee, for the OTHER managed type — and it did not
            // grow with it. `DbPool` (added when `reload_daemon` needed to
            // close/reopen around a daemon restart) is registered INSIDE the
            // closure above, so a panic before that point leaves it unmanaged.
            // `State<DbPool>` is what every dashboard command and the
            // notification drain extract, so on the panic path each one fails
            // extraction or panics on missing state — the very second, unrelated
            // panic the line above exists to prevent, just for the type that has
            // since replaced `Option<SqlitePool>` at those call sites.
            //
            // Guarded on `try_state` rather than leaning on `manage`'s no-op so
            // the normal path doesn't re-resolve the path (and re-log it) on
            // every launch. `meridian_db_path()` is re-read rather than threaded
            // out of the closure because a panicked closure has, by definition,
            // no value to hand back — and it is a pure env/.env lookup.
            //
            // The key is `None`: an encrypted DB cannot be reopened through this
            // fallback handle. That is honest about a startup that already
            // failed — `get()` correctly reports no pool either way — and it
            // buys back every command's ability to run and say so.
            if app.try_state::<db_pool::DbPool>().is_none() {
                tracing::warn!(
                    "tray setup did not register the database pool - registering an empty \
                     handle so commands degrade instead of failing state extraction"
                );
                app.manage(db_pool::DbPool::new(
                    None,
                    install::meridian_db_path(),
                    None,
                ));
            }
            #[cfg(feature = "capture")]
            let capture_pool = db_setup_result;

            // Single source of truth for the tray menu lives in `tray.rs`, so the
            // poll loop's health-driven rebuild can't drift out of sync. Initial
            // health is Unknown until the first poll resolves it.
            let menu = tray::build_tray_menu(app.handle(), &HealthStatus::Unknown)?;

            // The Meridian brand mark, rendered as a template so macOS tints it
            // to the light/dark menu bar — static, like other menu-bar apps.
            let tray_icon = tray_icon::logo_image();

            // Left-click toggles the popover (positioned under the tray icon);
            // right-click still opens the native menu. `show_menu_on_left_click`
            // must be false so the left-click reaches our handler instead of
            // auto-opening the menu and swallowing the popover.
            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(tray_icon)
                // Monochrome mark → render as a template so macOS tints it to the
                // light/dark menu bar instead of showing it full-colour.
                .icon_as_template(true)
                .on_tray_icon_event(|tray_handle, event| {
                    let app = tray_handle.app_handle();
                    match &event {
                        // Hide the tooltip on mouse-DOWN, which is the only click event
                        // a right-click reliably delivers. tray-icon's `rightMouseDown:`
                        // emits Down and then calls performClick, which runs the NSMenu
                        // in a modal tracking loop; that loop swallows `rightMouseUp:`,
                        // so the Up arm below never fires for a right-click, and `Leave`
                        // doesn't either while the menu holds the run loop. Hiding here
                        // is what actually stops the tooltip stranding behind the menu —
                        // the Up arm's hide alone could never run for a right-click.
                        // Left-click hides here too and is harmless: the Up arm then
                        // takes over and toggles the popover.
                        TrayIconEvent::Click {
                            button,
                            button_state: MouseButtonState::Down,
                            ..
                        } => {
                            tracing::info!(?button, "tray.event: Click(Down) — hiding tooltip");
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                let _ = tt.hide();
                            }
                        }
                        TrayIconEvent::Click {
                            button,
                            button_state: MouseButtonState::Up,
                            rect,
                            ..
                        } => {
                            tracing::info!(?button, "tray.event: Click");
                            // Belt-and-braces for the left-click path (and any platform
                            // that delivers Up without a preceding Down) — the Down arm
                            // above is what covers right-click.
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                let _ = tt.hide();
                            }
                            if *button != MouseButton::Left {
                                return;
                            }
                            if let Some(win) = app.get_webview_window("main") {
                                let visible = win.is_visible().unwrap_or(false);
                                tracing::info!(
                                    popover_visible = visible,
                                    "tray.click: toggling popover"
                                );
                                if visible {
                                    let _ = win.hide();
                                } else {
                                    #[cfg(target_os = "macos")]
                                    make_visible_over_fullscreen(&win);
                                    // Position the popover against the tray icon using the
                                    // click rect — same approach as the tooltip. Below the
                                    // icon on macOS (menu bar, top of screen; also sidesteps
                                    // tauri-plugin-positioner's TrayCenter overlapping the
                                    // menu bar on macOS 14+); above it on Windows, whose tray
                                    // icon sits at the bottom of the taskbar — see
                                    // `tray_anchor_position`.
                                    let pop_w = 344_i32;
                                    let icon_pos = rect.position.to_physical::<i32>(1.0);
                                    let icon_size = rect.size.to_physical::<i32>(1.0);
                                    let pop_h =
                                        win.outer_size().map(|s| s.height as i32).unwrap_or(0);
                                    let (x, y) = tray_anchor_position(
                                        (icon_pos.x, icon_pos.y),
                                        (icon_size.width, icon_size.height),
                                        (pop_w, pop_h),
                                        cfg!(target_os = "windows"),
                                        monitor_work_area(&win),
                                    );
                                    let _ = win.set_position(tauri::Position::Physical(
                                        tauri::PhysicalPosition::new(x, y),
                                    ));
                                    // Use orderFrontRegardless (same as the native right-click
                                    // menu) so the popover appears in the current Space without
                                    // signalling a Space switch. makeKeyAndOrderFront (what
                                    // win.show() calls) causes macOS to switch back to the home
                                    // Space when triggered from a fullscreen Space. Clicking
                                    // inside the popover naturally makes it key, so
                                    // Focused(false) auto-dismiss still works.
                                    #[cfg(target_os = "macos")]
                                    show_no_focus(&win);
                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        let _ = win.show();
                                        let _ = win.set_focus();
                                    }
                                    tracing::info!(
                                        x, y,
                                        size = ?win.inner_size().ok(),
                                        "tray.click: popover shown"
                                    );
                                }
                            }
                        }
                        TrayIconEvent::Enter { rect, .. } => {
                            tracing::info!("tray.event: Enter");
                            // Only show the tooltip when the main popover is closed.
                            let popover_open = app
                                .get_webview_window("main")
                                .map(|w| w.is_visible().unwrap_or(false))
                                .unwrap_or(false);
                            if popover_open {
                                tracing::info!(
                                    "tray.enter: popover already open — tooltip suppressed"
                                );
                                return;
                            }
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                // Position the tooltip centred against the tray icon —
                                // below it on macOS, above it on Windows. See
                                // `tray_anchor_position`. scale_factor 1.0 because the
                                // tray_icon crate already gives us physical pixel coords
                                // before the dpi wrapper.
                                let tt_w = 300_i32;
                                let icon_pos = rect.position.to_physical::<i32>(1.0);
                                let icon_size = rect.size.to_physical::<i32>(1.0);
                                let tt_h = tt.outer_size().map(|s| s.height as i32).unwrap_or(0);
                                let (x, y) = tray_anchor_position(
                                    (icon_pos.x, icon_pos.y),
                                    (icon_size.width, icon_size.height),
                                    (tt_w, tt_h),
                                    cfg!(target_os = "windows"),
                                    monitor_work_area(&tt),
                                );
                                let _ = tt.set_position(tauri::Position::Physical(
                                    tauri::PhysicalPosition::new(x, y),
                                ));
                                #[cfg(target_os = "macos")]
                                make_visible_over_fullscreen(&tt);
                                #[cfg(target_os = "macos")]
                                show_no_focus(&tt);
                                #[cfg(not(target_os = "macos"))]
                                let _ = tt.show();
                                tracing::info!(x, y, "tray.enter: tooltip shown");
                                // Push the latest status so the tooltip renders fresh data.
                                // Fall back to a default payload if the state lock is
                                // poisoned rather than panicking this tray-event handler.
                                let _ = app.emit(
                                    "status-update",
                                    app.try_state::<Arc<Mutex<AppState>>>()
                                        .and_then(|s| s.inner().lock().ok().map(|g| g.to_payload()))
                                        .unwrap_or_else(|| AppState::default().to_payload()),
                                );
                            }
                        }
                        TrayIconEvent::Leave { .. } => {
                            tracing::info!("tray.event: Leave — hiding tooltip");
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                let _ = tt.hide();
                            }
                        }
                        _ => {}
                    }
                })
                .on_menu_event(|app, event| tray::handle_menu_event(app, event.id.as_ref()))
                .build(app)?;

            {
                let mut s = app_state.lock().unwrap();
                s.tray_id = Some(tray.id().clone());
            }

            // Convert the popover + tooltip to NSPanel with non-activating style,
            // then raise their collection behavior and level so they appear in
            // fullscreen Spaces.
            //
            // Why NSPanel is necessary: a plain NSWindow (what Tauri creates) that
            // calls makeKeyAndOrderFront causes macOS to switch back to the home Space
            // before displaying the window, defeating fullscreen support. NSPanel +
            // NSWindowStyleMaskNonactivatingPanel never steals key-window status so
            // the fullscreen app's Space stays active and the panel appears within it.
            // NSPanel is a direct NSWindow subclass with identical ivar layout;
            // object_setClass between them is safe (the same technique used by
            // tauri-nspanel). The Tauri IPC bridge (WKWebView + __TAURI__) is
            // unaffected — it lives inside the view, not the window class.
            #[cfg(target_os = "macos")]
            for label in ["main", "tray-tooltip"] {
                if let Some(win) = app.get_webview_window(label) {
                    init_as_nspanel(&win);
                    make_visible_over_fullscreen(&win);
                }
            }

            // Clip the popover's window to the card's own corner radius so
            // the 4 corners are truly transparent (see `apply_corner_mask`'s
            // doc comment). Only "main" - the tooltip window is deliberately
            // larger than its card with transparent tail-placement padding.
            #[cfg(target_os = "macos")]
            if let Some(main_win) = app.get_webview_window("main") {
                apply_corner_mask(&main_win, 18.0);
            }

            // The popover is a non-activating NSPanel — it never becomes key so
            // Focused(false) never fires. Dismiss paths:
            //   • click-outside → global NSEvent monitor (macOS)
            //   • tray-icon click → toggle in on_tray_icon_event above
            //   • Escape key → app.js keydown → invoke('hide_popover')
            #[cfg(target_os = "macos")]
            if let Some(main_win) = app.get_webview_window("main") {
                install_click_outside_monitor(main_win);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let main_win = app.get_webview_window("main").unwrap();
                main_win.on_window_event({
                    let win = main_win.clone();
                    move |event| {
                        if let WindowEvent::Focused(false) = event {
                            let _ = win.hide();
                        }
                    }
                });
            }

            let app_handle = app.handle().clone();
            let state_clone = app_state.clone();
            tauri::async_runtime::spawn(async move {
                poll::run_poll_loop(app_handle, state_clone).await;
            });

            // Fast daemon supervisor, separate from the poll loop's slower,
            // notice-owning health tick: probes every 5 s and *starts* a daemon
            // that is confirmed stopped. It deliberately never signals a running
            // process — doing so on a slow probe corrupted `meridian.db` on
            // every macOS install. See `poll::watchdog` before changing it.
            tauri::async_runtime::spawn(async move {
                poll::run_daemon_watchdog().await;
            });

            // Launch-at-login AND morning relaunch: VERIFY-AND-REPAIR on every
            // launch (see autostart.rs), so the tray comes back after a reboot
            // and again the next morning if it was quit. Deliberately not
            // once-ever: the previous implementation trusted a
            // `~/.meridian/autostart_configured` marker, which survives the app
            // being trashed, reinstalled or moved — so an install could lose its
            // login item permanently with nothing able to notice. Bundled only:
            // an unbundled `cargo run` binary lives under `target/` and must
            // never be pinned. Spawned off the setup hook like the backend
            // install below, because it does file + `launchctl`/`schtasks` I/O
            // that shouldn't block tray startup.
            if sys::is_bundled() {
                tauri::async_runtime::spawn(async move {
                    autostart::ensure_registered().await;
                });
            }

            // Stage + register the bundled daemon on the self-contained .app DMG
            // path (also retires any leftover a11y-helper agent from an older
            // install — see backend_install's module docs). No-op under
            // dev/source (no bundled Resources/backend) and on launches where
            // the binary is unchanged. Spawned off the setup hook because the
            // launchd bootout-wait can take several seconds and must not block
            // tray startup.
            {
                let backend_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    backend_install::ensure_backend_installed(&backend_handle).await;
                });
            }

            // In-process capture (Gap-2 Bucket 2, behind the `capture` feature).
            // Spawns the screenpipe-screen frame engine + the input recorder, each
            // with its own consumer, on isolated tasks — a capture panic ends only
            // that task, never the tray (we gave up the screenpipe daemon's process
            // isolation, so this matters). Frames → capture_frames (slice 4a),
            // input events → capture_ui_events (slice 3c).
            #[cfg(feature = "capture")]
            start_capture(app_state.clone(), capture_pool);

            // Auto-open the setup wizard on first launch (no ~/.meridian/onboarded).
            // The 800 ms delay lets the tray menu settle before the window appears.
            //
            // Uses meridian_core::paths::meridian_dir() rather than a raw
            // `HOME` env var read — HOME is unset on Windows, which used to make
            // this check fall through to a bogus root-relative path, so the
            // marker could never be found there.
            {
                let wizard_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
                    // Same helper the dashboard gate uses, so "is this install
                    // onboarded" has one answer. This check used to be spelled
                    // out inline here, which is how it and the dashboard paths
                    // could have drifted apart.
                    if !onboarding_complete() {
                        tray::open_wizard_window(&wizard_handle);
                    }
                });
            }

            // No silent auto-install on launch: the DMG update surfaces as an
            // in-app banner (sidebar + popover) that checks on open via the
            // `check_update` command, so the user sees + consents to the update
            // rather than the app restarting itself underneath them. The ONE
            // exception is a release whose manifest declares a minimum
            // supported version below the running one — that installs
            // automatically (see update::enforce_minimum_version).
            update::enforce_minimum_version(app.handle());

            Ok(())
        })
        // Hand-maintained command list (grouped by domain). Adding a command
        // means: write it in a `commands/` submodule, glob-re-export it in
        // `commands.rs`, AND list it here — a missing entry fails the frontend
        // `invoke` at runtime ("command not found"), not at compile time.
        .invoke_handler(tauri::generate_handler![
            commands::repair::preview_repair,
            commands::repair::request_repair,
            // tray popover + daemon lifecycle
            commands::get_status,
            commands::open_dashboard,
            commands::open_worklogs,
            commands::open_setup,
            commands::restart_daemon,
            commands::toggle_daemon,
            commands::pause_for_duration,
            commands::pause_indefinitely,
            commands::get_daemon_status,
            // dashboard DB reads (ported /api/* GETs)
            commands::get_active,
            commands::get_today,
            commands::get_day_draft_states,
            commands::get_day_tasks,
            commands::dismiss_day_task,
            commands::merge_day_task,
            commands::restore_day_task,
            commands::get_week,
            commands::get_coding_agents,
            commands::get_recent_capture_apps,
            commands::get_worklogs,
            commands::get_hour_text,
            commands::get_hour_reports,
            commands::get_hour_status,
            commands::get_tasks,
            commands::get_task_detail,
            commands::get_plan,
            commands::get_settings,
            commands::get_triage,
            commands::get_integrations,
            commands::get_notices,
            commands::get_banner_notifications,
            commands::get_health,
            commands::export_diagnostics_bundle,
            commands::get_ticket_parents,
            commands::list_task_statuses,
            commands::get_day_task_worklog,
            // dev-only LLM Lab (refused in release builds - see commands::llm_lab)
            commands::get_llm_experiments,
            commands::get_llm_experiment,
            commands::run_llm_experiment,
            commands::draft_lab_worklog,
            commands::get_day_summary,
            commands::get_day_summary_data,
            commands::get_version,
            commands::get_app_info,
            commands::check_update,
            commands::install_update,
            commands::get_app_icon,
            commands::get_whats_new,
            commands::mark_whats_new_seen_cmd,
            commands::set_walkthrough_running,
            commands::disarm_walkthrough,
            // DB writes (ported /api/* POSTs/PATCH/DELETE)
            commands::plan_action,
            commands::get_board_tickets,
            commands::retarget_day_task_worklog,
            commands::dismiss_worklog_target,
            commands::set_worklog_provider,
            commands::plan_dismissed,
            commands::draft_plan_task,
            commands::create_plan_task,
            commands::edit_plan_task,
            commands::set_plan_task_done,
            commands::delete_plan_task,
            commands::triage_decision,
            commands::triage_ignore,
            commands::apply_ticket_fix,
            commands::set_task_status,
            commands::generate_day_task_worklog,
            commands::approve_day_task_worklog,
            commands::escalate_personal_task_create,
            commands::escalate_personal_task_match,
            // An LLM call, so it lives with the writes despite reading nothing back
            // but its own result.
            commands::generate_day_summary,
            commands::generate_day_summary_now,
            commands::dismiss_notification,
            commands::record_notification_response,
            commands::delete_notice,
            commands::edit_worklog,
            commands::rematch_worklog,
            commands::worklog_action,
            commands::edit_proposed_title,
            commands::edit_proposed_worklog,
            commands::proposed_action,
            commands::update_settings,
            // process / service control (ported /api process routes)
            commands::reload_daemon,
            commands::sync_tasks,
            commands::request_gated_sync_tasks,
            commands::run_update,
            // tracker connect/disconnect (ported /api/integrations + /api/auth/oauth)
            commands::disconnect_integration,
            commands::discover_azure_devops,
            commands::discover_github_projects,
            commands::discover_jira_projects,
            commands::save_integration_token,
            commands::start_oauth,
            commands::cancel_oauth,
            commands::get_oauth_status,
            commands::request_pm_tool,
            // OS/window actions
            commands::take_pending_deep_link,
            commands::open_permission_pane,
            commands::open_external_url,
            commands::quit_app,
            commands::hide_popover,
            commands::resize_setup_window,
            // Setup wizard (first-run, permissions, providers)
            commands::is_first_run,
            commands::mark_setup_started,
            commands::setup_elapsed_secs,
            commands::mark_setup_complete,
            commands::walkthrough_is_armed,
            commands::get_platform,
            commands::save_account_email,
            commands::get_account_email,
            commands::sign_in_required,
            commands::clear_account_email,
            commands::check_accessibility,
            commands::check_screen_recording,
            commands::request_screen_recording,
            commands::check_notifications,
            commands::request_notifications,
            commands::detect_llm_providers,
            commands::test_llm_provider,
            commands::disconnect_llm_provider,
            commands::test_all_llm_providers,
            commands::install_llm_provider,
            commands::cursor_sign_in,
            commands::codex_sign_in,
            commands::claude_sign_in,
            // Custom cloud endpoints (add/probe/remove). `add` + `probe` spend real metered
            // requests measuring the endpoint — only ever on explicit user action.
            commands::add_custom_llm_provider,
            commands::assess_llm_capacity,
            commands::probe_custom_llm_provider,
            commands::remove_custom_llm_provider,
            commands::replace_custom_llm_provider_key,
            commands::list_custom_llm_providers,
            commands::list_custom_llm_provider_models,
            // Uninstall wizard (plan + execute; see src/uninstall.rs for the CLI it drives)
            commands::get_uninstall_plan,
            commands::execute_uninstall,
            tray_debug,
        ])
        .build(tauri::generate_context!())
        .expect("error running meridian tray")
        .run(|app, event| {
            // Quitting Meridian takes the daemon with it.
            //
            // It is a separate launchd/Task Scheduler process, so before this
            // handler existed "Quit" left it polling, holding `meridian.db`
            // open, and running the summariser's third-party LLM CLIs after the
            // user had closed the app - and, worse, made the `db.corrupt`
            // notice's own instructions impossible to follow, because
            // `meridian db repair` refuses to run while the daemon is up.
            //
            // Handled here rather than in the menu item's "quit" arm so every
            // way out is covered by one rule: the tray menu, the popover
            // footer's Quit button (`commands::system::quit_app`), and macOS
            // Cmd+Q (reachable once the dashboard has switched the activation
            // policy to `Regular`). The first two call `AppHandle::exit`, which
            // routes through this same event.
            //
            // The stop is async and the exit is not, so the exit is held with
            // `prevent_exit` and re-issued once the stop finishes. That makes
            // "are we exiting?" three-state rather than a boolean - see
            // [`daemon_lifecycle::ExitPhase`] for why a single latch was wrong,
            // and why a restart must pass straight through.
            if let tauri::RunEvent::ExitRequested { code, api, .. } = &event {
                use daemon_lifecycle::{ExitAction, ExitPhase};
                match daemon_lifecycle::decide_exit(*code, daemon_lifecycle::exit_phase()) {
                    ExitAction::Proceed => {}
                    // Something else is already stopping the daemon and will
                    // issue the real exit. Just don't let this one through.
                    ExitAction::Hold => api.prevent_exit(),
                    ExitAction::HoldAndStop => {
                        daemon_lifecycle::set_exit_phase(ExitPhase::Stopping);
                        api.prevent_exit();
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            // Held on the unwind path too. `Stopping` blocks
                            // every later `ExitRequested`, and this task is the
                            // only thing that advances past it - so a panic
                            // anywhere below would leave the tray permanently
                            // unquittable, with no path left to reset the
                            // phase. See [`daemon_lifecycle::HeldExitGuard`].
                            let _released = daemon_lifecycle::HeldExitGuard;
                            daemon_lifecycle::stop_for_quit().await;
                            // Immediately before the exit, so the re-entrant
                            // `ExitRequested` this triggers is the one and only
                            // one allowed through.
                            daemon_lifecycle::set_exit_phase(ExitPhase::ReadyToExit);
                            handle.exit(0);
                        });
                    }
                }
            }

            // macOS: fires when the user re-activates the app externally
            // (Spotlight, dock click, `open -a Meridian`). The tray runs as an
            // Accessory app so there is no dock icon most of the time; without
            // this handler, hitting "Meridian" in Spotlight is silently a no-op
            // when the app is already running. Route to the dashboard when the
            // user has finished setup, otherwise re-open the wizard so a partial
            // onboard can be resumed. The setup hook already opens the wizard on
            // cold start, so this only matters for warm activation.
            //
            // `RunEvent::Reopen` is a macOS-only enum variant (it doesn't exist
            // on other targets), so this arm alone is cfg-gated. The closure
            // itself is no longer a no-op off macOS — the `ExitRequested` arm
            // above runs on every platform, which is why `app` and `event` are
            // plain names rather than underscore-prefixed. The routing decision
            // lives in the platform-independent [`reopen_target`] /
            // [`is_onboarded`] so it is unit-testable without a live Tauri app
            // (see the tests below).
            //
            // `has_visible_windows` is intentionally ignored (`{ .. }`): both
            // openers already reuse an existing dashboard/wizard window via
            // `get_webview_window(..)` + show/focus before building a new one,
            // so a Reopen while a window is already up just re-focuses it — the
            // flag would only matter if we wanted different behaviour for
            // "windows visible" vs "all minimised", which we don't.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                let home = std::env::var("HOME").unwrap_or_default();
                let onboarded = is_onboarded(std::path::Path::new(&home));
                tracing::info!(onboarded, "app.reopen: routing external activation");
                match reopen_target(onboarded) {
                    ReopenTarget::Dashboard => tray::open_native_dashboard(app),
                    ReopenTarget::Wizard => tray::open_wizard_window(app),
                }
            }
        });
}

/// What to do with the "encryption did not finish" notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbEncryptionNotice {
    /// Encryption was intended but the database on disk is still plaintext.
    Raise,
    /// Positively confirmed encrypted.
    Clear,
    /// The state could not be established. Leave whatever notice exists alone.
    Leave,
}

/// Decide the notice state from the two facts the startup path has.
///
/// Extracted from `run()` purely so it is testable. This is the one part of the
/// plaintext-key hotfix that actually surfaces the remaining failure mode to the
/// user, and inline in `run()` it had no CI coverage at all - the review called
/// that out as the single unverified piece of the headline fix.
///
/// The asymmetry matters: the notice is raised ONLY when a key was resolved AND
/// the file is still plaintext (a real, silent security downgrade). Every other
/// combination clears it, including "no key intended", so a user who never
/// enabled encryption is never nagged, and the notice disappears by itself once
/// the migration finally completes.
pub(crate) fn db_encryption_notice_action(
    encryption_intended: bool,
    plaintext: Option<bool>,
) -> DbEncryptionNotice {
    match (encryption_intended, plaintext) {
        // Unknown state. Clearing here was the bug: `is_plaintext_sqlite`
        // answers `false` for a file it cannot open, which is indistinguishable
        // from "confirmed encrypted" - so an unreadable database silently
        // dismissed a warning that was still true.
        (_, None) => DbEncryptionNotice::Leave,
        // Intended, and the file on disk is still plaintext.
        (true, Some(true)) => DbEncryptionNotice::Raise,
        // The only state that earns a clear: positively confirmed encrypted.
        (_, Some(false)) => DbEncryptionNotice::Clear,
        // Plaintext, but no key resolved this launch. That is either "the user
        // never enabled encryption" (nothing was ever raised, so leaving is a
        // no-op) or "key resolution failed this run" (the warning is still
        // true). Indistinguishable from here, and only one of them is safe.
        (false, Some(true)) => DbEncryptionNotice::Leave,
    }
}

#[cfg(test)]
mod db_encryption_notice_tests {
    use super::{db_encryption_notice_action as action, DbEncryptionNotice::*};

    /// The only state that warrants warning the user: encryption was asked for
    /// and did not happen.
    #[test]
    fn raises_only_when_encryption_was_intended_but_did_not_happen() {
        assert_eq!(action(true, Some(true)), Raise);
    }

    /// A clear must be EARNED by a positive result. Review finding: the probe
    /// answers `false` both for "confirmed encrypted" and for "could not read
    /// the file", so collapsing them let an unreadable database dismiss a
    /// warning that was still true.
    #[test]
    fn clears_only_on_a_confirmed_encrypted_database() {
        assert_eq!(action(true, Some(false)), Clear, "migration completed");
        assert_eq!(
            action(false, Some(false)),
            Clear,
            "no key, already encrypted"
        );
    }

    /// Never clear on ignorance. `None` is "the probe could not read the file",
    /// which is not evidence of anything.
    #[test]
    fn leaves_the_notice_alone_when_the_state_is_unknown() {
        assert_eq!(action(true, None), Leave, "unreadable, encryption intended");
        assert_eq!(action(false, None), Leave, "unreadable, no key resolved");
    }

    /// Plaintext with no key resolved is ambiguous: either the user never
    /// enabled encryption (nothing was raised, so this is a no-op) or key
    /// resolution failed this launch (the warning is still true). Only one of
    /// those is safe to act on, so act on neither.
    #[test]
    fn leaves_the_notice_alone_when_plaintext_but_no_key_resolved() {
        assert_eq!(action(false, Some(true)), Leave);
    }
}

/// Debug bridge: lets webview JS forward `window.onerror` reports and the measured
/// card height to the tray's log, so GUI-only faults (a JS exception, a
/// window/card size mismatch) are diagnosable without devtools. The injected
/// `window` names the caller and exposes its actual size for comparison.
///
/// **A packaged build has no devtools at all** — there is no `devtools` feature or
/// config anywhere in this crate — so anything a webview writes to `console.error`
/// is written to nothing. This command is the only way a JS-side failure reaches
/// the telemetry spool (and therefore an Export Diagnostics bundle).
///
/// # Who calls this
/// - `tray/src/app.js` and `tray/src/tooltip.js` — the popover's `dbg()` helper.
/// - The dashboard, via `ui/lib/bridge.ts::reportUiError` — added after a tracker
///   sync failure proved unrecoverable: the invoke was rejected before dispatch so
///   there was no span, and the reason existed only in an unopenable console.
#[tauri::command]
fn tray_debug(window: tauri::Window, msg: String) {
    tracing::info!(
        label = %window.label(),
        size = ?window.inner_size().ok(),
        msg = %msg,
        "tray_debug"
    );
}

/// Start (or restart) the in-process capture engine and UI event recorder.
///
/// Safe to call multiple times — aborts any previously stored tasks before
/// spawning fresh ones, so pausing then resuming is idempotent. Does nothing
/// when Screen Recording is not granted (logs and returns).
///
/// **The new UI-event recorder generation waits for the previous one to
/// fully tear down before registering its own observers.** That thread's
/// `run_app_observer` (screenpipe-a11y) registers `NotificationGuard`s on
/// the shared `NSWorkspace` notification center and unregisters them on
/// stop; letting a new recorder register while the old one is still
/// mid-teardown puts two threads on that one shared singleton at once,
/// which has produced a double-free abort
/// (`BUG_IN_CLIENT_OF_LIBMALLOC_POINTER_BEING_FREED_WAS_NOT_ALLOCATED` under
/// `removeObserver:`/`_Block_release`) in production.
///
/// That wait happens on the **new recorder's own OS thread**, never on this
/// function's caller: the old thread's teardown isn't actually bounded (a
/// backpressured UI-event channel can leave it blocked in `blocking_send`
/// well past its own 500ms poll), so joining it inline here could stall
/// whichever thread called `start_capture` — including a tokio worker, when
/// called from an async command handler.
///
/// The whole stop/spawn/store sequence runs under one `AppState` lock held
/// for this entire function, so two overlapping `start_capture` calls (e.g.
/// a schedule auto-resume racing a user-triggered resume) can't each spawn
/// a generation and clobber each other's stored thread handle — which would
/// leave the loser's generation unjoined by any future restart and reopen
/// the same race.
///
/// # Who calls this
/// Once from `lib.rs`'s `setup()` on launch, and again from
/// `commands::pause_for_duration` on resume.
#[cfg(feature = "capture")]
pub(crate) fn start_capture(
    app_state: std::sync::Arc<std::sync::Mutex<AppState>>,
    pool: Option<meridian_core::SqlitePool>,
) {
    use capture::{screenpipe::ScreenpipeEngine, CaptureEngine};

    // Launch-time gate: don't even spawn the engine when Screen Recording is
    // missing. Distinct from the engine's own `request_screen_capture_access`,
    // which prompts — this one is a silent preflight, so a first launch before
    // the wizard's grant doesn't race a cold TCC dialog.
    //
    // Windows has no TCC: Windows Graphics Capture needs no persistent per-app
    // grant, so there is nothing to preflight and the gate is always open —
    // matching the engine's own Windows permission stubs.
    #[cfg(target_os = "macos")]
    let screen_granted = {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        unsafe { CGPreflightScreenCaptureAccess() }
    };
    #[cfg(not(target_os = "macos"))]
    let screen_granted = true;

    if !screen_granted {
        tracing::info!(
            "capture: Screen Recording not granted — engine deferred until next launch after grant"
        );
        return;
    }

    // Read settings once for this capture start: seed the ignore list (so the
    // frame consumers enforce it from the first frame) and the streaming flag.
    // `update_settings` refreshes the ignore list live afterwards via the same
    // shared handle, so a Settings change never needs a capture restart.
    let settings = meridian_core::settings::load_runtime_settings();
    let pause_on_streaming_video = settings.pause_on_streaming_video;
    // On by default — see the field doc comment on RuntimeSettings.
    let capture_secondary_monitors = settings.capture_secondary_monitors;
    // Handed to the engine's secondary-monitor sweep (multi-screen capture):
    // unlike the primary a11y/OCR path, those windows are never
    // system-focused, so the fork never resolves their `browser_url` for the
    // usual downstream `should_drop_frame` check to catch — see
    // `ScreenpipeEngine::ignored_urls`.
    let ignored_urls = settings.ignored_urls.clone();
    // Held from here through the end of the function — see the doc comment
    // above for why the whole stop/spawn/store sequence must be one critical
    // section rather than separate lock/unlock pairs. Nothing spawned below
    // touches `app_state`, so there's no deadlock risk, only serialization
    // of concurrent `start_capture` calls.
    let mut s = app_state.lock().unwrap();
    *s.capture_ignore.lock().unwrap() =
        capture_ignore::CaptureIgnore::new(&settings.ignored_apps, &settings.ignored_urls);
    let capture_ignore = s.capture_ignore.clone();

    // Drop any previous cancel senders — this signals the old tasks to exit.
    // The old UI-event recorder thread's own teardown (screenpipe-a11y
    // unregistering its NSWorkspace observers) is awaited below, inside the
    // new recorder thread rather than here — see the doc comment.
    drop(s.engine_cancel.take());
    drop(s.ui_consumer_cancel.take());
    let previous_ui_recorder_thread = s.ui_recorder_thread.take();

    // Cancellation channels: dropping the Sender exits the receiving task.
    // tauri::async_runtime::spawn works from any thread (incl. macOS main);
    // tokio::task::spawn panics outside a tokio worker, so we avoid it here.
    let (engine_cancel_tx, mut engine_cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let (ui_cancel_tx, mut ui_cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<capture::CaptureItem>(64);

    // Frame consumer: exits when engine task finishes (tx drops → rx returns None).
    // Handles both item shapes the engine can send — the primary per-tick frame
    // and secondary-monitor context samples (multi-screen capture) — through the
    // same ignore-list gate, routed to their own tables.
    let consumer_pool = pool.clone();
    let frame_ignore = capture_ignore.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(item) = rx.recv().await {
            match item {
                capture::CaptureItem::Frame(frame) => {
                    tracing::debug!(
                        ts = %frame.timestamp,
                        app = ?frame.app_name,
                        chars = frame.text.len(),
                        "capture: frame received"
                    );
                    // User ignore list (apps + website domains): drop the frame before it
                    // is ever persisted, so an ignored app/site leaves no trace on the
                    // timeline. Going forward only — history is untouched.
                    if frame_ignore
                        .lock()
                        .unwrap()
                        .should_drop_frame(frame.app_name.as_deref(), frame.browser_url.as_deref())
                    {
                        tracing::debug!(
                            app = ?frame.app_name,
                            "capture: frame ignored (user ignore list) — not persisted"
                        );
                        continue;
                    }
                    let Some(p) = consumer_pool.as_ref() else {
                        continue;
                    };
                    let row = meridian_core::CaptureFrameInsert {
                        timestamp: frame.timestamp,
                        app_name: frame.app_name,
                        window_name: frame.window_name,
                        browser_url: frame.browser_url,
                        text: frame.text,
                        text_source: frame.text_source.as_str().to_string(),
                    };
                    if let Err(e) = meridian_core::insert_capture_frame(p, &row).await {
                        tracing::warn!(error = %e, "capture: failed to persist frame");
                    }
                }
                capture::CaptureItem::Secondary(sample) => {
                    tracing::debug!(
                        ts = %sample.timestamp,
                        monitor = %sample.monitor_name,
                        app = ?sample.app_name,
                        chars = sample.text.len(),
                        "capture: secondary-screen sample received"
                    );
                    if frame_ignore
                        .lock()
                        .unwrap()
                        .should_drop_frame(sample.app_name.as_deref(), None)
                    {
                        tracing::debug!(
                            app = ?sample.app_name,
                            "capture: secondary-screen sample ignored (user ignore list) — not persisted"
                        );
                        continue;
                    }
                    let Some(p) = consumer_pool.as_ref() else {
                        continue;
                    };
                    let row = meridian_core::CaptureSecondaryScreenInsert {
                        timestamp: sample.timestamp,
                        monitor_name: sample.monitor_name,
                        app_name: sample.app_name,
                        window_name: sample.window_name,
                        text: sample.text,
                    };
                    if let Err(e) = meridian_core::insert_capture_secondary_screen(p, &row).await {
                        tracing::warn!(error = %e, "capture: failed to persist secondary-screen sample");
                    }
                }
            }
        }
    });

    // Engine task: select races the cancel signal against the engine run.
    // When engine_cancel_tx is dropped, engine_cancel_rx resolves → task exits,
    // dropping tx → frame consumer loop ends naturally.
    tauri::async_runtime::spawn(async move {
        let engine = ScreenpipeEngine {
            pause_on_streaming_video,
            ignored_urls,
            capture_secondary_monitors,
        };
        tokio::select! {
            _ = &mut engine_cancel_rx => {
                tracing::info!("capture: engine stopped (pause)");
            }
            result = engine.run(tx) => {
                if let Err(e) = result {
                    tracing::error!(error = %e, "capture: engine exited with error");
                }
            }
        }
    });

    // UI event consumer: exits on cancel signal, which drops ui_rx, causing
    // the OS recorder thread to see tx.is_closed() within 500ms and exit.
    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<meridian_core::CaptureUiEventInsert>(256);
    let ui_pool = pool;
    let ui_ignore = capture_ignore;
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut ui_cancel_rx => break,
                ev = ui_rx.recv() => {
                    let Some(ev) = ev else { break };
                    // Ignored apps leave no UI-event trace either (keystroke
                    // counts, clipboard). App-only match — UI events carry no URL.
                    if ui_ignore.lock().unwrap().should_drop_app(ev.app_name.as_deref()) {
                        continue;
                    }
                    let Some(p) = ui_pool.as_ref() else { continue };
                    if let Err(e) = meridian_core::insert_capture_ui_event(p, &ev).await {
                        tracing::warn!(error = %e, "capture: failed to persist ui event");
                    }
                }
            }
        }
    });
    let ui_recorder_thread = std::thread::spawn(move || {
        // Wait for the outgoing generation's screenpipe-a11y NSWorkspace
        // observers to fully unregister before this generation registers its
        // own — on this dedicated thread, never on start_capture's caller
        // (see the fn's doc comment for why that distinction matters).
        if let Some(previous) = previous_ui_recorder_thread {
            if let Err(panic) = previous.join() {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "non-string panic payload".to_string());
                tracing::error!(
                    panic = %msg,
                    "capture: previous ui-event recorder thread panicked during restart"
                );
            }
        }
        capture::ui_events::run_ui_event_recorder(ui_tx);
    });

    // Store senders + thread handle in state; dropping the senders (on pause)
    // cancels the tasks, and the next start_capture joins the handle before
    // spawning a replacement.
    s.engine_cancel = Some(engine_cancel_tx);
    s.ui_consumer_cancel = Some(ui_cancel_tx);
    s.ui_recorder_thread = Some(ui_recorder_thread);
    tracing::info!("capture: engine and ui recorder started");
}
