//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The minimum feedback a press needs until the overlay exists: a sound when work starts
//! and a message when nothing was written.
//!
//! A draft takes a few seconds, and silence after pressing a key feels broken. A short
//! system sound marks "working", a different one marks "stopped", and a notification
//! explains why. `osascript` is used for the notification because the tray's own toast
//! plugin is absent in an unbundled `tauri dev` run, which is exactly where this is first
//! tested. All of it is best-effort: a failure here never affects the draft.
//!
//! # Who calls this
//! [`super::controller`].
//!
//! # Related
//! - [`crate::sys::notify`] - the bundled-app toast path, unavailable in dev builds.

use std::process::{Command, Stdio};

/// Run a helper without waiting for it, but still collect its exit so it never lingers as a
/// zombie process. A helper that cannot start is not worth reporting: this is a nicety.
fn run_detached(mut command: Command) {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn play(sound: &str) {
    let mut afplay = Command::new("afplay");
    afplay.arg(format!("/System/Library/Sounds/{sound}.aiff"));
    run_detached(afplay);
}

/// Escape text for an AppleScript double-quoted string.
fn applescript_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A draft was written into the field.
pub fn done() {
    play("Tink");
}

/// Nothing was written; tell the user why in one sentence.
pub fn stopped(message: &str) {
    play("Basso");
    let script = format!(
        "display notification {} with title \"Meridian\"",
        applescript_quote(message)
    );
    let mut osascript = Command::new("osascript");
    osascript.args(["-e", &script]);
    run_detached(osascript);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(
            applescript_quote(r#"say "hi" \ now"#),
            r#""say \"hi\" \\ now""#
        );
    }

    #[test]
    fn newlines_become_spaces_so_the_script_stays_one_line() {
        assert_eq!(applescript_quote("a\nb\rc"), "\"a b c\"");
    }

    #[test]
    fn an_injection_attempt_stays_inside_the_string() {
        let q = applescript_quote("x\" & (do shell script \"rm -rf ~\") & \"");
        // Every quote inside is escaped, so the whole thing is one string literal.
        assert!(q.starts_with('"') && q.ends_with('"'));
        assert!(!q[1..q.len() - 1].replace("\\\"", "").contains('"'));
    }
}
