//! Why alacritree died.
//!
//! A release build is `windows_subsystem = "windows"`, so stderr goes nowhere
//! when it is launched from a shortcut and a panic leaves no trace at all.
//! This records one artifact per GUI process: single writer, never shared, so
//! no cross-process protocol is needed to keep it intact.

use std::backtrace::Backtrace;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError, TryLockError};
use std::time::SystemTime;

use crate::logdir;

/// Whether a log directory has been chosen.  Read without the lock so the hook
/// can decline before contending for anything, and false until `install`, which
/// is what keeps the hook inert in unit tests that never opt in.
static ARMED: AtomicBool = AtomicBool::new(false);
/// Defaults on so a panic during `config::load()` is still recorded; lowered
/// once the preference is known.
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Latched by the first write failure so a broken disk is reported once, not
/// once per panic.
static BROKEN: AtomicBool = AtomicBool::new(false);
/// Panics the hook could not write because another thread held the lock.
static SKIPPED: AtomicUsize = AtomicUsize::new(0);

static STATE: Mutex<State> = Mutex::new(State::new());

struct State {
    version: &'static str,
    /// Where artifacts live.  Guarded rather than a `OnceLock` because every
    /// writer already holds this lock, and a directory that can only ever be
    /// set once is unreachable for a second test case.
    dir: Option<PathBuf>,
    /// The artifact this process has confirmed as its own, once `ensure_artifact`
    /// has created or reopened it.  Reused directly on every later call so a file
    /// that merely happens to already sit at our identity's path — debris from an
    /// unrelated writer — is never mistaken for ours; only a path we ourselves
    /// settled on through `create_new` is ever reopened for append.
    artifact: Option<PathBuf>,
}

impl State {
    const fn new() -> Self {
        Self { version: "", dir: None, artifact: None }
    }
}

/// Arm the recorder.  Creates the directory but no file: an artifact is only
/// created once something is worth writing, so a launch with crash logging off
/// leaves nothing behind.
pub fn install(dir: &Path, version: &'static str) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        let _ = writeln!(std::io::stderr(), "alacritree: cannot create {}: {e}", dir.display());
        BROKEN.store(true, Ordering::Relaxed);
        return;
    }
    {
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        state.version = version;
        state.dir = Some(dir.to_path_buf());
    }
    ARMED.store(true, Ordering::Relaxed);

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(info);
        previous(info);
    }));
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Create the artifact for this session.  Called after the gate is known.
pub fn session_begin() {
    if !writable() {
        return;
    }
    match STATE.lock() {
        Ok(mut state) => {
            let _ = ensure_artifact(&mut state);
        },
        Err(poisoned) => {
            let _ = ensure_artifact(&mut poisoned.into_inner());
        },
    }
}

pub fn record_exit(result: &Result<(), eframe::Error>) {
    if !writable() {
        return;
    }
    let mut event = String::new();
    let skipped = SKIPPED.load(Ordering::Relaxed);
    if skipped > 0 {
        event.push_str(&line(&format!("panic records skipped: {skipped}")));
    }
    match result {
        Ok(()) => event.push_str(&line("exit ok")),
        Err(e) => event.push_str(&line(&format!("exit error: {e}"))),
    }

    let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    write_event(&mut guard, &event);
}

fn writable() -> bool {
    ENABLED.load(Ordering::Relaxed)
        && !BROKEN.load(Ordering::Relaxed)
        && ARMED.load(Ordering::Relaxed)
}

fn timestamp() -> String {
    // Seconds since the epoch, rendered without a date crate: the artifact
    // name already carries the machine-readable start, so this only has to be
    // orderable and human-skimmable.
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    format!("t{secs}")
}

fn line(body: &str) -> String {
    format!("{} {body}\n", timestamp())
}

/// The single initializer.  The header has three possible authors — a panic
/// during config load, `session_begin`, and any write after the file has been
/// removed — and a record written into a headerless file has to be read back as
/// indeterminate, discarding information we actually had.
///
/// A file already sitting at our identity's path is reopened for append only if
/// it is the exact path this process itself settled on earlier; otherwise it is
/// left untouched and `create_new`'s collision retry claims the next ordinal
/// instead, so debris from an unrelated writer can never be corrupted by an
/// append and never gets mistaken for a readable header.
fn ensure_artifact(state: &mut State) -> Option<File> {
    let dir = state.dir.as_ref()?;

    if let Some(path) = &state.artifact
        && let Ok(file) = OpenOptions::new().append(true).open(path)
    {
        return Some(file);
    }
    // No confirmed artifact yet, or it was removed underneath us: either way,
    // fall through to the allocator below rather than losing the record.

    let mut id = logdir::process_id();
    // `create_new` is the allocator: a collision means debris under an
    // identity we believed unique, and truncating it would destroy a record.
    for _ in 0..32 {
        let path = dir.join(logdir::artifact_name(&id));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                logdir::set_ordinal(id.ordinal);
                let header = line(&format!("start {} pid={}", state.version, id.pid));
                if file.write_all(header.as_bytes()).is_err() {
                    break;
                }
                let _ = file.flush();
                state.artifact = Some(path);
                return Some(file);
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => id.ordinal += 1,
            Err(_) => break,
        }
    }
    fail_once("cannot create a crash artifact");
    None
}

fn fail_once(what: &str) {
    if !BROKEN.swap(true, Ordering::Relaxed) {
        let _ = writeln!(std::io::stderr(), "alacritree: {what}; crash logging is off");
    }
}

/// One `write_all` of a fully built string, to a handle opened and closed per
/// event: if a panic ever does reach an abort, abort skips destructors, so
/// nothing may be left sitting in a buffer.
fn write_event(state: &mut State, event: &str) {
    let Some(mut file) = ensure_artifact(state) else { return };
    if file.write_all(event.as_bytes()).is_err() || file.flush().is_err() {
        fail_once("cannot write the crash artifact");
    }
}

fn record_panic(info: &PanicHookInfo<'_>) {
    if !writable() {
        return;
    }

    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed").to_string();
    let location = info
        .location()
        .map(|l| format!("{}:{}", l.file(), l.line()))
        .unwrap_or_else(|| "unknown location".to_string());
    let payload = payload_of(info);
    // Captures regardless of RUST_BACKTRACE, which we cannot set: `set_var` is
    // unsafe in edition 2024 and PTY threads are already running by now.
    let backtrace = Backtrace::force_capture();

    let mut event = line(&format!("PANIC thread={thread}"));
    event.push_str(&format!("  at {location}\n"));
    event.push_str(&format!("  {payload}\n"));
    for bt_line in backtrace.to_string().lines() {
        event.push_str(&format!("  {bt_line}\n"));
    }

    // `try_lock`, never `lock`: a thread that panics while already holding this
    // mutex would wait on itself forever, and the mutex is not poisoned yet, so
    // recovering from poisoning cannot help.  A lost record beats a hang.
    match STATE.try_lock() {
        Ok(mut state) => write_event(&mut state, &event),
        Err(TryLockError::Poisoned(p)) => write_event(&mut p.into_inner(), &event),
        Err(TryLockError::WouldBlock) => {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
            let _ = writeln!(std::io::stderr(), "alacritree: panic record skipped (recorder busy)");
        },
    }
}

fn payload_of(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "non-string panic payload".to_string()
}

#[cfg(test)]
pub fn reset_for_tests(dir: &Path) {
    // Wholesale, so a field added later cannot leak between test cases.
    {
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        *state = State::new();
        state.dir = Some(dir.to_path_buf());
    }
    ARMED.store(true, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    BROKEN.store(false, Ordering::Relaxed);
    SKIPPED.store(0, Ordering::Relaxed);
    logdir::reset_identity_for_tests();
}

#[cfg(test)]
pub fn artifact_path_for_tests() -> Option<PathBuf> {
    let dir = STATE.lock().unwrap_or_else(PoisonError::into_inner).dir.clone()?;
    let path = dir.join(logdir::artifact_name(&logdir::process_id()));
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every hook-installing test runs through this so the harness's hook is
    /// restored: `take_hook` puts the default in place of what it removes
    /// rather than leaving a slot, so restoration has to be explicit — and it
    /// has to happen even when the body unwinds (a failing `assert!`), not just
    /// on the success path. Restoring from a `Drop` guard would not work here:
    /// `set_hook` itself panics when called from a thread that is already
    /// panicking, and a `Drop` runs while its unwind is still in flight, so a
    /// guard's `drop` would be a second panic during the first one's unwind —
    /// which Rust escalates straight to an abort. Catching the unwind first,
    /// restoring once it is no longer in flight, then resuming it is the only
    /// ordering that keeps `set_hook` outside of a panicking thread.
    fn with_recorder<T>(body: impl FnOnce(&Path) -> T) -> T {
        let dir = tempfile::tempdir().expect("a temp dir");
        let previous = std::panic::take_hook();
        reset_for_tests(dir.path());
        install(dir.path(), "test");
        set_enabled(true);

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(dir.path())));
        std::panic::set_hook(previous);
        match outcome {
            Ok(out) => out,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn artifact_text() -> String {
        let path = artifact_path_for_tests().expect("an artifact was created");
        String::from_utf8_lossy(&std::fs::read(path).expect("the artifact is readable")).into()
    }

    /// Counts header lines (`<timestamp> start ...`) rather than scanning for
    /// the substring " start " anywhere in the text: a backtrace frame symbol
    /// could otherwise render that substring and produce a spurious match.
    fn header_count(text: &str) -> usize {
        text.lines()
            .filter(|line| line.split_once(' ').is_some_and(|(_, rest)| rest.starts_with("start ")))
            .count()
    }

    /// The whole point: a panic that would otherwise vanish leaves a record
    /// naming what failed and where.
    #[test]
    fn a_panic_is_recorded_with_its_payload_location_and_thread() {
        with_recorder(|_| {
            let _ = std::panic::catch_unwind(|| panic!("boom-marker"));

            let text = artifact_text();
            assert!(text.contains("boom-marker"), "payload missing:\n{text}");
            assert!(text.contains("crash_log.rs"), "location missing:\n{text}");
            assert!(text.contains("thread="), "thread missing:\n{text}");
        });
    }

    /// A PTY thread panicking leaves the app running, so the record has to name
    /// the thread or it is unattributable.
    #[test]
    fn a_named_worker_thread_is_named_in_its_record() {
        with_recorder(|_| {
            std::thread::Builder::new()
                .name("pty-worker".into())
                .spawn(|| panic!("worker-boom"))
                .expect("spawn")
                .join()
                .expect_err("the thread panicked");

            let text = artifact_text();
            assert!(text.contains("pty-worker"), "thread name missing:\n{text}");
            assert!(text.contains("worker-boom"), "payload missing:\n{text}");
        });
    }

    /// A crash during config load happens before `session_begin`, and a file
    /// deleted underneath a live process has to come back — both go through the
    /// same initializer, so neither can produce a headerless artifact.
    #[test]
    fn every_writer_produces_exactly_one_header() {
        with_recorder(|_| {
            let _ = std::panic::catch_unwind(|| panic!("early"));
            session_begin();
            let text = artifact_text();

            assert_eq!(header_count(&text), 1, "not exactly one header:\n{text}");
        });
    }

    #[test]
    fn a_deleted_artifact_is_recreated_with_a_header() {
        with_recorder(|_| {
            session_begin();
            std::fs::remove_file(artifact_path_for_tests().unwrap()).expect("remove");

            let _ = std::panic::catch_unwind(|| panic!("after-delete"));

            let text = artifact_text();
            assert_eq!(header_count(&text), 1, "not exactly one header:\n{text}");
            assert!(text.contains("after-delete"), "payload missing:\n{text}");
        });
    }

    /// `create_new` is the allocator, never `create`: a file already occupying
    /// our identity's path is debris from an unrelated writer, not a header we
    /// can trust, so it must be left alone and the artifact has to land at the
    /// next ordinal — which also proves `set_ordinal` recorded what the retry
    /// actually settled on.
    #[test]
    fn a_colliding_path_is_left_untouched_and_the_next_ordinal_is_used() {
        with_recorder(|dir| {
            let id = logdir::process_id();
            let collision_path = dir.join(logdir::artifact_name(&id));
            std::fs::write(&collision_path, "not ours").expect("seed a collision");

            let _ = std::panic::catch_unwind(|| panic!("collision-marker"));

            let collision_content =
                std::fs::read_to_string(&collision_path).expect("still readable");
            assert_eq!(collision_content, "not ours", "the colliding file was overwritten");

            let text = artifact_text();
            assert!(text.contains("collision-marker"), "payload missing:\n{text}");
            assert_eq!(
                logdir::process_id().ordinal,
                id.ordinal + 1,
                "set_ordinal did not record the ordinal create_new settled on"
            );
        });
    }

    /// The gate is what `crash_log = false` buys, and it must silence writes
    /// without silencing the chained default hook.
    #[test]
    fn a_disabled_recorder_writes_nothing() {
        with_recorder(|dir| {
            set_enabled(false);

            let _ = std::panic::catch_unwind(|| panic!("silenced"));

            let entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
            assert!(entries.is_empty(), "wrote {} files while disabled", entries.len());
        });
    }

    /// `install` runs before config is read, so a directory that does not exist
    /// yet must not cost the first crash its record.
    #[test]
    fn a_missing_log_directory_is_created() {
        let root = tempfile::tempdir().expect("a temp dir");
        let nested = root.path().join("does").join("not").join("exist");
        let previous = std::panic::take_hook();
        reset_for_tests(&nested);
        install(&nested, "test");
        set_enabled(true);

        let _ = std::panic::catch_unwind(|| panic!("first-launch"));

        std::panic::set_hook(previous);
        assert!(nested.is_dir(), "the log directory was not created");
        assert!(artifact_text().contains("first-launch"));
    }

    #[test]
    fn a_clean_exit_is_recorded_and_nothing_is_deleted() {
        with_recorder(|_| {
            session_begin();

            record_exit(&Ok(()));

            let text = artifact_text();
            assert!(text.contains("exit ok"), "no exit marker:\n{text}");
        });
    }

    /// A detached worker can outlive the exit, and its record has to land in the
    /// same file rather than in a resurrected one.
    #[test]
    fn a_panic_after_the_exit_marker_lands_in_the_same_file() {
        with_recorder(|_| {
            session_begin();
            record_exit(&Ok(()));

            let _ = std::panic::catch_unwind(|| panic!("late-worker"));

            let text = artifact_text();
            assert!(text.contains("exit ok"), "exit marker lost:\n{text}");
            assert!(text.contains("late-worker"), "late panic lost:\n{text}");
        });
    }
}
