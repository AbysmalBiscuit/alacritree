//! alacritree is a GUI-subsystem binary with no console of its own, so on
//! Windows each `git`/`gh`/`cmd` child gets a fresh console window unless we
//! pass `CREATE_NO_WINDOW`.

use std::process::Command;

pub trait CommandExt {
    /// Suppress the console window Windows would spawn for this child. No-op
    /// elsewhere.
    fn hide_console(&mut self) -> &mut Self;
}

impl CommandExt for Command {
    #[cfg(windows)]
    fn hide_console(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn hide_console(&mut self) -> &mut Self {
        self
    }
}
