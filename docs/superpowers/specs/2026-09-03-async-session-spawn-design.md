# Async session spawn

Issue: AbysmalBiscuit/alacritree#29 (parent #22)
Branch: `perf/async-spawn`, off `fix-selecting-text-near-the-left-side-of-the` (PR #206), marker `[5]`

## Problem

Opening a session blocks the frame that asked for it. `Session::spawn_with` calls `tty::new` and then `EventLoop::new` synchronously inside the egui update, so the click or keystroke that spawns a shell pays for conpty process creation before the frame can return.

Measured with `ALACRITREE_FRAME_LOG=1`, this costs 8-12ms idle and reads as a stutter. Under 16 burners on a 16-core machine the spawn phases account for whole frames with nothing left over: `spawn pty [3]: 211.6ms`, `slow frame: 258.5756ms`.

Three entry points reach it, all funnelling into `spawn_session_with_shell`: sidebar click handlers, keyboard navigation through `dispatch_action`, and binding dispatch as `SpawnNewInstance`. Because the cost is charged to whatever phase happened to be running, it has been misattributed to sidebar paint before.

This is not the same problem as #25. That one is about a child not being scheduled once it exists. This one is about the parent blocking while it creates the child, and it persists whatever priority the child ends up at.

Out of scope: spawn-to-prompt latency. That wait already happens off the UI thread. Moving PTY creation stops the shell's start from freezing the window; it does not make the shell start.

## Approach

`Session::spawn_with` splits into three pieces that can run on different threads, plus one for the result nobody wants any more.

- `Session::pending(...) -> (Session, OpenRequest)` — ids, `Term`, `EventProxy`, env, `PtyOptions`, `window_id`. No IO, infallible, cheap enough for a frame. It takes the caller's real `size` and `cell_size` rather than the hardcoded 80x24 the call sites pass today, so the PTY is born at the geometry it will keep instead of being reflowed on attach.
- `session::open(OpenRequest) -> io::Result<Attachment>` — `ensure_working_directory`, `harden_dll_search_path`, `tty::new`, `pty_shell_pid`, `PriorityJob::adopt`, `RearmingPty`, `EventLoop::new`, `EventLoop::spawn`. Everything that costs milliseconds, in one `Send` function.
- `Session::attach(Attachment)` — fills in `shell_pid`, `priority_job`, `notifier` and `sender`, registers the WSL probe, replays the session's current size, and flushes buffered input.
- `impl Drop for Attachment` — `Msg::Shutdown` then drop, for an attachment whose tab closed while it was opening. A `Drop` rather than an `abandon()` method so a `PendingSpawns` dropped at quit, or a receiver that disconnects, cleans up without a caller remembering to.

`Session::spawn` and `Session::spawn_command` keep their signatures and become `pending` + `open` + `attach` back to back. That is also the path taken when the config gate is off, so both settings run the same three functions and disagree only about which thread `open` runs on. One implementation, not two.

`OpenRequest` carries the `PtyOptions`, the `WindowSize`, the `window_id`, clones of the `Arc<FairMutex<Term>>` and the `EventProxy` that `EventLoop::new` needs, and the two `[ui]` flags `PriorityJob::adopt` reads (`focus_priority_boost`, `reap_descendants_on_close`).

`PriorityJob` is `!Send` on Windows: it owns a raw `HANDLE` and nothing declares otherwise. `adopt` cannot move to the UI thread — it needs a pid that only exists after `tty::new`, and the job must exist before the shell starts anything — so `focus_priority::windows` gains `unsafe impl Send for PriorityJob`, justified by the handle being a process-wide kernel object. Its `Cell<bool>` keeps the type `!Sync`, which is correct. The vendored `Conpty` carries the same declaration for the same reason.

### Bookkeeping

A new `pending_spawn.rs` follows `project_refresh.rs`: the same `start` / `watch` / `poll` shape, for the same reason. It maps `SessionId` to the attachment receiver and any parked IPC reply channels. It does not cache the session's workspace; `move_session_to` can re-key a pending record, so the poll reads the workspace off the record it finds.

`spawn_session_with_shell` pushes the pending session, returns its id inside the frame, and hands `open` to a detached thread — one per spawn, as project refresh does, so opening several tabs at once does not serialize them. The worker calls `ctx.request_repaint()` after sending, exactly as `refresh_project` does. Without it the poll waits for whatever wakes the loop next, which under saturation is the shell's own first output seconds later, and a parked IPC reply would hit the 10s `APP_REPLY_TIMEOUT` while the session it asked for is sitting there working.

`poll_pending_spawns()` runs in `update` beside `poll_project_refreshes` and resolves each finished open:

- a session record with that id is still in `self.sessions` and has no `sender` yet: `attach`, answer waiters with the id
- no such record, because the tab was closed while it was opening: drop the `Attachment`, so the tab neither reopens nor leaks its child
- `Err(e)`: unwind the record as described below, then `report_spawn_failure(ctx, &ws, &e)` and answer waiters with the error

### Unwinding a failed open

Failure that arrives after the frame has returned cannot be a return value. `spawn_session_with_shell` keeps its `io::Result<SessionId>` for the failures it can still see synchronously — a worktree git has forgotten, a profile name with no profile — and everything `open` can fail at (a `[shell] program` that does not exist, a profile binary off PATH, `ensure_working_directory`) reaches the user through `report_spawn_failure` from the poll, with the text it reports today.

Unwinding is not `close_session` and not a bare `retain`. Both are wrong:

- `close_session` applies the `last_session_close = "respawn"` policy, which spawns a replacement. That replacement is another pending record whose open fails the same way, which closes and respawns again: an unbounded loop, one iteration per open, each raising `error_dialog`. Today this cannot happen, because a synchronous `Err` returns before any record exists.
- A bare `retain` leaves `active_session` pointing at an id that is gone. `adopt_active_session` self-heals only for `current_workspace`, so a spawn into another workspace — the sidebar's profile menu, IPC `create_session` — leaves a dangling entry that `workspace_activity` and `ListSessions`'s `is_active_tab` then read.

So: remove the record, repair `active_session` through `close_landing`, and stop. No respawn, and no `close_fallback` navigation either. Navigating on a failed open is the same loop wearing a different hat: `close_fallback` lands on a workspace, `ensure_active_session` finds it empty and spawns into it, that open fails the same way, and the pair go round forever. The pane shows the "no session" placeholder instead, which is what the workspace honestly holds. The sidebar's "hand the workspace back on failure" behaviour is gone: by the time the failure arrives the workspace switch has already happened.

### The focus-priority pass

`process_session_events` calls `set_priority_boost` on every session and feeds the result into `set_self_boosted(anything_raised)`. A pending session has no `priority_job` and answers `false`, so a frame whose visible session is still opening would demote the GUI to `NORMAL_PRIORITY_CLASS` for the length of the open, and re-raise it on attach. Under load that is exactly the window the issue is about.

The pass has to distinguish "no job yet" from "no job ever": a session counts as wanting the boost while it is pending, so a spawn no longer toggles the self-boost twice.

### IPC and MCP

`Req::CreateSession` answers with a session id today, and its failures reach the client as a failed request. It parks its reply channel through `watch` and is answered on attach, so an agent that calls `create_session` and then `send_text` cannot race its own PTY, and a spawn that fails still fails the call. The connection thread is already blocked waiting for a reply, so nothing new blocks.

`handle_ipc_request` has no reply channel to park. The claim happens before dispatch in `process_ipc_calls`, beside the one `RefreshProject` already makes, and the match arm answers with the same "was not deferred" error if it is ever reached without one.

### Three things the pending state breaks

`Session::write` no-ops without a notifier, so input typed during the gap would vanish. A `pending_writes: Option<Vec<u8>>` is `Some` only between `pending` and `attach` — never for scratchpad sessions, which are also notifier-less — and `attach` writes it through before anything else.

`Session::resize` returns before `self.term.lock().resize(size)` when there is no sender, so a pending session's `Term` would hold the size it was built with while the view resizes around it. The `Term` resize moves above the sender check, and `attach` sends one `Msg::Resize` for the size the session ended up at.

`wsl_helper::register_probe` runs in `attach`, not in `open`. `Session::drop` is the only `unregister_probe`, so a probe registered by a worker whose record has already been dropped is never removed: the key stays in `probe_cache` and the poller asks the helper about a dead pidfile every second for the life of the process. Registration is a HashMap insert and a `Once`; nothing about it needs a worker.

## Scope

Every PTY-creating path goes through `pending` / `open` / `attach`: shells, named profiles, WSL shells, the git sidebar's diff panes (`Session::spawn_command`), and the session created during app construction. `Session::spawn_scratchpad` has no PTY and is untouched.

## Known race, not fixed

`run_pending_delete` drops a workspace's session records and then runs `git worktree remove` on a worker. A pending record's `Drop` does nothing, while its `open` may be mid-`CreateProcess` with a cwd inside that directory, so the shell is born holding the directory and lives until its attachment returns and is dropped. Today's close is already fire-and-forget — a `Msg::Shutdown` with no join — so the delete already races shell teardown; this widens the window by the open time rather than introducing the race. Not worth a fix here.

## Config

`[ui] async_session_spawn`, default false, so the default experience does not move. Documented with a doc comment on the `RawUi` field, which is the hover text the published JSON Schema carries; regenerate with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.

## Files

- `alacritree/src/session.rs` — `pending`, `OpenRequest`, `open`, `Attachment` and its `Drop`, `attach`; `spawn` / `spawn_command` rebuilt on them; `write` buffering; the `resize` reorder; `register_probe` moved into `attach`.
- `alacritree/src/pending_spawn.rs` (new) — `PendingSpawns` with `start`, `watch`, `poll`, and its unit tests.
- `alacritree/src/app.rs` — `spawn_session_with_shell` posts the open and repaints, `poll_pending_spawns` adopts results and unwinds failures, the priority pass counts a pending session as wanting the boost, `process_ipc_calls` claims the `CreateSession` reply.
- `alacritree/src/focus_priority/windows.rs` — `unsafe impl Send for PriorityJob`.
- `alacritree/src/config.rs` — `async_session_spawn` on `Ui` and `RawUi`.
- `alacritree/src/frame_log.rs` — `spawn_phase`, ported from `perf/load-latency`.
- `alacritree/src/main.rs` — `mod pending_spawn`.
- `schema/alacritree-config.json` — regenerated.

## Testing

Unit tests in `pending_spawn.rs`, mirroring `project_refresh.rs`: an attachment adopted for a live session, one dropped for a session that no longer exists, a waiter answered with the error when the open fails.

Session tests:

- `open` on a worker, then `attach`, proving a byte written between `pending` and `attach` reaches the shell, and that bytes written before attach arrive before bytes written after.
- `pending` → `resize` → `open` → `attach`, asserting the child sees the resized geometry. The `resize` reorder has no test today.
- On Windows, that dropping an `Attachment` takes the child with it, built on the process-survival helpers already used for conpty teardown.
- That a closed pending WSL record leaves no key in `probe_cache` once its attachment is dropped.

App-level tests, against the failure modes this design creates:

- A failed open leaves no `active_session` entry pointing at the dead id, and does not spawn a replacement under `last_session_close = "respawn"`.
- A frame whose visible session is still pending leaves the self-boost where it was. This needs the priority pass factored out of `process_session_events` into the egui-free style the sidebar models already use.

A `steady_state` check that polling an empty `PendingSpawns` allocates nothing, since the reconcile it sits beside runs every frame.

### What tests cannot cover

Two acceptance criteria need 16 burners on this machine and a human reading `ALACRITREE_FRAME_LOG=1`: that spawning from a sidebar row, a keyboard binding, and the command palette each return within a frame under saturation, and that no `slow frame` is attributable to `spawn pty` or `spawn open`. Porting `spawn_phase` is what makes the second one observable at all. Neither is asserted by a test, and the work is not done until both have been run by hand.
