//! alacritree is a GUI-subsystem binary with no console of its own, so on
//! Windows each `git`/`gh`/`cmd` child gets a fresh console window unless we
//! pass `CREATE_NO_WINDOW`. `hidden` is the crate's one sanctioned way to
//! build a `Command`, so that flag can never be forgotten at a call site.

use std::ffi::OsStr;
use std::process::Command;

/// Build a `Command` for `program`, pre-armed to skip the console window
/// Windows would otherwise pop for it. No-op elsewhere.
#[allow(clippy::disallowed_methods)] // the sanctioned spawner
pub fn hidden(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
