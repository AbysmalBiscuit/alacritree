# Integration crates pilot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the shared process/job/WSL/tool modules into `crates/alacritree_common`, rebuild doppler as a checkout lifecycle hook in its own crate, add user-defined command hooks, and make doppler work for WSL worktrees.

**Architecture:** Three new workspace crates under `crates/`: `alacritree_common` (foundation), `alacritree_checkout_hooks` (the `CheckoutHook` trait, its error type, the command hook, a test fake), and `alacritree_doppler` (the doppler backend and its config). The app owns a `Hook` enum that derives `ambassador::Delegate` over every backend, builds the hook list from resolved config, and runs it at the three places doppler is called today.

**Tech Stack:** Rust 2024 (MSRV 1.85), ambassador 0.5.1, strum 0.26, thiserror 2, schemars 1.2, serde.

**Spec:** `docs/superpowers/specs/2026-09-23-integration-crates-pilot-design.md`

## Global Constraints

- Crate directories are `crates/alacritree_<name>`, and the package name equals the directory name.
- Every new crate uses `edition.workspace = true`, `rust-version.workspace = true`, `publish = false`, and license `Apache-2.0`.
- `alacritree/`, `alacritty*` and `egui-winit/` stay where they are.
- New crates report errors with `thiserror` enums, never `String`.
- Integration dispatch uses `ambassador` (`#[delegatable_trait]` on the trait, `#[derive(Delegate)]` on the app's enum). No `Box<dyn>` for integrations.
- No raw `std::process::Command::{new, output, status, spawn}` outside `command_ext` and the `jobs` helpers. Build children with `command_ext::hidden` and wait with `Blocking::run_cancellable`.
- Raw config type names stay unique across the workspace (`RawDoppler`, `RawCheckoutHooks`, `RawCommandHook`).
- A config key's default lives in its `Raw*` type's `Default` impl under `#[serde(default)]`. The one exception is `RawCommandHook`, which has a required `path` and therefore per-field defaults.
- Comments explain why, not what, matching the existing file headers. Commit messages follow Conventional Commits and end with the session's attribution lines.
- Behavior stays the same except for: doppler running in the checkout's WSL distro, the new `[integrations.doppler] enabled` key, and the new command hooks.
- Formatting: run `cargo fmt` before every commit.
- Test command for the whole fork: `cargo test --workspace --exclude alacritty --exclude alacritty_terminal --exclude alacritty_config --exclude alacritty_config_derive`. The plan calls this **the workspace tests**.

## Review Focus

1. **Checkout paths with spaces, quotes or non-ASCII characters.** Each `{checkout}`/`{main}` placeholder must expand to exactly one argument, with no shell word splitting. Task 8 pins this.
2. **A hook program that hangs.** Cancelling the worktree create (dialog closed, IPC deadline) must kill the child instead of holding a pool worker forever. Task 3 pins this through `run_cancellable`.
3. **A command hook defined in `alacritty.toml` and switched off in `alacritree.toml`.** After the merge, the hook must not run. Task 8 pins this against the real config merge.
4. **Removing a worktree reached through a relative or symlinked path.** `on_removed` must receive the path canonicalized before git deleted the directory. Task 6 pins this.
5. **Doppler installed but printing something other than JSON** (an old CLI, a login banner). The hook must report nothing and must not panic or block the create. Task 5 pins this.
6. **A configured Windows program path and a WSL checkout.** The distro must look the program up by its bare name, never receive `C:\...\tool.exe`, which would exit 127 and skip the hook without a word. Tasks 3 and 8 pin this.

---

### Task 1: Create `alacritree_common` and move the foundation modules

**Files:**
- Create: `crates/alacritree_common/Cargo.toml`, `crates/alacritree_common/src/lib.rs`, `crates/clippy.toml`
- Move (git mv): `alacritree/src/{command_ext,jobs,wsl,wsl_helper,tools}.rs` → `crates/alacritree_common/src/`
- Modify: `Cargo.toml` (root), `alacritree/Cargo.toml`, `alacritree/src/lib.rs`, `alacritree/src/multiplexer/mod.rs` (receives one test), `alacritree/src/app/git_panel.rs:1441-1451`, `.github/workflows/ci.yml:30,33,46,81,100,103`, `alacritree/tools/ui-thread-audit.py:54,100,104,134,188`, `alacritree/clippy.toml` (comment only)

**Interfaces:**
- Consumes: nothing.
- Produces: crate `alacritree_common` with public modules `command_ext`, `jobs`, `wsl`, `wsl_helper`, `tools`. The feature `test-support` exposes `tools::test_configuration()`, `tools::test_configuration_lock()`, `jobs::Job::ready()` and `jobs::Job::panicked()`. The app keeps its `crate::jobs`, `crate::wsl` (and so on) paths through re-exports.

This task is a move. Its test is that the existing suite passes unchanged, apart from the one test that moves.

- [ ] **Step 1: Move the files**

```bash
mkdir -p crates/alacritree_common/src
for m in command_ext jobs wsl wsl_helper tools; do
  git mv alacritree/src/$m.rs crates/alacritree_common/src/$m.rs
done
```

- [ ] **Step 2: Write the crate manifest**

`crates/alacritree_common/Cargo.toml`:

```toml
[package]
name = "alacritree_common"
version = "0.0.0"
license = "Apache-2.0"
description = "Process, job pool, WSL and tool-path plumbing shared by alacritree's crates"
edition.workspace = true
rust-version.workspace = true
publish = false

[features]
# Exposes the tool-table and ready-made `Job` test helpers to other crates'
# tests; `cfg(test)` only covers this crate's own.
test-support = []

[dependencies]
base64 = "0.22"
log = "0.4"
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
tempfile = "3"

[target.'cfg(windows)'.dependencies]
winreg = "0.55"
windows-sys = { version = "0.59", features = ["Win32_Foundation", "Win32_System_Threading"] }
```

- [ ] **Step 3: Write `lib.rs`**

`crates/alacritree_common/src/lib.rs`:

```rust
//! What every alacritree crate that runs an external program needs: a child
//! built without a console window, a pool that keeps the wait off the UI
//! thread, the WSL side of a Windows host, and where each tool lives.

pub mod command_ext;
pub mod jobs;
pub mod tools;
pub mod wsl;
pub mod wsl_helper;
```

- [ ] **Step 4: Widen visibility inside the moved files**

The app reaches items that were `pub(crate)`. They must be `pub` now.

```bash
sed -i 's/\bpub(crate) /pub /g' crates/alacritree_common/src/{jobs,wsl_helper,tools}.rs
```

Then gate the test helpers that the app's tests use, so they survive the move. `cfg(test)` is only true while this crate's own tests compile, so without this the app's test build loses them. Replace each of these `#[cfg(test)]` attributes:

- the one directly before `pub fn test_configuration` in `tools.rs` (old line 78),
- the one directly before `pub fn test_configuration_lock` in `tools.rs` (old line 83),
- the one on `impl<T> Job<T>` in `jobs.rs` (old line 270), which holds `Job::ready` and `Job::panicked`. The app's tests call them about 20 times, in `app.rs`, `herdr/poll.rs`, `herdr/host.rs` and `in_flight.rs`.

with:

```rust
#[cfg(any(test, feature = "test-support"))]
```

Check that no other test-only item crosses the boundary: `grep -n 'cfg(test)\|cfg(any(windows, test))' crates/alacritree_common/src/*.rs`, then grep `alacritree/src` for each item those lines gate. The review found none beyond the three above (`wsl_helper.rs` `over` and `set_cached_comm`, and the `any(windows, test)` items in `wsl.rs`, are used only inside the new crate).

- [ ] **Step 5: Move the multiplexer test out of `wsl_helper.rs`**

Cut the whole `multiplexer_command_keeps_the_probe_pid` test (the `#[test] #[ignore = "requires WSL"]` block at the old `wsl_helper.rs:1600-1617`) out of `crates/alacritree_common/src/wsl_helper.rs`. It exercises `multiplexer::Side::command`, which is app code. Paste it into the `#[cfg(test)] mod tests` of `alacritree/src/multiplexer/mod.rs` with the paths rewritten:

```rust
    #[test]
    #[ignore = "requires WSL"]
    fn multiplexer_command_keeps_the_probe_pid() {
        use crate::wsl_helper::{new_probe_key, wrap_exec_argv};
        let distro =
            crate::wsl::distros().into_iter().find(|d| d.is_default).expect("a default distro");
        let key = new_probe_key();
        let (program, args) = Side::Wsl(distro.name).command("sh", &[
            "-c",
            r#"f=${XDG_RUNTIME_DIR:-/tmp}/alacritree/session-$1.pid; p=$(cat "$f") || exit 1; rm -f "$f"; printf '%s\n%s\n' "$$" "$p""#,
            "sh",
            &key,
        ]);
        let args = wrap_exec_argv(&program, &args, &key).expect("wrap multiplexer command");
        #[allow(clippy::disallowed_methods)] // A test waiting on its own child.
        let output = crate::command_ext::hidden(program).args(args).output().expect("run in WSL");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8(output.stdout).expect("PID output is UTF-8");
        let pids: Vec<_> = stdout.lines().collect();
        assert_eq!(pids.len(), 2, "command and probe PIDs: {stdout:?}");
        assert_eq!(pids[0], pids[1], "the probe must track the command, not its login shell");
    }
```

Run `grep -n 'crate::multiplexer\|crate::test_util\|crate::config' crates/alacritree_common/src/*.rs`. Expected: no output.

- [ ] **Step 6: Wire the workspace**

Root `Cargo.toml`: add `"crates/*"` to `members`, and add these workspace dependencies under `[workspace.dependencies]`:

```toml
alacritree_common = { path = "crates/alacritree_common" }
ambassador = "0.5.1"
thiserror = "2"
```

`alacritree/Cargo.toml` `[dependencies]`: add `alacritree_common.workspace = true`. `[dev-dependencies]`: add `alacritree_common = { workspace = true, features = ["test-support"] }`. Leave the app's own `base64`/`winreg`/`windows-sys` entries alone, because other app modules still use them.

`alacritree/src/lib.rs`: replace the five `mod` lines with re-exports at the same visibility:

```rust
pub use alacritree_common::command_ext;
pub(crate) use alacritree_common::jobs;
pub use alacritree_common::tools;
pub use alacritree_common::wsl;
pub use alacritree_common::wsl_helper;
```

- [ ] **Step 7: Build and fix what the compiler names**

Run: `cargo check --workspace --all-targets --exclude alacritty --exclude alacritty_terminal --exclude alacritty_config --exclude alacritty_config_derive`

Expected failures and their fixes:
- An item still private to the new crate that the app uses: make it `pub`.
- `alacritree/src/app/git_panel.rs:1441-1451` and any other test that calls `crate::tools::test_configuration*` or `Job::ready`/`Job::panicked`: they resolve through the re-export once `test-support` is on. If they do not, Step 4's gating was missed.
- `#![warn(unreachable_pub)]` does not apply to the new crate. Do not add it there.

Repeat until it compiles cleanly.

- [ ] **Step 8: Carry the lint rules and the audit to `crates/`**

`crates/clippy.toml`, with the same list as `alacritree/clippy.toml`:

```toml
# Clippy reads the nearest config walking up from the crate it lints, so the
# crates under `crates/` need their own copy of alacritree/clippy.toml's list.
# Keep the two lists identical.

disallowed-methods = [
  { path = "std::process::Command::output", reason = "waiting on a process can hold the UI thread for an unknown duration; submit a job with alacritree_common::jobs" },
  { path = "std::process::Command::status", reason = "waiting on a process can hold the UI thread for an unknown duration; submit a job with alacritree_common::jobs" },
  { path = "std::process::Command::spawn", reason = "spawning a process can hold the UI thread for an unknown duration; submit a job with alacritree_common::jobs" },
  { path = "std::process::Command::new", reason = "a console child opens a window when the parent has no console; build it with command_ext::hidden" },
]
```

In `alacritree/clippy.toml`, append to the header comment: `# crates/clippy.toml carries the same list for the crates under crates/; keep the two identical.`

`alacritree/tools/ui-thread-audit.py`: scan both trees. Replace `SRC = "alacritree/src"` with `SRCS = ["alacritree/src", "crates"]`. Pass `*SRCS` where `SRC` was passed to `ast-grep` (lines 100 and 104). Replace line 134 with:

```python
for path in (p for src in SRCS for p in (ROOT / src).rglob("*.rs")):
```

Update the `scan_failed` message at line 188 to use `", ".join(SRCS)`. Functions are keyed by file stem, and the moved files keep their stems (`jobs`, `wsl`, ...), so `jobs::pool().spawn` extents and `module::name` resolution keep working.

`.github/workflows/ci.yml`: replace `-p alacritree` with the line below in every job that builds, tests or lints the app. That is the Linux Build, Test and Clippy steps (lines 30, 33, 46), the macOS Clippy step (line 81), and the Windows Clippy and Test steps (lines 100, 103).

```
--workspace --exclude alacritty --exclude alacritty_terminal --exclude alacritty_config --exclude alacritty_config_derive
```

The Windows job exists to lint and test the `#[cfg(windows)]` arms of `wsl.rs` and `wsl_helper.rs` (see its comment at lines 93-97). After the move those arms live in `alacritree_common`, and `-p alacritree` would no longer reach them. Update that comment to name the files' new crate.

- [ ] **Step 9: Run the workspace tests, clippy and the audit**

Run: the workspace tests.
Expected: PASS, with the same test count as before plus zero (one test moved crates).

Run: `cargo clippy --workspace --exclude alacritty --exclude alacritty_terminal --exclude alacritty_config --exclude alacritty_config_derive --all-targets --no-deps -- -D clippy::disallowed_methods`
Expected: PASS.

Run: `python3 alacritree/tools/ui-thread-audit.py .`
Expected: the same findings as on `master` (run it there first and diff), and exit status 0 or 1 matching `master`, never 2.

- [ ] **Step 10: Commit**

```bash
cargo fmt
git add -A crates Cargo.toml Cargo.lock alacritree .github
git commit -m "refactor(common): move process, job, WSL and tool modules into alacritree_common"
```

---

### Task 2: `Tool` as a strum enum, and tool config in `common`

**Files:**
- Modify: `crates/alacritree_common/src/tools.rs` (including its `the_helper_hello_probes_the_registry_in_order` test at old line 276), `crates/alacritree_common/Cargo.toml`, `alacritree/src/config.rs:598-672,3388-3395,4758`, `alacritree/src/cli/doctor.rs:184,248,277,321,364-367,865,897,907`, `alacritree/src/app/git_panel.rs:1444-1451`

**Interfaces:**
- Consumes: Task 1's `alacritree_common::tools`.
- Produces:
  - `Tool` derives `strum::EnumCount`, `strum::VariantArray`, `strum::IntoStaticStr`, `strum::Display` with `serialize_all = "lowercase"`.
  - `Tool::name(self) -> &'static str` (kept, now implemented through `IntoStaticStr`).
  - `Tool::table<T>(f: impl FnMut(Tool) -> T) -> [T; Tool::COUNT]`.
  - `tools::configure(paths: [ToolPaths; Tool::COUNT])`.
  - `tools::ToolConfig { path: String, wsl_path: Option<String> }` (moved from `config.rs`).
  - `tools::tool_config(path: String, wsl_path: String, tool: Tool) -> ToolConfig` (moved from `config.rs`, now `pub`).
  - `Tool::ALL` is removed.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `crates/alacritree_common/src/tools.rs`:

```rust
    use strum::{EnumCount, VariantArray};

    #[test]
    fn tool_names_are_the_lowercase_program_names() {
        let names: Vec<&str> = Tool::VARIANTS.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["git", "gh", "delta", "doppler", "herdr", "tuicr", "task"]);
        assert_eq!(Tool::Doppler.to_string(), "doppler");
    }

    #[test]
    fn the_table_is_indexed_by_discriminant() {
        assert_eq!(Tool::COUNT, Tool::VARIANTS.len());
        let table = Tool::table(|t| t);
        for (i, tool) in table.iter().enumerate() {
            assert_eq!(*tool as usize, i);
        }
    }

    #[test]
    fn an_empty_path_falls_back_to_the_tool_name() {
        let config = tool_config("  ".into(), "".into(), Tool::Doppler);
        assert_eq!(config, ToolConfig { path: "doppler".into(), wsl_path: None });
        let config = tool_config("/opt/doppler".into(), "/usr/bin/doppler".into(), Tool::Doppler);
        assert_eq!(config.wsl_path.as_deref(), Some("/usr/bin/doppler"));
    }
```

Rewrite the existing `the_helper_hello_probes_the_registry_in_order` test in the same module, which still names `Tool::ALL`. It checks order as well as membership, so keep it rather than adding a weaker membership-only test:

```rust
    #[test]
    fn the_helper_hello_probes_the_registry_in_order() {
        let (registry, rest) = crate::wsl_helper::HELLO_TOOLS.split_at(Tool::COUNT);
        assert_eq!(registry, Tool::table(Tool::name));
        assert_eq!(rest, ["zellij"]);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree_common tools::`
Expected: FAIL to compile (`VARIANTS`, `COUNT`, `table`, `tool_config`, `ToolConfig` not found).

- [ ] **Step 3: Implement**

`crates/alacritree_common/Cargo.toml` `[dependencies]`: add `strum = { version = "0.26", features = ["derive"] }`.

In `tools.rs`, replace the `Tool` enum and its `impl` (the old `ALL` constant and `name()` match) with:

```rust
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    strum::EnumCount,
    strum::VariantArray,
    strum::IntoStaticStr,
    strum::Display,
)]
#[strum(serialize_all = "lowercase")]
pub enum Tool {
    Git,
    Gh,
    Delta,
    Doppler,
    Herdr,
    Tuicr,
    Task,
}

impl Tool {
    /// The program's name, which is also its default path.
    pub fn name(self) -> &'static str {
        self.into()
    }

    /// One value per tool, indexed by discriminant: the shape the paths
    /// table and `configure` take.
    pub fn table<T>(mut f: impl FnMut(Tool) -> T) -> [T; Tool::COUNT] {
        std::array::from_fn(|i| f(Tool::VARIANTS[i]))
    }
}
```

Add `use strum::{EnumCount, VariantArray};` to the file's imports. Replace every `[ToolPaths; 7]` in `tools.rs` with `[ToolPaths; Tool::COUNT]`, and replace `Tool::ALL.map(ToolPaths::named)` in `configured()` with `Tool::table(ToolPaths::named)`.

Move `ToolConfig` (with its doc comments) and `tool_config` from `alacritree/src/config.rs` (lines 659-670 and 3388-3395) into `tools.rs`, both `pub`. Keep the original doc comments; the block below only shows the shape:

```rust
/// `[integrations.<tool>]` for a tool with nothing to configure but where
/// it lives.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolConfig {
    /// The tool's own name, or a native path that runs as written.
    pub path: String,
    /// A path that runs as written inside every WSL distro, or `None` to
    /// find the tool by name there.
    pub wsl_path: Option<String>,
}

/// Resolve a raw `path`/`wsl_path` pair: blank means "the tool's own name"
/// natively and "find it" inside WSL.
pub fn tool_config(path: String, wsl_path: String, tool: Tool) -> ToolConfig {
    ToolConfig {
        path: if path.trim().is_empty() { tool.name().to_string() } else { path },
        wsl_path: Some(wsl_path).filter(|path| !path.trim().is_empty()),
    }
}
```

In `config.rs`, import them: `use crate::tools::{ToolConfig, tool_config};`. Replace `tool_paths`:

```rust
    /// Indexed by [`Tool`] discriminant, the shape `tools::configure` takes.
    pub fn tool_paths(&self) -> [ToolPaths; Tool::COUNT] {
        Tool::table(|tool| self.paths(tool))
    }
```

Fix the remaining `Tool::ALL` users:
- `config.rs:4758`: `for tool in Tool::ALL` → `for &tool in Tool::VARIANTS`.
- `cli/doctor.rs:184`: `tools::Tool::ALL` iterator chain → `tools::Tool::VARIANTS.iter().copied()`.
- `cli/doctor.rs:277`: `tools::Tool::ALL.map(tools::Tool::name)` → `tools::Tool::table(tools::Tool::name)`.
- `cli/doctor.rs:321`: `for tool in tools::Tool::ALL` → `for &tool in tools::Tool::VARIANTS`.
- `cli/doctor.rs:367`: `tools::Tool::ALL.iter().position(|t| *t == tool)?` → `Some(tool as usize)`. Update the doc comments at lines 248 and 364 to say "discriminant" instead of `Tool::ALL`.
- `cli/doctor.rs:865,897,907`: `[None; 7]` → `[None; tools::Tool::COUNT]`.
- `app/git_panel.rs:1444`: `[crate::tools::ToolPaths; 7]` → `[crate::tools::ToolPaths; crate::tools::Tool::COUNT]`. Line 1451: `crate::tools::Tool::ALL.map(crate::tools::ToolPaths::named)` → `crate::tools::Tool::table(crate::tools::ToolPaths::named)`.

Add `use strum::{EnumCount, VariantArray};` wherever `COUNT`/`VARIANTS` are now used.

- [ ] **Step 4: Run the tests to verify they pass**

Run: the workspace tests.
Expected: PASS, including the three new tests and the rewritten hello-list test. `cli::doctor` tests that snapshot `--json` output still pass, because `Tool`'s serde output is unchanged.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add -A crates alacritree
git commit -m "refactor(tools): derive Tool's names and count with strum"
```

---

### Task 3: Side-aware program runner in `common`

**Files:**
- Create: `crates/alacritree_common/src/side.rs`
- Modify: `crates/alacritree_common/src/lib.rs`

**Interfaces:**
- Consumes: `wsl::{classify, Location, command}`, `command_ext::hidden`, `jobs::Blocking::run_cancellable`.
- Produces (all in `alacritree_common::side`):
  - `enum Side { Native, Wsl { distro: String } }`, with `Side::of(path: &Path) -> Side` and `Side::from_location(&wsl::Location) -> Side`.
  - `fn spelling(location: &wsl::Location) -> String`: the path as a program on that side spells it.
  - `struct Program { pub native: String, pub wsl: Option<String>, pub name: String }`. `name` is the bare program name a distro's login shell looks up when `wsl` is `None`. It is never a Windows path.
  - `struct Invocation { pub program: String, pub args: Vec<String>, pub via_login_shell: bool }`: the program and argv to run on the side itself. For a distro, that is what follows `wsl.exe -d <distro> [--cd <dir>] --exec`.
  - `fn invocation(side: &Side, program: &Program, args: &[String]) -> Invocation`.
  - `enum Ran { Missing, Finished(std::process::Output) }`.
  - `fn run(side: &Side, program: &Program, cwd: Option<&Path>, args: &[String], blocking: &Blocking) -> std::io::Result<Ran>`.

WSL paths only parse as UNC prefixes on Windows, so the classification tests build `wsl::Location` values directly and run on every OS.

- [ ] **Step 1: Write the failing tests**

`crates/alacritree_common/src/side.rs` (tests first; the module body comes in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn wsl_location(distro: &str, linux: &str) -> wsl::Location {
        wsl::Location::Wsl { distro: distro.into(), linux_path: linux.into() }
    }

    #[test]
    fn a_wsl_location_runs_in_its_distro() {
        assert_eq!(
            Side::from_location(&wsl_location("Ubuntu", "/home/u/wt")),
            Side::Wsl { distro: "Ubuntu".into() }
        );
        assert_eq!(Side::from_location(&wsl::Location::Windows(PathBuf::from("C:/wt"))), Side::Native);
    }

    #[test]
    fn a_wsl_location_is_spelled_as_its_linux_path() {
        assert_eq!(spelling(&wsl_location("Ubuntu", "/home/u/my wt")), "/home/u/my wt");
        assert_eq!(spelling(&wsl::Location::Windows(PathBuf::from("/srv/wt"))), "/srv/wt");
    }

    fn program(native: &str, wsl: Option<&str>, name: &str) -> Program {
        Program { native: native.into(), wsl: wsl.map(Into::into), name: name.into() }
    }

    #[test]
    fn a_native_program_runs_as_configured() {
        let inv = invocation(&Side::Native, &program("mise", None, "mise"), &["trust".into()]);
        assert_eq!(inv.program, "mise");
        assert_eq!(inv.args, ["trust"]);
        assert!(!inv.via_login_shell);
    }

    #[test]
    fn a_configured_wsl_path_is_executed_directly() {
        let side = Side::Wsl { distro: "Ubuntu".into() };
        let inv = invocation(&side, &program("mise", Some("/usr/bin/mise"), "mise"), &["trust".into()]);
        assert_eq!(inv.program, "/usr/bin/mise");
        assert_eq!(inv.args, ["trust"]);
        assert!(!inv.via_login_shell);
    }

    #[test]
    fn an_unconfigured_wsl_program_goes_through_the_login_shell() {
        let side = Side::Wsl { distro: "Ubuntu".into() };
        let inv = invocation(&side, &program("mise", None, "mise"), &[
            "trust".into(),
            "/home/u/wt".into(),
        ]);
        assert_eq!(inv.program, "sh");
        assert_eq!(inv.args[0], "-c");
        assert!(inv.args[1].contains(r#"-lc 'exec "$@"'"#), "{}", inv.args[1]);
        assert_eq!(&inv.args[2..], ["sh", "mise", "trust", "/home/u/wt"]);
        assert!(inv.via_login_shell);
    }

    /// A configured Windows path means nothing inside a distro; handing it to
    /// the login shell would exit 127 and silently skip the hook.
    #[test]
    fn a_native_path_is_not_handed_to_the_distro() {
        let side = Side::Wsl { distro: "Ubuntu".into() };
        let inv = invocation(&side, &program(r"C:\Tools\doppler.exe", None, "doppler"), &[]);
        assert_eq!(&inv.args[2..], ["sh", "doppler"]);
        assert!(!inv.args.iter().any(|a| a.contains(r"C:\Tools")), "{:?}", inv.args);
    }

    #[test]
    fn a_missing_native_program_is_missing_not_an_error() {
        let program = program("alacritree-no-such-program", None, "alacritree-no-such-program");
        let ran = jobs::on_this_thread(|b| run(&Side::Native, &program, None, &[], b)).unwrap();
        assert!(matches!(ran, Ran::Missing));
    }

    #[cfg(unix)]
    #[test]
    fn a_native_program_finishes_with_its_output() {
        let program = program("sh", None, "sh");
        let args = ["-c".into(), "echo out; echo err >&2; exit 3".into()];
        let ran = jobs::on_this_thread(|b| run(&Side::Native, &program, None, &args, b)).unwrap();
        let Ran::Finished(output) = ran else { panic!("sh is installed") };
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    /// A hook that never exits must not keep a pool worker once the caller
    /// is gone: the worktree dialog closing cancels the job.
    #[cfg(unix)]
    #[test]
    fn cancelling_the_job_kills_a_hanging_program() {
        let (tx, rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let job = jobs::pool().spawn(jobs::Priority::Interactive, move |b| {
            let _ = started_tx.send(());
            let program = program("sleep", None, "sleep");
            let _ = tx.send(run(&Side::Native, &program, None, &["30".into()], b).is_ok());
        });
        started_rx.recv_timeout(Duration::from_secs(5)).expect("the job never started");
        let begun = Instant::now();
        drop(job);
        rx.recv_timeout(Duration::from_secs(10)).expect("run never returned after cancel");
        assert!(begun.elapsed() < Duration::from_secs(10));
    }
}
```

Add `pub mod side;` to `crates/alacritree_common/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree_common side::`
Expected: FAIL to compile (`Side`, `Program`, `invocation`, `run` not found).

- [ ] **Step 3: Implement**

Put this above the test module in `side.rs`:

```rust
//! Where a program runs for a checkout, and running it there.
//!
//! A checkout under `\\wsl.localhost\<distro>\…` belongs to that distro, so
//! a tool acting on it is the distro's Linux build: the Windows one reads the
//! Windows side's config and knows none of the distro's paths.  A program not
//! installed on the checkout's side is not an error, because someone with
//! projects on both sides rarely installs every tool on both.

use std::io;
use std::path::Path;
use std::process::{Output, Stdio};

use crate::jobs::Blocking;
use crate::{command_ext, wsl};

/// Which side of a Windows and WSL installation a checkout lives on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    Native,
    Wsl { distro: String },
}

impl Side {
    pub fn of(path: &Path) -> Self {
        Self::from_location(&wsl::classify(path))
    }

    pub fn from_location(location: &wsl::Location) -> Self {
        match location {
            wsl::Location::Windows(_) => Side::Native,
            wsl::Location::Wsl { distro, .. } => Side::Wsl { distro: distro.clone() },
        }
    }
}

/// A path as a program on its own side spells it: unchanged natively, the
/// distro's Linux path inside WSL.
pub fn spelling(location: &wsl::Location) -> String {
    match location {
        wsl::Location::Windows(path) => path.to_string_lossy().into_owned(),
        wsl::Location::Wsl { linux_path, .. } => linux_path.clone(),
    }
}

/// A program as configured for each side.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Program {
    /// The name or path to run natively.
    pub native: String,
    /// The path to run inside a distro as written.  `None` finds `name`
    /// through the distro user's login shell, which has their PATH.
    pub wsl: Option<String>,
    /// The bare name a distro looks up.  Separate from `native`, which may be
    /// a Windows path that means nothing inside the distro.
    pub name: String,
}

/// A command line on the program's own side, and whether it passes through
/// a login shell, whose exit status 127 means the program was not found.
/// Inside a distro this is what follows `wsl.exe -d <distro> --exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub via_login_shell: bool,
}

/// What running a program came to.
#[derive(Debug)]
pub enum Ran {
    /// The program is not installed on this side.
    Missing,
    Finished(Output),
}

/// The status a POSIX shell exits with for a command it could not find.
const NOT_FOUND: i32 = 127;

/// The distro user's own login shell, resolved the way custom diff viewers
/// resolve it, since `wsl.exe --exec` sees only the system PATH.
const LOGIN_SHELL: &str =
    r#"s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7); [ -x "$s" ] || s=${SHELL:-/bin/sh}"#;

pub fn invocation(side: &Side, program: &Program, args: &[String]) -> Invocation {
    let (program, mut argv, via_login_shell) = match (side, &program.wsl) {
        (Side::Native, _) => (program.native.clone(), Vec::new(), false),
        (Side::Wsl { .. }, Some(path)) => (path.clone(), Vec::new(), false),
        (Side::Wsl { .. }, None) => {
            let script = format!(r#"{LOGIN_SHELL}; exec "$s" -lc 'exec "$@"' "$s" "$@""#);
            ("sh".to_string(), vec!["-c".into(), script, "sh".into(), program.name.clone()], true)
        },
    };
    argv.extend(args.iter().cloned());
    Invocation { program, args: argv, via_login_shell }
}

/// Run `program` on `side`, killing it if the job is cancelled.  Blocks, so
/// it takes the pool's token: call it from a job, never the UI thread.
pub fn run(
    side: &Side,
    program: &Program,
    cwd: Option<&Path>,
    args: &[String],
    blocking: &Blocking,
) -> io::Result<Ran> {
    let inv = invocation(side, program, args);
    // `wsl::command` sets WSL_UTF8, without which wsl.exe's own errors (a
    // stopped or missing distro) arrive as UTF-16LE and read as garbage.
    let mut cmd = match side {
        Side::Native => {
            let mut cmd = command_ext::hidden(&inv.program);
            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }
            cmd
        },
        Side::Wsl { distro } => {
            let mut cmd = wsl::command(distro, cwd);
            cmd.arg(&inv.program);
            cmd
        },
    };
    cmd.args(&inv.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    match blocking.run_cancellable(&mut cmd) {
        Err(e) if e.kind() == io::ErrorKind::NotFound && *side == Side::Native => Ok(Ran::Missing),
        Err(e) => Err(e),
        Ok(output) if inv.via_login_shell && output.status.code() == Some(NOT_FOUND) => {
            Ok(Ran::Missing)
        },
        Ok(output) => Ok(Ran::Finished(output)),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree_common side::`
Expected: PASS. If `cancelling_the_job_kills_a_hanging_program` fails, read `Blocking::run_cancellable` in `jobs.rs:79` and confirm the `Job` drop marks the token cancelled while the child runs. Do not weaken the test.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/alacritree_common
git commit -m "feat(common): run a program on a checkout's own side of WSL"
```

---

### Task 4: `alacritree_checkout_hooks` with the trait, error type and fake

**Files:**
- Create: `crates/alacritree_checkout_hooks/Cargo.toml`, `crates/alacritree_checkout_hooks/src/lib.rs`, `crates/alacritree_checkout_hooks/src/fake.rs`, `alacritree/tests/checkout_hook_dispatch.rs`
- Modify: root `Cargo.toml` (`[workspace.dependencies]`), `alacritree/Cargo.toml`

**Interfaces:**
- Consumes: `alacritree_common::jobs::{Blocking, on_this_thread}`.
- Produces (in `alacritree_checkout_hooks`):
  - `struct Checkout<'a> { pub main: &'a Path, pub checkout: &'a Path }`
  - `type Outcome = Result<Option<String>, HookError>`
  - `enum HookError { Failed { hook: String, status: ExitStatus, stderr: String }, Spawn { hook: String, source: io::Error } }`. `hook` is the hook's name (`doppler`, or a command hook's table key), which is what the user configured and can look up.
  - `trait CheckoutHook` (`#[ambassador::delegatable_trait]`) with `on_created`, `on_opened`, `on_removed`, each `(&self, event: &Checkout, blocking: &Blocking) -> Outcome`, defaulting to `Ok(None)`.
  - `trait CheckoutHooks` with `created`, `opened`, `removed`, each `(&self, event: &Checkout, blocking: &Blocking) -> Vec<Outcome>`, implemented for `[H]` where `H: CheckoutHook`.
  - Under the `test-support` feature, `fake::{FakeHook, Event}`: `FakeHook::silent()`, `FakeHook::reporting(line: &str)`, `FakeHook::failing()`, `FakeHook::events(&self) -> Vec<Event>`. `Event::{Created, Opened, Removed} { main: PathBuf, checkout: PathBuf }`.

The trait's method signatures use absolute paths. ambassador copies them verbatim into a `macro_rules!` expanded in the app crate, where a bare `Checkout` or `Blocking` would not resolve.

- [ ] **Step 1: Write the manifest and the failing tests**

`crates/alacritree_checkout_hooks/Cargo.toml`:

```toml
[package]
name = "alacritree_checkout_hooks"
version = "0.0.0"
license = "Apache-2.0"
description = "Steps other tools need when alacritree creates, opens or removes a worktree"
edition.workspace = true
rust-version.workspace = true
publish = false

[features]
test-support = []

[dependencies]
alacritree_common.workspace = true
ambassador.workspace = true
thiserror.workspace = true
log = "0.4"
```

No self dev-dependency is needed for the fake: `fake` is compiled under `cfg(test)` as well as `test-support`.

Root `Cargo.toml` `[workspace.dependencies]`: add `alacritree_checkout_hooks = { path = "crates/alacritree_checkout_hooks" }`.

Tests at the bottom of `crates/alacritree_checkout_hooks/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Event, FakeHook};
    use alacritree_common::jobs;
    use std::path::PathBuf;

    fn event() -> (PathBuf, PathBuf) {
        (PathBuf::from("/repo"), PathBuf::from("/wt"))
    }

    #[test]
    fn every_hook_sees_every_event_in_order() {
        let (main, checkout) = event();
        let hooks = [FakeHook::reporting("first"), FakeHook::reporting("second")];
        let e = Checkout { main: &main, checkout: &checkout };
        let lines: Vec<_> = jobs::on_this_thread(|b| hooks[..].created(&e, b))
            .into_iter()
            .map(|o| o.expect("fake succeeds"))
            .collect();
        assert_eq!(lines, [Some("first".to_string()), Some("second".to_string())]);
        jobs::on_this_thread(|b| hooks[..].removed(&e, b));
        let expected = |make: fn(PathBuf, PathBuf) -> Event| make(main.clone(), checkout.clone());
        for hook in &hooks {
            assert_eq!(hook.events(), [
                expected(|main, checkout| Event::Created { main, checkout }),
                expected(|main, checkout| Event::Removed { main, checkout }),
            ]);
        }
    }

    /// Each hook carries an unrelated tool; a broken one must not keep the
    /// next from running.
    #[test]
    fn a_failing_hook_does_not_stop_the_next() {
        let (main, checkout) = event();
        let hooks = [FakeHook::failing(), FakeHook::reporting("after")];
        let e = Checkout { main: &main, checkout: &checkout };
        let outcomes = jobs::on_this_thread(|b| hooks[..].opened(&e, b));
        assert!(outcomes[0].is_err());
        assert_eq!(outcomes[1].as_ref().expect("second hook ran"), &Some("after".to_string()));
        assert_eq!(hooks[1].events().len(), 1);
    }

    #[test]
    fn an_empty_list_reports_nothing() {
        let (main, checkout) = event();
        let hooks: [FakeHook; 0] = [];
        let e = Checkout { main: &main, checkout: &checkout };
        assert!(jobs::on_this_thread(|b| hooks[..].created(&e, b)).is_empty());
    }

    #[test]
    fn a_spawn_error_names_the_hook() {
        let err = HookError::Spawn { hook: "mise".into(), source: std::io::Error::other("boom") };
        assert_eq!(err.to_string(), "could not run mise");
        assert!(std::error::Error::source(&err).is_some());
    }
}
```

Cross-crate dispatch proof, `alacritree/tests/checkout_hook_dispatch.rs`:

```rust
//! The app derives `Delegate` for a trait defined in another crate.  This is
//! what enum_dispatch cannot do, and the reason the integration crates use
//! ambassador; if it stops compiling, every integration enum breaks with it.

use std::path::Path;

use alacritree_checkout_hooks::fake::{Event, FakeHook};
use alacritree_checkout_hooks::{Checkout, CheckoutHook, CheckoutHooks, ambassador_impl_CheckoutHook};
use alacritree_common::jobs;
use ambassador::Delegate;

#[derive(Delegate)]
#[delegate(CheckoutHook)]
enum Hook {
    First(FakeHook),
    Second(FakeHook),
}

#[test]
fn a_delegated_enum_dispatches_to_each_variant() {
    let first = FakeHook::reporting("one");
    let second = FakeHook::silent();
    let hooks = [Hook::First(first.clone()), Hook::Second(second.clone())];
    let e = Checkout { main: Path::new("/repo"), checkout: Path::new("/wt") };
    let outcomes = jobs::on_this_thread(|b| hooks[..].created(&e, b));
    assert_eq!(outcomes[0].as_ref().unwrap(), &Some("one".to_string()));
    assert_eq!(outcomes[1].as_ref().unwrap(), &None);
    assert_eq!(first.events(), [Event::Created { main: "/repo".into(), checkout: "/wt".into() }]);
    assert_eq!(second.events().len(), 1);
}
```

`alacritree/Cargo.toml`: `[dependencies]` add `alacritree_checkout_hooks.workspace = true` and `ambassador.workspace = true`. `[dev-dependencies]` add `alacritree_checkout_hooks = { workspace = true, features = ["test-support"] }`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree_checkout_hooks && cargo test -p alacritree --test checkout_hook_dispatch`
Expected: FAIL to compile (the crate has no items yet).

- [ ] **Step 3: Implement the trait**

Top of `crates/alacritree_checkout_hooks/src/lib.rs`:

```rust
//! Steps other tools need when alacritree creates, first opens, or removes a
//! linked worktree.  Several tools bind settings to absolute directory paths
//! (doppler scopes, `mise trust`, `direnv allow`), so a fresh worktree starts
//! without them; each hook carries one such tool's step.

// The trait's signatures are copied verbatim into the app crate by
// ambassador's delegation macro, so they name types by absolute path, and
// this crate must answer to its own name for those paths to resolve here too.
extern crate self as alacritree_checkout_hooks;

use std::path::Path;
use std::process::ExitStatus;

use alacritree_common::jobs::Blocking;

#[cfg(any(test, feature = "test-support"))]
pub mod fake;

/// A worktree event: `checkout` is the linked worktree, `main` the project's
/// main checkout it belongs to.
#[derive(Debug, Clone, Copy)]
pub struct Checkout<'a> {
    pub main: &'a Path,
    pub checkout: &'a Path,
}

/// A line for the progress UI, or nothing when the hook had nothing to do.
pub type Outcome = Result<Option<String>, HookError>;

/// `hook` is the hook's name as configured, not the program it runs: a
/// command hook's table key is what the user can find in their config.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("{hook} failed ({status}): {stderr}")]
    Failed { hook: String, status: ExitStatus, stderr: String },
    #[error("could not run {hook}")]
    Spawn {
        hook: String,
        #[source]
        source: std::io::Error,
    },
}

#[ambassador::delegatable_trait]
pub trait CheckoutHook {
    /// alacritree just created `event.checkout` as a worktree of `event.main`.
    fn on_created(
        &self,
        _event: &::alacritree_checkout_hooks::Checkout<'_>,
        _blocking: &::alacritree_common::jobs::Blocking,
    ) -> ::alacritree_checkout_hooks::Outcome {
        Ok(None)
    }

    /// This process opened its first shell in a linked worktree.  Fires again
    /// after a restart, so implementations must be idempotent.
    fn on_opened(
        &self,
        _event: &::alacritree_checkout_hooks::Checkout<'_>,
        _blocking: &::alacritree_common::jobs::Blocking,
    ) -> ::alacritree_checkout_hooks::Outcome {
        Ok(None)
    }

    /// The worktree at `event.checkout` was removed.  The path was resolved
    /// before git deleted the directory, which cannot be canonicalized after.
    fn on_removed(
        &self,
        _event: &::alacritree_checkout_hooks::Checkout<'_>,
        _blocking: &::alacritree_common::jobs::Blocking,
    ) -> ::alacritree_checkout_hooks::Outcome {
        Ok(None)
    }
}

/// Every event run on each hook in order.  One hook failing does not stop
/// the next: each carries an unrelated tool, and a broken `mise` must not keep
/// doppler from scoping the worktree.
pub trait CheckoutHooks {
    fn created(&self, event: &Checkout<'_>, blocking: &Blocking) -> Vec<Outcome>;
    fn opened(&self, event: &Checkout<'_>, blocking: &Blocking) -> Vec<Outcome>;
    fn removed(&self, event: &Checkout<'_>, blocking: &Blocking) -> Vec<Outcome>;
}

impl<H: CheckoutHook> CheckoutHooks for [H] {
    fn created(&self, event: &Checkout<'_>, blocking: &Blocking) -> Vec<Outcome> {
        self.iter().map(|hook| hook.on_created(event, blocking)).collect()
    }

    fn opened(&self, event: &Checkout<'_>, blocking: &Blocking) -> Vec<Outcome> {
        self.iter().map(|hook| hook.on_opened(event, blocking)).collect()
    }

    fn removed(&self, event: &Checkout<'_>, blocking: &Blocking) -> Vec<Outcome> {
        self.iter().map(|hook| hook.on_removed(event, blocking)).collect()
    }
}
```

- [ ] **Step 4: Implement the fake**

`crates/alacritree_checkout_hooks/src/fake.rs`:

```rust
//! A hook that records what it receives, for tests on either side of the
//! trait.  Clones share one log, so a test keeps a clone to read after
//! handing the hook away.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritree_common::jobs::Blocking;

use crate::{Checkout, CheckoutHook, HookError, Outcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Created { main: PathBuf, checkout: PathBuf },
    Opened { main: PathBuf, checkout: PathBuf },
    Removed { main: PathBuf, checkout: PathBuf },
}

#[derive(Debug, Clone, Default)]
pub struct FakeHook {
    events: Arc<Mutex<Vec<Event>>>,
    line: Option<String>,
    fails: bool,
}

impl FakeHook {
    pub fn silent() -> Self {
        Self::default()
    }

    pub fn reporting(line: &str) -> Self {
        Self { line: Some(line.to_string()), ..Self::default() }
    }

    pub fn failing() -> Self {
        Self { fails: true, ..Self::default() }
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record(&self, event: Event) -> Outcome {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).push(event);
        if self.fails {
            return Err(HookError::Spawn {
                hook: "fake".into(),
                source: std::io::Error::other("scripted failure"),
            });
        }
        Ok(self.line.clone())
    }
}

impl CheckoutHook for FakeHook {
    fn on_created(&self, e: &Checkout<'_>, _: &Blocking) -> Outcome {
        self.record(Event::Created { main: e.main.into(), checkout: e.checkout.into() })
    }

    fn on_opened(&self, e: &Checkout<'_>, _: &Blocking) -> Outcome {
        self.record(Event::Opened { main: e.main.into(), checkout: e.checkout.into() })
    }

    fn on_removed(&self, e: &Checkout<'_>, _: &Blocking) -> Outcome {
        self.record(Event::Removed { main: e.main.into(), checkout: e.checkout.into() })
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree_checkout_hooks && cargo test -p alacritree --test checkout_hook_dispatch`
Expected: PASS. If the dispatch test fails with an unresolved type inside the generated impl, the absolute paths in the trait are wrong. Fix the trait's paths. Do not add imports to the test to paper over it, because every future derive site would need the same imports.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A crates Cargo.toml Cargo.lock alacritree
git commit -m "feat(checkout-hooks): add the CheckoutHook trait with cross-crate dispatch"
```

---

### Task 5: `alacritree_doppler` as a native checkout hook

**Files:**
- Create: `crates/alacritree_doppler/Cargo.toml`, `crates/alacritree_doppler/src/lib.rs`, `crates/alacritree_doppler/src/settings.rs`
- Move (git mv): `alacritree/src/doppler.rs` → `crates/alacritree_doppler/src/scopes.rs`
- Modify: root `Cargo.toml`, `alacritree/Cargo.toml`, `alacritree/src/lib.rs` (drop `mod doppler`), `alacritree/src/config.rs:603,645,3142,3238,3357`, `alacritree/src/app.rs:53` (drop `doppler` from the `use crate::{…}` list), `alacritree/src/cli/doctor.rs:207-209,819-821` (comments naming `doppler.rs`), `alacritree/tests/stock-config.json` (regenerated), `schema/alacritree-config.json`, `docs/config-reference.md`

The config module is `settings.rs`, not `config.rs`. The UI-thread audit keys functions by file stem, and a second `config` stem would collide with `alacritree/src/config.rs` when it resolves `config::…` calls.

**Interfaces:**
- Consumes: `alacritree_common::{tools::{self, Tool, ToolConfig, tool_config}, jobs::Blocking, command_ext}`; `alacritree_checkout_hooks::{CheckoutHook, Checkout, Outcome}`.
- Produces (in `alacritree_doppler`):
  - `#[derive(Debug, Clone, Copy, Default)] pub struct DopplerHook;` implementing `CheckoutHook`.
  - `pub struct RawDoppler` (`Deserialize`, `JsonSchema`, `#[serde(default)]`) with `path: String`, `wsl_path: String`, `enabled: bool`.
  - `pub struct DopplerConfig { pub path: String, pub wsl_path: Option<String>, pub enabled: bool }` (`Debug, Clone, PartialEq, serde::Serialize`).
  - `RawDoppler::resolve(self) -> DopplerConfig`.

This task keeps doppler native-only. Task 7 adds the WSL side.

- [ ] **Step 1: Move the file and write the manifest**

```bash
mkdir -p crates/alacritree_doppler/src
git mv alacritree/src/doppler.rs crates/alacritree_doppler/src/scopes.rs
```

`crates/alacritree_doppler/Cargo.toml`:

```toml
[package]
name = "alacritree_doppler"
version = "0.0.0"
license = "Apache-2.0"
description = "Mirrors Doppler CLI scopes into alacritree's worktrees"
edition.workspace = true
rust-version.workspace = true
publish = false

[dependencies]
alacritree_common.workspace = true
alacritree_checkout_hooks.workspace = true
log = "0.4"
schemars = "1.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
alacritree_common = { workspace = true, features = ["test-support"] }
tempfile = "3"
```

Root `Cargo.toml` `[workspace.dependencies]`: add `alacritree_doppler = { path = "crates/alacritree_doppler" }`.

In `scopes.rs`, change `use crate::tools::{self, Tool}; use crate::{command_ext, jobs};` to `use alacritree_common::tools::{self, Tool}; use alacritree_common::{command_ext, jobs};` and make `mirror_scopes`/`forget_scopes` `pub(crate)` (they already are; they stay crate-private now).

- [ ] **Step 2: Write the failing tests**

`crates/alacritree_doppler/src/lib.rs` test module:

```rust
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use alacritree_checkout_hooks::{Checkout, CheckoutHook};
    use alacritree_common::jobs;
    use alacritree_common::tools::{self, Tool, ToolPaths};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// A `doppler` that answers `configure --all --json` from a file and logs
    /// every other invocation, set as the configured doppler path.
    struct FakeDoppler {
        _dir: tempfile::TempDir,
        log: PathBuf,
        _config: std::sync::MutexGuard<'static, ()>,
        restore: [ToolPaths; <Tool as strum::EnumCount>::COUNT],
    }

    impl FakeDoppler {
        fn answering(scopes_json: &str) -> Self {
            let config = tools::test_configuration_lock().lock().unwrap_or_else(|e| e.into_inner());
            let restore = tools::test_configuration();
            let dir = tempfile::tempdir().unwrap();
            let state = dir.path().join("scopes.json");
            let log = dir.path().join("calls.log");
            std::fs::write(&state, scopes_json).unwrap();
            let script = dir.path().join("doppler");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nif [ \"$1 $2\" = \"configure --all\" ]; then cat '{}'; else echo \"$@\" >> '{}'; fi\n",
                    state.display(),
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut paths = restore.clone();
            paths[Tool::Doppler as usize] =
                ToolPaths { native: script.to_string_lossy().into_owned(), wsl: None };
            tools::configure(paths);
            Self { _dir: dir, log, _config: config, restore }
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    impl Drop for FakeDoppler {
        fn drop(&mut self) {
            tools::configure(self.restore.clone());
        }
    }

    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(main.join("apps/web")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        let main = main.canonicalize().unwrap();
        let wt = wt.canonicalize().unwrap();
        (tmp, main, wt)
    }

    fn created(main: &Path, wt: &Path) -> alacritree_checkout_hooks::Outcome {
        let e = Checkout { main, checkout: wt };
        jobs::on_this_thread(|b| DopplerHook.on_created(&e, b))
    }

    #[test]
    fn creating_a_worktree_mirrors_each_main_checkout_scope() {
        let (_tmp, main, wt) = dirs();
        let scopes = format!(
            r#"{{"{m}": {{"enclave.project": "api", "enclave.config": "dev"}},
                "{m}/apps/web": {{"enclave.project": "web"}},
                "/elsewhere": {{"enclave.project": "other"}}}}"#,
            m = main.display()
        );
        let doppler = FakeDoppler::answering(&scopes);
        let outcome = created(&main, &wt).expect("doppler hook never errors");
        assert_eq!(outcome.as_deref(), Some("Linked 2 Doppler scope(s)"));
        let mut calls = doppler.calls();
        calls.sort();
        assert_eq!(calls, [
            format!("configure set project=api config=dev --no-check-version --scope {}", wt.display()),
            format!("configure set project=web --no-check-version --scope {}/apps/web", wt.display()),
        ]);
    }

    #[test]
    fn nothing_to_mirror_reports_nothing() {
        let (_tmp, main, wt) = dirs();
        let _doppler = FakeDoppler::answering("{}");
        assert_eq!(created(&main, &wt).unwrap(), None);
    }

    /// An old CLI or a login banner can put anything on stdout; the hook must
    /// stay silent rather than fail the create.
    #[test]
    fn output_that_is_not_json_reports_nothing() {
        let (_tmp, main, wt) = dirs();
        let doppler = FakeDoppler::answering("Welcome to Doppler!\nnot json");
        assert_eq!(created(&main, &wt).unwrap(), None);
        assert!(doppler.calls().is_empty());
    }

    #[test]
    fn removing_a_worktree_forgets_its_scopes() {
        let (_tmp, main, wt) = dirs();
        let scopes = format!(r#"{{"{}": {{"enclave.project": "api"}}}}"#, wt.display());
        let doppler = FakeDoppler::answering(&scopes);
        let e = Checkout { main: &main, checkout: &wt };
        let outcome = jobs::on_this_thread(|b| DopplerHook.on_removed(&e, b)).unwrap();
        assert_eq!(outcome.as_deref(), Some("Dropped 1 Doppler scope(s)"));
        assert_eq!(doppler.calls(), [format!(
            "configure unset project config --no-check-version --scope {}",
            wt.display()
        )]);
    }

    #[test]
    fn a_missing_doppler_reports_nothing() {
        let (_tmp, main, wt) = dirs();
        let _lock = tools::test_configuration_lock().lock().unwrap_or_else(|e| e.into_inner());
        let restore = tools::test_configuration();
        let mut paths = restore.clone();
        paths[Tool::Doppler as usize] =
            ToolPaths { native: "/nonexistent/doppler".into(), wsl: None };
        tools::configure(paths);
        let outcome = created(&main, &wt);
        tools::configure(restore);
        assert_eq!(outcome.unwrap(), None);
    }
}
```

`crates/alacritree_doppler/src/settings.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doppler_is_enabled_and_found_by_name_by_default() {
        assert_eq!(RawDoppler::default().resolve(), DopplerConfig {
            path: "doppler".into(),
            wsl_path: None,
            enabled: true,
        });
    }

    #[test]
    fn a_table_can_turn_doppler_off() {
        let raw: RawDoppler = toml::from_str("enabled = false").unwrap();
        assert!(!raw.resolve().enabled);
    }
}
```

Add `toml = { workspace = true }` and `strum = "0.26"` to the doppler crate's `[dev-dependencies]`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p alacritree_doppler`
Expected: FAIL to compile (`DopplerHook`, `RawDoppler`, `DopplerConfig` not found).

- [ ] **Step 4: Implement**

`crates/alacritree_doppler/src/settings.rs`:

```rust
use alacritree_common::tools::{Tool, tool_config};
use serde::Deserialize;

/// `[integrations.doppler]`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct RawDoppler {
    /// The program to run on Windows or natively. Its own name is looked up
    /// on PATH; any other value runs as written.
    path: String,
    /// The program to run inside every WSL distro, as written. Empty finds it
    /// by name through the distro's login shell.
    wsl_path: String,
    /// Copy the main checkout's Doppler scopes into each new worktree, and
    /// drop them again when the worktree is removed.
    enabled: bool,
}

impl Default for RawDoppler {
    fn default() -> Self {
        Self { path: Tool::Doppler.name().to_string(), wsl_path: String::new(), enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DopplerConfig {
    pub path: String,
    pub wsl_path: Option<String>,
    pub enabled: bool,
}

impl RawDoppler {
    pub fn resolve(self) -> DopplerConfig {
        let tool = tool_config(self.path, self.wsl_path, Tool::Doppler);
        DopplerConfig { path: tool.path, wsl_path: tool.wsl_path, enabled: self.enabled }
    }
}
```

`crates/alacritree_doppler/src/lib.rs` (above its test module):

```rust
//! Doppler as a checkout hook: a new worktree gets the main checkout's
//! scopes, a removed one gives them back.  See `scopes` for why doppler needs
//! this at all.

mod scopes;
mod settings;

use alacritree_checkout_hooks::{Checkout, CheckoutHook, Outcome};
use alacritree_common::jobs::Blocking;

pub use settings::{DopplerConfig, RawDoppler};

/// Best-effort throughout: no doppler binary, or nothing to copy, reports
/// nothing rather than an error, as the create flow always has.
#[derive(Debug, Clone, Copy, Default)]
pub struct DopplerHook;

impl DopplerHook {
    fn mirror(event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        let linked = scopes::mirror_scopes(event.main, event.checkout, blocking);
        Ok((linked > 0).then(|| format!("Linked {linked} Doppler scope(s)")))
    }
}

impl CheckoutHook for DopplerHook {
    fn on_created(&self, event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        Self::mirror(event, blocking)
    }

    /// Covers worktrees created outside alacritree, which otherwise hit
    /// "Doppler Error: You must specify a project".
    fn on_opened(&self, event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        Self::mirror(event, blocking)
    }

    fn on_removed(&self, event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        let dropped = scopes::forget_scopes(event.checkout, blocking);
        Ok((dropped > 0).then(|| format!("Dropped {dropped} Doppler scope(s)")))
    }
}
```

Keep the `scopes.rs` module comment (it is the original `doppler.rs` header and explains why mirroring exists).

In `alacritree/src/config.rs`:
- Delete `raw_tool_table!(RawDoppler, "doppler");` (line 3238).
- `RawIntegrations.doppler` (line 3142) becomes `doppler: alacritree_doppler::RawDoppler,`. Keep its doc comment.
- `IntegrationsConfig.doppler` (line 603) becomes `pub doppler: alacritree_doppler::DopplerConfig,`.
- `RawIntegrations::resolve` (line 3357): `doppler: self.doppler.resolve(),`.
- `IntegrationsConfig::paths` keeps `Tool::Doppler => (&self.doppler.path, &self.doppler.wsl_path)`, which still type-checks.

`alacritree/Cargo.toml` `[dependencies]`: add `alacritree_doppler.workspace = true`. `alacritree/src/lib.rs`: delete `pub(crate) mod doppler;`. `alacritree/src/app.rs:53`: drop `doppler` from the `use crate::{…}` list.

`alacritree/src/cli/doctor.rs`: the comments on `doppler_need` (line 209) and on `doppler_is_only_worth_warning_about_once_it_has_been_set_up` (line 821) say "`doppler.rs` reads scopes". Change them to "`alacritree_doppler` reads scopes".

The three `doppler::` call sites (`worktree.rs:173,587`, `app.rs:1155`) no longer compile. Task 6 replaces them. For this task only, make them compile through the hook so the tree stays green:
- `worktree.rs:173-176` → `if let Ok(Some(line)) = alacritree_doppler::DopplerHook.on_created(&alacritree_checkout_hooks::Checkout { main: &req.project_root, checkout: &target }, blocking) { send(&line); }`
- `worktree.rs:587-590` → `if let Ok(Some(line)) = alacritree_doppler::DopplerHook.on_removed(&alacritree_checkout_hooks::Checkout { main: project_root, checkout: &scope_root }, blocking) { log::info!("{line} under {}", scope_root.display()); }`
- `app.rs:1155-1158` → `if let Ok(Some(line)) = alacritree_doppler::DopplerHook.on_opened(&alacritree_checkout_hooks::Checkout { main: &main_checkout, checkout: &worktree }, blocking) { log::info!("{line} into {}", worktree.display()); }`

Import `alacritree_checkout_hooks::CheckoutHook` in those two files.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree_doppler`, then the workspace tests.
Expected: two fixture tests fail, because both fixtures change on purpose.

- `config_schema`: `enabled` is a new key. Regenerate with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema` and read the diff. It must show only two changes. The `RawDoppler` definition gains `enabled` and a `description` from the new struct doc comment (`[integrations.doppler].`; the old `raw_tool_table!` struct had none). `docs/config-reference.md` gains the matching `enabled` line and the table's description line.
- `config::tests::the_stock_config_is_unchanged` (`config.rs:6199`): the resolved stock config now carries `integrations.doppler.enabled`. Regenerate with `ALACRITREE_UPDATE_STOCK=1 cargo test -p alacritree --lib the_stock_config_is_unchanged` and read the diff of `alacritree/tests/stock-config.json`. It must add only `"enabled": true` under `integrations.doppler`.

Then run the workspace tests again. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A crates Cargo.toml Cargo.lock alacritree schema docs
git commit -m "refactor(doppler): move doppler into its own crate as a checkout hook"
```

---

### Task 6: The app's hook list at every call site

**Files:**
- Create: `alacritree/src/checkout_hooks.rs`
- Modify: `alacritree/src/lib.rs`, `alacritree/src/worktree.rs:24-32,85-103,111-116,173-176,560-592,601-622,700-715,848,910`, `alacritree/src/app.rs:383,507,1040,1135-1159`, `alacritree/src/app/modals.rs:455,480,988`, `alacritree/src/app/ipc_handler.rs:15`, `alacritree/src/ipc/server.rs:78-83,132-215,235-245,316,381,417`, `alacritree/src/cli/offline.rs:23-30,67,144-156`, `alacritree/src/cli/mod.rs:542-547`

**Interfaces:**
- Consumes: `CheckoutHook`, `CheckoutHooks` (Task 4), `DopplerHook`, `DopplerConfig` (Task 5), `FakeHook`/`Event` (Task 4, test-support).
- Produces:
  - `crate::checkout_hooks::Hook`: `#[derive(Debug, Clone, Delegate)] #[delegate(CheckoutHook)] enum Hook { Doppler(DopplerHook) }`. Task 8 adds `Command(CommandHook)`.
  - `crate::checkout_hooks::from_config(integrations: &IntegrationsConfig) -> Vec<Hook>`.
  - `crate::checkout_hooks::report(outcomes: Vec<Outcome>, mut line: impl FnMut(&str))`: sends each `Ok(Some)` line as it is, and each error as `"Hook failed: {e}"`.
  - `worktree::create<H: CheckoutHooks + ?Sized>(req: &CreateRequest, hooks: &H, on_step: impl FnMut(&str), blocking: &Blocking) -> Result<PathBuf, String>`
  - `worktree::spawn_create<H: CheckoutHook + Send + 'static>(req: CreateRequest, hooks: Vec<H>, repaint: impl Repaint) -> (Receiver<Progress>, jobs::Job<()>)`
  - `worktree::delete_worktree<H: CheckoutHooks + ?Sized>(project_root, worktree_path, branch, force, hooks: &H, blocking) -> Result<(), String>`
  - `worktree::spawn_delete<H: CheckoutHook + Send + 'static>(project_root: PathBuf, job: DeleteJob, hooks: Vec<H>, repaint: impl Repaint) -> jobs::Job<Result<(), String>>`
  - `ipc::server::spawn_listener(repaint, workspace: WorkspaceConfig, hooks: Vec<Hook>)`
  - `cli::offline::handle(request, workspace, hooks: &[Hook])`

- [ ] **Step 1: Write the failing tests**

In `alacritree/src/worktree.rs` tests:

```rust
    use alacritree_checkout_hooks::fake::{Event, FakeHook};

    #[test]
    fn create_hands_the_new_checkout_to_every_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let project = crate::test_util::clone_with_origin(tmp.path());
        let req = CreateRequest {
            project_root: project.clone(),
            default_branch: None,
            branch: "hooked".into(),
            base_dir: Some(tmp.path().join("worktrees")),
        };
        let hook = FakeHook::reporting("Linked 1 fake scope");
        let mut steps = Vec::new();
        let target = jobs::on_this_thread(|b| {
            create(&req, &[hook.clone()][..], |s| steps.push(s.to_string()), b)
        })
        .expect("create succeeds");
        assert_eq!(hook.events(), [Event::Created { main: project, checkout: target }]);
        assert!(steps.iter().any(|s| s == "Linked 1 fake scope"), "{steps:?}");
    }

    #[test]
    fn a_failing_hook_shows_in_the_steps_and_does_not_fail_the_create() {
        let tmp = tempfile::tempdir().unwrap();
        let project = crate::test_util::clone_with_origin(tmp.path());
        let req = CreateRequest {
            project_root: project,
            default_branch: None,
            branch: "hook-fails".into(),
            base_dir: Some(tmp.path().join("worktrees")),
        };
        let mut steps = Vec::new();
        let result = jobs::on_this_thread(|b| {
            create(&req, &[FakeHook::failing()][..], |s| steps.push(s.to_string()), b)
        });
        assert!(result.is_ok(), "{result:?}");
        assert!(steps.iter().any(|s| s.starts_with("Hook failed: could not run fake")), "{steps:?}");
    }

    /// Doppler keys scopes by canonical path, and a removed directory can no
    /// longer be canonicalized, so the hook must get the path resolved first.
    #[cfg(unix)]
    #[test]
    fn removal_hands_hooks_the_path_resolved_before_git_deleted_it() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = crate::test_util::init_repo(&repo_dir);
        let wt_path = crate::test_util::add_worktree(&repo, "linked");
        let canonical = wt_path.canonicalize().unwrap();
        let link = tmp.path().join("via-link");
        std::os::unix::fs::symlink(&wt_path, &link).unwrap();
        let hook = FakeHook::silent();
        jobs::on_this_thread(|b| {
            delete_worktree(&repo_dir, &link, Some("linked"), true, &[hook.clone()][..], b)
        })
        .expect("delete succeeds");
        assert_eq!(hook.events(), [Event::Removed { main: repo_dir, checkout: canonical }]);
    }
```

`git worktree remove --force` accepts the symlinked path (checked against git 2.43, which resolves real paths), and `delete_worktree` already canonicalizes before removal (`worktree.rs:573-576`), so this test pins existing behavior through the new hook path.

Update the existing callers in the same test module so they compile: lines 848 and 910 become `create(&req, &[] as &[FakeHook], |_| {}, blocking)`, and line 707 becomes `spawn_delete(repo_dir, job, Vec::<FakeHook>::new(), repaint.clone())`.

New file `alacritree/src/checkout_hooks.rs`, with tests only for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doppler_joins_the_list_only_when_enabled() {
        let mut integrations = crate::config::IntegrationsConfig::default();
        assert!(matches!(from_config(&integrations)[..], [Hook::Doppler(_)]));
        integrations.doppler.enabled = false;
        assert!(from_config(&integrations).is_empty());
    }

    #[test]
    fn report_forwards_lines_and_names_failures() {
        use alacritree_checkout_hooks::HookError;
        let outcomes = vec![
            Ok(Some("Linked 2 Doppler scope(s)".to_string())),
            Ok(None),
            Err(HookError::Spawn { hook: "mise".into(), source: std::io::Error::other("x") }),
        ];
        let mut lines = Vec::new();
        report(outcomes, |l| lines.push(l.to_string()));
        assert_eq!(lines, ["Linked 2 Doppler scope(s)", "Hook failed: could not run mise"]);
    }
}
```

Add `pub(crate) mod checkout_hooks;` to `alacritree/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree --lib worktree:: checkout_hooks::`
Expected: FAIL to compile (`from_config`, `report`, `Hook` not found; `create` takes three arguments).

- [ ] **Step 3: Implement `checkout_hooks.rs`**

Above the test module:

```rust
//! The checkout hooks this build knows, dispatched by `match` rather than a
//! vtable, and the one place that decides which of them a config turns on.

use alacritree_checkout_hooks::{CheckoutHook, Outcome, ambassador_impl_CheckoutHook};
use alacritree_doppler::DopplerHook;
use ambassador::Delegate;

use crate::config::IntegrationsConfig;

#[derive(Debug, Clone, Delegate)]
#[delegate(CheckoutHook)]
pub(crate) enum Hook {
    Doppler(DopplerHook),
}

/// Built-in hooks first, in a fixed order, so the progress steps read the
/// same on every create.
pub(crate) fn from_config(integrations: &IntegrationsConfig) -> Vec<Hook> {
    let mut hooks = Vec::new();
    if integrations.doppler.enabled {
        hooks.push(Hook::Doppler(DopplerHook));
    }
    hooks
}

/// Hand each outcome to `line` as one progress line: a hook's own report,
/// or the error that stopped it.  Hooks with nothing to say add nothing.
pub(crate) fn report(outcomes: Vec<Outcome>, mut line: impl FnMut(&str)) {
    for outcome in outcomes {
        match outcome {
            Ok(Some(text)) => line(&text),
            Ok(None) => {},
            Err(e) => line(&format!("Hook failed: {e}")),
        }
    }
}
```

- [ ] **Step 4: Thread hooks through `worktree.rs`**

`create`: add the generic parameter and replace Task 5's interim doppler call (around line 173):

```rust
pub(crate) fn create<H: CheckoutHooks + ?Sized>(
    req: &CreateRequest,
    hooks: &H,
    mut on_step: impl FnMut(&str),
    blocking: &jobs::Blocking,
) -> Result<PathBuf, String> {
```

```rust
    let event = Checkout { main: &req.project_root, checkout: &target };
    crate::checkout_hooks::report(hooks.created(&event, blocking), &mut *send);
```

`spawn_create`:

```rust
pub(crate) fn spawn_create<H: CheckoutHook + Send + 'static>(
    req: CreateRequest,
    hooks: Vec<H>,
    repaint: impl Repaint,
) -> (Receiver<Progress>, jobs::Job<()>) {
```

and inside the job closure call `create(&req, hooks.as_slice(), |step| { ... }, blocking)`.

`delete_worktree` gains `hooks: &H` (with `H: CheckoutHooks + ?Sized`) before `blocking`. Replace the interim call after `run_git`/branch deletion:

```rust
    let event = Checkout { main: project_root, checkout: &scope_root };
    crate::checkout_hooks::report(hooks.removed(&event, blocking), |line| {
        log::info!("{line} (removed {})", scope_root.display())
    });
```

`spawn_delete` gains `hooks: Vec<H>` (with `H: CheckoutHook + Send + 'static`) after `job`, and passes `hooks.as_slice()` to `delete_worktree`. `prune_worktree` is unchanged: its directory is already gone, and the pilot keeps today's behavior of running no cleanup there. Three comments still name doppler cleanup; change each to "checkout hooks":
- `worktree.rs:573-574`: "the doppler cleanup below runs after git has deleted it".
- `worktree.rs:602`: the `spawn_delete` doc, "The git shellouts and doppler cleanup are slow enough to stutter paint".
- `app/modals.rs:455`: "The git removal (shellouts, branch delete, doppler cleanup) is slow".

Imports in `worktree.rs`: `use alacritree_checkout_hooks::{Checkout, CheckoutHook, CheckoutHooks};`. Remove the Task 5 interim `DopplerHook` import.

- [ ] **Step 5: Update the callers**

- `app/modals.rs:988`: `wt::spawn_create(req, crate::checkout_hooks::from_config(&self.config.integrations), ctx.clone())`.
- `app/modals.rs:480`: `wt::spawn_delete(project_root, delete_job, crate::checkout_hooks::from_config(&self.config.integrations), ctx.clone())`.
- `ipc/server.rs`: `spawn_listener(repaint, workspace, hooks: Vec<Hook>)` passes `hooks` to `listen_at(path, repaint, workspace, hooks)`, which wraps it in `Arc<Vec<Hook>>` next to `workspace` and clones the `Arc` into each connection thread. `handle_connection` and `dispatch` gain `hooks: &[Hook]`. `create_worktree` gains `hooks: &[Hook]` and calls `wt::spawn_create(req, hooks.to_vec(), repaint.clone())`. Line 316: `dispatch(request.clone(), &self.app_tx, &self.repaint, &WorkspaceConfig::default(), &[])`. Lines 381 and 417: pass `Vec::new()` as the new last argument.
- `app/ipc_handler.rs:15`: `ipc::server::spawn_listener(ctx.clone(), config.workspace.clone(), crate::checkout_hooks::from_config(&config.integrations))`.
- `cli/offline.rs`: `handle(request, workspace, hooks: &[Hook])` and `handle_at(state_path, request, workspace, hooks)` thread `hooks` to `create_worktree(project_root, branch, workspace, hooks)`, which calls `wt::create(&request, hooks, |step| steps.push(step.to_string()), blocking)`. Update this module's tests that call `handle_at` to pass `&[]`.
- `cli/mod.rs:547`: `offline::handle(request, &resolved.workspace, &crate::checkout_hooks::from_config(&resolved.integrations))`.

Import `crate::checkout_hooks::Hook` where the signatures name it.

- [ ] **Step 6: Replace the first-open path in `app.rs`**

- Line 383: `doppler_synced: HashSet<PathBuf>` → `hooks_opened: HashSet<PathBuf>`. Line 507: its initializer.
- Line 1040: `self.sync_doppler_scopes(dir.clone());` → `self.sync_checkout_hooks(dir.clone());`. Replace "the scope mirror itself" / "`doppler run`" in the comment above it with "the hooks themselves" / "a hooked tool such as `doppler run`".
- Replace `sync_doppler_scopes` (lines 1135-1159) with:

```rust
    /// Run every checkout hook's `on_opened` the first time this process
    /// opens a shell in a linked worktree.  The create path covers worktrees
    /// alacritree makes; this covers ones created outside it, which would
    /// otherwise lack, for example, their Doppler scopes.
    fn sync_checkout_hooks(&mut self, worktree: PathBuf) {
        if !self.hooks_opened.insert(worktree.clone()) {
            return;
        }
        let main_checkout = self.projects.iter().find_map(|p| {
            let owns = p.worktrees.iter().any(|wt| !wt.is_main && wt.path == worktree);
            if !owns {
                return None;
            }
            p.worktrees.iter().find(|wt| wt.is_main).map(|wt| wt.path.clone())
        });
        let Some(main_checkout) = main_checkout else {
            return;
        };
        let hooks = crate::checkout_hooks::from_config(&self.config.integrations);
        self.detached_jobs.push(jobs::pool().spawn(jobs::Priority::Background, move |blocking| {
            let event = Checkout { main: &main_checkout, checkout: &worktree };
            crate::checkout_hooks::report(hooks.opened(&event, blocking), |line| {
                log::info!("{line} ({})", worktree.display())
            });
        }));
    }
```

`hooks` is a `Vec<Hook>`; `hooks.opened` resolves through the `[H]` impl via auto-deref.

Imports in `app.rs`: add `use alacritree_checkout_hooks::{Checkout, CheckoutHooks};`, and remove the `CheckoutHook` import Task 5 added for its interim call.

Add an app test next to the existing session-spawn tests in `app.rs` (search for `fn test_app()` to find them):

```rust
    #[test]
    fn checkout_hooks_open_once_per_worktree_per_process() {
        let mut app = test_app();
        let wt = PathBuf::from("/not/a/project/worktree");
        app.sync_checkout_hooks(wt.clone());
        app.sync_checkout_hooks(wt.clone());
        assert!(app.hooks_opened.contains(&wt));
        assert_eq!(app.hooks_opened.len(), 1);
        assert!(app.detached_jobs.is_empty(), "no project owns it, so nothing runs");
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: the workspace tests.
Expected: PASS, including the three new `worktree` tests, the two `checkout_hooks` tests and the app test. `grep -rn 'doppler::' alacritree/src` prints nothing.

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add -A alacritree
git commit -m "refactor(worktree): run checkout hooks where doppler was called directly"
```

---

### Task 7: Doppler on the checkout's WSL side

**Files:**
- Modify: `crates/alacritree_doppler/src/scopes.rs`, `alacritree/src/cli/doctor.rs:268-275,340-361,887-909`

**Interfaces:**
- Consumes: `alacritree_common::side::{Side, Program, Ran, run}`, `alacritree_common::wsl::{classify, Location}`, `alacritree_common::tools::{program, wsl_located, Tool}`.
- Produces: no new public API. `scopes::mirror_scopes`/`forget_scopes` keep their signatures and now act on the checkout's side.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `scopes.rs` (these run on every OS because they build `Location` values directly):

```rust
    use alacritree_common::wsl::Location;

    fn in_distro(linux: &str) -> Location {
        Location::Wsl { distro: "Ubuntu".into(), linux_path: linux.into() }
    }

    #[test]
    fn a_wsl_checkout_is_scoped_by_its_linux_path() {
        assert_eq!(scope_path(&in_distro("/home/u/wt")), PathBuf::from("/home/u/wt"));
        assert_eq!(scope_path(&Location::Windows(PathBuf::from("/srv/wt"))), PathBuf::from("/srv/wt"));
    }

    #[test]
    fn a_wsl_checkout_runs_the_distros_doppler() {
        assert_eq!(side_for(&in_distro("/home/u/wt")), Side::Wsl { distro: "Ubuntu".into() });
        assert_eq!(side_for(&Location::Windows(PathBuf::from("C:/wt"))), Side::Native);
    }

    #[test]
    fn scopes_rebase_between_linux_paths() {
        let target = rebase_scope(
            "/home/u/repo/apps/web",
            &scope_path(&in_distro("/home/u/repo")),
            &scope_path(&in_distro("/home/u/wt")),
        );
        assert_eq!(target, Some(PathBuf::from("/home/u/wt/apps/web")));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree_doppler scopes::`
Expected: FAIL to compile (`scope_path`, `side_for` not found).

- [ ] **Step 3: Implement**

In `scopes.rs`, add to the module header comment:

```rust
//! A worktree inside WSL is scoped by the distro's own doppler, under its
//! Linux path: the Windows doppler keeps a separate config file that the
//! distro's `doppler run` never reads.
```

Replace `canonical`, `run`, `all_scopes`, and the path handling in `mirror_scopes`/`forget_scopes` with side-aware versions:

```rust
use alacritree_common::side::{self, Program, Ran, Side};
use alacritree_common::wsl::{self, Location};

/// Where doppler runs for a checkout at `location`.
fn side_for(location: &Location) -> Side {
    Side::from_location(location)
}

/// The path doppler keys a scope by on the checkout's own side.
fn scope_path(location: &Location) -> PathBuf {
    PathBuf::from(side::spelling(location))
}

/// A checkout's location, canonicalized first so a symlinked or relative
/// path names the same scope as the one `doppler setup` wrote.
fn locate(path: &Path) -> Location {
    wsl::classify(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn doppler_on(side: &Side, blocking: &jobs::Blocking) -> Program {
    let wsl = match side {
        Side::Native => None,
        Side::Wsl { distro } => tools::wsl_located(Tool::Doppler, distro, blocking),
    };
    Program { native: tools::program(Tool::Doppler), wsl, name: Tool::Doppler.name().into() }
}

/// Run doppler on `side`, returning stdout on success and `None` on any
/// failure, including doppler not being installed there, which is the
/// common case and must stay quiet.
fn run(side: &Side, args: &[&str], scope: Option<&Path>, blocking: &jobs::Blocking) -> Option<Vec<u8>> {
    let mut argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    argv.push("--no-check-version".into());
    if let Some(scope) = scope {
        argv.push("--scope".into());
        argv.push(scope.to_string_lossy().into_owned());
    }
    match side::run(side, &doppler_on(side, blocking), None, &argv, blocking) {
        Ok(Ran::Finished(output)) if output.status.success() => Some(output.stdout),
        _ => None,
    }
}

fn all_scopes(side: &Side, blocking: &jobs::Blocking) -> Option<Scopes> {
    let stdout = run(side, &["configure", "--all", "--json"], None, blocking)?;
    serde_json::from_slice(&stdout).ok()
}
```

In `mirror_scopes`: compute `let main_at = locate(main_checkout); let wt_at = locate(worktree); let side = side_for(&wt_at);`. Return 0 when `side_for(&main_at) != side` (a main checkout and a worktree on different sides share no doppler config). Use `scope_path(&main_at)`/`scope_path(&wt_at)` where `canonical(...)` was used, call `all_scopes(&side, blocking)`, and call `run(&side, &args, Some(&target), blocking)`.

In `forget_scopes`: `let wt_at = locate(worktree); let side = side_for(&wt_at); let worktree = scope_path(&wt_at);`, then `all_scopes(&side, blocking)` and `run(&side, &["configure", "unset", "project", "config"], Some(Path::new(scope)), blocking)`.

Delete the old `canonical` and the old `run` (with its `#[allow(clippy::disallowed_methods)]`). The spawn now goes through `side::run`.

`alacritree doctor` still says the distro's doppler is never used, which this task makes false. In `alacritree/src/cli/doctor.rs`:
- Delete `wsl_doppler_check` (lines 340-361) and its call at line 269. The per-distro line from `wsl_distro_check` already lists where doppler resolves inside each distro, and that is now the path the hook runs.
- Delete its two tests, `doppler_inside_a_distro_is_reported_as_unused` and `no_distro_has_doppler_and_nothing_is_said` (lines 887-909).
- Rewrite the `probe_distros` doc comment (lines 273-274), which says nothing runs doppler inside a distro: "Probes every registry tool, since each is one alacritree runs for a project inside the distro."

`README.md:197-201` already describes the distro paths as the ones alacritree uses, which is now true for doppler too. It needs no change.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree_doppler`, then the workspace tests.
Expected: PASS: the three new tests plus Task 5's end-to-end tests, which exercise the native path through `side::run`, and `cli::doctor` without the two deleted tests.

Run: `cargo clippy -p alacritree_doppler --all-targets --no-deps -- -D clippy::disallowed_methods`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/alacritree_doppler alacritree/src/cli/doctor.rs
git commit -m "fix(doppler): mirror scopes with the distro's doppler for WSL worktrees"
```

---

### Task 8: The user-defined command hook

**Files:**
- Create: `crates/alacritree_checkout_hooks/src/command.rs`
- Modify: `crates/alacritree_checkout_hooks/src/lib.rs`, `crates/alacritree_checkout_hooks/Cargo.toml`, `alacritree/src/config.rs:3135-3155,598-612,3353-3366`, `alacritree/src/checkout_hooks.rs`, `alacritree/src/cli/config_reference.rs:38-61`, `alacritree/tests/schema-defaults-allowlist.txt`, `alacritree/tests/stock-config.json` (regenerated), `schema/alacritree-config.json`, `docs/config-reference.md`

**Interfaces:**
- Consumes: `side::{Side, Program, Ran, run, spelling}`, `wsl::classify`, `HookError`, `CheckoutHook`.
- Produces (in `alacritree_checkout_hooks::command`, re-exported at the crate root):
  - `pub struct RawCheckoutHooks { pub command: BTreeMap<String, RawCommandHook> }` (`Default, Deserialize, JsonSchema`, `#[serde(default)]`).
  - `pub struct RawCommandHook` with `enabled: bool` (default `true`), `path: String` (required), `wsl_path: String`, `on_created`, `on_opened`, `on_removed: Vec<String>`.
  - `pub struct CommandHook { pub name: String, pub program: Program, pub on_created: Vec<String>, pub on_opened: Vec<String>, pub on_removed: Vec<String> }` (`Debug, Clone, PartialEq, Eq, serde::Serialize`), implementing `CheckoutHook`.
  - `RawCheckoutHooks::resolve(self) -> Vec<CommandHook>`: enabled hooks only, sorted by name. `program.name` is the file stem of `path` (so `C:\Tools\mise.exe` looks up `mise` inside a distro), or `path` itself when it has none.
  - `pub fn expand(template: &[String], event: &Checkout<'_>) -> Vec<String>`.
  - The app: `IntegrationsConfig.checkout_hooks: Vec<CommandHook>`, `Hook::Command(CommandHook)`.

- [ ] **Step 1: Write the failing tests**

`crates/alacritree_checkout_hooks/src/command.rs` test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alacritree_common::jobs;
    use std::path::Path;

    fn hook(program: &str, on_created: &[&str]) -> CommandHook {
        CommandHook {
            name: "test".into(),
            program: Program { native: program.into(), wsl: None, name: program.into() },
            on_created: on_created.iter().map(|s| s.to_string()).collect(),
            on_opened: Vec::new(),
            on_removed: Vec::new(),
        }
    }

    fn created(hook: &CommandHook, main: &Path, checkout: &Path) -> Outcome {
        let e = Checkout { main, checkout };
        jobs::on_this_thread(|b| hook.on_created(&e, b))
    }

    /// A path with spaces or quotes must stay one argument: the program is
    /// run directly, never through a shell that would split it.
    #[test]
    fn placeholders_expand_to_exactly_one_argument_each() {
        let e = Checkout { main: Path::new("/src/my repo"), checkout: Path::new("/wt/it's ü") };
        let args = expand(&["trust".into(), "{checkout}".into(), "--from={main}".into()], &e);
        assert_eq!(args, ["trust", "/wt/it's ü", "--from=/src/my repo"]);
    }

    #[test]
    fn an_empty_template_skips_the_event() {
        let h = hook("alacritree-no-such-program", &[]);
        assert_eq!(created(&h, Path::new("/m"), Path::new("/c")).unwrap(), None);
    }

    #[test]
    fn a_missing_program_is_skipped() {
        let h = hook("alacritree-no-such-program", &["x"]);
        assert_eq!(created(&h, Path::new("/m"), Path::new("/c")).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn success_reports_the_hook_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("seen");
        let h = hook("sh", &["-c", &format!("printf %s \"$1\" > '{}'", out.display()), "sh", "{checkout}"]);
        let outcome = created(&h, Path::new("/m"), tmp.path()).unwrap();
        assert_eq!(outcome.as_deref(), Some("Ran test"));
        assert_eq!(std::fs::read_to_string(out).unwrap(), tmp.path().to_string_lossy());
    }

    #[cfg(unix)]
    #[test]
    fn a_non_zero_exit_fails_with_the_first_stderr_line() {
        let tmp = tempfile::tempdir().unwrap();
        let h = hook("sh", &["-c", "echo 'not trusted' >&2; echo more >&2; exit 2"]);
        let err = created(&h, Path::new("/m"), tmp.path()).unwrap_err();
        match err {
            HookError::Failed { hook, status, stderr } => {
                assert_eq!(hook, "test");
                assert_eq!(status.code(), Some(2));
                assert_eq!(stderr, "not trusted");
            },
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn resolve_keeps_enabled_hooks_sorted_by_name() {
        let raw: RawCheckoutHooks = toml::from_str(
            r#"
            [command.zeta]
            path = "z"
            [command.alpha]
            path = "a"
            on_created = ["{checkout}"]
            [command.off]
            path = "o"
            enabled = false
            "#,
        )
        .unwrap();
        let hooks = raw.resolve();
        let names: Vec<_> = hooks.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(hooks[0].program, Program { native: "a".into(), wsl: None, name: "a".into() });
        assert_eq!(hooks[0].on_created, ["{checkout}"]);
    }

    /// Inside a distro the login shell looks the program up by name; a
    /// Windows path handed to it would never be found.
    #[test]
    fn a_path_is_looked_up_in_a_distro_by_its_file_stem() {
        let raw: RawCheckoutHooks =
            toml::from_str("[command.mise]\npath = '/opt/tools/mise.exe'").unwrap();
        assert_eq!(raw.resolve()[0].program.name, "mise");
    }

    #[test]
    fn a_hook_without_a_path_is_a_parse_error() {
        assert!(toml::from_str::<RawCheckoutHooks>("[command.x]\non_created = []").is_err());
    }
}
```

`crates/alacritree_checkout_hooks/Cargo.toml`: add `schemars = "1.2"`, `serde = { version = "1", features = ["derive"] }` to `[dependencies]`, and `tempfile = "3"`, `toml = { workspace = true }` to `[dev-dependencies]`.

Merge test in `alacritree/src/config.rs` tests (next to the existing merge tests near line 5298):

```rust
    /// alacritty.toml is shared with alacritty and loaded first; a hook it
    /// defines must be switchable off from alacritree.toml.
    #[test]
    fn a_command_hook_can_be_disabled_by_the_later_file() {
        let base: toml::Value = toml::from_str(
            "[integrations.checkout_hooks.command.mise]\npath = \"mise\"\non_created = [\"trust\"]",
        )
        .unwrap();
        let over: toml::Value =
            toml::from_str("[integrations.checkout_hooks.command.mise]\nenabled = false").unwrap();
        let merged = merge(base, over);
        let raw: RawConfig = merged.try_into().unwrap();
        assert!(raw.into_config().integrations.checkout_hooks.is_empty());
    }
```

This follows `font_fallback_arrays_concatenate_across_files` (`config.rs:~5293`), which turns a merged `toml::Value` into a `Config` the same way.

`alacritree/src/checkout_hooks.rs` test:

```rust
    #[test]
    fn command_hooks_follow_the_built_in_ones() {
        let mut integrations = crate::config::IntegrationsConfig::default();
        integrations.checkout_hooks = vec![alacritree_checkout_hooks::CommandHook {
            name: "mise".into(),
            program: alacritree_common::side::Program {
                native: "mise".into(),
                wsl: None,
                name: "mise".into(),
            },
            on_created: vec!["trust".into()],
            on_opened: Vec::new(),
            on_removed: Vec::new(),
        }];
        assert!(matches!(from_config(&integrations)[..], [Hook::Doppler(_), Hook::Command(_)]));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree_checkout_hooks command::` and `cargo test -p alacritree --lib checkout_hooks:: config::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `command.rs`**

Above its tests:

```rust
//! Hooks the user defines: a program and one argv template per event, in
//! the same shape as the custom diff viewer.

use std::collections::BTreeMap;

use alacritree_common::jobs::Blocking;
use alacritree_common::side::{self, Program, Ran, Side};
use alacritree_common::wsl;
use serde::Deserialize;

use crate::{Checkout, CheckoutHook, HookError, Outcome};

/// `[integrations.checkout_hooks]`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct RawCheckoutHooks {
    /// Programs to run when a worktree is created, first opened, or removed,
    /// keyed by a name of your choice. A table rather than a list, so a hook
    /// defined in alacritty.toml can be changed or disabled by name from
    /// alacritree.toml.
    pub command: BTreeMap<String, RawCommandHook>,
}

/// `[integrations.checkout_hooks.command.<name>]`.  `path` is required, so
/// defaults are per field rather than from a `Default` impl.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RawCommandHook {
    /// Run this hook.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// The program to run on Windows or natively. A bare name is looked up
    /// on PATH.
    pub path: String,
    /// The program to run inside a WSL distro for a worktree there, as
    /// written. Empty looks up the file name of `path`, without directory or
    /// extension, through the distro's login shell, and a distro where that
    /// finds nothing skips the hook.  A Windows `path` is never run there.
    #[serde(default)]
    pub wsl_path: String,
    /// Arguments when alacritree creates a worktree. `{checkout}` is the new
    /// worktree and `{main}` the project's main checkout. Empty skips it.
    #[serde(default)]
    pub on_created: Vec<String>,
    /// Arguments the first time this process opens a shell in a worktree,
    /// including ones created outside alacritree. Runs again after a restart,
    /// so the command must be safe to repeat. Empty skips it.
    #[serde(default)]
    pub on_opened: Vec<String>,
    /// Arguments after a worktree is removed. Runs in the main checkout,
    /// since the worktree is gone. Empty skips it.
    #[serde(default)]
    pub on_removed: Vec<String>,
}

fn enabled_by_default() -> bool {
    true
}

/// The name a distro's login shell finds `path` by: a native path, possibly
/// a Windows one, means nothing inside the distro.
fn lookup_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map_or_else(|| path.to_string(), |stem| stem.to_string_lossy().into_owned())
}

impl RawCheckoutHooks {
    /// Enabled hooks only, in name order, so the progress steps read the
    /// same on every create.
    pub fn resolve(self) -> Vec<CommandHook> {
        self.command
            .into_iter()
            .filter(|(_, raw)| raw.enabled)
            .map(|(name, raw)| CommandHook {
                name,
                program: Program {
                    name: lookup_name(&raw.path),
                    native: raw.path,
                    wsl: Some(raw.wsl_path).filter(|p| !p.trim().is_empty()),
                },
                on_created: raw.on_created,
                on_opened: raw.on_opened,
                on_removed: raw.on_removed,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommandHook {
    pub name: String,
    pub program: Program,
    pub on_created: Vec<String>,
    pub on_opened: Vec<String>,
    pub on_removed: Vec<String>,
}

/// Fill `{checkout}` and `{main}` with each path as the checkout's side
/// spells it.  Each template word stays one argument.
pub fn expand(template: &[String], event: &Checkout<'_>) -> Vec<String> {
    let checkout = side::spelling(&wsl::classify(event.checkout));
    let main = side::spelling(&wsl::classify(event.main));
    template.iter().map(|word| word.replace("{checkout}", &checkout).replace("{main}", &main)).collect()
}

fn first_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).lines().next().unwrap_or_default().trim().to_string()
}

impl CommandHook {
    fn run(&self, template: &[String], event: &Checkout<'_>, cwd: &std::path::Path, blocking: &Blocking) -> Outcome {
        if template.is_empty() {
            return Ok(None);
        }
        let side = Side::of(event.checkout);
        let cwd_spelled = std::path::PathBuf::from(side::spelling(&wsl::classify(cwd)));
        let cwd = if side == Side::Native { cwd } else { cwd_spelled.as_path() };
        match side::run(&side, &self.program, Some(cwd), &expand(template, event), blocking) {
            Err(source) => Err(HookError::Spawn { hook: self.name.clone(), source }),
            Ok(Ran::Missing) => {
                log::debug!("checkout hook {}: {} is not installed on this side", self.name, self.program.name);
                Ok(None)
            },
            Ok(Ran::Finished(output)) if output.status.success() => Ok(Some(format!("Ran {}", self.name))),
            Ok(Ran::Finished(output)) => Err(HookError::Failed {
                hook: self.name.clone(),
                status: output.status,
                stderr: first_line(&output.stderr),
            }),
        }
    }
}

impl CheckoutHook for CommandHook {
    fn on_created(&self, event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        self.run(&self.on_created, event, event.checkout, blocking)
    }

    fn on_opened(&self, event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        self.run(&self.on_opened, event, event.checkout, blocking)
    }

    /// The worktree is gone, so the command runs in the main checkout.
    fn on_removed(&self, event: &Checkout<'_>, blocking: &Blocking) -> Outcome {
        self.run(&self.on_removed, event, event.main, blocking)
    }
}
```

In `lib.rs`: add `pub mod command;` and `pub use command::{CommandHook, RawCheckoutHooks, RawCommandHook};`.

The `a_missing_program_is_skipped` test passes `/c` as the checkout, which does not exist. Natively, `current_dir` on a missing directory makes the spawn fail with `NotFound`, which `side::run` reports as `Missing`, so the test still passes. Keep it: it pins that a vanished directory does not surface as an error either.

- [ ] **Step 4: Wire the config and the app enum**

`alacritree/src/config.rs`:
- `RawIntegrations` gains, after `doppler`:
  ```rust
      /// Programs to run when a worktree is created, first opened, or removed.
      checkout_hooks: alacritree_checkout_hooks::RawCheckoutHooks,
  ```
- `IntegrationsConfig` gains `pub checkout_hooks: Vec<alacritree_checkout_hooks::CommandHook>,`.
- `RawIntegrations::resolve` gains `checkout_hooks: self.checkout_hooks.resolve(),`.

`alacritree/src/checkout_hooks.rs`:

```rust
use alacritree_checkout_hooks::CommandHook;

#[derive(Debug, Clone, Delegate)]
#[delegate(CheckoutHook)]
pub(crate) enum Hook {
    Doppler(DopplerHook),
    Command(CommandHook),
}

/// Built-in hooks first, in a fixed order, then the user's in name order, so
/// the progress steps read the same on every create.
pub(crate) fn from_config(integrations: &IntegrationsConfig) -> Vec<Hook> {
    let mut hooks = Vec::new();
    if integrations.doppler.enabled {
        hooks.push(Hook::Doppler(DopplerHook));
    }
    hooks.extend(integrations.checkout_hooks.iter().cloned().map(Hook::Command));
    hooks
}
```

`alacritree/tests/schema-defaults-allowlist.txt`: two new lines, in sorted position.
- `RawCommandHook.path`: the header's "the field is required" covers it.
- `RawCheckoutHooks.command`: schemars 1.2 publishes a field's default only when the field type is `Serialize` (`schemars-1.2.2/src/_private/mod.rs:169-193`), and `RawCommandHook` is not, so the map gets none. The header's "a collection whose element type implements no `Serialize`" covers it, as it does `RawKeyboard.bindings`.

Regenerate with `ALACRITREE_UPDATE_ALLOWLIST=1 cargo test -p alacritree --test schema_defaults` (the allowlist header's `devkit run task test --env ALACRITREE_UPDATE_ALLOWLIST=1` does the same) and confirm the diff is exactly those two lines.

`alacritree/src/cli/config_reference.rs`: the renderer descends only through `properties` and `items`, so a map of tables renders as one line, `command (table)`, and `RawCommandHook`'s fields never reach `docs/config-reference.md`. Teach `Reference::table` to follow `additionalProperties`. In its loop over `properties(schema)`, before the plain-key branch, add:

```rust
            } else if let Some(entry) = self.map_entry(child) {
                // A table of named tables: document the key, then one
                // `<name>` table for the shape every entry takes.
                self.key(key, &path, child);
                nested.push((format!("{path}.<name>"), entry, false));
```

and the helper:

```rust
    /// The entry schema of a map whose values are tables, such as named
    /// command hooks.
    fn map_entry(&self, prop: &'a Value) -> Option<&'a Value> {
        let entry = self.resolve(prop).get("additionalProperties")?;
        self.is_table(entry).then_some(entry)
    }
```

Add a test module to `config_reference.rs`:

```rust
#[cfg(test)]
mod tests {
    /// Named tables are a map in the schema; their fields must still reach
    /// the reference, or command hooks are undocumented.
    #[test]
    fn a_map_of_tables_documents_its_entry_fields() {
        let doc = super::document();
        assert!(doc.contains("`[integrations.checkout_hooks.command.<name>]`"), "{doc}");
        assert!(doc.contains("- `on_created` (array of string"), "{doc}");
    }
}
```

Run it before the renderer change and confirm it fails.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`, then `ALACRITREE_UPDATE_STOCK=1 cargo test -p alacritree --lib the_stock_config_is_unchanged`, then the workspace tests.
Expected: PASS. Read three diffs:
- `schema/alacritree-config.json` and `docs/config-reference.md` add `[integrations.checkout_hooks]`, the `command` key, and `[integrations.checkout_hooks.command.<name>]` with the six fields and the doc comments above, and nothing else. If another section's rendering changed, the renderer change reached a map it should not have; check which.
- `alacritree/tests/stock-config.json` adds only `"checkout_hooks": []` under `integrations`.
- `schema-defaults-allowlist.txt` adds only `RawCheckoutHooks.command` and `RawCommandHook.path`.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A crates alacritree schema docs
git commit -m "feat(checkout-hooks): run user-defined commands on worktree events"
```

---

### Task 9: Documentation and final verification

**Files:**
- Modify: `AGENTS.md`, `docs/alacritree.md` (only if it lists integrations; check with `grep -n -i doppler docs/alacritree.md`)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing new.

- [ ] **Step 1: Update AGENTS.md**

In "Repository layout", after the `alacritree/` bullet, add:

```markdown
- `crates/` — alacritree's own library crates, split out of `alacritree/` so the compiler enforces their boundaries. `alacritree_common` holds process spawning, the job pool, WSL support and tool paths. Each integration type gets a crate holding its trait, shared models, error type and a test fake (`alacritree_checkout_hooks`), and each backend gets its own crate (`alacritree_doppler`). Only the `alacritree` app depends on specific backends. These crates are original work, like `alacritree/`, and agent-edited code may live in them.
```

In "Big-picture architecture": replace the `command_ext.rs`, `wsl.rs`, and doppler mentions with their new homes (`alacritree_common::command_ext`, `alacritree_common::wsl`, `alacritree_doppler`), and add:

```markdown
- `checkout_hooks.rs` — the app's `Hook` enum over every checkout hook backend (`#[derive(ambassador::Delegate)]`) and `from_config`, which decides which run. Worktree create, first shell open, and removal run the list; `worktree.rs` sees only the `CheckoutHooks` trait.
```

In "Conventions specific to this fork", add:

```markdown
- Integration traits are `#[ambassador::delegatable_trait]` and the app dispatches through a `#[derive(Delegate)]` enum. Not `enum_dispatch`, which cannot link a trait and an enum in different crates, and not `Box<dyn>`. Trait signatures name types by absolute path, because ambassador copies them into the crate that derives. Closed sets of names use strum derives.
- New crates report errors with `thiserror` enums, not `String`. See #135 for the rest of the app.
- A config section belongs to the crate of the integration it configures. The app's `RawIntegrations` names it with one field.
```

Update the "Build / run" block so `cargo test -p alacritree` also shows the workspace form used by CI.

- [ ] **Step 2: Verify release-please accepts the new crates**

Run: `npx --yes release-please@16 release-pr --dry-run --repo-url=mathix420/alacritree --token=dummy --target-branch=master 2>&1 | head -40`

If this needs network access or a token that is not available, read `release-please-config.json` together with the `cargo-workspace` plugin's documentation (use the `devkit:docs` skill: `docm add release-please --eco js`, then `docm info release-please`), and confirm whether members missing from `packages` are ignored. If they are not, add each new crate to `packages` with `"skip-github-release": true` and `"release-type": "rust"`. Record the finding in the PR description either way.

- [ ] **Step 3: Full verification**

Run each and confirm it passes:

```bash
cargo fmt --check
cargo test --workspace --exclude alacritty --exclude alacritty_terminal --exclude alacritty_config --exclude alacritty_config_derive
cargo clippy --workspace --exclude alacritty --exclude alacritty_terminal --exclude alacritty_config --exclude alacritty_config_derive --all-targets --no-deps -- -D clippy::disallowed_methods
python3 alacritree/tools/ui-thread-audit.py .
cargo build -p alacritree --release
```

Expected: all pass. The audit's findings match `master`'s, and none of them names a function under `crates/`.

Run: `grep -rn 'doppler::\|Tool::ALL\|\[ToolPaths; 7\]' alacritree/src crates`
Expected: no output.

- [ ] **Step 4: Commit and push**

```bash
cargo fmt
git add -A AGENTS.md docs release-please-config.json
git commit -m "docs(agents): describe crates/, checkout hooks and the dispatch conventions"
git push -u origin claude/integration-refactoring-issue-h27hpb
```

- [ ] **Step 5: Open the draft PR against upstream**

Per the spec's Delivery section, open the PR against `mathix420/alacritree`, base `master`, with head `AbysmalBiscuit:claude/integration-refactoring-issue-h27hpb`. Do not open it against the fork. The PR body lists the three new crates, the behavior changes (WSL doppler, `[integrations.doppler] enabled`, command hooks), the release-please finding, and a manual test plan:

- On a Windows host with a WSL worktree whose main checkout has `doppler setup` scopes, create a worktree from the sidebar and confirm `doppler run -- env` works in it.
- Add a `[integrations.checkout_hooks.command.echo]` hook with `on_created = ["{checkout}"]` and `path = "cmd"` (Windows) or `"echo"`, create a worktree, and confirm "Ran echo" appears in the progress steps.
