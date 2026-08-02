//! Where diagnostics live, and who owns each file.
//!
//! Logs are machine-local state rather than roaming config: a redirected or
//! UNC-backed `%APPDATA%` can block during the synchronous panic hook, and a
//! synced one copies crash data off the machine.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Which process wrote a file, and which attempt it was.
///
/// `start` is UTC epoch nanoseconds rather than a readable date because the
/// name has to be a collision-proof identity: pid reuse inside one second, a
/// clock stepped backwards, and the repeated hour at a DST fall-back all
/// reproduce a second-resolution local name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId {
    pub start: u128,
    pub pid: u32,
    pub ordinal: u32,
}

static START: OnceLock<u128> = OnceLock::new();
static ORDINAL: AtomicU32 = AtomicU32::new(0);

pub fn process_id() -> ProcessId {
    let start = *START.get_or_init(|| {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
    });
    ProcessId { start, pid: std::process::id(), ordinal: ORDINAL.load(Ordering::Relaxed) }
}

/// Record the ordinal the first successful `create_new` settled on, so the
/// second file reuses it instead of racing for its own.
pub fn set_ordinal(ordinal: u32) {
    ORDINAL.store(ordinal, Ordering::Relaxed);
}

pub fn artifact_name(id: &ProcessId) -> String {
    name_with_prefix("crash-", id)
}

pub fn session_log_name(id: &ProcessId) -> String {
    name_with_prefix("alacritree-", id)
}

fn name_with_prefix(prefix: &str, id: &ProcessId) -> String {
    match id.ordinal {
        0 => format!("{prefix}{}-{}.log", id.start, id.pid),
        n => format!("{prefix}{}-{}-{n}.log", id.start, id.pid),
    }
}

pub fn parse_name(prefix: &str, file_name: &str) -> Option<ProcessId> {
    let rest = file_name.strip_prefix(prefix)?.strip_suffix(".log")?;
    let mut parts = rest.split('-');
    let start = parts.next()?.parse().ok()?;
    let pid = parts.next()?.parse().ok()?;
    let ordinal = match parts.next() {
        Some(raw) => raw.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(ProcessId { start, pid, ordinal })
}

/// Newest first, then by retry ordinal. `start` alone leaves two artifacts from
/// one `create_new` retry unordered, and "newest" undefined with them.
pub fn sort_key(id: &ProcessId) -> (Reverse<u128>, u32) {
    (Reverse(id.start), id.ordinal)
}

/// Machine-local state, not roaming config — see the module header.
#[cfg(windows)]
pub fn log_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .or_else(home::home_dir)
        .map(|dir| dir.join("alacritree"))
}

#[cfg(not(windows))]
pub fn log_dir() -> Option<PathBuf> {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return Some(PathBuf::from(state).join("alacritree"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local").join("state").join("alacritree"))
}

/// Whether a pid belongs to a running process.
///
/// Pid reuse can make a dead pid look live, which only ever defers a deletion —
/// the safe direction. It cannot produce a wrong identity, because the filename
/// carries the start value too.
#[cfg(windows)]
pub fn pid_is_live(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    if pid == 0 {
        return false;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    unsafe { CloseHandle(handle) };
    true
}

#[cfg(target_os = "linux")]
pub fn pid_is_live(pid: u32) -> bool {
    pid != 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(target_os = "macos")]
pub fn pid_is_live(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // EPERM means it exists and belongs to someone else.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || *libc::__error() == libc::EPERM }
}

#[cfg(test)]
pub fn reset_identity_for_tests() {
    ORDINAL.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name is an identity, not a date: two artifacts from one `create_new`
    /// retry share a start and a pid, so the ordinal has to survive the round
    /// trip or they become indistinguishable.
    #[test]
    fn an_artifact_name_round_trips_through_parsing() {
        let id = ProcessId { start: 1785708131123456789, pid: 50916, ordinal: 0 };

        let name = artifact_name(&id);

        assert_eq!(name, "crash-1785708131123456789-50916.log");
        assert_eq!(parse_name("crash-", &name), Some(id));
    }

    #[test]
    fn a_nonzero_ordinal_survives_the_round_trip() {
        let id = ProcessId { start: 1785708131123456789, pid: 50916, ordinal: 2 };

        let name = artifact_name(&id);

        assert_eq!(name, "crash-1785708131123456789-50916-2.log");
        assert_eq!(parse_name("crash-", &name), Some(id));
    }

    #[test]
    fn the_two_files_share_one_identity() {
        let id = ProcessId { start: 42, pid: 7, ordinal: 1 };

        assert_eq!(parse_name("crash-", &artifact_name(&id)), Some(id));
        assert_eq!(parse_name("alacritree-", &session_log_name(&id)), Some(id));
    }

    #[test]
    fn unrelated_names_do_not_parse() {
        assert_eq!(parse_name("crash-", "state.toml"), None);
        assert_eq!(parse_name("crash-", "crash-notanumber-1.log"), None);
        assert_eq!(parse_name("crash-", "crash-1.log"), None);
    }

    /// Newest first, and a retry ordinal breaks the tie that a shared start
    /// would otherwise leave undefined.
    #[test]
    fn artifacts_sort_newest_first_then_by_ordinal() {
        let old = ProcessId { start: 10, pid: 1, ordinal: 0 };
        let new_a = ProcessId { start: 20, pid: 2, ordinal: 0 };
        let new_b = ProcessId { start: 20, pid: 2, ordinal: 1 };
        let mut all = vec![new_b, old, new_a];

        all.sort_by_key(sort_key);

        assert_eq!(all, vec![new_a, new_b, old]);
    }

    /// The process asking the question is the one process guaranteed to exist.
    #[test]
    fn the_current_process_is_live() {
        assert!(pid_is_live(std::process::id()));
    }

    /// Pid 0 is never a normal user process on any supported platform.
    #[test]
    fn an_impossible_pid_is_not_live() {
        assert!(!pid_is_live(0));
    }

    /// Start and pid are fixed for the process; only the ordinal moves.
    #[test]
    fn the_identity_is_stable_except_for_the_ordinal() {
        reset_identity_for_tests();
        let first = process_id();

        set_ordinal(3);
        let second = process_id();

        assert_eq!(first.start, second.start);
        assert_eq!(first.pid, second.pid);
        assert_eq!(second.ordinal, 3);
        reset_identity_for_tests();
    }
}
