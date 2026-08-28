//! Raise the shell above the background work competing with it.
//!
//! A keystroke's round trip is mostly the shell redrawing its prompt line, and
//! a line editor that highlights and completes as you type spends milliseconds
//! of CPU doing it.  At the same scheduling priority as a build saturating
//! every core, that work waits: against 64 spinning threads a nushell prompt
//! echoed a character in 128 ms at the median and 1.9 s at the 95th
//! percentile, while the same PTY stack hosting `cmd.exe` — which answers from
//! its own read loop and needs no CPU — stayed at 0.2 ms.  One class above the
//! load removes the wait entirely: 10 ms across the same run, which is what
//! the shell costs on an idle machine.
//!
//! Everything the shell starts inherits the class, so a build launched from a
//! boosted session outranks the rest of the desktop for its whole life.  That
//! trade is the reason this is opt-in.

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{ABOVE_NORMAL_PRIORITY_CLASS, SetPriorityClass};

/// Raise `shell` one priority class, so a busy machine cannot starve the
/// prompt the user is typing at.
///
/// Above normal rather than high: high outranks much of the system's own work,
/// and measured no faster.
pub fn boost(shell: HANDLE) {
    if unsafe { SetPriorityClass(shell, ABOVE_NORMAL_PRIORITY_CLASS) } == 0 {
        log::warn!("could not raise the shell's priority: {}", std::io::Error::last_os_error());
    }
}
