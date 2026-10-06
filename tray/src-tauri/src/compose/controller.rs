//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! One press, start to finish: read the field, draft, check it is still safe, write.
//!
//! # Threads
//! The event-tap thread only notices the key and sends a message. All the real work - slow
//! Accessibility reads, the model call, the write - happens here on one worker thread, one
//! press at a time. Presses that arrive while a draft is in flight are discarded rather
//! than queued, because a stale trigger answered seconds later would write into whatever
//! the user is doing by then.
//!
//! # On by default, switchable
//! On unless `compose_enabled` is false in `settings.json` (Settings, Capture and Privacy,
//! Writing key). A supervisor thread re-reads the settings every couple of seconds and installs
//! or removes the event tap to match, and every press re-reads them too, so switching it off
//! stops it at once and switching it on needs no restart.
//!
//! # Who calls this
//! [`start`] is called once from `lib.rs`'s setup hook.
//!
//! # Related
//! - [`meridian::compose::generate`] - the model half.
//! - [`super::delivery`] - the safety checks run before every write.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

use meridian::compose::classify::{classify, Classification};
use meridian::compose::generate::{draft, DraftOutcome};
use meridian::compose::types::{DraftRequest, PreviousAttempt, SurfaceKind};

use super::delivery::{self, FieldIdentity, Origin};
use super::dots::TypedDots;
use super::feedback;
use super::ranges;
use super::reader::{self, ReadField};
use super::tap::{self, TapHandle};
use super::trigger::TriggerKey;
use super::typing::TypingSound;
use super::writer;

/// A re-run counts only if it comes this soon after the draft it replaces.
const RERUN_WINDOW: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy)]
struct Config {
    enabled: bool,
    key: TriggerKey,
    /// Play the typing sound while a draft is being written. On unless switched off.
    sound: bool,
    /// Also read other windows visible on screen. On unless switched off.
    other_windows: bool,
    /// Type "..." into the box while waiting, like someone typing. On unless switched off.
    dots: bool,
}

fn read_config() -> Config {
    let settings = meridian_core::settings::load_runtime_settings();
    Config {
        enabled: settings.compose_enabled,
        key: TriggerKey::from_setting(settings.compose_trigger_key.as_deref()),
        sound: settings.compose_sound,
        other_windows: settings.compose_other_windows,
        dots: settings.compose_typing_dots,
    }
}

/// The last draft written, kept to recognise a re-run on the same field.
struct LastDraft {
    identity: FieldIdentity,
    output: String,
    at: Instant,
}

/// Set once [`start`] has run, so a second call does nothing.
static STARTED: AtomicBool = AtomicBool::new(false);
/// How often the supervisor re-reads the settings to start or stop the key.
const SETTINGS_POLL: Duration = Duration::from_secs(2);

/// Start the writing key's threads. The worker is idle until a tap arrives; the supervisor
/// installs the event tap while compose is enabled and removes it when it is switched off, so
/// the Settings switch takes effect without a restart. Nothing happens on a second call.
pub fn start(app: tauri::AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let (tx, rx) = mpsc::channel::<()>();
    if let Err(e) = std::thread::Builder::new()
        .name("compose-worker".into())
        .spawn(move || worker(rx, app))
    {
        tracing::error!(error = %e, "compose: could not start the worker thread");
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("compose-supervisor".into())
        .spawn(move || supervise(tx))
    {
        tracing::error!(error = %e, "compose: could not start the supervisor thread");
    }
}

/// Keep the event tap in step with the settings: on while enabled, off otherwise, replaced when
/// the trigger key changes.
fn supervise(tx: mpsc::Sender<()>) {
    let mut running: Option<(TriggerKey, TapHandle)> = None;
    loop {
        let config = read_config();
        let wanted = config.enabled.then_some(config.key);
        if running.as_ref().map(|(key, _)| *key) != wanted {
            // Stop the old tap before creating the new one, so the two never overlap.
            drop(running.take());
            running = wanted.map(|key| {
                tracing::info!(?key, "compose: writing key on");
                let tx = tx.clone();
                // A closed channel means the worker is gone; nothing useful to do.
                (
                    key,
                    tap::spawn(key, move || {
                        let _ = tx.send(());
                    }),
                )
            });
            if running.is_none() {
                tracing::info!("compose: writing key off");
            }
        }
        std::thread::sleep(SETTINGS_POLL);
    }
}

fn worker(rx: mpsc::Receiver<()>, app: tauri::AppHandle) {
    let mut last: Option<LastDraft> = None;
    while rx.recv().is_ok() {
        let config = read_config();
        if config.enabled {
            // The typing sound starts before any slow work, so the user knows at once that
            // the tap was heard, and it keeps going until the draft is written or refused.
            let mut typing = config.sound.then(TypingSound::start);
            let _busy = BusyTitle::show(&app);
            let ctx = read_context(&app, config);
            // A panic in one press must not kill the worker: that would leave the key dead
            // until the app restarts.
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_press(&mut last, &mut typing, &ctx, config.dots)
            }));
            if ran.is_err() {
                tracing::error!("compose: a press panicked; the key keeps working");
                typing.take();
            }
        }
        // Drop presses that piled up while we were busy.
        while rx.try_recv().is_ok() {}
    }
}

/// What the reader needs from settings and the user's capture ignore list.
fn read_context(app: &tauri::AppHandle, config: Config) -> reader::ReadContext {
    use tauri::Manager;
    let ignore = app
        .try_state::<std::sync::Arc<std::sync::Mutex<crate::state::AppState>>>()
        .and_then(|state| state.lock().ok().map(|s| s.capture_ignore.clone()));
    reader::ReadContext {
        other_windows: config.other_windows,
        is_ignored: std::sync::Arc::new(move |app_name, url| {
            // With no ignore list available, err towards skipping.
            ignore.as_ref().is_none_or(|list| {
                list.lock()
                    .map(|l| l.should_drop_frame(Some(app_name), url))
                    .unwrap_or(true)
            })
        }),
    }
}

/// End the typing sound. Called just before the final sound so the two never overlap.
fn hush(typing: &mut Option<TypingSound>) {
    typing.take();
}

/// The user's own name, so the model can pick out their earlier messages in a thread.
/// Looked up once; `None` when the system will not say.
fn user_name() -> Option<String> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| {
        let out = std::process::Command::new("id").arg("-F").output().ok()?;
        let name = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (!name.is_empty()).then_some(name)
    })
    .clone()
}

/// Shows "Writing..." next to the menu-bar icon for as long as a press is being handled,
/// and removes it when dropped - including if the press ends early or panics.
struct BusyTitle {
    app: tauri::AppHandle,
}

impl BusyTitle {
    fn show(app: &tauri::AppHandle) -> BusyTitle {
        set_tray_title(app, Some("Writing..."));
        BusyTitle { app: app.clone() }
    }
}

impl Drop for BusyTitle {
    fn drop(&mut self) {
        set_tray_title(&self.app, None);
    }
}

fn set_tray_title(app: &tauri::AppHandle, title: Option<&str>) {
    use tauri::Manager;
    let Some(state) = app.try_state::<std::sync::Arc<std::sync::Mutex<crate::state::AppState>>>()
    else {
        return;
    };
    let id = state.lock().ok().and_then(|s| s.tray_id.clone());
    if let Some(tray) = id.and_then(|id| app.tray_by_id(&id)) {
        let _ = tray.set_title(title);
    }
}

#[tracing::instrument(skip_all, fields(outcome))]
fn handle_press(
    last: &mut Option<LastDraft>,
    typing: &mut Option<TypingSound>,
    ctx: &reader::ReadContext,
    dots: bool,
) {
    let read = match reader::read_focused_field(ctx) {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(reason = e.as_str(), "compose: no field to write into");
            hush(typing);
            // Say so: a press that is heard but does nothing is indistinguishable from a
            // press that was never heard, which is the first thing a tester asks.
            feedback::stopped(match e {
                reader::ReadError::AccessibilityDenied => {
                    "Meridian needs Accessibility access to write for you."
                }
                reader::ReadError::NoFocusedApp => "Meridian could not tell which app is in front.",
                reader::ReadError::NoFocusedField => "Meridian found no focused text box here.",
                reader::ReadError::NotATextField => {
                    "The focused item is not a text box Meridian can write in."
                }
            });
            return;
        }
    };

    // Real full stops typed at the caret while the model works, then taken back out. Only on
    // a bare caret; `finish` proves the box is back to what it was before anything is written.
    let typed_dots = (dots && dots_are_safe(&read))
        .then(|| TypedDots::start(&read.handle))
        .flatten();
    let previous = rerun_of(last, &read);
    let request = DraftRequest {
        field: read.snapshot.clone(),
        previous,
        user_name: user_name(),
    };
    let outcome = tauri::async_runtime::block_on(draft(&request));
    tracing::Span::current().record("outcome", outcome_name(&outcome));

    if !typed_dots.map(TypedDots::finish).unwrap_or(true) {
        // The box no longer provably holds what it did, so writing over it is not safe.
        tracing::warn!("compose: could not clear the typing dots; not writing");
        hush(typing);
        if let DraftOutcome::Text { text, .. } = &outcome {
            writer::leave_on_clipboard(text);
            feedback::stopped_with_draft(
                "Meridian could not clear its typing dots safely, so it did not write the draft.",
                text,
            );
        } else {
            feedback::stopped("Meridian could not clear its typing dots safely.");
        }
        return;
    }

    match outcome {
        DraftOutcome::Text {
            text,
            intent,
            surface,
            ..
        } => {
            deliver(&read, intent, surface, &text, last, typing);
        }
        DraftOutcome::Refused(reason) => {
            hush(typing);
            feedback::stopped(reason.user_message())
        }
        DraftOutcome::NoContext => say_in_box(
            &read,
            "Meridian could not read enough here to write something. Try typing a few words first.",
            typing,
        ),
        DraftOutcome::Unusable => say_in_box(
            &read,
            "The model did not return anything usable. Try again.",
            typing,
        ),
        DraftOutcome::Failed { message } => say_in_box(&read, &message, typing),
    }
}

/// Tell the user why nothing was drafted. The message goes into the box itself, where it is
/// easiest to read, but only when the box is empty and safe to write in; anywhere else (text
/// already there, a refused box, a write that does not land) it is a notification as before.
fn say_in_box(read: &ReadField, message: &str, typing: &mut Option<TypingSound>) {
    hush(typing);
    if !write_note(read, message) {
        feedback::stopped(message);
    }
}

fn write_note(read: &ReadField, message: &str) -> bool {
    let handle = &read.handle;
    if !handle.value.trim().is_empty() || handle.protected_prefix > 0 {
        return false;
    }
    let Classification::Ready(plan) = classify(&read.snapshot) else {
        return false;
    };
    let origin = Origin {
        identity: handle.identity.clone(),
        value: handle.value.clone(),
    };
    let now = reader::current_state(handle);
    if delivery::guard(&origin, &now, plan.surface, message).is_err() {
        return false;
    }
    let empty = ranges::Utf16Range {
        location: 0,
        length: 0,
    };
    writer::write(handle, empty, message, &mut || {}).is_ok()
}

/// Whether real keystrokes may be typed into this box while waiting. Only where the press will
/// actually be answered (a refused box - password, search, address bar - gets nothing), and
/// never where a typed full stop does something: a terminal runs suggestions on it and a code
/// editor opens autocomplete.
fn dots_are_safe(read: &ReadField) -> bool {
    if read.snapshot.secure_input || read.snapshot.private_window {
        return false;
    }
    match classify(&read.snapshot) {
        Classification::Ready(plan) => !matches!(
            plan.surface,
            SurfaceKind::Terminal | SurfaceKind::CodeEditor
        ),
        Classification::Refused(_) => false,
    }
}

fn outcome_name(outcome: &DraftOutcome) -> &'static str {
    match outcome {
        DraftOutcome::Text { .. } => "text",
        DraftOutcome::Refused(_) => "refused",
        DraftOutcome::NoContext => "no_context",
        DraftOutcome::Unusable => "unusable",
        DraftOutcome::Failed { .. } => "failed",
    }
}

/// A previous attempt, when this press is the user trying again on the same field.
fn rerun_of(last: &Option<LastDraft>, read: &ReadField) -> Option<PreviousAttempt> {
    let last = last.as_ref()?;
    let same_field = last.identity == read.handle.identity;
    let fresh = last.at.elapsed() <= RERUN_WINDOW;
    // The field still holds the draft: that is what makes this a re-run, not a new message.
    let still_there =
        !last.output.trim().is_empty() && read.handle.value.contains(last.output.trim());
    (same_field && fresh && still_there).then(|| PreviousAttempt {
        output: last.output.clone(),
    })
}

fn deliver(
    read: &ReadField,
    intent: meridian::compose::types::Intent,
    surface: meridian::compose::types::SurfaceKind,
    text: &str,
    last: &mut Option<LastDraft>,
    typing: &mut Option<TypingSound>,
) {
    let handle = &read.handle;
    let range = delivery::plan_edit(
        intent,
        handle.selection,
        ranges::utf16_len(&handle.value),
        handle.protected_prefix,
    );
    let origin = Origin {
        identity: handle.identity.clone(),
        value: handle.value.clone(),
    };
    let now = reader::current_state(handle);

    if let Err(refusal) = delivery::guard(&origin, &now, surface, text) {
        tracing::info!(reason = refusal.as_str(), "compose: draft not written");
        writer::leave_on_clipboard(text);
        hush(typing);
        feedback::stopped_with_draft(refusal.user_message(), text);
        return;
    }
    // The sound stops the moment the text is seen in the box.
    let mut stop_waiting = || {
        typing.take();
    };
    match writer::write(handle, range, text, &mut stop_waiting) {
        Ok(method) => {
            tracing::info!(
                method = method.as_str(),
                chars = text.chars().count(),
                "compose: draft written"
            );
            hush(typing);
            feedback::done();
            *last = Some(LastDraft {
                identity: handle.identity.clone(),
                output: text.to_string(),
                at: Instant::now(),
            });
        }
        Err(e) => {
            tracing::warn!(reason = e.as_str(), "compose: draft could not be written");
            writer::leave_on_clipboard(text);
            hush(typing);
            feedback::stopped_with_draft("Meridian could not write into this box.", text);
        }
    }
}
