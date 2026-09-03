# Async session spawn implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opening a terminal session stops blocking the egui frame that asked for it.

**Architecture:** `Session::spawn_with` splits into `pending` (cheap, on the UI thread), `open` (everything that blocks, `Send`, runnable on a worker) and `attach` (adopt the result). A `pending_spawn.rs` bookkeeping module modelled on `project_refresh.rs` holds the in-flight opens and is polled once per frame. Behind `[ui] async_session_spawn` (default false) the open runs on a detached thread; with the gate off the same three functions run back to back, so there is one implementation rather than two.

**Tech stack:** Rust 2024, MSRV 1.85. egui/eframe, `alacritty_terminal` (vendored, read-only), `windows-sys` 0.59.

**Design spec:** `docs/superpowers/specs/2026-09-03-async-session-spawn-design.md` in the main checkout. Read it before Task 1; it carries the reasoning this plan only executes.

**Issue:** AbysmalBiscuit/alacritree#29 (parent #22).

## Global constraints

- All work lives in `alacritree/`. `alacritty/`, `alacritty_terminal/`, `alacritty_config/`, `alacritty_config_derive/` and `egui-winit/` are vendored and read-only. A change that requires editing them is a blocker, not a licence.
- The test command in this checkout is `cargo nextest run -p alacritree`, not `cargo test`.
- Never run bare `cargo fmt`. It reformats the whole workspace including vendored crates. Format one file at a time: `rustup run nightly rustfmt --edition 2024 alacritree/src/session.rs`. `session.rs` and `app.rs` carry pre-existing rustfmt hunks that are not yours to fix; leave them.
- Never use `git stash`. Several agents share this repository. To read another revision use `git show REV:path`.
- Write absolute paths into commands rather than `cd`-ing first. Use `git -C <abs path>` for git.
- Comments explain *why*, not *what*, and are timeless: no `this PR`, no `now we`, no `used to`, no issue numbers, no RED/GREEN narration. Do not delete existing comments unless the change makes them wrong.
- New behaviour is opt-in. `[ui] async_session_spawn` defaults to false and nothing observable changes with it off.
- Every commit is a Conventional Commit with an imperative subject under ~72 chars, and ends with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Do not push, do not open a PR. That is the human's call.

---

### Task 0: Worktree

**Files:** none in the repository.

- [ ] **Step 1: Create the worktree**

```sh
devkit issue setup 29 --slug perf/async-spawn
```

It prints JSON; the `worktree` field is the absolute path. Every path below is relative to it.

- [ ] **Step 2: Re-point the branch onto its real base**

`devkit issue setup` always cuts from `origin/master`. This branch stacks on PR #206, which is not merged:

```sh
git -C ../alacritree-worktrees/perf/async-spawn fetch https://github.com/AbysmalBiscuit/alacritree.git
git -C ../alacritree-worktrees/perf/async-spawn reset --hard fix-selecting-text-near-the-left-side-of-the
```

Do this before the branch has any commits of its own.

- [ ] **Step 3: Confirm the tree is green before you touch it**

```sh
cargo nextest run -p alacritree
```

Expected: PASS. If it does not pass here, stop and report — you have not broken anything yet, and you need to know that.

---

### Task 1: Spawn phase timing

The `spawn pty` and `spawn open` markers the issue quotes live on the `perf/load-latency` branch and not on this one. Without them acceptance criterion 5 cannot be observed at all, so they come first: the baseline they measure is the before half of the measurement.

**Files:**
- Modify: `alacritree/src/frame_log.rs`

**Interfaces:**
- Produces: `pub fn spawn_phase(session: Option<u64>, phase: &str, elapsed: Duration)`. Task 2 calls it.

- [ ] **Step 1: Add the helper**

At the end of `frame_log.rs`, before the `#[cfg(test)] mod tests`:

```rust
/// One phase of opening a session, logged only under `ALACRITREE_FRAME_LOG`.
///
/// Spawn cost is charged to whichever frame phase happened to be running when
/// the click arrived, so without a marker of its own it reads as a sidebar or
/// shortcut problem.  The session id is what pairs a phase with the tab it
/// belongs to when several are opening at once.
pub fn spawn_phase(session: Option<u64>, phase: &str, elapsed: Duration) {
    if !enabled() {
        return;
    }
    let millis = elapsed.as_secs_f64() * 1000.0;
    match session {
        Some(id) => log::info!("spawn {phase} [{id}]: {millis:.1}ms"),
        None => log::info!("spawn {phase}: {millis:.1}ms"),
    }
}
```

Check the imports at the top of the file: `Duration` may already be in scope. Add it if not.

- [ ] **Step 2: Build**

```sh
cargo check -p alacritree
```

Expected: clean. A dead-code warning for `spawn_phase` is expected until Task 2 calls it; if the crate denies warnings, add the call sites in Task 2 in the same commit as this one instead of splitting them.

- [ ] **Step 3: Commit**

```sh
git -C ../alacritree-worktrees/perf/async-spawn add alacritree/src/frame_log.rs
git -C ../alacritree-worktrees/perf/async-spawn commit -m "$(cat <<'EOF'
perf(frame-log): mark the phases of opening a session

Spawn cost lands on whatever frame phase was running when the click
arrived, which has read as a sidebar paint problem before.  A marker of
its own is what makes the cost attributable.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Split `spawn_with`

A pure refactor. External behaviour is identical: every existing caller still gets a fully attached session back, synchronously. Nothing yet runs on a worker.

**Files:**
- Modify: `alacritree/src/session.rs` (`spawn_with` and its callers `spawn`, `spawn_command`)
- Modify: `alacritree/src/focus_priority/windows.rs`
- Modify: `alacritree/src/app.rs` (the two `TermSize::new(80, 24), (8.0, 16.0)` call sites)

**Interfaces:**
- Consumes: `frame_log::spawn_phase` from Task 1.
- Produces, all in `session.rs`:
  - `pub struct OpenRequest` — opaque to callers, `Send`.
  - `pub struct Attachment` — opaque to callers, `Send`.
  - `impl Session { fn pending(...) -> (Session, OpenRequest); pub fn attach(&mut self, attachment: Attachment); }`
  - `pub fn open(request: OpenRequest) -> std::io::Result<Attachment>` — a free function in `session`, not a method.
  - `Session::pending_shell(...)` and `Session::pending_command(...)`, public wrappers mirroring the argument lists of `spawn` and `spawn_command`, each returning `(Session, OpenRequest)`. Task 6 calls both; the diff pane needs the second.

- [ ] **Step 1: Read what you are splitting**

Read `Session::spawn_with` in full (it starts around `session.rs:1153`) plus `Session::spawn` and `Session::spawn_command` above it. Note the ordering comments already in the body — the one about `harden_dll_search_path` and `conpty.dll`, and the one about a job having to exist before the shell starts anything. Those constraints survive the split and their comments move with the code they explain.

- [ ] **Step 2: Make `PriorityJob` movable between threads**

In `alacritree/src/focus_priority/windows.rs`, next to the `PriorityJob` definition:

```rust
// A job handle is a process-wide kernel object: the thread that closes it
// need not be the thread that opened it.  Moving one is what lets the PTY be
// opened off the UI thread, since the job can only be created once the shell
// it adopts has a pid.  The interior `Cell` keeps the type `!Sync`, which is
// what we want: only one thread may drive the boost at a time.
unsafe impl Send for PriorityJob {}
```

Add a compile-time check in that module's test block (create one if the module has none):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_priority_job_can_move_to_the_thread_that_opens_the_pty() {
        fn assert_send<T: Send>() {}
        assert_send::<PriorityJob>();
    }
}
```

- [ ] **Step 3: Run it and watch it fail**

```sh
cargo nextest run -p alacritree focus_priority
```

Expected before the `unsafe impl`: a compile error, `PriorityJob cannot be sent between threads safely`. Expected after: PASS. If it passes without the `unsafe impl`, something else already declared it — stop and report rather than adding a second declaration.

- [ ] **Step 4: Carve out `OpenRequest` and `open`**

In `session.rs`, above `impl Session`:

```rust
/// Everything opening a PTY needs, and nothing that has to stay on the UI
/// thread.  Built by [`Session::pending`], consumed by [`open`].
pub struct OpenRequest {
    id: SessionId,
    window_id: u64,
    pty_options: PtyOptions,
    window_size: WindowSize,
    term: Arc<FairMutex<Term<EventProxy>>>,
    proxy: EventProxy,
    boost: bool,
    reap: bool,
}

/// The half of a session that only exists once its PTY does.  Applied by
/// [`Session::attach`]; dropping one instead shuts the PTY down, which is
/// what happens when the tab it belongs to closes mid-open.
pub struct Attachment {
    shell_pid: Option<u32>,
    priority_job: Option<crate::focus_priority::PriorityJob>,
    sender: EventLoopSender,
}

/// Open the PTY for a pending session: process creation, the job that owns
/// it, and the event loop that drains it.  This is the part that costs
/// milliseconds, which is why it is a free function rather than a method —
/// it must be callable from a thread that holds no `Session`.
pub fn open(request: OpenRequest) -> std::io::Result<Attachment> {
    let started = std::time::Instant::now();
    let OpenRequest { id, window_id, pty_options, window_size, term, proxy, boost, reap } = request;

    ensure_working_directory(pty_options.working_directory.as_deref())?;

    // `tty::new` is where `LoadLibraryW("conpty.dll")` happens, and the
    // module it loads answers every later one for the life of the process.
    #[cfg(windows)]
    crate::harden_dll_search_path();

    let pty = tty::new(&pty_options, window_size, window_id)?;
    crate::frame_log::spawn_phase(Some(id), "pty", started.elapsed());
    let shell_pid = pty_shell_pid(&pty);

    // Jobbed here rather than on focus: a process joins a job when it is
    // created, so anything the shell starts before the job exists escapes
    // it for its whole life.  One job serves both settings, so it is
    // created when either wants it.
    let priority_job = shell_pid
        .filter(|_| boost || reap)
        .and_then(|pid| crate::focus_priority::PriorityJob::adopt(pid, reap));

    #[cfg(windows)]
    let pty = crate::pty_rearm::RearmingPty::new(pty);

    let event_loop = EventLoop::new(term, proxy, pty, false, false)?;
    let sender = event_loop.channel();
    event_loop.spawn();
    crate::frame_log::spawn_phase(Some(id), "open", started.elapsed());

    Ok(Attachment { shell_pid, priority_job, sender })
}
```

`ensure_working_directory` currently takes the resolved cwd; check its signature and pass whatever it wants. `SessionId`'s concrete type is whatever `next_session_id()` returns — match it.

- [ ] **Step 5: Carve out `pending` and `attach`**

Replace the body of `spawn_with` with a `pending` that returns both halves. `pending` takes the same arguments `spawn_with` takes today and is infallible, so the `ensure_working_directory` call moves into `open` (Step 4 already has it).

```rust
    /// The half of a session that costs nothing: ids, the grid, the event
    /// channel and the arguments its PTY will be opened with.  Cheap enough
    /// for a frame, which is the whole point of the split.
    fn pending(
        ctx: egui::Context,
        config: &Config,
        working_directory: Option<PathBuf>,
        size: TermSize,
        cell_size: (f32, f32),
        shell: Option<Shell>,
        title: String,
        kind: SessionKind,
        escape_args: bool,
        wsl_probe: Option<WslProbe>,
    ) -> (Self, OpenRequest) {
```

Its body is today's `spawn_with` up to but excluding `harden_dll_search_path`, and it ends by building both the `Session` (with `shell_pid: None`, `priority_job: None`, `notifier: None`, `sender: None`) and the `OpenRequest`. Keep the `escape_args` and `env` comments where they are.

Then:

```rust
    /// Adopt a PTY opened elsewhere.  Everything a session cannot do without
    /// one is switched on here, in one place, so there is a single answer to
    /// "when does this session become live".
    pub fn attach(&mut self, attachment: Attachment) {
        let Attachment { shell_pid, priority_job, sender } = attachment;
        self.shell_pid = shell_pid;
        self.priority_job = priority_job;
        self.notifier = Some(Notifier(sender.clone()));
        self.sender = Some(sender);
    }
```

Task 3 adds the probe registration, the size replay and the write flush to this function. Leave them out for now.

- [ ] **Step 6: Rebuild `spawn_with` on the three pieces**

```rust
    fn spawn_with(
        // …unchanged argument list…
    ) -> std::io::Result<Self> {
        let (mut session, request) = Self::pending(
            ctx, config, working_directory, size, cell_size, shell, title, kind, escape_args,
            wsl_probe,
        );
        session.attach(open(request)?);
        Ok(session)
    }
```

The `?` is what keeps every existing caller's error behaviour identical.

Then expose the two halves for callers that want to open the PTY themselves. `pending_shell` takes `spawn`'s argument list and `pending_command` takes `spawn_command`'s, each doing that function's argument massaging (the `escape_args` and title derivation in `spawn`, the `Shell::new` in `spawn_command`) and returning `(Self, OpenRequest)` instead of calling `open`. Rewrite `spawn` and `spawn_command` on top of them so the massaging lives in one place:

```rust
    pub fn spawn(/* …unchanged… */) -> std::io::Result<Self> {
        let (mut session, request) = Self::pending_shell(/* … */);
        session.attach(open(request)?);
        Ok(session)
    }
```

- [ ] **Step 7: Give `pending` the real geometry**

In `app.rs`, `spawn_session_with_shell` and `spawn_scratchpad` both pass `TermSize::new(80, 24), (8.0, 16.0)`. Leave `spawn_scratchpad` alone (it has no PTY). For the shell path, pass the size the visible session is at when there is one, falling back to `TermSize::new(80, 24)` and `(8.0, 16.0)` for the first session of a run:

```rust
        // The PTY is born at the geometry it will keep.  Under the gate this
        // matters: a session that opened at 80x24 and was resized on attach
        // makes a fast shell print its first prompt into a grid that is about
        // to be reflowed under it.
        let (size, cell_size) = self
            .active_session_index()
            .map(|idx| (self.sessions[idx].size, self.sessions[idx].cell_size))
            .unwrap_or((TermSize::new(80, 24), (8.0, 16.0)));
```

`active_session_index` is `Option<usize>` and `Session::size` / `Session::cell_size` are already `pub`, so this compiles as written.

Know what it does not cover. `active_session_index` is `None` in exactly the two cases the fallback names, and both are real: `close_session` removes the active entry before the respawn branch spawns, and the constructor has no session yet. Both land on 80x24 and are reflowed on attach, which is the behaviour today. Widen the fallback to `self.sessions.last()` before the constant if you want the respawn case to inherit a live pane's geometry; leaving it on the constant is also defensible. Say which in the comment.

- [ ] **Step 8: Run the whole suite**

```sh
cargo nextest run -p alacritree
```

Expected: PASS, with no test changed. This task is a refactor; a failing test means the split moved behaviour.

- [ ] **Step 9: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 ../alacritree-worktrees/perf/async-spawn/alacritree/src/session.rs
rustup run nightly rustfmt --edition 2024 ../alacritree-worktrees/perf/async-spawn/alacritree/src/focus_priority/windows.rs
```

Review `git diff` and drop any hunk rustfmt made in code you did not touch.

```sh
git -C ../alacritree-worktrees/perf/async-spawn commit -am "$(cat <<'EOF'
refactor(session): separate opening a PTY from having one

Spawning did ids, the grid, process creation and the event loop in one
function, so the frame that asked for a session paid for conpty.  Split
it into the part that costs nothing, the part that costs milliseconds,
and the adoption of the result, with the middle one callable from any
thread.  Behaviour is unchanged: the three still run back to back.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Make the pending state survivable

A `Session` will now exist without a PTY for a stretch of frames. Three things break in that window. Fix them while the code still runs synchronously, so each fix has a test that does not need a worker.

**Files:**
- Modify: `alacritree/src/session.rs`

**Interfaces:**
- Consumes: `pending`, `attach`, `Attachment` from Task 2.
- Produces: `Session::is_pending()`, `impl Drop for Attachment`.

- [ ] **Step 1: Write the failing tests**

In `session.rs`'s `mod tests`. The PTY-using tests already there are the pattern: spawn a real command, then poll `session.term.lock()` with a deadline until the grid says what you are waiting for. The event loop writes into the `Term` itself, so nothing has to drain a channel to observe the child.

Add a helper beside them:

```rust
    /// Poll the grid until `needle` appears, or fail saying what was there
    /// instead.  A deadline rather than a sleep: the shells these tests drive
    /// take wildly different times to come up on a loaded runner.
    fn grid_contains(session: &Session, needle: &str, patience: Duration) -> bool {
        let deadline = Instant::now() + patience;
        while Instant::now() < deadline {
            let text: String = {
                let term = session.term.lock();
                term.grid()
                    .display_iter()
                    .map(|cell| cell.c)
                    .collect()
            };
            if text.contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }
```

`term.grid().display_iter()` yields `Indexed<&Cell>`, which derefs, so `.c` resolves; `Dimensions` is already imported in `session.rs`, so `screen_lines()` and `columns()` do too. `Session::screen_snapshot(0).lines` is the shorter road to the same text and is what the existing Windows test in this module uses. Either is fine. Pick one and use it in all three tests.

```rust
    /// Input typed into a tab whose PTY is still opening has to arrive, and
    /// in order.  Under load that gap is long enough to swallow a command.
    #[cfg(windows)]
    #[test]
    fn input_written_before_attach_arrives_before_input_written_after() {
        let mut config = Config::default();
        config.env.insert("TERM".to_string(), "xterm-256color".to_string());
        let (mut session, request) = Session::pending_command(
            egui::Context::default(),
            &config,
            std::env::current_dir().ok(),
            TermSize::new(80, 24),
            (8.0, 16.0),
            "cmd.exe".to_string(),
            vec!["/q".to_string(), "/k".to_string(), "prompt $g".to_string()],
            "probe".to_string(),
            SessionKind::Shell,
        );

        session.write(b"echo alpha\r\n".to_vec());
        session.attach(open(request).expect("open the pty"));
        session.write(b"echo beta\r\n".to_vec());

        assert!(
            grid_contains(&session, "beta", Duration::from_secs(20)),
            "the shell never answered the write made after attach"
        );
        let text: String =
            session.term.lock().grid().display_iter().map(|cell| cell.c).collect();
        let alpha = text.find("alpha").expect("the write made before attach was dropped");
        let beta = text.find("beta").expect("the write made after attach was dropped");
        assert!(alpha < beta, "buffered input was replayed out of order");
    }

    /// A pending session's grid tracks the pane it is drawn in.  Without
    /// this the shell prints its first prompt into a grid that is about to
    /// be reflowed under it.
    #[test]
    fn a_resize_before_attach_reaches_the_grid() {
        let (mut session, _request) = Session::pending_command(
            egui::Context::default(),
            &Config::default(),
            None,
            TermSize::new(80, 24),
            (8.0, 16.0),
            "cmd.exe".to_string(),
            vec![],
            "probe".to_string(),
            SessionKind::Shell,
        );

        session.resize(TermSize::new(120, 40), (8.0, 16.0));

        assert_eq!(session.term.lock().screen_lines(), 40);
        assert_eq!(session.term.lock().columns(), 120);
    }

    /// The size the PTY is opened at is the size the request carried, so a
    /// pane resized while it was opening has to be replayed — and replayed
    /// before any buffered input, or the shell answers at the old width.
    #[cfg(windows)]
    #[test]
    fn a_resize_before_attach_reaches_the_pty() {
        let mut config = Config::default();
        config.env.insert("TERM".to_string(), "xterm-256color".to_string());
        let (mut session, request) = Session::pending_command(
            egui::Context::default(),
            &config,
            std::env::current_dir().ok(),
            TermSize::new(80, 24),
            (8.0, 16.0),
            "cmd.exe".to_string(),
            vec!["/q".to_string(), "/k".to_string(), "prompt $g".to_string()],
            "probe".to_string(),
            SessionKind::Shell,
        );

        session.resize(TermSize::new(120, 40), (8.0, 16.0));
        session.write(b"mode con\r\n".to_vec());
        session.attach(open(request).expect("open the pty"));

        assert!(
            grid_contains(&session, "120", Duration::from_secs(20)),
            "the child sees the size the PTY was opened at, not the one the pane ended up at"
        );
    }
```

Enter is `\r\n` in all three, not `\n`: alacritty encodes Return as `\r`, and a bare `\n` is not reliably a line submission to `cmd.exe` through a pseudoconsole.

`mode con` prints the console's column count, which is the child's own view rather than the grid's. If it turns out not to run inside a pseudoconsole on this Windows build, drop that third test, say in a comment on `attach` that the replay is covered only on the grid side, and report it — do not weaken it into an assertion about the `Term`, which the second test already makes.

- [ ] **Step 2: Run them and watch them fail**

```sh
cargo nextest run -p alacritree session::tests
```

Expected: `input_written_before_attach_arrives_before_input_written_after` fails on `"the write made before attach was dropped"`. Expected: `a_resize_before_attach_reaches_the_grid` fails on `screen_lines()`, because `resize` returns before touching the `Term` when there is no sender.

Expected: `a_resize_before_attach_reaches_the_pty` fails on the dropped write, not on the missing replay. `write` is still a no-op without a notifier at this point, so `mode con` never reaches the shell at all. Its real reason only becomes observable after Step 3 restores the write. Re-run it there and confirm it then fails because the size was never replayed, before Step 4 makes it pass.

Confirm each fails for the reason named. A failure to compile is not this test failing.

- [ ] **Step 3: Buffer writes made before attach**

Add the field to `Session`:

```rust
    /// Bytes written before the PTY existed, replayed by `attach`.  `Some`
    /// only between `pending` and `attach`, which also makes it the answer to
    /// whether this session is still opening — a scratchpad has no PTY either
    /// and must not be mistaken for one that is coming.
    pending_writes: Option<Vec<u8>>,
```

`pending()` sets `Some(Vec::new())`. Every other constructor, `spawn_scratchpad` included, sets `None`. Then:

```rust
    pub fn write(&self, bytes: Vec<u8>) {
        if let Some(notifier) = &self.notifier {
            notifier.notify(bytes);
        }
    }
```

becomes a `&mut self` method that appends to `pending_writes` when there is no notifier.

`paste::paste(session: &Session, ...)` in `paste.rs` calls it, so that signature becomes `&mut Session` too. Every `terminal_view.rs` caller of both already sits in a function taking `session: &mut Session`, so none of them change. Three `app.rs` call sites do:

- the `paste::paste(&self.sessions[idx], text, true)` in the clipboard paste path: `&mut self.sessions[idx]`, nothing else moves.
- the IPC `send_text` arm's `let session = &self.sessions[idx];`: `&mut`. `paste::on_terminal_input_start` takes `&Session`, and a `&mut` reborrows into it.
- the file-drop arm, which needs restructuring rather than an `&mut`. It holds `let session = &self.sessions[idx];` across a read of `&self.config.ui.drop.spelling`, and those two borrows cannot both be live if one is mutable. Do the read first, take the mutable borrow second:

```rust
                let text = file_drop::shell_payload(
                    &paths,
                    self.sessions[idx].wsl_distro(),
                    &self.config.ui.drop.spelling,
                );
                if !text.is_empty() {
                    paste::paste(&mut self.sessions[idx], &text, true);
                }
```

```rust
    /// Whether this session is waiting for a PTY that is on its way.
    pub fn is_pending(&self) -> bool {
        self.pending_writes.is_some()
    }
```

In `attach`, after the size replay of Step 4 and before anything else:

```rust
        // After the resize and before anything the app writes this frame, so
        // the shell answers a buffered command at the size the pane is at
        // rather than the one the PTY was opened with.
        if let Some(pending) = self.pending_writes.take() {
            if !pending.is_empty() {
                self.notifier.as_ref().expect("attach set the notifier").notify(pending);
            }
        }
```

- [ ] **Step 4: Resize the grid whether or not there is a PTY**

In `resize`, move `self.term.lock().resize(size)` above the `let Some(sender) = &self.sender else { return };` guard, so the guard only skips the `Msg::Resize`. Then in `attach`, immediately after the sender is set and before the write flush, send the size the session actually ended up at:

```rust
        // The pane may have been resized while the PTY was opening, and the
        // size the request carried is the one it was born with.
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Resize(window_size(self.size, self.cell_size)));
        }
```

The order inside `attach` is fixed and worth keeping in that order: sender, then resize, then flush.

- [ ] **Step 5: Register the WSL probe on attach, not on open**

Move the `wsl_helper::register_probe` call out of `open` (Task 2 left it in `pending`/`spawn_with`; wherever it sits, it moves) into `attach`:

```rust
        // Registered here rather than where the PTY is opened: `Session::drop`
        // is the only unregister, so a probe registered for a session that no
        // longer exists would stay in the cache and be polled for the life of
        // the process.
        if let Some(probe) = &self.wsl_probe {
            wsl_helper::register_probe(&probe.distro, &probe.key);
        }
```

- [ ] **Step 6: Make an unwanted attachment clean up after itself**

```rust
impl Drop for Attachment {
    /// An attachment nobody adopted belongs to a tab that closed while its
    /// PTY was opening.  Shutting the loop down here rather than at the call
    /// site means a quit mid-open, or a receiver that hung up, cleans up too.
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}
```

`attach` destructures the `Attachment`, which would run this `Drop` on the remains. Use `std::mem::ManuallyDrop`, or give `Attachment` an `into_parts()` that `mem::forget`s the shell, whichever is cleaner in context.

Do not add a test that adopting an attachment leaves the session alive. `is_exited()` reads a field only `drain_events` sets, so a test that never drains passes whether or not `attach` shut the shell down. Step 1's first test already proves the property properly: it asserts a byte written after `attach` comes back from the shell, which a shut-down loop cannot do.

- [ ] **Step 7: Run the tests**

```sh
cargo nextest run -p alacritree
```

Expected: PASS, including the three new tests.

- [ ] **Step 8: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 ../alacritree-worktrees/perf/async-spawn/alacritree/src/session.rs
git -C ../alacritree-worktrees/perf/async-spawn commit -am "$(cat <<'EOF'
feat(session): let a session survive not having a PTY yet

A session that exists before its PTY dropped every keystroke typed into
it, kept the grid at the size it was built with, and registered a WSL
probe that only its own drop could remove.  Buffer the writes, resize
the grid regardless, register the probe on adoption, and shut down an
attachment nobody adopts.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: The `pending_spawn` module

Pure bookkeeping, no app wiring, testable on its own.

**Files:**
- Create: `alacritree/src/pending_spawn.rs`
- Modify: `alacritree/src/main.rs`

**Interfaces:**
- Consumes: `session::Attachment`, `SessionId` from Task 2.
- Produces: `PendingSpawns` with `start`, `watch`, `take_finished`, `answer`, `is_empty`.

- [ ] **Step 1: Read the precedent**

Read `alacritree/src/project_refresh.rs` end to end, tests included. This module is the same shape for the same reason, and matching it is worth more than any improvement you can think of.

- [ ] **Step 2: Write the module**

```rust
//! Bookkeeping for PTYs opened on a worker.
//!
//! A session's record exists from the frame that asked for it, but its PTY
//! arrives some frames later.  This holds the receivers in between, and the
//! IPC replies parked until a caller's session is actually live — a client
//! that creates a session in order to write to it would otherwise race its
//! own PTY.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use crate::ipc::IpcResult;
use crate::session::{Attachment, SessionId};

struct Pending {
    rx: Receiver<std::io::Result<Attachment>>,
    waiters: Vec<Sender<IpcResult>>,
}

#[derive(Default)]
pub struct PendingSpawns {
    pending: HashMap<SessionId, Pending>,
}

/// What a finished open turned out to be, once the session list has been
/// consulted.  The caller owns the record, so it decides; this type is how
/// it says what it decided.
pub enum Finished {
    Opened(SessionId, Attachment),
    Failed(SessionId, std::io::Error),
}

impl PendingSpawns {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn start(&mut self, id: SessionId, rx: Receiver<std::io::Result<Attachment>>) {
        self.pending.insert(id, Pending { rx, waiters: Vec::new() });
    }

    /// Park `reply_tx` until the session's PTY is live.  Hands the channel
    /// back when nothing is opening for that id, leaving the caller to answer
    /// it however it sees fit.
    pub fn watch(&mut self, id: SessionId, reply_tx: Sender<IpcResult>) -> Option<Sender<IpcResult>> {
        match self.pending.get_mut(&id) {
            Some(pending) => {
                pending.waiters.push(reply_tx);
                None
            },
            None => Some(reply_tx),
        }
    }

    /// Take every open that has finished.  The workspace a session belongs to
    /// is deliberately not stored here: a pending session can be moved to
    /// another workspace, so the caller reads it off the record it finds.
    pub fn take_finished(&mut self) -> Vec<Finished> {
        let mut done = Vec::new();
        self.pending.retain(|id, pending| match pending.rx.try_recv() {
            Ok(Ok(attachment)) => {
                done.push(Finished::Opened(*id, attachment));
                false
            },
            Ok(Err(e)) => {
                done.push(Finished::Failed(*id, e));
                false
            },
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                done.push(Finished::Failed(
                    *id,
                    std::io::Error::other("the session's PTY worker stopped"),
                ));
                false
            },
        });
        done
    }

    /// Answer `waiters` with `reply`.  `take_finished` hands them over with
    /// the result they were parked on, so there is no id to look up here.
    pub fn answer(waiters: Vec<Sender<IpcResult>>, reply: IpcResult) {
        for waiter in waiters {
            let _ = waiter.send(reply.clone());
        }
    }
}
```

That `answer` is not what the block above it implies, and the difference is load-bearing. `take_finished`'s `retain` returns `false` for a finished open, which drops the whole `Pending`, waiters included. An `answer(id, ...)` called afterwards would find no entry, and the dropped `Sender`s make the connection thread's `recv_timeout` return `Disconnected` at once. Every `create_session` over IPC or MCP would fail with "alacritree did not respond" while the session it asked for sat there working.

So `Finished` carries the waiters out with the result:

```rust
/// An open that resolved, with whoever was parked on it.
pub enum Finished {
    Opened(SessionId, Attachment, Vec<Sender<IpcResult>>),
    Failed(SessionId, std::io::Error, Vec<Sender<IpcResult>>),
}
```

`retain` cannot move fields out of the value it inspects, so collect the finished ids in one pass and `remove` them in a second, or use `HashMap::extract_if`. Either is fine. The constraint is that the waiters leave the map with their result.

- [ ] **Step 3: Write the tests**

Mirror `project_refresh.rs`'s test style. Cover:

- a finished open comes back as `Opened` with its id
- a worker that drops its sender without sending comes back as `Failed`, not as a pending entry that never resolves
- a waiter parked with `watch` is answered once the open resolves, error included
- `watch` on an id nothing is opening hands the channel straight back

An `Attachment` cannot be built without a PTY. Either open a real one in the tests that need it (Task 3's helper does this) or give the tests a `Failed` path only and cover `Opened` in the app-level test in Task 6. Say in the test module's doc comment which you chose and why.

- [ ] **Step 4: Register the module**

In `main.rs`, `mod pending_spawn;` in the existing alphabetical run of module declarations.

- [ ] **Step 5: Run**

```sh
cargo nextest run -p alacritree pending_spawn
```

Expected: PASS.

- [ ] **Step 6: Commit**

```sh
git -C ../alacritree-worktrees/perf/async-spawn add alacritree/src/pending_spawn.rs alacritree/src/main.rs
git -C ../alacritree-worktrees/perf/async-spawn commit -m "$(cat <<'EOF'
feat(session): track PTYs that are still opening

Holds the receiver for each in-flight open and the IPC replies parked
until the session is live, so a client that creates a session in order
to write to it cannot race its own PTY.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: The config key

**Files:**
- Modify: `alacritree/src/config.rs`
- Modify: `docs/alacritree.md`
- Modify: `schema/alacritree-config.json` (generated, do not hand-edit)

- [ ] **Step 1: Add the field to both structs**

On `Ui`, beside `focus_priority_boost`:

```rust
    /// `[ui] async_session_spawn`: open a session's PTY on a worker instead
    /// of inside the frame that asked for it.  Creating a console process
    /// costs milliseconds when the machine is idle and hundreds when it is
    /// busy, and the frame pays all of it, so the click that opens a tab is
    /// what stutters.  The tab appears at once and starts painting when its
    /// PTY attaches; anything typed in between is replayed.  Off by default.
    pub async_session_spawn: bool,
```

On `RawUi`, matching the terser style of its neighbours:

```rust
    /// Open a session's PTY on a worker rather than in the frame that asked
    /// for it, so spawning does not stutter.  Default false.
    async_session_spawn: Option<bool>,
```

And in both the `Default for Ui` impl and the `RawConfig` → `Config` conversion: `async_session_spawn: self.ui.async_session_spawn.unwrap_or(false)`.

- [ ] **Step 2: Document it**

In `docs/alacritree.md`, in the `[ui]` block, beside `focus_priority_boost`. Match the surrounding entries' shape, including whether they say "restart required" — this one does not require a restart, since the flag is read per spawn.

- [ ] **Step 3: Regenerate the schema and confirm the gate**

```sh
cargo nextest run -p alacritree --test config_schema
```

Expected: FAIL, reporting the schema is stale. Then:

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo nextest run -p alacritree --test config_schema
cargo nextest run -p alacritree --test config_schema
```

Expected: PASS, with `schema/alacritree-config.json` carrying the doc comment as hover text.

- [ ] **Step 4: Commit**

```sh
git -C ../alacritree-worktrees/perf/async-spawn commit -am "$(cat <<'EOF'
feat(config): add [ui] async_session_spawn

Off by default, so the default experience keeps the synchronous spawn.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Wire it into the app

The task that actually fixes the bug.

**Files:**
- Modify: `alacritree/src/app.rs` (`spawn_session_with_shell`, `open_diff_pane`, `close_session`, `process_session_events`, `update`)
- Modify: `alacritree/src/steady_state.rs`

**Interfaces:**
- Consumes: `session::{open, OpenRequest, Attachment}`, `Session::{pending_shell, pending_command, attach, is_pending}`, `pending_spawn::{PendingSpawns, Finished}`, `config.ui.async_session_spawn`.
- Produces: `AlacritreeApp::{open_session, poll_pending_spawns}`, `CloseReason`, `close_navigation`, `close_session_with`.

- [ ] **Step 1: Write the failing test for the failure path**

Unwinding a failed open through `close_session` loops, and it loops through two separate doors.

The respawn policy is the obvious one: `last_session_close = "respawn"` puts a replacement in place, whose open fails identically. The navigation is the one that is easy to miss, and it fires under both policies. `close_fallback` returns `Home`, `apply_close_fallback` calls `activate_home`, `ensure_active_session` finds home empty and spawns into it, and that open fails the same way. One `error_dialog` per iteration, forever. `sidebar_focus = follow` only defers the same verdict to the reconciler rather than breaking it.

One move shuts both doors, because the respawn branch already requires `verdict != CloseFallback::Stay`: force the verdict to `Stay` when the close is a failed open. That is a pure decision and gets a pure test.

In `app.rs`'s `mod tests`:

```rust
    /// A user's close navigates: away from an emptied workspace, or into a
    /// replacement shell.  A failed open must do neither.  Wherever it
    /// navigates to, `ensure_active_session` spawns into it, and that open
    /// fails the same way.
    #[test]
    fn a_failed_spawn_neither_navigates_nor_respawns() {
        assert_eq!(
            close_navigation(CloseReason::User, CloseFallback::Home),
            CloseFallback::Home
        );
        assert_eq!(
            close_navigation(CloseReason::SpawnFailed, CloseFallback::Home),
            CloseFallback::Stay
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```sh
cargo nextest run -p alacritree a_failed_spawn_neither_navigates
```

Expected: a compile error, `cannot find function close_navigation`. That is the right failure.

- [ ] **Step 3: Add the reason and the rule**

```rust
/// Why a session record is going away.  The distinction exists because
/// neither half of a close, the respawn policy or the navigation, may apply
/// to a session that never got a PTY.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CloseReason {
    User,
    SpawnFailed,
}

/// The verdict a close acts on.  A failed open stays put whatever the
/// workspace's state says: every destination `close_fallback` can name is one
/// `ensure_active_session` will spawn into, and that open fails the same way.
/// Staying leaves the pane on the "no session" placeholder, which is what the
/// workspace honestly holds.
fn close_navigation(reason: CloseReason, verdict: CloseFallback) -> CloseFallback {
    match reason {
        CloseReason::User => verdict,
        CloseReason::SpawnFailed => CloseFallback::Stay,
    }
}
```

Rename `close_session` to `close_session_with(&mut self, ctx, id, reason)` and add back `fn close_session(&mut self, ctx, id) { self.close_session_with(ctx, id, CloseReason::User) }` so its existing callers are untouched. Inside, wrap the verdict:

```rust
        let verdict = close_navigation(
            reason,
            close_fallback(&workspace, &self.current_workspace, &remaining, main),
        );
```

Everything downstream already tests `verdict != CloseFallback::Stay`, so this one line shuts the respawn branch, the deferred-close branch and `apply_close_fallback` together. The `close_landing` repair above it still runs, which is the part a failed open does need.

`CloseFallback` already derives `PartialEq`, as the existing `verdict != CloseFallback::Stay` proves. Add `Debug` if `assert_eq!` needs it.

Run the test again. Expected: PASS.

- [ ] **Step 4: Hold the app's own boost across a spawn**

`process_session_events` feeds `set_priority_boost`'s answers into `set_self_boosted(anything_raised)`. A pending session has no job and answers `false`, so a frame whose visible session is still opening drops the GUI to normal priority and the next frame after attach raises it again — under exactly the load this whole change is about.

Fold the rule into the loop already there. `process_session_events` runs every frame, so a helper taking a slice would mean building a `Vec` of per-session pairs every frame, in a file whose whole premise is that an idle frame allocates nothing:

```rust
        for (idx, session) in self.sessions.iter().enumerate() {
            let wanted = Some(idx) == target;
            // A session still opening its PTY has no job to raise yet but will
            // have one within a frame or two.  Counting it is what stops a
            // spawn dropping the GUI to normal priority for the whole open and
            // raising it again on attach.
            anything_raised |=
                session.set_priority_boost(wanted) || (wanted && session.is_pending());
        }
```

`set_priority_boost` is the left operand of the `||`, so it still runs for every session. It is the call that does the work, and short-circuiting past it would leave stale boosts behind.

Cover it in `session.rs` instead: a session from `pending` answers `is_pending()` true, and one that has been through `attach` answers false. The frame-level rule is two operators long and reads correctly in place. A pure helper for it would cost more than it explains.

- [ ] **Step 5: Add the one place a session's PTY gets opened**

Add the field to `AlacritreeApp`:

```rust
    /// PTYs opened on a worker, adopted in `poll_pending_spawns`.
    pending_spawns: crate::pending_spawn::PendingSpawns,
```

and initialise it in the one `AlacritreeApp { ... }` literal in the constructor. `app.rs` imports session items by name (`use crate::session::{...}`) and has no `use crate::session;`, so add `open`, `OpenRequest` and `Attachment` to that list, plus `pending_spawn::{Finished, PendingSpawns}`. `json!`, `Value`, `mpsc` and `LastSessionClose` are already in scope.

Then the shared helper. Every session that has a PTY goes through it, which is what stops the diff pane keeping a second, synchronous spawn path:

```rust
    /// Push a session record and get its PTY opened: inline when the gate is
    /// off, on a worker when it is on.  The record exists before this
    /// returns either way, so a caller can activate the tab without waiting
    /// for a shell.  Callers own `active_session`; this owns `self.sessions`.
    fn open_session(
        &mut self,
        ctx: &Context,
        session: Session,
        request: session::OpenRequest,
    ) -> std::io::Result<SessionId> {
        let id = session.id;
        self.sessions.push(session);

        if !self.config.ui.async_session_spawn {
            match session::open(request) {
                Ok(attachment) => {
                    let idx = self.sessions.iter().position(|s| s.id == id).expect("just pushed");
                    self.sessions[idx].attach(attachment);
                    return Ok(id);
                },
                Err(e) => {
                    // The record went in before the open, so it comes back out
                    // before the error does: with the gate off, a caller that
                    // gets `Err` must see no trace of the session.
                    self.sessions.retain(|s| s.id != id);
                    return Err(e);
                },
            }
        }

        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(session::open(request));
            // Without this the result waits for whatever wakes the loop next,
            // which under load is the shell's own first output seconds later.
            ctx.request_repaint();
        });
        self.pending_spawns.start(id, rx);
        Ok(id)
    }
```

`spawn_session_with_shell` then becomes `Session::pending_shell(...)`, `self.open_session(ctx, session, request)?`, `self.active_session.insert(working_directory, id)`. With the gate off it returns the same `Err` at the same moment it does today, so every existing caller behaves identically.

- [ ] **Step 6: Route the diff pane through it too**

`open_diff_pane` calls `Session::spawn_command` directly and pushes the result itself, so without this it keeps a synchronous spawn on a git-sidebar click — the same freeze this change exists to remove, on a different button. Replace the `match Session::spawn_command(...)` with `Session::pending_command(...)` followed by `self.open_session(...)`, keeping its `active_session` insert and its `error_dialog` on `Err`. With the gate on, an open that fails now reports through `poll_pending_spawns` as "failed to spawn shell" rather than through the pane's own wording. The diff pane no longer has a failure of its own to report.

The `Session::spawn_command` in `terminal_view.rs` is a test and stays as it is.

- [ ] **Step 7: Adopt the results**

```rust
    /// Adopt every PTY that finished opening.  A session whose record is gone
    /// was closed while it was opening: dropping the attachment shuts its
    /// shell down rather than resurrecting the tab.
    fn poll_pending_spawns(&mut self, ctx: &Context) {
        for finished in self.pending_spawns.take_finished() {
            match finished {
                Finished::Opened(id, attachment, waiters) => {
                    match self.sessions.iter().position(|s| s.id == id) {
                        Some(idx) => {
                            self.sessions[idx].attach(attachment);
                            PendingSpawns::answer(waiters, Ok(json!({ "session_id": id })));
                        },
                        None => {
                            drop(attachment);
                            PendingSpawns::answer(
                                waiters,
                                Err("the session was closed while its shell was starting".into()),
                            );
                        },
                    }
                },
                Finished::Failed(id, e, waiters) => {
                    let ws = self
                        .sessions
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.working_directory.clone());
                    if let Some(ws) = ws {
                        self.close_session_with(ctx, id, CloseReason::SpawnFailed);
                        self.report_spawn_failure(ctx, &ws, &e);
                    }
                    PendingSpawns::answer(waiters, Err(format!("failed to spawn shell: {e}")));
                },
            }
        }
    }
```

Call it in `update` immediately after `poll_project_refreshes`.

Reading the workspace off the record rather than off the pending entry is deliberate: `move_session_to` can re-key a session while its PTY is opening.

A session closed while it was opening answers its waiters too. Dropping them silently would leave an IPC client on `recv_timeout` for the full 10s `APP_REPLY_TIMEOUT` and then report a timeout for something that already has a definite answer.

- [ ] **Step 8: Keep the steady-state gate honest**

`steady_state.rs` asserts an unchanged frame allocates nothing. Add a case there for an empty `PendingSpawns`, following the file's existing measurement shape:

```rust
    /// The poll runs every frame with no setting that disables it, so the
    /// common case — nothing opening — has to be free.
    #[test]
    fn polling_no_pending_spawns_allocates_nothing() {
```

`take_finished` returns a `Vec`, and `Vec::new()` does not allocate until pushed to, so this should pass as written. If it does not, make the empty case return early before the vector exists.

- [ ] **Step 9: Run everything**

```sh
cargo nextest run -p alacritree
```

Expected: PASS.

- [ ] **Step 10: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 ../alacritree-worktrees/perf/async-spawn/alacritree/src/app.rs
```

Check `git diff` for rustfmt hunks in code you did not touch and drop them.

```sh
git -C ../alacritree-worktrees/perf/async-spawn commit -am "$(cat <<'EOF'
perf(session): open a session's PTY off the UI thread

Creating a console process cost the frame that asked for it: 8-12ms
idle, over 200ms under load, charged to whichever sidebar or shortcut
phase happened to be running.  Behind [ui] async_session_spawn the tab
now appears at once and adopts its PTY when it lands.

A spawn that fails after the frame returned reports through the same
notification, and is removed without consulting the respawn policy: the
replacement would fail identically, once per open.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Park the IPC reply

**Files:**
- Modify: `alacritree/src/app.rs`

- [ ] **Step 1: Read the precedent**

`process_ipc_calls` claims `RefreshProject` before dispatch, because `handle_ipc_request` has no reply channel. `defer_project_refresh` is the shape to copy, and the `Req::RefreshProject { .. } => Err("refresh was not deferred")` arm is the guard to copy.

- [ ] **Step 2: Claim `CreateSession` the same way**

In `process_ipc_calls`, beside the `RefreshProject` claim:

```rust
            // The reply has to wait for the PTY: a client that creates a
            // session in order to write to it would otherwise be told the id
            // before anything can receive what it writes.
            if let ipc::IpcRequest::CreateSession { workspace } = request {
                self.defer_create_session(ctx, workspace, reply_tx);
                continue;
            }
```

`defer_create_session` resolves the workspace exactly as the current `Req::CreateSession` arm does, calls `spawn_session`, and then parks:

```rust
        let id = match self.spawn_session(ctx, workspace) {
            Ok(id) => id,
            // `defer_create_session` answers the client itself, so a failure
            // the frame can still see has to be sent rather than returned.
            Err(e) => {
                let _ = reply_tx.send(Err(format!("failed to spawn shell: {e}")));
                return;
            },
        };
        match self.pending_spawns.watch(id, reply_tx) {
            // Nothing is opening for this id: the PTY is already live, which
            // is what the gate-off path does.
            Some(reply_tx) => {
                let _ = reply_tx.send(Ok(json!({ "session_id": id })));
            },
            None => {},
        }
```

Replace the `Req::CreateSession` arm in `handle_ipc_request` with the not-deferred guard, worded like `RefreshProject`'s.

- [ ] **Step 3: Verify by hand**

The IPC path has no automated coverage that reaches a real socket. With a debug build running and `[ui] async_session_spawn = true`:

```sh
cargo run -p alacritree
alacritree session create --json
```

Expected: the command returns a `session_id` and the tab is live by the time it does. Then point `[terminal.shell] program` at something that does not exist and repeat: the command must fail with the error, not report success.

- [ ] **Step 4: Commit**

```sh
git -C ../alacritree-worktrees/perf/async-spawn commit -am "$(cat <<'EOF'
fix(ipc): answer create_session once its PTY is live

The reply carried a session id whose PTY might not exist yet, so a
client that created a session in order to write to it raced its own
shell, and a spawn that failed later was never reported to the caller
at all.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Measure it

The two acceptance criteria no test can assert. This task produces evidence, not code.

**Files:** none.

- [ ] **Step 1: Build release**

```sh
cargo build -p alacritree --release
```

- [ ] **Step 2: Saturate the machine**

Start 16 busy processes (a `cargo build -j16` of this workspace works, or the burner harness on `perf/load-latency` if you prefer a steady load). Confirm they are actually running before measuring.

- [ ] **Step 3: Measure with the gate off, then on**

Run with `ALACRITREE_FRAME_LOG=1` and, for each of the two settings, spawn a session from a sidebar row, from the `SpawnNewInstance` binding, and from the command palette. Capture the `spawn pty`, `spawn open`, `slow frame` and `slow action` lines.

- [ ] **Step 4: Report**

Write up the before/after for each of the three entry points: the `spawn pty` and `spawn open` figures, and whether any `slow frame` remains attributable to them. The criterion is that with the gate on, no `slow frame` names a spawn phase. If one does, that is a finding to report, not a number to bury.

Paste the numbers into the PR body when the human asks for a PR. Do not open one.

---

## Notes for whoever runs this

- Tasks 1 through 5 are safe to run without a Windows machine; Tasks 6 through 8 need one, since the whole problem is conpty.
- A known race is documented in the spec and deliberately not fixed: deleting a worktree while a session in it is still opening lets the shell be born holding the directory. Today's close is already fire-and-forget, so this widens an existing window rather than opening a new one. Do not fix it here.
- If a task turns out to be wrong about the code, say so and stop. The spec was reviewed against the branch, but line numbers move.
