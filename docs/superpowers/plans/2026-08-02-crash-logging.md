# Crash Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record why alacritree died — panics, exit reason, and session markers — into per-process artifacts a developer can read with one command, because a release build currently throws every diagnostic away.

**Architecture:** A panic hook writes to a per-process, single-writer artifact under a machine-local log directory. Nothing is shared between processes, so there is no locking protocol across processes and no reconciliation. The single reader-facing view is *derived* by a read-only `alacritree crashes` subcommand, never maintained as a file. A separate opt-in continuous log tees `env_logger`'s stderr stream to a per-process file.

**Tech Stack:** Rust 2024, `env_logger` 0.11, `std::backtrace`, `windows-sys` (one added feature), `libc` (macOS only, already present), `tempfile` (dev-dependency, already present).

**Spec:** `docs/superpowers/specs/2026-08-02-crash-logging-design.md`. Read it before starting. This plan implements it; where they disagree, the spec wins.

## Global Constraints

- Edition **2024**. `std::env::set_var` is `unsafe` in this edition — never call it; use `Backtrace::force_capture()` instead of setting `RUST_BACKTRACE`.
- `Cargo.toml` declares `rust-version = "1.85.0"`, but the crate already uses let-chains (`input.rs:29`, `terminal_view.rs:1083`), which need **1.88**. The declared MSRV is stale; the real floor is 1.88. This plan uses let-chains freely, matching the surrounding code. Do not "fix" the declaration as part of this work — it is a pre-existing inaccuracy and out of scope.
- **No new crate dependencies.** The only dependency change permitted is adding the `Win32_System_Threading` feature to the existing `windows-sys` entry.
- Only `alacritree/` may be modified. `alacritty/`, `alacritty_terminal/`, `alacritty_config/`, `alacritty_config_derive/` are vendored upstream and read-only.
- `cargo fmt` is enforced. Run it before every commit.
- Comments explain *why*, never *what*. No comment may reference this plan, a task number, a PR, or the TDD cycle.
- Commit messages are Conventional Commits, imperative, ≤72 chars, and must carry the trailer `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`.
- The panic hook may only format, lock, append, and flush. It must never rename, read other files, delete, propagate an error, or panic.
- Every test that installs a panic hook must restore the previous one before returning.

## Branch Setup

Do this once, before Task 1.

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
```

Take the entry whose title carries the highest `[n]` marker; its `headRefName` is the base. As of 2026-08-02 that was `feat/sidebar-upstream-status` (PR #166, `[7]`) — verify, do not assume.

```sh
git worktree add ../alacritree-worktrees/feat/crash-logging -b feat/crash-logging origin/<base>
cd ../alacritree-worktrees/feat/crash-logging
```

## File Structure

| File | Responsibility |
| --- | --- |
| `alacritree/src/logdir.rs` | **new** — `log_dir()`, `pid_is_live()`, `ProcessId` identity, artifact filename format/parse/order. Shared by the recorder and the tee. |
| `alacritree/src/crash_log.rs` | **new** — the panic hook, artifact writing, suppression, and artifact pruning. |
| `alacritree/src/logging.rs` | **new** — `Tee`, the deferred file sink, and continuous-log pruning. |
| `alacritree/src/cli/crashes.rs` | **new** — the read-only `crashes` subcommand. |
| `alacritree/src/cli/mod.rs` | modify — add `Command::Crashes`, route it locally. |
| `alacritree/src/cli/doctor.rs` | modify — add the crash-artifact summary check. |
| `alacritree/src/config.rs` | modify — add `RawDebug`, `DebugConfig`, and the `[debug]` wiring. |
| `alacritree/src/main.rs` | modify — module declarations and the initialization order. |
| `alacritree/Cargo.toml` | modify — add the `Win32_System_Threading` windows-sys feature. |
| `alacritree/tests/cli_isolation.rs` | **new** — subprocess tests against the real binary. |
| `install.local.ps1` | modify — add `alacritree.pdb` to `$Payload`. |

---

### Task 1: Log directory, process identity, and liveness

**Files:**
- Create: `alacritree/src/logdir.rs`
- Modify: `alacritree/src/main.rs` (add `mod logdir;` alphabetically, between `mod links;` and `mod mcp;`)
- Modify: `alacritree/Cargo.toml:83` (windows-sys features)

**Interfaces:**
- Produces:
  - `pub fn log_dir() -> Option<PathBuf>`
  - `pub fn pid_is_live(pid: u32) -> bool`
  - `pub struct ProcessId { pub start: u128, pub pid: u32, pub ordinal: u32 }`
  - `pub fn process_id() -> ProcessId` — start and pid captured once per process; ordinal reads the shared atomic
  - `pub fn set_ordinal(ordinal: u32)`
  - `pub fn artifact_name(id: &ProcessId) -> String` → `crash-<start>-<pid>[-<ordinal>].log`
  - `pub fn session_log_name(id: &ProcessId) -> String` → `alacritree-<start>-<pid>[-<ordinal>].log`
  - `pub fn parse_name(prefix: &str, file_name: &str) -> Option<ProcessId>`
  - `pub fn sort_key(id: &ProcessId) -> (Reverse<u128>, u32)`
  - `#[cfg(test)] pub fn reset_identity_for_tests()`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/logdir.rs` with only the test module and stub signatures absent — the tests must not compile against anything yet. Write this file:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree logdir`
Expected: FAIL to compile — `cannot find function 'artifact_name'`, `cannot find type 'ProcessId'`, and so on.

- [ ] **Step 3: Implement the module**

Insert above the `#[cfg(test)] mod tests` block in `alacritree/src/logdir.rs`:

```rust
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
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

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
```

- [ ] **Step 4: Add the windows-sys feature**

In `alacritree/Cargo.toml`, extend the existing `windows-sys` features list (around line 83) with `"Win32_System_Threading"`, keeping the list alphabetical:

```toml
windows-sys = { version = "0.59", features = [
    "Win32_Foundation",
    "Win32_System_Console",
    "Win32_System_LibraryLoader",
    "Win32_System_Threading",
    "Win32_UI_WindowsAndMessaging",
] }
```

Also update the comment above it to mention the new use, keeping its existing style:

```toml
# Restricts the DLL search path at startup (`harden_dll_search_path`), borrows
# the launching shell's console so the CLI can print (`attach_parent_console`),
# reads the cursor during a file drag, which winit discards, and asks whether a
# pid is still running before pruning its log.
```

- [ ] **Step 5: Declare the module**

In `alacritree/src/main.rs`, add `mod logdir;` between `mod links;` and `mod mcp;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree logdir`
Expected: PASS, 8 tests.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/logdir.rs alacritree/src/main.rs alacritree/Cargo.toml
git commit -m "feat(logging): add log directory and process identity

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 2: `[debug]` config section

**Files:**
- Modify: `alacritree/src/config.rs` (add `DebugConfig` beside `Config`, `RawDebug`, the `debug` field on `RawConfig` at ~line 979, and the conversion in `into_config` at ~line 1635)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `pub struct DebugConfig { pub crash_log: bool, pub persistent_logging: bool }`
  - `Config.debug: DebugConfig`

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` in `alacritree/src/config.rs`:

```rust
/// A derived `Default` on a bare `bool` would make this false and silently
/// invert the intended default, so the raw field is an `Option` resolved with
/// `unwrap_or` — the same shape `wsl.resident_helper` uses.
#[test]
fn crash_logging_is_on_unless_asked_otherwise() {
    let raw: RawConfig = toml::from_str("").unwrap();

    let config = raw.into_config();

    assert!(config.debug.crash_log);
    assert!(!config.debug.persistent_logging);
}

#[test]
fn crash_logging_can_be_turned_off() {
    let raw: RawConfig = toml::from_str("[debug]\ncrash_log = false").unwrap();

    assert!(!raw.into_config().debug.crash_log);
}

#[test]
fn persistent_logging_can_be_turned_on() {
    let raw: RawConfig = toml::from_str("[debug]\npersistent_logging = true").unwrap();

    assert!(raw.into_config().debug.persistent_logging);
}

/// `[debug]` in both files merges key by key rather than the later table
/// replacing the earlier one wholesale.
#[test]
fn a_debug_table_in_both_files_merges_key_by_key() {
    let alacritty: toml::Value = toml::from_str("[debug]\npersistent_logging = true").unwrap();
    let alacritree: toml::Value = toml::from_str("[debug]\ncrash_log = false").unwrap();

    let merged = merge(alacritty, alacritree);
    let config: RawConfig = merged.try_into().unwrap();
    let config = config.into_config();

    assert!(config.debug.persistent_logging, "the alacritty.toml key was dropped");
    assert!(!config.debug.crash_log, "the alacritree.toml key was dropped");
}
```

The merge helper is `fn merge(base: toml::Value, replacement: toml::Value) -> toml::Value` at `config.rs:949`, and alacritty is the base — that argument order is what the last test asserts.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree config::tests::crash_logging`
Expected: FAIL to compile — `no field 'debug' on type 'Config'`.

- [ ] **Step 3: Add the config types**

Add beside the other config structs in `alacritree/src/config.rs`:

```rust
/// alacritty's `[debug]` section, plus one alacritree-only key.
#[derive(Debug, Clone)]
pub struct DebugConfig {
    /// alacritree-only, set in `alacritree.toml`.  Default on: a crash that
    /// leaves no record is the failure this exists to prevent.
    pub crash_log: bool,
    /// Upstream's name and upstream's default.
    pub persistent_logging: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self { crash_log: true, persistent_logging: false }
    }
}
```

Add the raw counterpart beside `RawGeneral`:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawDebug {
    crash_log: Option<bool>,
    persistent_logging: Option<bool>,
}
```

- [ ] **Step 4: Wire it into `Config` and `RawConfig`**

Add to `pub struct Config` (after `ipc_socket`):

```rust
    pub debug: DebugConfig,
```

Add to `struct RawConfig` (after `general`):

```rust
    debug: RawDebug,
```

Add to the `Config { .. }` construction at the end of `into_config` (after `ipc_socket`):

```rust
            debug: DebugConfig {
                crash_log: self.debug.crash_log.unwrap_or(true),
                persistent_logging: self.debug.persistent_logging.unwrap_or(false),
            },
```

`impl Default for Config` at `config.rs:688` constructs every field explicitly, so it needs the new field too — add `debug: DebugConfig::default()` alongside the others. `DebugConfig::default()` and the `unwrap_or` values above must agree; a disagreement means a config file that omits `[debug]` behaves differently from one that includes it empty.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree config`
Expected: PASS, including every pre-existing config test.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): add the [debug] section

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 3: The crash recorder — hook, artifact, and gate

**Files:**
- Create: `alacritree/src/crash_log.rs`
- Modify: `alacritree/src/main.rs` (add `mod crash_log;` between `mod config;` and `mod doppler;`)

**Interfaces:**
- Consumes: `logdir::{log_dir, process_id, set_ordinal, artifact_name, ProcessId}`
- Produces:
  - `pub fn install(dir: &Path, version: &'static str)` — `'static` because the version is stored in a `static Mutex<State>`; `env!("CARGO_PKG_VERSION")` satisfies it
  - `pub fn set_enabled(enabled: bool)`
  - `pub fn session_begin()`
  - `pub fn record_exit(result: &Result<(), eframe::Error>)`
  - `#[cfg(test)] pub fn reset_for_tests(dir: &Path)`
  - `#[cfg(test)] pub fn artifact_path_for_tests() -> Option<PathBuf>`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/crash_log.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Every hook-installing test runs through this so the harness's hook is
    /// restored: `take_hook` puts the default in place of what it removes
    /// rather than leaving a slot, so restoration has to be explicit.
    fn with_recorder<T>(body: impl FnOnce(&Path) -> T) -> T {
        let dir = tempfile::tempdir().expect("a temp dir");
        let previous = std::panic::take_hook();
        reset_for_tests(dir.path());
        install(dir.path(), "test");
        set_enabled(true);
        let out = body(dir.path());
        std::panic::set_hook(previous);
        out
    }

    fn artifact_text() -> String {
        let path = artifact_path_for_tests().expect("an artifact was created");
        String::from_utf8_lossy(&std::fs::read(path).expect("the artifact is readable")).into()
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

            assert_eq!(text.matches(" start ").count(), 1, "not exactly one header:\n{text}");
        });
    }

    #[test]
    fn a_deleted_artifact_is_recreated_with_a_header() {
        with_recorder(|_| {
            session_begin();
            std::fs::remove_file(artifact_path_for_tests().unwrap()).expect("remove");

            let _ = std::panic::catch_unwind(|| panic!("after-delete"));

            let text = artifact_text();
            assert_eq!(text.matches(" start ").count(), 1, "not exactly one header:\n{text}");
            assert!(text.contains("after-delete"), "payload missing:\n{text}");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree crash_log`
Expected: FAIL to compile — `cannot find function 'install'`.

This is the RED test for the whole feature: before the hook exists, no artifact is ever created, so `artifact_path_for_tests()` returns `None` and the tests fail on the missing file rather than on a formatting detail.

- [ ] **Step 3: Implement the recorder**

Insert above the test module in `alacritree/src/crash_log.rs`:

```rust
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
use std::sync::{Mutex, OnceLock, PoisonError, TryLockError};
use std::time::SystemTime;

use crate::logdir::{self, ProcessId};

/// Where artifacts live.  Unset until `install`, which is what keeps the hook
/// inert in unit tests that never opt in.
static DIR: OnceLock<PathBuf> = OnceLock::new();
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
    panics: usize,
}

impl State {
    const fn new() -> Self {
        Self { version: "", panics: 0 }
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
    let _ = DIR.set(dir.to_path_buf());
    if let Ok(mut state) = STATE.lock() {
        state.version = version;
    }

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
        Ok(state) => {
            let _ = ensure_artifact(&state);
        },
        Err(poisoned) => {
            let _ = ensure_artifact(&poisoned.into_inner());
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

    let guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    write_event(&guard, &event);
}

fn writable() -> bool {
    ENABLED.load(Ordering::Relaxed) && !BROKEN.load(Ordering::Relaxed) && DIR.get().is_some()
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
fn ensure_artifact(state: &State) -> Option<File> {
    let dir = DIR.get()?;
    let mut id = logdir::process_id();
    let path = dir.join(logdir::artifact_name(&id));

    if path.exists() {
        return OpenOptions::new().append(true).open(&path).ok();
    }

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
fn write_event(state: &State, event: &str) {
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
        Ok(state) => write_event(&state, &event),
        Err(TryLockError::Poisoned(p)) => write_event(&p.into_inner(), &event),
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
    // `DIR` is a `OnceLock`, so tests share whichever directory won the race;
    // each test reads its artifact through `artifact_path_for_tests`, which
    // resolves against that same directory.
    let _ = DIR.set(dir.to_path_buf());
    ENABLED.store(true, Ordering::Relaxed);
    BROKEN.store(false, Ordering::Relaxed);
    SKIPPED.store(0, Ordering::Relaxed);
    logdir::reset_identity_for_tests();
}

#[cfg(test)]
pub fn artifact_path_for_tests() -> Option<PathBuf> {
    let dir = DIR.get()?;
    let path = dir.join(logdir::artifact_name(&logdir::process_id()));
    path.exists().then_some(path)
}
```

**Implementer note:** `DIR` is a `OnceLock` and the tests above each create their own `tempdir`. If more than one crash_log test runs concurrently they will contend for it. Run this module's tests with `cargo test -p alacritree crash_log -- --test-threads=1` and add `#[ignore]`-free serialization by keeping every crash_log test inside one `#[test]` function if contention appears. Prefer the single-threaded flag first; only merge tests if that is insufficient.

- [ ] **Step 4: Declare the module**

In `alacritree/src/main.rs`, add `mod crash_log;` between `mod config;` and `mod doppler;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree crash_log -- --test-threads=1`
Expected: PASS, 8 tests.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add alacritree/src/crash_log.rs alacritree/src/main.rs
git commit -m "feat(logging): record panics to a per-process artifact

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 4: Suppression, collapsing, and the lock-skip contract

**Files:**
- Modify: `alacritree/src/crash_log.rs`

**Interfaces:**
- Consumes: everything from Task 3.
- Produces: no new public functions; `State` gains `last: Option<String>`, `repeats: usize`.

- [ ] **Step 1: Write the failing tests**

Add to `crash_log.rs`'s test module:

```rust
/// A panicking PTY thread leaves the app running and the IPC listener spawns a
/// thread per connection, so one repeatable defect could otherwise append a
/// backtrace per request forever.
#[test]
fn panic_records_stop_after_the_cap() {
    with_recorder(|_| {
        for i in 0..25 {
            let _ = std::panic::catch_unwind(|| panic!("cap-{}", i));
        }

        let text = artifact_text();
        assert_eq!(text.matches("PANIC thread=").count(), 20, "cap not applied:\n{text}");
        assert!(text.contains("panic records suppressed after 20"), "no notice:\n{text}");
    });
}

#[test]
fn identical_consecutive_panics_collapse_into_a_count() {
    with_recorder(|_| {
        for _ in 0..3 {
            let _ = std::panic::catch_unwind(|| panic!("same-place"));
        }

        let text = artifact_text();
        assert_eq!(text.matches("PANIC thread=").count(), 1, "not collapsed:\n{text}");
        assert!(text.contains("x3"), "no repeat count:\n{text}");
    });
}

/// The exit marker reports what the recorder could not write.  It is
/// best-effort by construction: a skip that races `record_exit`'s read may be
/// absent, and that is not a failure.
#[test]
fn skipped_records_are_counted_for_the_exit_marker() {
    with_recorder(|_| {
        let held = STATE.lock().expect("the recorder lock");
        let _ = std::panic::catch_unwind(|| panic!("while-held"));
        drop(held);

        record_exit(&Ok(()));

        let text = artifact_text();
        assert!(text.contains("panic records skipped: 1"), "no skip marker:\n{text}");
    });
}

/// A blocking `lock()` here waits on a mutex this very thread holds and never
/// becomes poisoned.  This test hangs against that implementation.
#[test]
fn a_panic_while_holding_the_lock_does_not_hang() {
    with_recorder(|_| {
        let held = STATE.lock().expect("the recorder lock");

        let result = std::panic::catch_unwind(|| panic!("self-deadlock"));

        drop(held);
        assert!(result.is_err(), "the panic did not unwind");
    });
}

/// An earlier panic must not silence the next one.
#[test]
fn a_poisoned_lock_still_records() {
    with_recorder(|_| {
        let _ = std::panic::catch_unwind(|| {
            let _guard = STATE.lock().expect("the recorder lock");
            panic!("poisoning");
        });

        let _ = std::panic::catch_unwind(|| panic!("after-poison"));

        let text = artifact_text();
        assert!(text.contains("after-poison"), "record lost after poisoning:\n{text}");
    });
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree crash_log -- --test-threads=1`
Expected: FAIL — `panic_records_stop_after_the_cap` finds 25 records, `identical_consecutive_panics_collapse_into_a_count` finds 3.

- [ ] **Step 3: Extend `State` and gate the writes**

Replace `struct State` and its `impl` in `crash_log.rs`:

```rust
/// How many panic records one process may write before it starts costing more
/// than it explains.
const MAX_PANIC_RECORDS: usize = 20;

struct State {
    version: &'static str,
    panics: usize,
    /// Location of the previous panic, for collapsing a repeat that fires from
    /// the same place every frame.
    last: Option<String>,
    repeats: usize,
}

impl State {
    const fn new() -> Self {
        Self { version: "", panics: 0, last: None, repeats: 0 }
    }
}
```

- [ ] **Step 4: Apply the cap and the collapse**

Replace the tail of `record_panic` — the `match STATE.try_lock()` block — with a call into a new function, and add that function:

```rust
    match STATE.try_lock() {
        Ok(mut state) => record_bounded(&mut state, &location, &event),
        Err(TryLockError::Poisoned(p)) => record_bounded(&mut p.into_inner(), &location, &event),
        Err(TryLockError::WouldBlock) => {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
            let _ = writeln!(std::io::stderr(), "alacritree: panic record skipped (recorder busy)");
        },
    }
}

/// Write a panic record unless this process has already said enough.
fn record_bounded(state: &mut State, location: &str, event: &str) {
    if state.last.as_deref() == Some(location) {
        state.repeats += 1;
        return;
    }
    flush_repeats(state);

    if state.panics == MAX_PANIC_RECORDS {
        state.panics += 1;
        let notice = line(&format!("panic records suppressed after {MAX_PANIC_RECORDS}"));
        write_event(state, &notice);
        return;
    }
    if state.panics > MAX_PANIC_RECORDS {
        return;
    }

    state.panics += 1;
    state.last = Some(location.to_string());
    write_event(state, event);
}

/// Close a collapsed run, so the count reaches the file rather than living
/// only in memory that a crash discards.
fn flush_repeats(state: &mut State) {
    if state.repeats == 0 {
        return;
    }
    let repeats = state.repeats + 1;
    state.repeats = 0;
    let notice = line(&format!("  x{repeats} from the same location"));
    write_event(state, &notice);
}
```

`write_event`'s signature is unchanged — `fn write_event(state: &State, event: &str)`. Calling it as `write_event(state, event)` from inside `record_bounded`, where `state: &mut State`, reborrows automatically. Likewise `record_bounded(&mut state, …)` with a `MutexGuard<State>` coerces through `DerefMut`.

- [ ] **Step 5: Flush a pending collapse at exit**

In `record_exit`, replace the lock acquisition and write with:

```rust
    let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    flush_repeats(&mut guard);
    write_event(&guard, &event);
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree crash_log -- --test-threads=1`
Expected: PASS, 13 tests.

If `a_panic_while_holding_the_lock_does_not_hang` hangs, `try_lock` was not used — that is the regression the test exists to catch.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/crash_log.rs
git commit -m "feat(logging): bound panic records per process

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 5: Artifact pruning

**Files:**
- Modify: `alacritree/src/crash_log.rs`

**Interfaces:**
- Consumes: `logdir::{log_dir, pid_is_live, parse_name}`
- Produces: `pub fn prune()`

- [ ] **Step 1: Write the failing tests**

Add to `crash_log.rs`'s test module:

```rust
/// Retention is by age and liveness alone.  Reading a file to decide whether to
/// keep it is what let two earlier designs delete the only record of a crash.
#[test]
fn pruning_ignores_contents_entirely() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let dead = ProcessId { start: 1, pid: 0, ordinal: 0 };
    let old_clean = dir.path().join(logdir::artifact_name(&dead));
    std::fs::write(&old_clean, "t1 start v pid=0\nt2 exit ok\n").unwrap();
    let old_crash = dir.path().join("crash-2-0.log");
    std::fs::write(&old_crash, "t1 start v pid=0\nt2 PANIC thread=main\n").unwrap();
    set_mtime_days_ago(&old_clean, 40);
    set_mtime_days_ago(&old_crash, 40);

    prune_in(dir.path());

    assert!(!old_clean.exists(), "an old dead-pid artifact survived");
    assert!(!old_crash.exists(), "contents changed the decision");
}

#[test]
fn a_live_producers_artifact_is_never_pruned() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mine = ProcessId { start: 1, pid: std::process::id(), ordinal: 0 };
    let path = dir.path().join(logdir::artifact_name(&mine));
    std::fs::write(&path, "t1 start v\n").unwrap();
    set_mtime_days_ago(&path, 400);

    prune_in(dir.path());

    assert!(path.exists(), "a live process's artifact was deleted");
}

#[test]
fn a_fresh_artifact_survives_even_from_a_dead_pid() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("crash-1-0.log");
    std::fs::write(&path, "t1 start v\n").unwrap();

    prune_in(dir.path());

    assert!(path.exists(), "a fresh artifact was deleted");
}

/// A concurrently starting instance can delete the same path first.
#[test]
fn a_vanished_path_is_not_an_error() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("crash-1-0.log");
    std::fs::write(&path, "t1 start v\n").unwrap();
    set_mtime_days_ago(&path, 40);

    prune_in(dir.path());
    prune_in(dir.path());

    assert!(!path.exists());
}

#[test]
fn files_that_are_not_artifacts_are_left_alone() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let state = dir.path().join("state.toml");
    std::fs::write(&state, "x").unwrap();
    set_mtime_days_ago(&state, 400);

    prune_in(dir.path());

    assert!(state.exists(), "an unrelated file was deleted");
}

fn set_mtime_days_ago(path: &Path, days: u64) {
    let when = SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    let file = OpenOptions::new().write(true).open(path).expect("open for mtime");
    file.set_modified(when).expect("set mtime");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree crash_log::tests::pruning -- --test-threads=1`
Expected: FAIL to compile — `cannot find function 'prune_in'`.

- [ ] **Step 3: Implement pruning**

Add to `crash_log.rs`:

```rust
/// How long an artifact outlives the process that wrote it.
const RETAIN_DAYS: u64 = 30;

pub fn prune() {
    if let Some(dir) = DIR.get() {
        prune_in(dir);
    }
}

/// Delete by filename and `stat` only — nothing is opened.
///
/// This is safe against a concurrent pruner without any claim protocol because
/// identities are never reused: a path is only deleted when its producer is
/// dead and the file is over `RETAIN_DAYS` old, and recreating that exact path
/// would need the same start nanosecond, pid, and ordinal.  If that invariant
/// is ever broken, deletion has to verify identity first.
fn prune_in(dir: &Path) {
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(RETAIN_DAYS * 86_400);
    let Ok(entries) = std::fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = logdir::parse_name("crash-", name) else { continue };
        if logdir::pid_is_live(id.pid) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if modified > cutoff {
            continue;
        }
        // A concurrent pruner reaching it first is the expected outcome, not a
        // problem worth reporting.
        let _ = std::fs::remove_file(entry.path());
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree crash_log -- --test-threads=1`
Expected: PASS, 18 tests.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/crash_log.rs
git commit -m "feat(logging): prune artifacts by age and liveness

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 6: The log tee and the continuous log

**Files:**
- Create: `alacritree/src/logging.rs`
- Modify: `alacritree/src/main.rs` (add `mod logging;` between `mod logdir;` and `mod mcp;`)

**Interfaces:**
- Consumes: `logdir::{log_dir, pid_is_live, parse_name, process_id, session_log_name, set_ordinal}`
- Produces:
  - `pub struct Tee { sink: Arc<Mutex<Option<File>>> }`
  - `pub fn tee() -> (Tee, Arc<Mutex<Option<File>>>)`
  - `pub fn open_session_log(dir: &Path) -> Option<File>`
  - `pub fn prune_session_logs(dir: &Path)`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/logging.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A sink filled after `Target::Pipe` has already moved the writer is the
    /// whole reason the handle is shared.
    #[test]
    fn a_sink_filled_after_construction_receives_writes() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("late.log");
        let (mut tee, sink) = tee();

        tee.write_all(b"before\n").expect("write");
        *sink.lock().unwrap() = Some(File::create(&path).expect("create"));
        tee.write_all(b"after\n").expect("write");

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains("before"), "wrote to a sink that was not set yet");
        assert!(text.contains("after"), "the late sink got nothing");
    }

    /// If stderr accepts only a prefix, env_logger retries the suffix.  Writing
    /// the whole buffer while returning the short count duplicates it.
    #[test]
    fn a_short_write_mirrors_only_the_accepted_prefix() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("short.log");
        let sink = Arc::new(Mutex::new(Some(File::create(&path).expect("create"))));
        let mut tee = Tee { sink: sink.clone(), primary: Box::new(ShortWriter { limit: 3 }) };

        let written = tee.write(b"abcdefgh").expect("write");

        assert_eq!(written, 3);
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text, "abc", "the file got more than stderr accepted");
    }

    /// A full disk must degrade to today's behavior, not fail the log call.
    #[test]
    fn an_erroring_sink_is_dropped_without_failing_the_write() {
        let sink = Arc::new(Mutex::new(Some(broken_file())));
        let mut tee = Tee { sink: sink.clone(), primary: Box::new(Vec::new()) };

        let written = tee.write(b"hello").expect("the write must still succeed");

        assert_eq!(written, 5);
        assert!(sink.lock().unwrap().is_none(), "the broken sink was kept");
    }

    #[test]
    fn a_dead_producers_stale_log_is_pruned() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let stale = dir.path().join("alacritree-1-0.log");
        std::fs::write(&stale, "x").unwrap();
        set_mtime_days_ago(&stale, 10);

        prune_session_logs(dir.path());

        assert!(!stale.exists(), "a stale dead-pid log survived");
    }

    /// A window can idle for a week without logging, leaving a stale mtime while
    /// the process is alive.  Deleting it would leave that process writing into
    /// an unlinked file no path reaches.
    #[test]
    fn a_live_producers_stale_log_is_spared() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let id = ProcessId { start: 1, pid: std::process::id(), ordinal: 0 };
        let mine = dir.path().join(logdir::session_log_name(&id));
        std::fs::write(&mine, "x").unwrap();
        set_mtime_days_ago(&mine, 10);

        prune_session_logs(dir.path());

        assert!(mine.exists(), "a live process's log was deleted");
    }

    struct ShortWriter {
        limit: usize,
    }

    impl Write for ShortWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len().min(self.limit))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn broken_file() -> File {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("closed.log");
        let file = File::create(&path).expect("create");
        // Dropping the directory removes the file; on every supported platform
        // the subsequent write to this handle fails.
        drop(dir);
        file
    }

    fn set_mtime_days_ago(path: &Path, days: u64) {
        let when = SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
        let file = OpenOptions::new().write(true).open(path).expect("open for mtime");
        file.set_modified(when).expect("set mtime");
    }
}
```

**Implementer note:** `broken_file` may not reliably produce a failing write on every platform. If `an_erroring_sink_is_dropped_without_failing_the_write` does not fail before the fix, replace `broken_file()` with a `File` opened read-only on an existing path, which fails on write everywhere.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree logging`
Expected: FAIL to compile — `cannot find type 'Tee'`.

- [ ] **Step 3: Implement the tee**

Insert above the test module in `alacritree/src/logging.rs`:

```rust
//! Duplicating the log stream to a file.
//!
//! `env_logger` writes to exactly one target, so mirroring to a file means
//! wrapping that target.  The sink is filled after `init()` because the
//! preference that enables it is not known until config has loaded, and
//! env_logger cannot be retargeted once built.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::logdir::{self, ProcessId};

/// How long a continuous log outlives the process that wrote it.
const RETAIN_DAYS: u64 = 7;

pub struct Tee {
    sink: Arc<Mutex<Option<File>>>,
    /// stderr in production; a buffer in tests.
    primary: Box<dyn Write + Send>,
}

/// A tee plus the handle that fills its sink later.  `Target::Pipe` takes
/// `Box<dyn Write + Send>` and moves it, so the caller can only reach the sink
/// afterwards through a share it kept.
pub fn tee() -> (Tee, Arc<Mutex<Option<File>>>) {
    let sink = Arc::new(Mutex::new(None));
    (Tee { sink: sink.clone(), primary: Box::new(std::io::stderr()) }, sink)
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.primary.write(buf)?;

        // Only the prefix stderr accepted: env_logger retries the suffix, and
        // mirroring the whole buffer would write it to the file twice.
        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = sink.as_mut()
            && file.write_all(&buf[..written]).is_err()
        {
            // Straight to stderr, never through `log::*`: env_logger holds its
            // own pipe mutex across this call, so logging here deadlocks.
            let _ = self.primary.write_all(b"alacritree: log file write failed; disabling it\n");
            *sink = None;
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = sink.as_mut() {
            let _ = file.flush();
        }
        self.primary.flush()
    }
}

/// This process's continuous log, sharing the artifact's identity so the two
/// files correlate.
pub fn open_session_log(dir: &Path) -> Option<File> {
    if std::fs::create_dir_all(dir).is_err() {
        return None;
    }
    let mut id = logdir::process_id();
    for _ in 0..32 {
        let path = dir.join(logdir::session_log_name(&id));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => {
                logdir::set_ordinal(id.ordinal);
                return Some(file);
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => id.ordinal += 1,
            Err(_) => return None,
        }
    }
    None
}

/// Liveness first, age second.  An idle window can leave a week-old mtime while
/// still running, and Windows honors a delete against an open handle — the
/// process would keep writing into a file no path reaches.
pub fn prune_session_logs(dir: &Path) {
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(RETAIN_DAYS * 86_400);
    let Ok(entries) = std::fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = logdir::parse_name("alacritree-", name) else { continue };
        if logdir::pid_is_live(id.pid) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if modified > cutoff {
            continue;
        }
        let _ = std::fs::remove_file(entry.path());
    }
}
```

- [ ] **Step 4: Declare the module**

In `alacritree/src/main.rs`, add `mod logging;` between `mod logdir;` and `mod mcp;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree logging`
Expected: PASS, 5 tests.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add alacritree/src/logging.rs alacritree/src/main.rs
git commit -m "feat(logging): tee the log stream to a per-process file

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 7: Initialization order in `main.rs`

**Files:**
- Modify: `alacritree/src/main.rs:86-123`

**Interfaces:**
- Consumes: `crash_log::{install, set_enabled, session_begin, record_exit, prune}`, `logging::{tee, open_session_log, prune_session_logs}`, `logdir::log_dir`, `config.debug`
- Produces: nothing.

- [ ] **Step 1: Rewrite `main`**

Replace `fn main()` in `alacritree/src/main.rs` with:

```rust
fn main() -> eframe::Result<()> {
    harden_dll_search_path();

    // egui_winit warns on every cold X11 clipboard probe even when it recovers.
    let default_filter = "info,egui_winit::clipboard=error";
    let (tee, log_sink) = logging::tee();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter))
        .target(env_logger::Target::Pipe(Box::new(tee)))
        .init();

    // A subcommand talks to an alacritree instead of being one.  Log output
    // goes to stderr (env_logger's default), leaving stdout to the reply.
    attach_parent_console();
    if let Some(code) = cli::run(cli::Cli::parse()) {
        std::process::exit(code);
    }

    // Only the GUI path records crashes.  Every subcommand exits before config
    // is read, so no gate could govern them, and `alacritree mcp` is a
    // long-lived loop that would write records nothing could disable.
    let log_dir = logdir::log_dir();
    if let Some(dir) = &log_dir {
        crash_log::install(dir, env!("CARGO_PKG_VERSION"));
    }

    let config = config::load();

    // The gate defaults on so a panic in `config::load` above is still
    // recorded; that is the one case where `crash_log = false` leaves a file.
    crash_log::set_enabled(config.debug.crash_log);
    crash_log::session_begin();
    crash_log::prune();

    if config.debug.persistent_logging
        && let Some(dir) = &log_dir
    {
        logging::prune_session_logs(dir);
        *log_sink.lock().unwrap_or_else(|e| e.into_inner()) = logging::open_session_log(dir);
    }

    wsl::set_automount_root(config.wsl_automount_root.clone());
    wsl_helper::set_enabled(config.wsl_resident_helper);
    let translucent = config.window.opacity < 1.0;

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([640.0, 400.0])
        .with_title("Alacritree")
        .with_transparent(translucent);
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options =
        eframe::NativeOptions { viewport, vsync: config.ui.vsync, ..Default::default() };

    let result = eframe::run_native(
        "Alacritree",
        native_options,
        Box::new(move |cc| Ok(Box::new(AlacritreeApp::new(cc, config)))),
    );

    // Only reached when `run_native` returns.  A panic unwinds past this — winit
    // resumes it outside the window procedure — so the hook is what records
    // that case.
    crash_log::record_exit(&result);
    result
}
```

- [ ] **Step 2: Build**

Run: `cargo check -p alacritree`
Expected: clean. If `config` is moved into the closure before `config.debug` is read, reorder so both reads happen first.

- [ ] **Step 3: Run the full suite**

Run: `cargo test -p alacritree -- --test-threads=1`
Expected: PASS.

- [ ] **Step 4: Verify by hand**

```bash
cargo run -p alacritree -- --help
```

Expected: help prints, and no log directory is created. Check `%LOCALAPPDATA%\alacritree` (Windows) or `~/.local/state/alacritree` (Unix) does not appear.

Then launch the GUI, close it, and confirm exactly one `crash-*.log` exists containing a `start` line and an `exit ok` line.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/main.rs
git commit -m "feat(logging): arm crash recording on the GUI path

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 8: The `crashes` subcommand

**Files:**
- Create: `alacritree/src/cli/crashes.rs`
- Modify: `alacritree/src/cli/mod.rs` (add `mod crashes;` at ~line 12, `Command::Crashes` to the enum, and a route in `run`)

**Interfaces:**
- Consumes: `logdir::{log_dir, artifact_name, parse_name, sort_key}`
- Produces: `pub fn run(as_json: bool) -> i32`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/cli/crashes.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn seed(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("seed");
    }

    #[test]
    fn artifacts_are_ordered_newest_first_then_by_ordinal() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-10-1.log", "old\n");
        seed(dir.path(), "crash-20-2.log", "new-a\n");
        seed(dir.path(), "crash-20-2-1.log", "new-b\n");

        let listed = collect(dir.path());

        let names: Vec<_> = listed.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["crash-20-2.log", "crash-20-2-1.log", "crash-10-1.log"]);
    }

    /// A truncated artifact may not be valid UTF-8, and the one thing a crash
    /// reader must not do is refuse to show a damaged record.
    #[test]
    fn invalid_utf8_is_still_rendered() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("crash-1-1.log"), [0x74, 0x78, 0xff, 0x0a]).unwrap();

        let listed = collect(dir.path());

        assert_eq!(listed.len(), 1);
        assert!(!listed[0].bytes.is_empty(), "a damaged artifact was dropped");
    }

    #[test]
    fn unrelated_files_are_ignored() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "state.toml", "x");
        seed(dir.path(), "alacritree-1-1.log", "continuous");

        assert!(collect(dir.path()).is_empty());
    }

    /// Nothing has crashed is a success, not an error.
    #[test]
    fn a_missing_directory_lists_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let absent = dir.path().join("gone");

        assert!(collect(&absent).is_empty());
    }

    #[test]
    fn the_json_shape_carries_the_parsed_identity() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-42-7.log", "body\n");

        let value = to_json(&collect(dir.path()));

        let first = &value.as_array().expect("an array")[0];
        assert_eq!(first["name"], "crash-42-7.log");
        assert_eq!(first["start"], 42);
        assert_eq!(first["pid"], 7);
        assert_eq!(first["contents"], "body\n");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree crashes`
Expected: FAIL to compile — `cannot find function 'collect'`.

- [ ] **Step 3: Implement the subcommand**

Insert above the test module in `alacritree/src/cli/crashes.rs`:

```rust
//! `alacritree crashes` — every crash artifact, newest first.
//!
//! Strictly read-only.  The artifacts are per-process files that nothing
//! merges on disk; this derives the single view instead, so it can run at any
//! time without coordinating with a live instance.

use std::path::Path;

use serde_json::{Value, json};

use crate::logdir::{self, ProcessId};

struct Artifact {
    name: String,
    id: ProcessId,
    bytes: Vec<u8>,
}

pub fn run(as_json: bool) -> i32 {
    let Some(dir) = logdir::log_dir() else {
        eprintln!("alacritree: no log directory on this platform");
        return 1;
    };
    let artifacts = collect(&dir);

    if as_json {
        println!("{:#}", to_json(&artifacts));
    } else {
        print_human(&artifacts);
    }
    0
}

fn collect(dir: &Path) -> Vec<Artifact> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };

    let mut artifacts: Vec<Artifact> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let id = logdir::parse_name("crash-", &name)?;
            // A file pruned by a concurrently starting instance is expected;
            // anything else read stops being reported rather than aborting the
            // whole listing.
            let bytes = std::fs::read(entry.path()).ok()?;
            Some(Artifact { name, id, bytes })
        })
        .collect();

    artifacts.sort_by(|a, b| {
        logdir::sort_key(&a.id).cmp(&logdir::sort_key(&b.id)).then_with(|| a.name.cmp(&b.name))
    });
    artifacts
}

fn print_human(artifacts: &[Artifact]) {
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for artifact in artifacts {
        let _ = writeln!(out, "==> {} <==", artifact.name);
        // Byte-copy: a truncated artifact may not be valid UTF-8, and refusing
        // to print a damaged crash record is the one unacceptable outcome.
        let _ = out.write_all(&artifact.bytes);
        let _ = writeln!(out);
    }
}

fn to_json(artifacts: &[Artifact]) -> Value {
    Value::Array(
        artifacts
            .iter()
            .map(|a| {
                json!({
                    "name": a.name,
                    "start": a.id.start as u64,
                    "pid": a.id.pid,
                    "contents": String::from_utf8_lossy(&a.bytes),
                })
            })
            .collect(),
    )
}
```

- [ ] **Step 4: Route the command**

In `alacritree/src/cli/mod.rs`, add `mod crashes;` beside the other `mod` lines at the top (alphabetically, before `mod doctor;`).

Add to `enum Command`, beside `Doctor`:

```rust
    /// Every recorded crash, newest first.
    Crashes,
```

Add to `run`, beside the `Doctor` arm:

```rust
        // Reads files rather than asking an instance, so it answers when
        // nothing is running — which is exactly when a crash is being chased.
        Command::Crashes => return Some(crashes::run(cli.json)),
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree crashes`
Expected: PASS, 5 tests.

- [ ] **Step 6: Verify by hand**

```bash
cargo run -p alacritree -- crashes
cargo run -p alacritree -- crashes --json
```

Expected: the artifacts from Task 7's manual check, newest first, with `==>` separators; the JSON form is a valid array.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/cli/crashes.rs alacritree/src/cli/mod.rs
git commit -m "feat(cli): add the crashes subcommand

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 9: Doctor summary

**Files:**
- Modify: `alacritree/src/cli/doctor.rs` (add `crash_checks()` and call it from `report`)

**Interfaces:**
- Consumes: `logdir::{log_dir, parse_name, pid_is_live}`, doctor's own `check(section, name, status, detail)` helper and `Status`
- Produces: `fn crash_checks() -> Vec<Check>`

- [ ] **Step 1: Write the failing tests**

Add to `doctor.rs`'s existing test module:

```rust
/// A crash last week must not make `doctor` exit nonzero in someone's script.
/// `Fail` is reserved for crash logging being broken right now.
#[test]
fn a_past_crash_warns_but_does_not_fail() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("crash-1-0.log"), "t1 start v\nt2 PANIC thread=main\n").unwrap();

    let checks = crash_checks_in(dir.path());

    assert!(checks.iter().any(|c| c.status == Status::Warn), "a recorded crash did not warn");
    assert_eq!(exit_code(&checks), 0, "a past crash made doctor exit nonzero");
}

#[test]
fn no_artifacts_is_ok() {
    let dir = tempfile::tempdir().expect("a temp dir");

    let checks = crash_checks_in(dir.path());

    assert!(checks.iter().all(|c| c.status == Status::Ok), "an empty directory was not ok");
}

/// A clean shutdown is the common case and must not accumulate warnings.
#[test]
fn a_clean_artifact_is_ok() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("crash-1-0.log"), "t1 start v pid=0\nt2 exit ok\n").unwrap();

    let checks = crash_checks_in(dir.path());

    assert!(checks.iter().all(|c| c.status == Status::Ok), "a clean artifact warned");
}

/// A record written after the exit marker means a detached worker outlived the
/// shutdown — a real defect, even though the process exited cleanly.
#[test]
fn a_record_after_the_exit_marker_warns() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let body = "t1 start v pid=0\nt2 exit ok\nt3 PANIC thread=pty-1\n";
    std::fs::write(dir.path().join("crash-1-0.log"), body).unwrap();

    let checks = crash_checks_in(dir.path());

    assert!(checks.iter().any(|c| c.status == Status::Warn), "a late worker panic was missed");
}

/// A truncated artifact is neither clean nor a live process; saying either
/// would be a lie about the only evidence there is.
#[test]
fn a_headerless_artifact_is_indeterminate() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("crash-1-0.log"), "PANIC without a header\n").unwrap();

    let checks = crash_checks_in(dir.path());

    let text: String = checks.iter().map(|c| c.detail.clone()).collect();
    assert!(text.contains("indeterminate"), "not reported as indeterminate: {text}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree doctor`
Expected: FAIL to compile — `cannot find function 'crash_checks_in'`.

- [ ] **Step 3: Implement the checks**

Add to `alacritree/src/cli/doctor.rs`:

```rust
/// Reading more of an artifact than this buys nothing: the markers that
/// classify it are lines, and one oversized malformed file must not be read in
/// full on every invocation.
const ARTIFACT_READ_CAP: u64 = 256 * 1024;

/// What an artifact says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Clean,
    Running,
    Crashed,
    Indeterminate,
}

fn crash_checks() -> Vec<Check> {
    match crate::logdir::log_dir() {
        Some(dir) => crash_checks_in(&dir),
        None => vec![check(
            "crashes",
            "log directory",
            Status::Fail,
            "no log directory on this platform".to_string(),
        )],
    }
}

fn crash_checks_in(dir: &Path) -> Vec<Check> {
    if !dir.exists() {
        return vec![check("crashes", "artifacts", Status::Ok, "none recorded".to_string())];
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        let detail = format!("cannot read {}", dir.display());
        return vec![check("crashes", "log directory", Status::Fail, detail)];
    };

    let mut crashed = 0usize;
    let mut indeterminate = 0usize;
    let mut running = 0usize;
    let mut clean = 0usize;
    let mut newest: Option<(u128, String)> = None;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = crate::logdir::parse_name("crash-", name) else { continue };

        let verdict = classify(&entry.path(), id.pid);
        match verdict {
            Verdict::Crashed => crashed += 1,
            Verdict::Indeterminate => indeterminate += 1,
            Verdict::Running => running += 1,
            Verdict::Clean => clean += 1,
        }
        if newest.as_ref().is_none_or(|(start, _)| id.start > *start) {
            newest = Some((id.start, name.to_string()));
        }
    }

    let total = crashed + indeterminate + running + clean;
    if total == 0 {
        return vec![check("crashes", "artifacts", Status::Ok, "none recorded".to_string())];
    }

    let status = if crashed > 0 || indeterminate > 0 { Status::Warn } else { Status::Ok };
    let newest = newest.map(|(_, n)| n).unwrap_or_default();
    let detail = format!(
        "{total} artifacts: {crashed} crashed, {indeterminate} indeterminate, {running} running, \
         {clean} clean; newest {newest}"
    );
    vec![check("crashes", "artifacts", status, detail)]
}

fn classify(path: &Path, pid: u32) -> Verdict {
    let Ok(meta) = std::fs::metadata(path) else { return Verdict::Indeterminate };
    if meta.len() > ARTIFACT_READ_CAP {
        return Verdict::Indeterminate;
    }
    let Ok(bytes) = std::fs::read(path) else { return Verdict::Indeterminate };
    let text = String::from_utf8_lossy(&bytes);

    let mut lines = text.lines();
    if !lines.next().is_some_and(|first| first.contains(" start ")) {
        return Verdict::Indeterminate;
    }

    let mut exited = false;
    let mut after_exit = false;
    let mut panicked = false;
    for entry in lines {
        if entry.contains("PANIC thread=") || entry.contains("panic records skipped:") {
            panicked = true;
            if exited {
                after_exit = true;
            }
        }
        if entry.contains("exit error:") {
            return Verdict::Crashed;
        }
        if entry.contains("exit ok") {
            exited = true;
        }
    }

    if panicked || after_exit {
        return Verdict::Crashed;
    }
    if exited {
        return Verdict::Clean;
    }
    if crate::logdir::pid_is_live(pid) { Verdict::Running } else { Verdict::Crashed }
}
```

Add the call in `report`, after the IPC checks:

```rust
    checks.extend(crash_checks());
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree doctor`
Expected: PASS, including every pre-existing doctor test.

- [ ] **Step 5: Verify by hand**

```bash
cargo run -p alacritree -- doctor
cargo run -p alacritree -- doctor --json
```

Expected: a `crashes` section appears; the exit code is 0 even with a recorded crash present.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add alacritree/src/cli/doctor.rs
git commit -m "feat(doctor): report recorded crashes

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 10: Ship the PDB with local installs

**Files:**
- Modify: `install.local.ps1:25`

**Interfaces:** none — this task touches no Rust.

- [ ] **Step 1: Add the PDB to the payload**

In `install.local.ps1`, change:

```powershell
$Payload = @('alacritree.exe', 'conpty.dll', 'OpenConsole.exe')
```

to:

```powershell
$Payload = @('alacritree.exe', 'alacritree.pdb', 'conpty.dll', 'OpenConsole.exe')
```

Extend the script's `.DESCRIPTION` block to say why, matching its existing tone:

```powershell
Installs alacritree.exe together with the vendored console host it loads by
name, plus its PDB so a captured backtrace symbolizes instead of printing bare
addresses.  A running alacritree pins its exe and conpty.dll, so an install that
cannot overwrite renames the pinned file aside and sweeps the leftovers on a
later run, once the process holding them has exited.
```

- [ ] **Step 2: Verify the script still installs**

```bash
pwsh -File install.local.ps1 -SkipBuild
```

Expected: `installed alacritree.pdb` appears among the other lines. If the release build for the `integration/all-features` worktree does not exist, run without `-SkipBuild`, or point `-Branch` at this feature branch's worktree.

Confirm the PDB landed:

```bash
ls ~/.local/bin/alacritree.pdb
```

- [ ] **Step 3: Commit**

```bash
git add install.local.ps1
git commit -m "build: install the pdb so backtraces symbolize

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 11: Subprocess tests

**Files:**
- Create: `alacritree/tests/cli_isolation.rs`
- Modify: `alacritree/src/cli/mod.rs` (add the debug-only `Provoke` command)

**Interfaces:**
- Consumes: the real binary via `env!("CARGO_BIN_EXE_alacritree")`
- Produces: nothing consumed by other tasks.

These cannot be unit tests: the defect they guard against is about which *process* installs the hook, and the crate is binary-only so integration tests cannot call private modules. They spawn the real executable instead.

- [ ] **Step 1: Add the debug-only stimulus**

The lock-timeout regression cannot be provoked from outside without a way in, and a stimulus that ships in release builds is not acceptable. In `alacritree/src/cli/mod.rs`, add to `enum Command`:

```rust
    /// Take the crash recorder lock and panic, to prove the hook does not
    /// deadlock against itself.  Debug builds only.
    #[cfg(debug_assertions)]
    #[command(hide = true)]
    ProvokeLockPanic,
```

And to `run`, beside the other locally handled arms:

```rust
        #[cfg(debug_assertions)]
        Command::ProvokeLockPanic => {
            crate::crash_log::provoke_lock_panic();
            return Some(0);
        },
```

In `alacritree/src/crash_log.rs`, add:

```rust
/// Panic while holding the recorder lock, so a test can prove the hook takes
/// the skip path instead of waiting on a mutex this thread already owns.
#[cfg(debug_assertions)]
pub fn provoke_lock_panic() {
    let dir = std::env::temp_dir().join("alacritree-provoke");
    install(&dir, "provoke");
    set_enabled(true);
    let _guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    panic!("provoked while holding the recorder lock");
}
```

- [ ] **Step 2: Write the failing tests**

Create `alacritree/tests/cli_isolation.rs`:

```rust
//! The CLI must not become a crash-logging process.
//!
//! Every check here is about which *process* does what, which no in-crate test
//! can observe: the crate is binary-only, so these drive the real executable.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_alacritree")
}

/// Point every log-directory environment variable at a scratch path so the
/// developer's real artifacts are never touched.
fn run_isolated(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .env("LOCALAPPDATA", home)
        .env("APPDATA", home)
        .env("XDG_STATE_HOME", home)
        .env("HOME", home)
        .output()
        .expect("the binary runs")
}

fn log_dir(home: &Path) -> PathBuf {
    home.join("alacritree")
}

/// An earlier design installed the hook before `cli::run`, so every `--help`
/// created a log directory and `alacritree mcp` wrote records no config could
/// disable.  This is the regression guard for that.
#[test]
fn help_creates_no_log_directory() {
    let home = tempfile::tempdir().expect("a temp dir");

    let out = run_isolated(home.path(), &["--help"]);

    assert!(out.status.success(), "--help failed");
    assert!(!log_dir(home.path()).exists(), "--help created a log directory");
}

#[test]
fn doctor_creates_no_log_directory() {
    let home = tempfile::tempdir().expect("a temp dir");

    run_isolated(home.path(), &["doctor"]);

    assert!(!log_dir(home.path()).exists(), "doctor created a log directory");
}

#[test]
fn crashes_reports_nothing_when_nothing_has_crashed() {
    let home = tempfile::tempdir().expect("a temp dir");

    let out = run_isolated(home.path(), &["crashes"]);

    assert!(out.status.success(), "crashes failed on an empty directory");
    assert!(out.stdout.is_empty(), "unexpected output: {:?}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn crashes_lists_seeded_artifacts_newest_first() {
    let home = tempfile::tempdir().expect("a temp dir");
    let dir = log_dir(home.path());
    std::fs::create_dir_all(&dir).expect("create");
    std::fs::write(dir.join("crash-10-1.log"), "older\n").unwrap();
    std::fs::write(dir.join("crash-20-2.log"), "newer\n").unwrap();

    let out = run_isolated(home.path(), &["crashes"]);

    let text = String::from_utf8_lossy(&out.stdout);
    let newer = text.find("newer").expect("the newer artifact is missing");
    let older = text.find("older").expect("the older artifact is missing");
    assert!(newer < older, "not newest first:\n{text}");
    assert!(text.contains("==> crash-20-2.log <=="), "no separator:\n{text}");
}

/// `--json` is global, so it must work in either position and must never emit
/// raw concatenation.
#[test]
fn crashes_emits_json_in_either_flag_position() {
    let home = tempfile::tempdir().expect("a temp dir");
    let dir = log_dir(home.path());
    std::fs::create_dir_all(&dir).expect("create");
    std::fs::write(dir.join("crash-42-7.log"), "body\n").unwrap();

    for args in [["crashes", "--json"], ["--json", "crashes"]] {
        let out = run_isolated(home.path(), &args);

        let text = String::from_utf8_lossy(&out.stdout);
        let value: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{args:?} is not JSON: {e}\n{text}"));
        assert_eq!(value[0]["pid"], 7, "{args:?} lost the pid");
    }
}

/// A blocking `lock()` in the hook waits on a mutex the panicking thread already
/// holds and never becomes poisoned.  A timeout here is the failure.
#[test]
fn a_panic_holding_the_recorder_lock_does_not_hang() {
    let home = tempfile::tempdir().expect("a temp dir");
    let mut child = Command::new(binary())
        .arg("provoke-lock-panic")
        .env("LOCALAPPDATA", home.path())
        .env("APPDATA", home.path())
        .env("XDG_STATE_HOME", home.path())
        .env("HOME", home.path())
        .spawn()
        .expect("spawn");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the process hung: the hook is waiting on a lock it already holds");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
```

- [ ] **Step 3: Add the test dependency**

`tempfile` is already a dev-dependency (`Cargo.toml:65`). `serde_json` is a regular dependency and is therefore *not* available to integration tests — add it under `[dev-dependencies]`:

```toml
[dev-dependencies]
# Throwaway repos and state files for worktree, prune, and persistence tests.
tempfile = "3"
# Integration tests parse the `--json` output of the real binary.
serde_json = "1"
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p alacritree --test cli_isolation`
Expected: FAIL — `crashes_lists_seeded_artifacts_newest_first` and the JSON test fail if Task 8 is incomplete; `a_panic_holding_the_recorder_lock_does_not_hang` fails to find the subcommand until Step 1 is done.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree --test cli_isolation`
Expected: PASS, 6 tests.

- [ ] **Step 6: Confirm the stimulus is debug-only**

Run: `cargo build -p alacritree --release && ./target/release/alacritree provoke-lock-panic`
Expected: the command is rejected as unknown. If it runs, the `#[cfg(debug_assertions)]` gate is missing.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/tests/cli_isolation.rs alacritree/src/cli/mod.rs alacritree/src/crash_log.rs alacritree/Cargo.toml
git commit -m "test(logging): guard the CLI against crash-logging side effects

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 12: Documentation and final verification

**Files:**
- Modify: `CLAUDE.md` (the module list in "Big-picture architecture")

- [ ] **Step 1: Document the new modules**

Add to the bulleted module list in `CLAUDE.md`, after the `state.rs` entry:

```markdown
- `logdir.rs` — where diagnostics live (`%LOCALAPPDATA%` / `$XDG_STATE_HOME`, deliberately not the roaming config dir) plus the per-process identity — UTC epoch nanos + pid + retry ordinal — that names both the crash artifact and the continuous log, and the per-platform "is this pid alive" check pruning depends on.
- `crash_log.rs` — the panic hook. Writes one artifact per GUI process; single writer, never shared, so no cross-process protocol. Armed only on the GUI path (after `cli::run` declines) because subcommands exit before config loads and no gate could govern them. Uses `try_lock`, never `lock`: a thread panicking while holding the recorder mutex would otherwise wait on itself forever. Retention is by age and liveness only — contents never decide deletion. Gated by `[debug] crash_log`, default on.
- `logging.rs` — `Tee`, which mirrors env_logger's stream into a per-process file whose sink is filled after config loads (env_logger cannot be retargeted post-`init`). Gated by `[debug] persistent_logging`, default off.
```

- [ ] **Step 2: Run the whole suite**

Run: `cargo test -p alacritree -- --test-threads=1`
Expected: PASS, everything.

- [ ] **Step 3: Check formatting and lints**

```bash
cargo fmt --check
cargo clippy -p alacritree --all-targets
```

Expected: no diff, no new warnings.

- [ ] **Step 4: End-to-end verification**

1. `cargo run -p alacritree` — open the GUI, close it normally.
2. `cargo run -p alacritree -- crashes` — one artifact with `start` and `exit ok`.
3. Set `crash_log = false` under `[debug]` in `alacritree.toml`, relaunch and close, and confirm no new artifact appears.
4. Remove that setting, add `[debug] persistent_logging = true` to `alacritty.toml`, relaunch, and confirm an `alacritree-*.log` appears with log lines in it.
5. `cargo run -p alacritree -- doctor` — the `crashes` section is present and the exit code is 0.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add CLAUDE.md
git commit -m "docs: describe the crash logging modules

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

## Spec Coverage

| Spec requirement | Task |
| --- | --- |
| `log_dir()` on `%LOCALAPPDATA%` / `$XDG_STATE_HOME` | 1 |
| Collision-proof identity, `create_new`, ordinal retry, ordering | 1, 3, 6 |
| Per-platform liveness, `Win32_System_Threading` feature | 1 |
| `[debug] crash_log` / `persistent_logging`, `Option<bool>` + `unwrap_or` | 2 |
| Key-by-key merge of `[debug]` across both files | 2 |
| Panic hook: payload, location, thread, forced backtrace | 3 |
| `ensure_artifact` idempotent from all three callers | 3 |
| Gate defaults on; disabled launch leaves no artifact | 3, 7 |
| Write failures swallowed once, never propagated | 3 |
| `try_lock` with Poisoned / WouldBlock arms | 4 |
| Skip counter and the best-effort exit marker | 4 |
| 20-record cap and consecutive-panic collapsing | 4 |
| Retention by age and liveness only, `NotFound` as success | 5 |
| `Tee` short-write mirroring, direct-stderr diagnostics, late sink | 6 |
| Continuous log per process, 7-day liveness-first pruning | 6 |
| Initialization order, GUI path only, `record_exit` | 7 |
| `alacritree crashes` — order, separators, byte-copy, missing dir, `--json` | 8 |
| Doctor summary, Ok/Warn/Fail mapping, bounded read, indeterminate | 9 |
| PDB in `install.local.ps1` only | 10 |
| Subprocess guards for CLI isolation and the lock timeout | 11 |

## Known Deviations From the Spec

Two spec tests are not implemented as written, for reasons the implementer should not spend time rediscovering:

- **Spec test 12 (two concurrent pruners).** Deterministically racing two pruners over one directory requires either process spawning or injected synchronization points. Task 5 covers the observable contract instead — `a_vanished_path_is_not_an_error` proves double-pruning is safe — and the spec's own invariant argument is why no claim protocol is needed.
- **Spec test 17 (a pid reused by a different executable).** There is no portable way to force pid reuse in a test. The behavior falls out of `pid_is_live` returning true, which `a_live_producers_artifact_is_never_pruned` already covers.

## Not In This Plan

Deferred per the spec: in-process minidumps (`SetUnhandledExceptionFilter` + `MiniDumpWriteDump`), which would catch access violations but provably **not** the `__fastfail` aborts of 2026-07-25. Also out: `debug.log_level`, crash capture for CLI/MCP processes, WER `LocalDumps`, and shipping the PDB in release archives.
