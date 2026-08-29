//! Raise the session in front of the user above the load competing with it.
//!
//! A keystroke's round trip is mostly the program redrawing its own line, and
//! a line editor that highlights and completes as you type spends milliseconds
//! of CPU doing it.  At the same scheduling priority as a build saturating
//! every core, that work waits: against 64 spinning threads a nushell prompt
//! echoed a character in 128 ms at the median and 1.9 s at the 95th
//! percentile, while the same PTY stack hosting `cmd.exe` — which answers from
//! its own read loop and needs no CPU — stayed at 0.2 ms.  One class above the
//! load restores the idle figure.
//!
//! Windows does not spread the class on its own: `CreateProcess` gives a new
//! process the normal class unless its creator is at *idle* or *below* normal,
//! so a raise only ever travels downward.  Nothing here leaks — but nothing
//! inherits either, which is why boosting the shell alone left an agent
//! running inside it as slow as before.  The set to raise is chosen
//! explicitly: see `session::windows_process_probe`.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess, PROCESS_SET_INFORMATION,
    SetPriorityClass,
};

/// A handle this module opened and is responsible for closing.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Put `pid` one class above the load, or return it to normal.
///
/// Best effort by design.  A process that exits between being listed and being
/// opened, or one this user may not touch, is skipped: the cost of missing it
/// is the latency that was there anyway.
pub fn set_boosted(pid: u32, boosted: bool) {
    let handle = unsafe { OpenProcess(PROCESS_SET_INFORMATION, 0, pid) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        log::debug!("could not open {pid} to set its priority: {}", io::Error::last_os_error());
        return;
    }
    let handle = Owned(handle);

    let class = if boosted { ABOVE_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
    if unsafe { SetPriorityClass(handle.0, class) } == 0 {
        log::debug!("{pid} refused priority {class:#x}: {}", io::Error::last_os_error());
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};

    use windows_sys::Win32::System::Threading::{GetPriorityClass, PROCESS_QUERY_INFORMATION};

    use super::*;

    /// `pause` blocks on a piped stdin nobody writes to, so the child sits
    /// still for the length of a test without spinning a core.
    struct Subject(Child);

    impl Drop for Subject {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn subject() -> Subject {
        Subject(
            Command::new("cmd.exe")
                .args(["/c", "pause"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .expect("spawn cmd.exe"),
        )
    }

    fn class_of(pid: u32) -> u32 {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
        assert!(!handle.is_null(), "open {pid} for query");
        let handle = Owned(handle);
        unsafe { GetPriorityClass(handle.0) }
    }

    #[test]
    fn a_boost_is_applied_and_taken_back() {
        let subject = subject();
        let pid = subject.0.id();
        assert_eq!(class_of(pid), NORMAL_PRIORITY_CLASS);

        set_boosted(pid, true);
        assert_eq!(class_of(pid), ABOVE_NORMAL_PRIORITY_CLASS);

        set_boosted(pid, false);
        assert_eq!(class_of(pid), NORMAL_PRIORITY_CLASS);
    }

    /// Windows only spreads a priority class downward, so a child of a boosted
    /// process starts at normal.  The whole shape of the feature rests on this:
    /// were it otherwise, raising a shell would raise every build launched from
    /// it for the build's whole life.
    #[test]
    fn a_child_does_not_inherit_the_boost() {
        // The boosted process has to be the one doing the spawning, so this
        // test raises itself rather than a stand-in, and lowers itself before
        // asserting so a failure cannot leave the runner elevated.
        let me = std::process::id();
        set_boosted(me, true);
        let child = subject();
        let inherited = class_of(child.0.id());
        set_boosted(me, false);

        assert_eq!(inherited, NORMAL_PRIORITY_CLASS);
    }

    /// Nothing here may panic on a pid that has gone, because the set is taken
    /// from a process table that is stale the moment it is read.
    #[test]
    fn a_vanished_pid_is_ignored() {
        let pid = {
            let subject = subject();
            subject.0.id()
        };

        set_boosted(pid, true);
        set_boosted(pid, false);
    }
}
