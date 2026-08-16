//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Windows interactive toasts - buttons, inline reply, and retraction - via
//! `Windows.UI.Notifications` directly.
//!
//! # Why this exists at all
//!
//! `tauri-plugin-notifications` implements its desktop backend as either
//! `cfg(all(desktop, feature = "notify-rust"))` or `cfg(all(target_os =
//! "macos", not(feature = "notify-rust")))`. macOS takes the second arm (a real
//! Swift `UNUserNotificationCenter` layer: categories, buttons, inline reply,
//! action events). Windows has to take the first, because with the feature off
//! it matches neither arm and has no notification backend whatsoever. And
//! `notify-rust` has no action API — `set_click_listener_active` and
//! `remove_active` both return a hard `Err("… not supported with notify-rust")`.
//!
//! So on Windows the outbox previously *delivered* interactive notifications
//! that could not be answered (their rows resolved only by expiry) and could
//! not be retracted (every expiry logged `toast retraction failed` and left the
//! toast on screen). None of that is a Windows limitation: toast XML has
//! supported `<actions>` and `<input>` for years. It was a backend limitation,
//! and this module routes around it.
//!
//! # Two toast stacks on Windows, deliberately
//!
//! Only the OUTBOX path moves here. Plain [`crate::sys::notify`] toasts
//! (pause/update/health) still go through the plugin: they have no id, no
//! buttons, no tag and nothing to retract, so rewriting them would risk a
//! working path to buy nothing. Both stacks deliver under the same AUMID (the
//! Tauri bundle identifier), so Windows treats them as one app and the setting
//! [`crate::sys::notification_permission_state`] reads governs both. Do not
//! "unify" them without re-reading this paragraph.
//!
//! # Everything runs on the main thread
//!
//! WinRT calls need an initialised COM apartment, and STA event callbacks are
//! delivered by a message pump. The tray's main thread has both (tao
//! `OleInitialize`s it and pumps it); the poll-loop tokio workers that call
//! [`crate::sys::notify_outbox`] / [`crate::sys::retract_toast`] have neither.
//! So both entry points hop via `run_on_main_thread` — delivery AND retraction.
//! Registering a handler off the pump would leave the buttons dead.
//!
//! # What is testable and what is not
//!
//! Nothing here can be compiled or run from the macOS dev machines this is
//! written on, so the module is kept as thin as it can be: every decision with
//! a possible logic bug (escaping, the button cap, element ordering,
//! `hint-inputId` wiring) lives in [`crate::toast_actions`], which compiles and
//! tests everywhere. What is left is WinRT plumbing, type-checked from macOS by
//! copying the call sequence into a scratch crate depending only on `windows`
//! and running `cargo check --target x86_64-pc-windows-msvc` (a full
//! cross-target check of the tray is not possible — it dies in `aws-lc-sys`,
//! whose build needs `windows.h`).
//!
//! # Who calls this
//! [`crate::sys::notify_outbox`] and [`crate::sys::retract_toast`], on their
//! Windows arms only.
//!
//! # Related
//! - [`crate::toast_actions`] — the XML this hands to WinRT.
//! - [`crate::commands::notifications::apply_response`] — where an answer lands
//!   (shared with the macOS plugin path, so both platforms write identically).
//! - `docs/notifications.md` — the outbox → deliver → respond → expire lifecycle.

use tauri::Manager;
use tracing::Instrument;
use windows::core::{IInspectable, Interface, Ref, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissalReason, ToastDismissedEventArgs, ToastNotification,
    ToastNotificationManager,
};

/// WinRT group for every toast this module delivers.
///
/// Retraction on an unpackaged (non-MSIX) app has to name the tag, the group
/// AND the AUMID — `ToastNotificationHistory::RemoveGroupedTagWithId` — so a
/// constant group is not decoration, it is half the address of the toast we
/// need to be able to withdraw.
const GROUP: &str = "meridian";

/// The action id reported when the user clicks the toast body rather than a
/// button. Matches what macOS reports, and what
/// [`crate::commands::notifications::apply_response`] treats as a
/// click-through.
const TAP: &str = "tap";

/// Deliver an outbox notification as a toast, with its category's buttons.
///
/// The outbox row id becomes the toast's WinRT **tag**, which is this
/// platform's equivalent of the macOS notification identifier: the correlation
/// that comes back on activation and the handle [`retract`] targets. Unlike
/// macOS (whose plugin id is an `i32`), a tag is a string, so the full `i64`
/// round-trips and there is no clamping case to degrade.
///
/// Buttons come from the row's own `actions` JSON — the bytes the daemon
/// stamped when it enqueued — falling back to the category registry if the
/// column is empty. A row with neither still delivers, as plain title+body.
pub fn show(app: &tauri::AppHandle, n: &meridian_core::notifications::PendingNotification) {
    // Spanned because delivery CROSSES A THREAD: the work happens inside a
    // `run_on_main_thread` closure, so without an explicit span the WinRT
    // failure and the call that caused it land in telemetry unrelated. The
    // closure enters this span, which is what keeps them one trace.
    //
    // `buttons` is on the span rather than only the success log because it is
    // the diagnostic for "the toast appeared but had no way to answer it" -
    // a category whose descriptors failed to parse delivers 0 and looks
    // identical to a plain toast otherwise. Title/body are NOT recorded: they
    // carry the user's ticket titles and window names.
    let span = tracing::info_span!(
        "notifications.toast.deliver",
        id = n.id,
        category = n.category.as_deref().unwrap_or("plain"),
        buttons = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    let _e = span.enter();

    let actions =
        crate::toast_actions::resolve_actions(n.actions.as_deref(), n.category.as_deref());
    let xml = crate::toast_actions::build_toast_xml(&n.title, &n.body, &actions);
    let aumid = app.config().identifier.clone();
    let id = n.id;
    let buttons = actions.len();
    span.record("buttons", buttons);

    let handle = app.clone();
    let delivery = span.clone();
    let dispatched = app.run_on_main_thread(move || {
        let _e = delivery.enter();
        match show_on_main(&handle, id, &aumid, &xml) {
            Ok(()) => {
                delivery.record("otel.status_code", "OK");
                tracing::info!(id, buttons, "outbox toast delivered")
            }
            Err(e) => {
                delivery.record("otel.status_code", "ERROR");
                tracing::warn!(error = %e, id, "notify_outbox: toast delivery failed")
            }
        }
    });
    if let Err(e) = dispatched {
        span.record("otel.status_code", "ERROR");
        tracing::warn!(error = %e, id, "notify_outbox: main-thread dispatch failed");
    }
}

/// Withdraw a delivered toast from the screen and Action Center - the delivery
/// half of expiry. Best-effort, exactly like the macOS path: the outbox row is
/// stamped regardless, so a failure leaves a stale toast but never a
/// re-delivery.
pub fn retract(app: &tauri::AppHandle, id: i64) {
    // Same cross-thread reason as `show`. Worth spanning in its own right: this
    // is the path that was silently broken on Windows for the whole life of the
    // notify-rust backend (`remove_active` is a hard `Err` there), so "did the
    // expiry actually withdraw the toast" needs to be answerable from telemetry
    // rather than by reading a stale toast off someone's screen.
    let span = tracing::info_span!(
        "notifications.toast.retract",
        id,
        otel.status_code = tracing::field::Empty,
    );
    let _e = span.enter();

    let aumid = app.config().identifier.clone();
    let removal = span.clone();
    let dispatched = app.run_on_main_thread(move || {
        let _e = removal.enter();
        let removed = ToastNotificationManager::History().and_then(|h| {
            h.RemoveGroupedTagWithId(
                &HSTRING::from(id.to_string()),
                &HSTRING::from(GROUP),
                &HSTRING::from(&aumid),
            )
        });
        match removed {
            Ok(()) => {
                removal.record("otel.status_code", "OK");
                tracing::info!(id, "expired toast retracted")
            }
            Err(e) => {
                removal.record("otel.status_code", "ERROR");
                tracing::warn!(error = %e, id, "toast retraction failed")
            }
        }
    });
    if let Err(e) = dispatched {
        span.record("otel.status_code", "ERROR");
        tracing::warn!(error = %e, id, "retract_toast: main-thread dispatch failed");
    }
}

/// Build, wire and show the toast. Main thread only (see the module header).
fn show_on_main(
    app: &tauri::AppHandle,
    id: i64,
    aumid: &str,
    xml: &str,
) -> windows::core::Result<()> {
    let doc = XmlDocument::new()?;
    // A rejected document is the failure mode escaping exists to prevent; it
    // surfaces here rather than as a toast that silently never appears.
    doc.LoadXml(&HSTRING::from(xml))?;

    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(&HSTRING::from(id.to_string()))?;
    toast.SetGroup(&HSTRING::from(GROUP))?;

    let activated_app = app.clone();
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
        move |_, args| {
            let (action, text) = read_activation(args);
            record(&activated_app, id, action, text);
            Ok(())
        },
    ))?;

    let dismissed_app = app.clone();
    toast.Dismissed(&TypedEventHandler::<
        ToastNotification,
        ToastDismissedEventArgs,
    >::new(move |_, args| {
        // ONLY a real user dismissal counts. `ApplicationHidden` is what our
        // own `retract` raises, and recording 'dismiss' for it would race the
        // 'expired' stamp for the same row; `TimedOut` just means the banner
        // rolled into Action Center, where its buttons are still live and the
        // user has not answered anything.
        let reason = args
            .ok()
            .ok()
            .and_then(|a| a.Reason().ok())
            .unwrap_or(ToastDismissalReason::ApplicationHidden);
        if reason == ToastDismissalReason::UserCanceled {
            record(&dismissed_app, id, "dismiss".to_string(), None);
        }
        Ok(())
    }))?;

    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))?.Show(&toast)
}

/// Extract the pressed action id and any inline-reply text from an activation.
///
/// `Arguments()` carries what the XML put in `arguments=` — an action id, or
/// [`TAP`] from the toast's `launch` for a body click. The typed text is keyed
/// in `UserInput` by the input's id, which [`crate::toast_actions`] sets to the
/// action's own id precisely so this lookup needs no second mapping.
///
/// Every failure degrades to a bare tap rather than dropping the answer: a
/// recorded response with a less specific action still resolves the outbox row,
/// while no response leaves it hanging until expiry.
fn read_activation(args: Ref<'_, IInspectable>) -> (String, Option<String>) {
    // `Ref::ok()` yields a `windows::core::Result`, hence the second `.ok()`
    // to reach a plain Option.
    let Some(args) = args
        .ok()
        .ok()
        .and_then(|a| a.cast::<ToastActivatedEventArgs>().ok())
    else {
        return (TAP.to_string(), None);
    };
    let action = match args.Arguments() {
        Ok(a) if !a.is_empty() => a.to_string_lossy(),
        _ => TAP.to_string(),
    };
    let text = args.UserInput().ok().and_then(|inputs| {
        let value = inputs.Lookup(&HSTRING::from(&action)).ok()?;
        let typed = value.cast::<IPropertyValue>().ok()?.GetString().ok()?;
        let typed = typed.to_string_lossy();
        (!typed.is_empty()).then_some(typed)
    });
    (action, text)
}

/// Stamp the answer onto the outbox row, off the message pump.
///
/// The write is async and touches the DB pool, so it is spawned rather than
/// awaited: this runs inside a WinRT callback on the main thread, and blocking
/// there stalls the pump that delivers every other toast event.
fn record(app: &tauri::AppHandle, id: i64, action: String, text: Option<String>) {
    // Clone the pool out of the managed state before the await — a `State`
    // guard borrows the app and cannot be held across one.
    let pool = app
        .try_state::<Option<meridian_core::SqlitePool>>()
        .and_then(|s| s.inner().clone());
    let Some(pool) = pool else {
        tracing::warn!(id, action, "toast answer dropped — meridian.db is not open");
        return;
    };
    let app = app.clone();
    // The spawned task is a THIRD thread hop (pump -> tokio), and the one where
    // the answer is actually persisted. `apply_response` opens its own
    // `notifications.respond` span; `.instrument` is what makes that span a
    // child of the toast's rather than an orphan root, so a lost answer can be
    // followed from the button press to the write in one trace.
    //
    // `has_text` rather than the text itself: an inline reply IS the user's own
    // writing and must never egress - whether one was supplied is the
    // diagnostic, its content is not. Same rule as `apply_response`'s own span.
    let span = tracing::info_span!(
        "notifications.toast.answer",
        id,
        action,
        has_text = text.is_some(),
        otel.status_code = tracing::field::Empty,
    );
    tauri::async_runtime::spawn(
        async move {
            if let Err(e) = crate::commands::notifications::apply_response(
                app,
                &pool,
                id,
                &action,
                text.as_deref(),
            )
            .await
            {
                // The answer is now lost - the user pressed a button and
                // nothing recorded it. Marked on the span as well as logged, so
                // it is findable by span status rather than only by grepping
                // WARN bodies.
                tracing::Span::current().record("otel.status_code", "ERROR");
                tracing::warn!(error = %e, id, action, "toast answer could not be recorded");
            }
        }
        .instrument(span),
    );
}
