# Configurable frame log, log directory and state directory

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** let the config file decide three things that today only an environment variable or nothing at all can decide: whether the frame log runs, where logs are written, and where `state.toml` and the scratchpad notes live.

**Architecture:** each key is a value resolved once during startup and then published to the code that needs it, because all three consumers run somewhere the loaded `Config` cannot reach. `frame_log` fills the `OnceLock` its PTY-thread readers already consult. `log_dir` swaps the directory the crash recorder holds, after the hook is armed and before any artifact exists. `state_dir` replaces `state::config_dir()`'s environment lookup with a resolved value that both the GUI and the offline CLI set from the same config.

**Tech Stack:** Rust 2024 (MSRV 1.85), egui/eframe 0.31.1, `alacritty_terminal`. Tests are in-module `#[cfg(test)]`, run with `cargo nextest run -p alacritree`.

**Spec:** https://github.com/AbysmalBiscuit/alacritree/issues/76

**Status:** Tasks 0 through 3 are implemented and committed on `feat/config-dirs`, and the schema is regenerated. What remains of Task 4 is opening the PR.

One thing this plan got wrong, recorded because the reasoning was wrong rather than just the outcome: it said to test that `set_dir` is refused once an artifact exists, which the obvious test cannot prove. With an artifact open, `ensure_artifact` reopens the file it remembers and never consults the directory, so that test passes with the guard deleted. It only bites on the path `ensure_artifact` documents, where the artifact was removed underneath, and the test has to remove it to mean anything.

## Global constraints

- Only `alacritree/` is edited. `alacritty*` and `egui-winit/` are vendored and read-only.
- Default behaviour must not change. Each key's default resolves to exactly the path or flag state in effect before this branch, so an unmodified config keeps every file where it already is.
- Config doc comments are the published JSON Schema's hover text. After touching `config.rs`, regenerate with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.
- Commits use Conventional Commits, imperative subject, no trailing period, lowercase after the colon, wrapped at 72. Every commit ends with the trailer `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`.
- Comments explain *why*, never restate the *what*. No task references, no PR narration.
- A path key accepts a leading `~`, matching `[general] working_directory`, which is the only other path a user types into these files.
- Every one of these settings is read before or outside the UI thread. A value published too late is not an error, it is a silently ignored setting. Each task's test must assert the value actually reached its reader, not merely that it parsed.

---

### Task 0: Worktree setup

Already done. The branch is `feat/config-dirs`, the worktree is `../alacritree-worktrees/feat/config-dirs`, and it sits on `origin/perf/font-coverage-scan` (PR 213, marker `[11]`), making this branch marker `[12]`.

It already carries four commits lifted out of PR 209, which are the CLI and startup-logging work that belongs with these keys rather than with a session-spawn change:

- `feat(logging): record build and config at startup`
- `feat(cli): take a config directory and a log path`
- `fix(config): keep the built-in bindings when the config is invalid`
- `feat(cli): override config values from the command line`

- [ ] **Step 1: Confirm the baseline is green**

```sh
cargo nextest run -p alacritree
```

The suite is flaky under full parallel load on Windows: `focus_priority::windows::tests` and `session::tests::input_written_before_attach_arrives_before_input_written_after` fail on unmodified branches too. Re-run before treating a failure as real.

---

### Task 1: `[debug] frame_log`

**Files:**
- Modify: `alacritree/src/frame_log.rs`, `alacritree/src/config.rs`, `alacritree/src/main.rs`

**Interfaces:**
- Consumes: `Config::debug`.
- Produces: `frame_log::set_enabled(bool)`, called once before any session spawns.

`frame_log::enabled()` caches its `ALACRITREE_FRAME_LOG` read in a `OnceLock` because the PTY threads call `output_arrived()` and `keystroke_sent()` without a handle on the `FrameLog` the UI thread owns. The config value has to reach that same `OnceLock`.

- [ ] **Step 1: Add the key**

`RawDebug` gains `frame_log: Option<bool>`, defaulting to `false`, with a doc comment saying it is alacritree-only and that `ALACRITREE_FRAME_LOG` still overrides it.

- [ ] **Step 2: Let the config fill the `OnceLock`**

Add `set_enabled` beside `enabled()`. The environment variable keeps priority: it is the only switch available before config loads, and a run that exported it is asking for measurements whatever the file says. `enabled()` therefore reads the variable first and falls back to whatever `set_enabled` stored.

- [ ] **Step 3: Call it from `main.rs`**

After `config::load`, before `AlacritreeApp` is constructed. `FrameLog::from_env` reads `enabled()` during construction, so a call placed after that point silently does nothing.

- [ ] **Step 4: Test**

Assert that a config with `frame_log = true` and no environment variable makes `enabled()` true, and that an environment variable set to `0` beats a config that says true.

---

### Task 2: `[debug] log_dir`

**Files:**
- Modify: `alacritree/src/config.rs`, `alacritree/src/crash_log.rs`, `alacritree/src/logdir.rs`, `alacritree/src/main.rs`

**Interfaces:**
- Consumes: `Config::debug`.
- Produces: `crash_log::set_dir(&Path)`.

`crash_log::install` runs before `config::load` on purpose, so a panic inside the load is still recorded. The key naming the directory is therefore unknown when the hook is armed.

`install` creates the directory but no file: `State` holds `dir` with `artifact: None`, and `ensure_artifact` allocates lazily on the first thing worth writing, which is `session_begin()` or a panic. So the directory can be swapped after arming and before anything exists on disk.

- [ ] **Step 1: Add the key**

`RawDebug` gains `log_dir: Option<String>`, unset meaning today's resolution. Document that it governs both the crash artifact and the session log, and that a panic raised before the config parses lands in the default directory because no other answer is available yet.

- [ ] **Step 2: Add `crash_log::set_dir`**

It replaces `State::dir` and calls `logdir::prepare_log_dir` on the new path. It must not re-run `install`: `install` wraps the previous panic hook, so a second call chains two hooks and records every panic twice.

Refuse the swap and keep the current directory when `State::artifact` is already `Some`, so a directory that changes after something was written cannot orphan the record.

- [ ] **Step 3: Resolve the directory in `main.rs`**

Between `config::load` and `crash_log::session_begin()`. Both the session log opened further down and `logging::prune_session_logs` must use the resolved directory, not `logdir::log_dir()`, or the two kinds of file split across two directories.

`--log-file` still wins for the session log: it names one file outright and is documented as turning logging on by itself.

- [ ] **Step 4: Test**

Assert the crash artifact lands in the configured directory, that a swap is refused once an artifact exists, and that `--log-file` beats the key.

---

### Task 3: `[general] state_dir`

**Files:**
- Modify: `alacritree/src/config.rs`, `alacritree/src/state.rs`, `alacritree/src/main.rs`, `alacritree/src/cli/mod.rs`, `alacritree/src/cli/offline.rs`

**Interfaces:**
- Consumes: `Config::general`.
- Produces: `state::set_dir(&Path)`.

This key has more consumers than `state.toml`. `scratchpad.rs` builds its notes path from `state::config_dir()` too, so the key moves the notes with the state file. That is the coherent behaviour: they are one body of per-user data.

It also has a consumer on a path where no config is loaded. `cli::offline::handle` calls `state::config_path()` directly, and the offline CLI answers from `state.toml` without an app. An offline command that reads a different `state.toml` than the GUI writes is a correctness bug, so the CLI path has to resolve the key too.

- [ ] **Step 1: Add the key**

`RawGeneral` gains `state_dir: Option<String>`, unset meaning today's resolution, which is the config directory rather than a state directory. Document that today's default is deliberate and that whether the default should move is an open question this key does not answer.

- [ ] **Step 2: Make `state::config_dir` consult a resolved value**

A `set_dir` mirroring `crash_log`'s. Unset keeps the current environment lookup exactly.

Rename nothing. `config_dir` is `pub(crate)` with several callers and the rename is churn this branch does not need.

- [ ] **Step 3: Publish it on both entry paths**

`main.rs` after `config::load`, and `cli::run` before it dispatches to `offline`. The CLI already carries `--config-dir` and `-o`, so it can resolve the same config the GUI would.

Watch the cost: this makes the offline CLI parse config on every request. Measure `alacritree git-status` before and after and record the delta in the commit body. If it is material, load config only for the requests that reach `offline`, not for every subcommand.

- [ ] **Step 4: Existing notes stay where they are**

Nothing migrates. A user who sets the key and finds their scratchpads missing has lost data by configuration, so the doc comment says plainly that the key does not move existing files.

- [ ] **Step 5: Test**

Assert `state.toml` and a scratchpad both resolve under a configured directory, and that an offline CLI request reads the state file the GUI would have written.

---

### Task 4: Schema, docs and the PR

- [ ] **Step 1: Regenerate the schema**

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

- [ ] **Step 2: Full suite**

```sh
cargo nextest run -p alacritree
```

- [ ] **Step 3: Confirm the base has not moved**

The stack grows while a branch sits unimplemented.

```sh
git -C ../alacritree-worktrees/feat/config-dirs merge-base --is-ancestor origin/perf/font-coverage-scan HEAD
```

- [ ] **Step 4: Open the PR**

Marker `[12]` unless the stack grew. Closes 76 and 57, the second being the CLI flags issue the four lifted commits already implement.
