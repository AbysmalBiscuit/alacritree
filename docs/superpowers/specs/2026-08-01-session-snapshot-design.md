# Session snapshot and restore

Date: 2026-08-01
Revised: 2026-08-01 after a codex review against the code.

Restore a window's tab layout after a crash, a reboot, or an upgrade restart.
Child processes are not preserved — shells respawn fresh in their workspace.
Each tab's last visible screen is seeded into the restored tab as plain text so
you can read where you left off.

## Branching

Branch off the tip of the current stacked-PR chain, not off `master`.

- Base: `origin/fix/sidebar-row-tooltips` (PR #165, marked `[6]`), at
  977d01b0 as of 2026-08-01. Re-check the tip before branching — the stack
  grows.
- Branch name: `feat/session-snapshot`, in a worktree under
  `../alacritree-worktrees/feat/session-snapshot`.
- PR title carries the next stack marker: `feat(session): snapshot and restore
  a window's tabs [7]`.
- The PR opens against `mathix420/alacritree` `master`, matching every other
  PR in the stack, and merges into `all-features` afterwards.

## Scope

In:

- Per-window snapshot file, written continuously while the app runs.
- A `RestoreSession` action that opens a picker and rebuilds tabs into the
  current window.
- Plain-text seeding of each restored tab with its captured screen.
- Config gate, default off.

Out:

- Preserving running processes. Restored shells are new processes.
- Automatic restore on launch. Restore is always explicit.
- Relaunching agents (`claude --continue` and friends).
- Restoring diff panes, which are throwaway by design.
- Scrollback above the visible screen.
- Detecting whether another window is live. See "Liveness, deliberately
  omitted".

## Storage

`state.rs` is built so that a window never writes a whole snapshot: every
window shares `state.toml`, so writes go through `mutate`, which re-reads the
file and changes one field. Session layout is per-window and has no such
sharing problem, so it lives in its own file rather than growing
`PersistedState`.

One file per window at `$CONFIG/alacritree/snapshots/<window-id>.toml`,
written tmp-first and renamed into place. `state::save_to` already does that
dance (`state.rs:170`); extract it into a shared helper both call. It does not
`fsync` the temp file or the directory, and this feature does not add that —
the durability promise is "survives a process crash", not "survives power
loss".

The alternative — a small layout file plus one screen sidecar per tab — was
rejected. With 50 lines of roughly 80 characters across ten tabs the whole
file is about 40 KB, and rewriting 40 KB every five seconds costs nothing
worth the extra files and their orphan-cleanup problem.

```toml
window = "a3f1c0de-…"
pid = 48212
saved_at = 1785312000
current_workspace = "C:/Users/Lev/Git/github/alacritree"
capture_columns = 120

[[sessions]]
project_root = "C:/Users/Lev/Git/github/alacritree"
workspace = "C:/Users/Lev/Git/alacritree-worktrees/feat/x"
title = "claude"
kind = "shell"
active = true
screen = """
$ cargo test -p alacritree
test result: ok. 412 passed
"""

[sessions.launch]
kind = "profile"
name = "wsl-kali"
```

- `window` is minted once at startup and stable for that window's life.
- `current_workspace` records `AlacritreeApp::current_workspace` (`app.rs:858`).
  A per-session `active` flag identifies each workspace's active tab but cannot
  say which workspace was selected; both are needed.
- `project_root` is the root of the project that owns this workspace, recorded
  separately because `workspace` may be a linked worktree. `Project::discover`
  keeps whatever root it is handed (`projects.rs:90`), so discovering a
  worktree path would create a second, wrongly-rooted project.
- `workspace` absent means the home tab.
- `capture_columns` is the grid width at capture time, so restore can decide
  whether to truncate.
- `[sessions.launch]` is the launch descriptor; see below.
- Array order is tab order.

Fields are `#[serde(default)]` throughout, so a snapshot written by an older
build still parses.

### The launch descriptor requires a new `Session` field

`Session` (`session.rs:141`) stores no record of how it was launched, and
`spawn_profile_session` (`app.rs:922`) bypasses `resolve_shell` entirely, so a
profile-spawned tab's identity exists nowhere after spawn. Re-resolving at
restore time would silently relaunch such a tab with the project default.

So: add `launch: LaunchDescriptor` to `Session`, set by
`spawn_session_with_shell` and `spawn_profile_session`, with variants
`Default` (resolve against the project as usual), `Profile { name }`, and
`Shell { choice }` for an explicit `ShellChoice`. Restore dispatches on it:
`Profile` goes back through `profile_session_shell`, `Shell` through the WSL
resolution helper, `Default` through `resolve_shell`.

WSL launches must go through those helpers rather than replaying argv, because
`Session::spawn` registers a `WslProbe` (`session.rs:1195`) that it
unregisters on drop (`session.rs:1451`). A hand-built argv would produce a
session with no probe, and the sidebar would read every idle WSL shell as busy.

## Capture

Driven from `AlacritreeApp::update`:

- A layout dirty flag, set by spawn, close, move, reorder, activate, and
  workspace switch. Debounced at roughly 250 ms.
- A capture tick every 5 s.

The tick hashes the **whole normalized snapshot** — titles, order, active
flags, kinds, launch descriptors, workspaces, and screens — not just the screen
text. Titles change through `TermEvent::Title` (`session.rs:1460`) without any
layout mutation, and agent spinners make that constant; hashing screens alone
would miss it.

Screen text comes from `screen_snapshot(0)`. The argument is scrollback lines
*added above* the visible screen (`session.rs:1408-1411`), so `0` means "the
visible screen, and nothing more" — which is what this feature wants. The
`screen_lines` config value is then applied as a hard cap on the *tail* of the
returned lines, plus a byte cap, after capture. Scratchpad sessions ignore the
argument and return the entire document (`session.rs:1391`); they are never
captured as screen text at all (see below).

`screen_snapshot` reads the live unscrolled screen regardless of the user's
display offset, by design (`session.rs:1386`). A snapshot therefore records
where output actually is, not what a user scrolled back to. That is the
intended behavior and the spec does not change it.

Cost control, because `screen_snapshot` allocates every line while holding the
terminal lock (`session.rs:1403`) and `terminal_view.rs:769` already warns that
time under that lock stalls PTY parsing:

- Captures are staggered across frames — at most a few sessions per frame —
  rather than every session in one tick.
- A session whose lock is contended is skipped and retried on the next tick.
- Line buffers are reused between captures, as `terminal_view.rs:127` does.

`steady_state.rs` asserts that an unchanged frame allocates nothing
(`steady_state.rs:111`). The tick check on an ordinary frame must therefore be
a plain instant comparison with no allocation; allocation happens only on a
capture tick. Add steady-state coverage for both the disabled and the
idle-enabled configuration.

The snapshot value is built on the UI thread, where the terminal locks can be
taken, and handed to a single writer thread. The handoff is a
`Mutex<Option<Snapshot>>` plus a condvar, not a `sync_channel(1)` — a bounded
channel with `try_send` retains the *oldest* queued value, which is backwards
here. The writer replaces any pending value, so a burst collapses to one write.
On clean shutdown the app flushes the pending value and joins the writer.

Loss window on a hard kill: up to about 5 s of screen text and up to about
250 ms of layout change.

## Restore

`NamedAction::RestoreSession` is unbound by default. There is no sidebar entry
point and no launch-time prompt.

Adding the action touches more than the enum. All of these need an entry:
`NamedAction` and its description/config-name mapping (`bindings.rs:149`), the
binding parser (`bindings.rs:797`), palette registration
(`command_palette.rs:218`), app dispatch, the modal-open gate (`app.rs:1461`),
and dialog rendering (near `app.rs:7387`). `NamedAction` is also reachable over
IPC through `RunAction` (`ipc.rs:110`), so when the feature is disabled the
action must be rejected there and as a keybinding too — hiding the palette row
is not enough.

The picker is an `egui::Modal`, constructed the way `show_delete_dialog`
(`app.rs:5938`) is. Rows are snapshots, newest first, labelled from the
workspaces they contain, the tab count, and their age — "3 workspaces · 7 tabs
· 4 minutes ago". Selecting a row previews the captured screen of its active
tab. Enter restores, Delete removes a snapshot, Esc closes.

Restoring closes the modal. The snapshot file is kept, and the window remembers
which snapshots it has restored, so reopening the picker shows those rows
marked "restored". Restoring the same snapshot twice duplicates tabs; the mark
is the only guard, and that is deliberate, since restoring one layout into two
windows is legitimate.

### Restore runs as a state machine, not a loop

`spawn_session_with_shell` calls `Session::spawn` synchronously (`app.rs:842`),
and `Project::discover` is slow enough that the app already refreshes it on a
worker (`app.rs:374`). Restoring a dozen tabs and several projects in one frame
would freeze the window for seconds.

So restore is a small state machine driven from `update`:

1. Collect the distinct `project_root` values, dedupe against projects already
   present, and discover the missing ones on the existing refresh worker.
2. As each project lands, merge its persisted metadata from `state.toml` —
   `shell`, `label`, `expanded` — onto the discovered `Project` before anything
   calls `persist_project`. A freshly discovered project has
   `shell_override: None` (`projects.rs:121`), and `persist_project`
   (`app.rs:739`) writes that back over the stored override, silently erasing
   it.
3. Spawn a bounded number of sessions per frame, in recorded order, requesting
   a repaint each time.
4. When every session is placed, apply each workspace's `active` tab, then set
   `current_workspace`.

Failures are collected per session and reported together in a summary line
rather than aborting the restore.

Because restore mutates projects and sessions across frames, each frame's
mutations must complete before that frame's sidebar-focus reconciliation
(`app.rs:1811`, and the post-dialog pass at `app.rs:7416`). The restore modal
participates in the modal-open gate while it is up.

### Seeding, not replaying

The captured screen is fed into the terminal **before the PTY exists**, not
after `Session::spawn` returns. `Session::spawn` calls `tty::new` at
`session.rs:1185`, which starts the child, and `event_loop.spawn()` at
`session.rs:1193`. Both happen before it returns, so anything written
afterwards races the shell's first output — and `FairMutex` (`sync.rs:24`)
only makes lock acquisition fair, it imposes no ordering between the seed and
the PTY reader that acquires the same lock while parsing
(`event_loop.rs:116`).

So `Session::spawn` gains a `seed: Option<&str>` parameter, applied to the
`Term` constructed at `session.rs:1159` — after `Term::new`, before
`tty::new`. At that point the session is single-threaded and no PTY reader
exists, so ordering is guaranteed by construction rather than by scheduling.

Seed encoding:

- Lines are joined with `\r\n`. `ScreenSnapshot` trims trailing blanks
  (`session.rs:1410`) and a bare `\n` moves down without returning to column
  zero.
- Text is sanitized: C0 and C1 control bytes and `ESC` are stripped, so a
  recorded screen can never inject escape sequences into the fresh terminal.
- The separator is emitted with explicit SGR set and reset around it, so it
  cannot leak attributes into the shell's output:
  `── restored 2026-08-01 14:03 ──`.
- Sessions spawn at a hard-coded 80×24 (`app.rs:846`) and are resized to fit by
  `terminal_view` on the first paint. Lines longer than the seeding grid are
  truncated to `capture_columns` and not reflowed; the seed is a readable
  record, not a faithful reconstruction.

### Scratchpads are restored, never seeded

The app allows one scratchpad per workspace and finds it through
`scratchpad_session_index` before creating one (`app.rs:857`); the editor
autosaves (`scratchpad.rs:87`). Two editors over one file would race their
autosaves and one would win with stale content.

So a snapshot records only that a workspace *had* a scratchpad open. Restore
skips it when that workspace already has one, and otherwise goes through the
normal `spawn_scratchpad` / `scratchpad::ensure_file` path (`app.rs:875`). Its
text is never captured and never seeded — the file on disk is the truth.

`SessionKind::Scratchpad` holds a plain `PathBuf` (`session.rs:133`) with no
absoluteness invariant, so the snapshot stores the workspace and lets
`ensure_file` derive the path, rather than recording a path that may not
resolve.

### Missing workspaces

A workspace that is merely unreachable is not a workspace that is gone.
`state.rs:111` already draws that line: only a conclusive `NotFound` counts,
while a permission error or a sleeping WSL mount is inconclusive. Restore uses
the same rule — conclusively-missing workspaces are skipped and reported;
inconclusive ones are attempted, and their spawn failure is reported per
session. WSL reachability checks happen on the worker, never inline.

## Liveness, deliberately omitted

An earlier draft hid snapshots belonging to still-running windows, using the
IPC socket as the oracle. That was wrong: the socket name contains only the pid
(`ipc.rs:495`), so a reused pid produces the same name, and on Unix a stale
socket file needs an actual connection attempt to disprove (`ipc.rs:456`).
Doing it properly needs a handshake returning a window nonce — a new IPC
request, and a connection attempt per row when the picker opens.

It is not worth it. Restore is explicit, and restoring another live window's
snapshot is harmless: it copies those tabs into your window, which is a
reasonable thing to want. So:

- The picker hides exactly one file — the current window's own, which it knows
  by path without asking anyone.
- Every other snapshot is listed with its age. A live window's snapshot looks
  like a very recent one, which is honest.
- Pruning is by age only (30 days), never by count, so it cannot delete a live
  window's file out from under it.
- A writer whose own target file has vanished forces a full rewrite on the next
  tick, rather than skipping because its cached hash still matches.

## Config

Under `[ui]` in `alacritree.toml`, since this is an alacritree-only option:

```toml
[ui.session_restore]
enabled = false
screen_lines = 50
max_screen_bytes = 16384
keep_days = 30
```

`enabled = false` captures nothing, writes nothing, hides the palette row, and
rejects the action from keybindings and IPC, so a build that does not opt in
behaves exactly as it does today. `screen_lines = 0` keeps the layout and drops
the screen text.

## Known limits

1. The shell's live working directory is not captured. Nothing in the crate
   handles OSC 7 — `Session::working_directory` is the workspace key, not the
   child's cwd — so a restored tab opens at the workspace root even if you had
   changed directory below it. Capturing the real cwd, by handling OSC 7 or by
   probing `shell_pid`, is a follow-up.
2. Only the visible screen is captured, and only as text. Scrollback above the
   fold, colors, and attributes are lost.
3. Diff panes are not restored.
4. Home tabs (`workspace` absent) inherit `$PWD` at spawn. Restoring into a
   window launched from a different directory puts them somewhere else. This
   drift is accepted rather than recorded.
5. A WSL session whose distro has gone away fails to spawn, readably, as it
   already does.
6. Long lines are truncated to the capture width rather than reflowed.

## Testing

Unit tests in `snapshot.rs`:

- TOML round-trip, including a multi-line `screen` value and each
  `LaunchDescriptor` variant.
- A snapshot missing newer fields still parses.
- Pruning drops only snapshots older than `keep_days` and never the current
  window's own file.
- Diff sessions are excluded; a scratchpad session records no screen text.
- A title change with an unchanged grid still produces a write.
- The writer mailbox keeps the newest pending value, not the oldest.
- Seed sanitization strips `ESC` and C0/C1 bytes and joins lines with `\r\n`.

Integration:

- Seed a `Term` through the new `Session::spawn` parameter and assert the text
  is in the grid above the shell's first output, run enough times to catch an
  ordering regression.
- Restoring into a workspace that already has a scratchpad does not create a
  second one.
- Restoring a project already present in `state.toml` with a `shell` override
  leaves that override intact afterwards.
- A restore of N sessions spans multiple frames and never blocks one frame for
  more than a bounded spawn count.
- `steady_state` passes with the feature disabled, and with it enabled on idle
  frames between capture ticks.

Manual, in a release build:

1. Enable `[ui.session_restore]`, open three workspaces with two tabs each,
   including one WSL tab and one profile-spawned tab, and run something in each
   so the screens differ.
2. `taskkill /f /im alacritree.exe`.
3. Relaunch, Ctrl+K, "Restore session", pick the snapshot.
4. Verify workspace set, tab order, active tab per workspace, restored current
   workspace, that the WSL tab's sidebar activity glyph behaves (proving its
   probe registered), that the profile tab came back as that profile, and that
   each tab shows its captured screen above the prompt.
5. Confirm that with `enabled = false` no snapshot file is written, the palette
   row is gone, and `alacritree action RestoreSession` over IPC is rejected.
