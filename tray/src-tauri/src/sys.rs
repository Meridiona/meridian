//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Small shared runtime helpers used across the tray: the current uid, the
//! bundle probe, and the native-notification facade.
//!
//! These each had 2–3 copy-pasted definitions scattered across `lib.rs`,
//! `commands`, `poll`, and `health` before this module consolidated them.
//!
//! The notification facade fronts the community `tauri-plugin-notifications`
//! (real `UNUserNotificationCenter` on macOS: action buttons + inline reply).
//! Everything above this module talks only to [`notify`] /
//! [`notify_interactive`], so if the plugin disappoints it can be swapped for
//! in-house `objc2-user-notifications` bindings without touching callers.
//! The plugin requires a packaged `.app` bundle ([`is_bundled`]); in an
//! unbundled run (`tauri dev`, `cargo run`) it is not registered at all and
//! both facades degrade to a logged no-op — interactive toasts are verified on
//! packaged builds only.
//!
//! # Related
//! - [`crate::poll`] — the loop that toasts via [`notify`] / [`notify_interactive`].
//! - [`crate::commands::notifications`] — records the user's answer
//!   (`record_notification_response`) that [`notify_interactive`]'s buttons produce.
//! - [`crate::install`] — install-mode + path resolution (a separate concern from these).

use tauri::Manager;
use tauri_plugin_notifications::Notifications;

/// The current user's numeric uid as a string (for `launchctl gui/<uid>/…`
/// domain targets). Falls back to `"501"` (the first macOS user) if `id -u`
/// can't be read — better than failing the whole launchctl call.
pub fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}

/// True when running from a packaged `.app` bundle (`…/Foo.app/Contents/MacOS/bin`).
///
/// `UNUserNotificationCenter` (and therefore the notifications plugin, whose
/// init hard-fails unbundled) only works from a bundle. `lib.rs` gates plugin
/// registration on this; the notify facades gate their delivery attempt on the
/// plugin state being present. Mirrors the plugin's own `require_bundle` check.
pub fn is_bundled() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let macos = exe.parent()?;
            let contents = macos.parent()?;
            let bundle = contents.parent()?;
            Some(
                macos.ends_with("MacOS")
                    && contents.ends_with("Contents")
                    && bundle.to_string_lossy().ends_with(".app"),
            )
        })
        .unwrap_or(false)
}

/// The plugin state, if the plugin was registered (bundled runs only).
fn notifier(app: &tauri::AppHandle) -> Option<&Notifications<tauri::Wry>> {
    let state = app.try_state::<Notifications<tauri::Wry>>()?;
    Some(state.inner())
}

/// Show a plain native macOS toast (title + body, no buttons).
///
/// Fire-and-forget: the plugin's `show()` is async, so delivery is spawned and
/// a failure is logged, never surfaced — no caller has a meaningful recovery.
/// Unbundled runs (no plugin) log at debug and drop the toast.
pub fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    let Some(n) = notifier(app) else {
        tracing::debug!(
            title,
            "notify: plugin absent (unbundled run) — toast dropped"
        );
        return;
    };
    let builder = n.builder().title(title).body(body);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = builder.show().await {
            tracing::warn!(error = %e, "notify: toast delivery failed");
        }
    });
}

/// Show an interactive toast for an outbox row: the row's `category` selects
/// the button set registered at startup (`lib.rs`), and the row `id` +
/// `deep_link` ride in `extra` so the `actionPerformed` listener (popover JS →
/// `record_notification_response`) can stamp the answer back onto the row.
///
/// Every `extra` value MUST be a string: the plugin's Swift layer decodes
/// `extra` as `[String: String]` and a single non-string value fails the whole
/// notification decode — `show()` errors and NO toast appears (found the hard
/// way with a numeric `outbox_id`).
///
/// The notification identifier is the outbox id, so the OS collapses a
/// re-delivered row onto the same toast instead of duplicating it.
///
/// Known limitation: the plugin's `actionPerformed` only fires for toasts shown
/// by the CURRENT tray process (its notification map is in-memory), so an
/// answer given after a tray restart is dropped. Expiries keep stale
/// interactive toasts rare; a v2 could reconcile via `notificationClicked`.
pub fn notify_interactive(
    app: &tauri::AppHandle,
    n: &meridian_core::notifications::PendingNotification,
) {
    let Some(nf) = notifier(app) else {
        tracing::debug!(
            id = n.id,
            "notify_interactive: plugin absent (unbundled run) — toast dropped"
        );
        return;
    };
    // Outbox ids are AUTOINCREMENT and realistically < 2^31; wrap rather than
    // fail if one ever isn't (the true id still rides in `extra.outbox_id`).
    let toast_id = i32::try_from(n.id).unwrap_or_else(|_| (n.id % i64::from(i32::MAX)) as i32);
    let mut builder = nf
        .builder()
        .id(toast_id)
        .title(&n.title)
        .body(&n.body)
        .extra("outbox_id", n.id.to_string());
    if let Some(category) = &n.category {
        builder = builder.action_type_id(category);
    }
    if let Some(link) = &n.deep_link {
        builder = builder.extra("deep_link", link.clone());
    }
    let id = n.id;
    let category = n.category.clone();
    tauri::async_runtime::spawn(async move {
        match builder.show().await {
            Ok(()) => {
                tracing::info!(id, category = ?category, "interactive toast delivered")
            }
            Err(e) => {
                tracing::warn!(error = %e, id, "notify_interactive: toast delivery failed")
            }
        }
    });
}

/// Register the fixed interactive-category set (`meridian_core`'s
/// [`meridian_core::notifications::categories`]) with the OS. Called once at
/// startup from `lib.rs` on bundled runs; a failure downgrades every
/// interactive toast to plain title+body (macOS ignores an unknown
/// `action_type_id`), so it's logged loudly but never fatal.
pub fn register_notification_categories(app: &tauri::AppHandle) {
    use meridian_core::notifications::categories;
    let Some(nf) = notifier(app) else {
        tracing::debug!("category registration skipped — plugin absent (unbundled run)");
        return;
    };
    // ActionType's fields are private but it derives Deserialize, so the
    // categories are built from the shared JSON descriptors — the same bytes
    // the daemon stamps into the outbox `actions` column, one source of truth.
    let types: Vec<tauri_plugin_notifications::ActionType> = categories::ALL
        .iter()
        .filter_map(|id| {
            let actions: serde_json::Value =
                serde_json::from_str(categories::actions_json(id)?).ok()?;
            serde_json::from_value(serde_json::json!({ "id": id, "actions": actions })).ok()
        })
        .collect();
    if types.len() != categories::ALL.len() {
        tracing::error!(
            built = types.len(),
            expected = categories::ALL.len(),
            "notification category descriptors failed to parse — buttons will be missing"
        );
    }
    match nf.register_action_types(types) {
        Ok(()) => tracing::info!(
            categories = categories::ALL.len(),
            "notification categories registered"
        ),
        Err(e) => tracing::error!(error = %e, "notification category registration failed"),
    }
}
