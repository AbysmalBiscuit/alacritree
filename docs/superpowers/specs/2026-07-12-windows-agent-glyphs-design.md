# Windows agent glyphs — design

Branch: `feat/windows-agent-glyphs` (worktree off `master`, PR upstream to
mathix420/alacritree). Spec is local-only, not committed; the PR description
carries the context.

## Problem

The sidebar agent glyph (`Session::agent_glyph`) identifies which agent CLI
(claude, codex, gemini, aider, cursor-agent, continue) is running in a
session by probing the PTY's foreground process. The probe is Linux-only:

- `pty_shell_pid` returns `None` on non-unix (`session.rs:165-168`), so
  Windows sessions never even have a shell PID.
- `foreground_process_glyph` reads `/proc/<pid>/stat` (tpgid) then
  `/proc/<tpgid>/comm` + `cmdline`; the `#[cfg(not(target_os = "linux"))]`
  arm returns `None`.

On Windows the glyph falls back to title-decoration heuristics only, which
miss agents that don't decorate their titles. Goal: full glyph parity on
Windows. macOS stays out of scope (still `None`).

## Decisions

### Shell PID (Windows)

`alacritty_terminal`'s Windows `Pty` exposes
`child_watcher() -> &ChildExitWatcher`, and `ChildExitWatcher::pid() ->
Option<NonZeroU32>` (`alacritty_terminal/src/tty/windows/child.rs:122`).
Replace `pty_shell_pid`'s `#[cfg(not(unix))]` arm with a `#[cfg(windows)]`
arm returning `pty.child_watcher().pid().map(NonZeroU32::get)`, plus a
`#[cfg(not(any(unix, windows)))] → None` fallback arm. No vendored-crate
edits needed. Under ConPTY the PTY child is the shell itself; user-launched
processes are its descendants.

### Foreground semantics: any-descendant (user-confirmed 2026-07-12)

Windows/ConPTY has no foreground process group (no tpgid equivalent). The
glyph shows when **any** descendant of the shell matches the agent list.
Rationale: the glyph means "an agent is running in this session", and
any-descendant doesn't misfire while an agent runs its own subprocesses
(bash/node children), which a deepest-descendant heuristic would.
Divergence from Linux: a backgrounded agent would still show a glyph —
acceptable; Windows shells effectively don't background jobs Unix-style.

### Enumeration: sysinfo, two-phase, shared snapshot

New dependency `sysinfo` under `[target.'cfg(windows)'.dependencies]`,
default features off, enabling only the feature(s) the process APIs need
(exact flag names verified against the chosen sysinfo version at
implementation time). Linux keeps `/proc` (no new dep
there); this mirrors the hybrid pattern used for font fallback.

Probe shape (`#[cfg(windows)] foreground_process_glyph`):

1. Consult a process-global snapshot cache (a `Mutex`-guarded
   `sysinfo::System` + `Instant`), refreshed only when older than ~900 ms —
   just under the per-session 1 s `AGENT_CACHE_TTL`, so N sessions share one
   enumeration per tick instead of doing N.
2. Phase 1 (cheap): refresh all processes with names + parent PIDs only
   (single system call class), build the descendant set of `shell_pid`,
   match names against `AGENT_PROCESS_GLYPHS` with `starts_with` (handles
   `claude.exe`).
3. Phase 2 (only if phase 1 found no match): refresh cmdlines for just the
   descendant PIDs (`ProcessesToUpdate::Some`), match with `contains` —
   catches `node C:\...\claude\cli.js`-style wrappers. Same
   comm-then-cmdline order as Linux.

Everything downstream (`AgentCache` TTL, `agent_glyph()`'s preference for
the title's live spinner char over the static glyph) is unchanged.

### Structure & testing

The descendant-walk + matching core is a pure, platform-neutral function
over `(pid, parent_pid, name, cmd)` records so it unit-tests on any OS
(TDD; written RED first). The sysinfo glue is a thin `cfg(windows)` shim
around it. All code stays in `session.rs` next to the Linux probe.

Edge cases: PID reuse / stale parent links / vanished processes degrade to
"no glyph" — never panic, never block. Cycle-safety in the parent-link walk
(visited set or depth bound) since snapshot parent data can be stale.

## Out of scope

- macOS probe (would be `libproc`/`tcgetpgrp`; comment stays).
- Any change to the agent list or title heuristics.
- Persisting/configuring the glyph map.
