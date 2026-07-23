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
