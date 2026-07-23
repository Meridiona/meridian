//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Suppress the console-window flash Windows creates when a GUI process (the
//! tray) or a console-less daemon spawns a console-subsystem child — every
//! plain Win32 console app (`schtasks`, `taskkill`, `tasklist`, `powershell`)
//! and every npm-shimmed `.cmd`/`.bat` CLI (`claude`, `codex`, …, which
//! `Command::new` routes through `cmd.exe`) is one. Piping the child's
//! stdio does NOT suppress this — Windows still allocates and briefly shows
//! a console window unless the process is created with `CREATE_NO_WINDOW`.
//!
//! A no-op on every other OS, so callers can chain `.no_window()`
//! unconditionally rather than wrapping each call site in `#[cfg(windows)]`.

/// [`CREATE_NO_WINDOW`](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Chainable `.no_window()` for both `std::process::Command` and
/// `tokio::process::Command` — see the module docs for why this matters.
pub trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl NoWindow for tokio::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

// Windows-only: the trait does nothing off Windows, and `cmd.exe` (the console
// child these exercise) exists only there. Both tests assert `.no_window()`
// still produces a working spawn — the flag suppresses the console window, it
// must not break launching the child. That the window is actually suppressed
// was confirmed out-of-band on a real Windows host (a nested `cmd.exe →
// powershell` grandchild reports a console `HWND` of 0 with the flag set and a
// non-zero one without it); there is no non-flaky way to assert the window's
// absence from a headless test, so these lock in the spawn-still-works half.
#[cfg(all(test, windows))]
mod tests {
    use super::NoWindow;

    #[test]
    fn create_no_window_has_the_documented_value() {
        assert_eq!(super::CREATE_NO_WINDOW, 0x0800_0000);
    }

    #[test]
    fn no_window_std_command_still_spawns() {
        let status = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .no_window()
            .status()
            .expect("cmd should spawn with CREATE_NO_WINDOW set");
        assert!(status.success());
    }

    #[tokio::test]
    async fn no_window_tokio_command_still_spawns() {
        let status = tokio::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .no_window()
            .status()
            .await
            .expect("cmd should spawn with CREATE_NO_WINDOW set (tokio)");
        assert!(status.success());
    }
}
