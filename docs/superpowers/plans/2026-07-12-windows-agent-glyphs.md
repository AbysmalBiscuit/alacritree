# Windows Agent Glyphs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sidebar agent glyphs (✳ claude, ◇ codex, …) work on Windows by probing the shell's descendant process tree, matching the existing Linux `/proc` behavior.

**Architecture:** A pure, platform-neutral matching core (process-tree walk + name/cmdline agent matching) lives in `session.rs` and is unit-tested on any OS. A thin `#[cfg(windows)]` shim feeds it from a process-global, TTL-throttled `sysinfo::System` snapshot (two-phase: cheap names+parents refresh for everyone, cmdline fetch only for the shell's descendants when names don't match). `pty_shell_pid` gains a Windows arm via `ChildExitWatcher::pid()`.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), `sysinfo` 0.36 (Windows-only dep; bumped from the plan's original 0.33 during final review so Cargo.lock reuses the already-present `windows 0.61` instead of adding 0.57), existing `alacritty_terminal` vendored crate (no edits to it).

## Global Constraints

- All changes in `alacritree/` only; vendored crates (`alacritty/`, `alacritty_terminal/`, …) are read-only.
- Linux `/proc` probe stays byte-for-byte untouched; macOS stays `None`.
- `sysinfo` goes under `[target.'cfg(windows)'.dependencies]`, `default-features = false`.
- Any-descendant semantics (user-confirmed): glyph shows if any process in the shell's tree (root inclusive) matches `AGENT_PROCESS_GLYPHS`; name `starts_with` first, cmdline `contains` second.
- Probe must never panic or block on stale/cyclic/vanished process data — degrade to `None`.
- `cargo fmt` enforced. Comments explain *why*, present tense, no change-narration.
- **One commit for the whole feature** at the end (user rule: one logical change per commit; the matching core has no standalone value). Conventional Commits format.
- The feature worktree lives at `C:\Users\Lev\Git\github\alacritree-worktrees\feat\windows-agent-glyphs` (matches the existing `fix/input-encoding` layout). All file paths below are relative to that worktree root.

---

### Task 1: Worktree setup

**Files:** none (git only)

**Interfaces:**
- Produces: worktree at `C:\Users\Lev\Git\github\alacritree-worktrees\feat\windows-agent-glyphs` on new branch `feat/windows-agent-glyphs` cut from `master`.

- [ ] **Step 1: Create the worktree + branch**

Run (from `C:\Users\Lev\Git\github\alacritree`):
```powershell
git worktree add ../alacritree-worktrees/feat/windows-agent-glyphs -b feat/windows-agent-glyphs master
```
Expected: `Preparing worktree (new branch 'feat/windows-agent-glyphs')`, checkout at `e27e3a0d` or later master.

- [ ] **Step 2: Verify the toolchain works there**

Run (from the new worktree root):
```powershell
cargo check -p alacritree
```
Expected: `Finished` with no errors (warnings from vendored crates are pre-existing and fine).

---

### Task 2: Platform-neutral matching core (TDD)

**Files:**
- Modify: `alacritree/src/session.rs` (helpers go right after `AGENT_PROCESS_GLYPHS` at ~line 124; tests module at end of file — the file currently has no tests module)

**Interfaces:**
- Consumes: `AGENT_PROCESS_GLYPHS: &[(&str, char)]` (exists at `session.rs:117`).
- Produces (all `#[cfg(any(test, windows))]`, module-private):
  - `fn process_tree_pids(procs: &[(u32, Option<u32>)], root: u32) -> Vec<u32>` — pids in the tree rooted at `root`, root inclusive, cycle-safe.
  - `fn agent_glyph_by_name(names: impl IntoIterator<Item = impl AsRef<str>>) -> Option<char>`
  - `fn agent_glyph_by_cmdline(cmds: impl IntoIterator<Item = impl AsRef<str>>) -> Option<char>`

- [ ] **Step 1: Write the failing tests**

Append at the end of `alacritree/src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_walk_collects_root_and_descendants_only() {
        // 1 → {10 → {20 → 30}, 40 → 50}; rooting at 10 must exclude 40's branch.
        let procs =
            [(1, None), (10, Some(1)), (20, Some(10)), (30, Some(20)), (40, Some(1)), (50, Some(40))];
        let mut tree = process_tree_pids(&procs, 10);
        tree.sort_unstable();
        assert_eq!(tree, vec![10, 20, 30]);
    }

    #[test]
    fn tree_walk_includes_root_even_without_children() {
        // A session can be spawned with the agent as the shell program itself.
        assert_eq!(process_tree_pids(&[(7, None)], 7), vec![7]);
    }

    #[test]
    fn tree_walk_survives_cyclic_parent_links() {
        // Snapshot parent data can be stale (pid reuse) and form cycles.
        let procs = [(10, Some(20)), (20, Some(10))];
        let mut tree = process_tree_pids(&procs, 10);
        tree.sort_unstable();
        assert_eq!(tree, vec![10, 20]);
    }

    #[test]
    fn name_match_handles_exe_suffix_and_case() {
        assert_eq!(agent_glyph_by_name(["pwsh.exe", "Claude.exe"]), Some('✳'));
        assert_eq!(agent_glyph_by_name(["cursor-agent.exe"]), Some('❖'));
        assert_eq!(agent_glyph_by_name(["pwsh.exe", "git.exe"]), None);
        assert_eq!(agent_glyph_by_name(std::iter::empty::<&str>()), None);
    }

    #[test]
    fn cmdline_match_catches_runtime_wrappers() {
        let cmd = r"node C:\Users\lev\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js";
        assert_eq!(agent_glyph_by_cmdline([cmd]), Some('✳'));
        assert_eq!(agent_glyph_by_cmdline([r"pwsh.exe -NoLogo"]), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail for the right reason**

Run: `cargo test -p alacritree`
Expected: compile error — `cannot find function process_tree_pids in this scope` (and the two matchers). This is the RED state: the functions don't exist yet.

- [ ] **Step 3: Implement the three helpers**

Insert into `alacritree/src/session.rs` directly after the `AGENT_PROCESS_GLYPHS` const (after line 124):

```rust
/// Pids in the tree rooted at `root` (inclusive), from a `(pid, parent)`
/// snapshot.  Root-inclusive so a session whose spawned program *is* the
/// agent still matches.  Parent links in a snapshot can be stale or cyclic
/// (pid reuse), so the walk tracks visited pids.
#[cfg(any(test, windows))]
fn process_tree_pids(procs: &[(u32, Option<u32>)], root: u32) -> Vec<u32> {
    use std::collections::HashSet;
    let mut tree = vec![root];
    let mut visited: HashSet<u32> = tree.iter().copied().collect();
    let mut cursor = 0;
    while cursor < tree.len() {
        let parent = tree[cursor];
        cursor += 1;
        for &(pid, ppid) in procs {
            if ppid == Some(parent) && visited.insert(pid) {
                tree.push(pid);
            }
        }
    }
    tree
}

/// Match process names against the agent map.  Lowercased `starts_with`,
/// mirroring the Linux `comm` match while tolerating Windows' `.exe`
/// suffix and case-insensitive filenames.
#[cfg(any(test, windows))]
fn agent_glyph_by_name(names: impl IntoIterator<Item = impl AsRef<str>>) -> Option<char> {
    names.into_iter().find_map(|n| {
        let n = n.as_ref().to_ascii_lowercase();
        AGENT_PROCESS_GLYPHS.iter().find(|(name, _)| n.starts_with(name)).map(|(_, g)| *g)
    })
}

/// Match full command lines against the agent map — picks up
/// `node ...\claude-code\cli.js`-style wrappers that hide behind their
/// runtime's name, same as the Linux cmdline pass.
#[cfg(any(test, windows))]
fn agent_glyph_by_cmdline(cmds: impl IntoIterator<Item = impl AsRef<str>>) -> Option<char> {
    cmds.into_iter().find_map(|c| {
        let c = c.as_ref().to_ascii_lowercase();
        AGENT_PROCESS_GLYPHS.iter().find(|(name, _)| c.contains(name)).map(|(_, g)| *g)
    })
}
```

Note: the `O(tree × procs)` walk is deliberate — a process table is a few hundred rows once a second; a children-multimap would be premature.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: `test result: ok. 5 passed`

No commit yet (single feature commit lands in Task 4).

---

### Task 3: Windows probe — dependency, shell pid, sysinfo shim

**Files:**
- Modify: `alacritree/Cargo.toml` (add `[target.'cfg(windows)'.dependencies]` section — currently only unix deps + windows *build*-deps exist)
- Modify: `alacritree/src/session.rs:92-95` (field doc), `:160-168` (`pty_shell_pid`), `:223-228` (non-Linux fallback), new `#[cfg(windows)]` module

**Interfaces:**
- Consumes: `process_tree_pids`, `agent_glyph_by_name`, `agent_glyph_by_cmdline` from Task 2; `pty.child_watcher().pid() -> Option<NonZeroU32>` from vendored `alacritty_terminal` (`tty/windows/child.rs:122`, already `pub`).
- Produces: `#[cfg(windows)] fn foreground_process_glyph(shell_pid: u32) -> Option<char>` — same signature the existing call site (`session.rs:416`) already uses.

- [ ] **Step 1: Add the Windows-only dependency**

In `alacritree/Cargo.toml`, insert between the `[target.'cfg(unix)'.dependencies]` block and `[target.'cfg(windows)'.build-dependencies]`:

```toml
[target.'cfg(windows)'.dependencies]
# Foreground-agent detection: walks the shell's descendant process tree.
# Linux reads /proc directly, so the dependency is Windows-only.
sysinfo = { version = "0.33", default-features = false, features = ["system"] }
```

Run: `cargo check -p alacritree`
Expected: sysinfo downloads and builds. (If `features = ["system"]` is rejected, run `cargo add --dry-run sysinfo@0.33` to list valid features — the process APIs live behind the `system` feature in 0.33.)

- [ ] **Step 2: Wire the Windows shell pid**

Replace `session.rs:165-168`:

```rust
#[cfg(not(unix))]
fn pty_shell_pid(_pty: &alacritty_terminal::tty::Pty) -> Option<u32> {
    None
}
```

with:

```rust
#[cfg(windows)]
fn pty_shell_pid(pty: &alacritty_terminal::tty::Pty) -> Option<u32> {
    // Under ConPTY the PTY child *is* the shell; everything the user runs
    // is spawned beneath it.
    pty.child_watcher().pid().map(std::num::NonZeroU32::get)
}

#[cfg(not(any(unix, windows)))]
fn pty_shell_pid(_pty: &alacritty_terminal::tty::Pty) -> Option<u32> {
    None
}
```

Also update the `shell_pid` field doc (`session.rs:92-95`) — it currently says "None on platforms where we don't yet capture it (Windows)". Replace the last sentence with: `None on platforms where we don't yet capture it.`

- [ ] **Step 3: Implement the Windows probe**

Replace the `#[cfg(not(target_os = "linux"))]` fallback (`session.rs:223-228`):

```rust
#[cfg(not(target_os = "linux"))]
fn foreground_process_glyph(_shell_pid: u32) -> Option<char> {
    // macOS would use `libproc::proc_pidfdinfo` / `tcgetpgrp` on the master
    // FD; Windows is its own world.  Not wired up yet.
    None
}
```

with:

```rust
/// Windows has no foreground process group, so "foreground" is approximated
/// as *any* recognized agent in the shell's descendant tree.  This is what
/// the glyph means to the user — "an agent is running here" — and it stays
/// stable while agents run their own subprocesses, where a deepest-leaf
/// heuristic would flicker.
#[cfg(windows)]
fn foreground_process_glyph(shell_pid: u32) -> Option<char> {
    windows_process_probe::agent_glyph_under(shell_pid)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn foreground_process_glyph(_shell_pid: u32) -> Option<char> {
    // macOS would use `libproc::proc_pidfdinfo` / `tcgetpgrp` on the master
    // FD.  Not wired up yet.
    None
}

#[cfg(windows)]
mod windows_process_probe {
    //! Shared, throttled process-table snapshot.  Every session probes at
    //! its own `AGENT_CACHE_TTL` cadence; keeping one global `System` means
    //! N sessions cost one enumeration per tick, not N.  Two-phase refresh:
    //! names + parent pids for the whole table (one cheap system call
    //! class), command lines only for the shell's descendants and only when
    //! no name matched.
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    use super::{agent_glyph_by_cmdline, agent_glyph_by_name, process_tree_pids};

    /// Slightly under `AGENT_CACHE_TTL` so the first session to tick
    /// refreshes and the rest reuse the same table.
    const SNAPSHOT_TTL: Duration = Duration::from_millis(900);

    static SNAPSHOT: Mutex<Option<(Instant, System)>> = Mutex::new(None);

    pub(super) fn agent_glyph_under(shell_pid: u32) -> Option<char> {
        let mut guard = SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.as_ref().is_none_or(|(at, _)| at.elapsed() >= SNAPSHOT_TTL) {
            let mut sys = guard.take().map(|(_, sys)| sys).unwrap_or_else(System::new);
            sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing(),
            );
            *guard = Some((Instant::now(), sys));
        }
        let (_, sys) = guard.as_mut().expect("snapshot populated above");

        let table: Vec<(u32, Option<u32>)> = sys
            .processes()
            .iter()
            .map(|(pid, p)| (pid.as_u32(), p.parent().map(|pp| pp.as_u32())))
            .collect();
        let tree = process_tree_pids(&table, shell_pid);
        let tree: Vec<Pid> = tree.into_iter().map(Pid::from_u32).collect();

        let names = tree.iter().filter_map(|pid| sys.process(*pid)).map(|p| p.name().to_string_lossy());
        if let Some(glyph) = agent_glyph_by_name(names) {
            return Some(glyph);
        }

        // Names missed: fetch command lines for just the tree to catch
        // agents launched through node/python shims.
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&tree),
            false,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let cmds = tree.iter().filter_map(|pid| sys.process(*pid)).map(|p| {
            p.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
        });
        agent_glyph_by_cmdline(cmds)
    }
}
```

Contingency (compiler-driven, do not guess): if `cargo check` rejects a sysinfo call, the crate version differs from the 0.33 API this code targets — check the error against `cargo doc -p sysinfo --no-deps` (`refresh_processes_specifics` arity, `name()` returning `&str` vs `&OsStr`) and adapt the call sites only; the shape of the probe stays.

Also update the `AGENT_PROCESS_GLYPHS` doc comment (`session.rs:114-116`) to cover both platforms. Replace:

```rust
/// Map a foreground process name (from `/proc/<pid>/comm`) to its static
/// sidebar glyph.  `comm` is kernel-truncated to 15 bytes, so we compare with
/// `starts_with` — `cursor-agent` would otherwise miss.
```

with:

```rust
/// Map a foreground process name (`/proc/<pid>/comm` on Linux, image name
/// on Windows) to its static sidebar glyph.  Compared with `starts_with`:
/// Linux `comm` is kernel-truncated to 15 bytes (`cursor-agent` would
/// otherwise miss) and Windows names carry an `.exe` suffix.
```

And the `agent_cache` field doc (`session.rs:96-98`) says "instead of polling `/proc` every frame" — replace with "instead of polling the process table every frame".

- [ ] **Step 4: Verify everything compiles and tests still pass**

Run: `cargo check -p alacritree && cargo test -p alacritree && cargo clippy -p alacritree -- -D warnings 2>$null; cargo fmt`
Then: `git diff --stat` — only `alacritree/Cargo.toml`, `Cargo.lock`, `alacritree/src/session.rs` should be touched.
Expected: check clean, 5 tests pass, no new clippy warnings in `alacritree` (pre-existing vendored-crate noise is out of scope), fmt makes no unexpected reflows.

---

### Task 4: End-to-end verification and commit

**Files:**
- Modify: none (verification + git)

**Interfaces:**
- Consumes: the full working tree from Tasks 2-3.

- [ ] **Step 1: Live probe verification (real process tree, no GUI)**

The GUI needs a human to eyeball the sidebar, but the probe itself can be exercised headlessly: temporarily add this test at the end of the `tests` module in `session.rs`:

```rust
    /// Scaffolding for manual verification only — exercises the real
    /// sysinfo snapshot against this test process' own tree.
    #[test]
    #[cfg(windows)]
    fn live_probe_smoke() {
        // Our own pid is a valid root; asserts the probe neither panics
        // nor blocks, regardless of whether an agent is running above us.
        let _ = super::windows_process_probe::agent_glyph_under(std::process::id());
    }
```

Run: `cargo test -p alacritree live_probe_smoke`
Expected: PASS (returns without panic; if this cargo test itself runs under a `claude` process tree the probe may even return `Some('✳')` — either is fine).

Then **delete the scaffolding test** and run `cargo test -p alacritree` again — expected: 5 passed.

- [ ] **Step 2: Manual GUI check**

Run: `cargo run -p alacritree` (from the worktree), open a session, run `claude` in it, and confirm the sidebar row shows ✳ (or the live spinner char) within ~2 s; quit claude and confirm it clears within ~2 s. If this session cannot drive a GUI, hand this step to the user as the final acceptance check and say so explicitly in the report.

- [ ] **Step 3: Commit (single feature commit)**

```powershell
git add alacritree/Cargo.toml Cargo.lock alacritree/src/session.rs
git diff --staged --stat
git commit -m @'
feat(windows): show sidebar agent glyphs via process-tree probe

Agent glyphs were Linux-only: pty_shell_pid returned None off-unix and
the probe read /proc for the foreground process group. Windows/ConPTY
has no process groups, so approximate "foreground" as any recognized
agent among the shell's descendants — the glyph means "an agent is
running here", and a descendant scan stays stable while agents run
their own subprocesses.

The shell pid comes from ChildExitWatcher::pid(); the tree comes from
a shared sysinfo snapshot throttled to one enumeration per second,
fetching command lines only for the shell's descendants when image
names don't match. The tree walk and matching are platform-neutral
and unit-tested; sysinfo is a Windows-only dependency, /proc on Linux
is untouched.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
'@
```

Before committing, review `git diff --staged` and confirm the diff does what the message says (user rule).

- [ ] **Step 4: Update local tracking**

In the **main checkout** (`C:\Users\Lev\Git\github\alacritree`), append a status line to `docs/specs/planned_features.md` under the Windows-support findings noting: agent glyphs implemented on `feat/windows-agent-glyphs` (any-descendant sysinfo probe, 5 unit tests), pending user GUI verification / push decision. Do not push or open a PR — user decides (global rule).
