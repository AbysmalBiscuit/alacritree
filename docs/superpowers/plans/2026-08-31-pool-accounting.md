# Pool Accounting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the job pool's admission limits stop lying, a cancelled job gives its worker back, no request can park a worker forever, and PR lookups stop spending one subprocess per worktree.

**Architecture:** `alacritree/src/jobs.rs` gains a second admission ceiling and a shared `Cancel` value that lets a dropped `Job` kill the child its worker is waiting on. `worktree.rs` opts its `git fetch` into that. `ipc.rs` gives its create request an absolute deadline. `pr_status.rs` stops overshooting the pool's background ceiling, stops reading `Instant::now()` directly, and stops spending one `gh` process per worktree.

**Tech Stack:** Rust edition 2024, std threads only (no async runtime), `git2`, `gh` CLI, `serde_json`, egui.

**Spec:** `docs/superpowers/specs/2026-08-31-pool-accounting-design.md`

**Issues:** #32, #33, #37, #44, all sub-issues of #22.

## Global Constraints

- Branch is `perf/pool-accounting`, worktree at `C:\Users\Lev\Git\github\alacritree-worktrees\perf\pool-accounting`. Work only there.
- Only `alacritree/` and `schema/` change. The vendored `alacritty*` crates and `egui-winit/` are read-only.
- Every commit uses Conventional Commits and ends with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- `cargo test -p alacritree` passes before every commit.
- `python3 alacritree/tools/ui-thread-audit.py .` reports `blocking leaves reachable from update: 0`. It needs `ast-grep` 0.45.1 on PATH; an empty scan exits 2 and proves nothing.
- `cargo clippy -p alacritree --all-targets -- -D clippy::disallowed_methods` passes.
- Never run bare `cargo +nightly fmt`; it reformats vendored crates. Scope it: `rustup run nightly rustfmt --edition 2024 alacritree/src/<file>.rs`.
- No new config flag. These are fixes, not new UX.
- Windows and Linux both compile and pass. `wsl.rs`, `wsl_helper.rs` and the Windows arm of `links.rs` only build on Windows.
- MSRV is 1.85. Let-chains (`if let … && let …`) are not available; nest instead.
- Comments explain *why*, never restate *what*. No PR or task references in code comments.
- `git fetch` gets no timeout. Cancellation replaces it. This contradicts #32's title deliberately; see the spec.
- Chunk batched GraphQL at 100 aliases. This is measured, not guessed.

---

### Task 1: Interactive admission ceiling

Closes the first half of #32. `take()` pops interactive work unconditionally today, so enough interactive jobs hold every worker while background is capped at `workers - 1`.

**Files:**
- Modify: `alacritree/src/jobs.rs` (`State`, `take`, `BackgroundSlot`, `worker`, `Pool::spawn`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `enum Slot { Interactive, Background }` (private), `State.interactive_running: usize`, `struct SlotGuard<'a> { shared: &'a Shared, slot: Slot }` (private). `take` becomes `fn take(state: &mut State, workers: usize) -> Option<(Task, Slot)>`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `alacritree/src/jobs.rs`:

```rust
/// Interactive work must not be able to take every worker.  A pool with all
/// its workers on interactive jobs cannot refresh a git status, poll worktree
/// liveness, or look up a PR, and nothing on screen says why.
#[test]
fn interactive_work_leaves_a_worker_for_background() {
    let pool = Pool::new(4);
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));

    let mut held = Vec::new();
    for _ in 0..4 {
        let rx = Arc::clone(&release_rx);
        held.push(pool.spawn(Priority::Interactive, move |_| {
            let _ = rx.lock().expect("the release channel is poisoned").recv();
        }));
    }

    let (ran_tx, ran_rx) = mpsc::channel();
    let background = pool.spawn(Priority::Background, move |_| {
        let _ = ran_tx.send(());
    });

    assert!(
        ran_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "the background job never ran: interactive work took every worker"
    );

    for _ in 0..4 {
        let _ = release_tx.send(());
    }
    drop(held);
    drop(background);
}
```

If `use std::time::Duration;` is not already in the test module, add it.

- [ ] **Step 2: Run the test and watch it fail**

```sh
cargo test -p alacritree --bin alacritree jobs::tests::interactive_work_leaves_a_worker_for_background -- --exact
```

Expected: FAIL on `the background job never ran`, after a 5 second wait. Four interactive jobs occupy all four workers.

- [ ] **Step 3: Add the second counter and the slot enum**

Replace `State` and add `Slot` above `take`:

```rust
#[derive(Default)]
struct State {
    interactive: VecDeque<Task>,
    background: VecDeque<Task>,
    interactive_running: usize,
    background_running: usize,
}

/// Which class a running task occupies, so the guard that frees its slot
/// knows which counter to decrement.
#[derive(Clone, Copy)]
enum Slot {
    Interactive,
    Background,
}
```

- [ ] **Step 4: Gate both classes in `take`**

Replace `take`:

```rust
/// The next task this worker may run, and the slot it occupies.  Each class
/// is capped one below the worker count, so neither can shut the other out:
/// a click never queues behind a pool full of git walks, and a burst of
/// creates never stops a status refresh.  Interactive keeps first refusal.
fn take(state: &mut State, workers: usize) -> Option<(Task, Slot)> {
    if state.interactive_running + 1 < workers {
        if let Some(task) = state.interactive.pop_front() {
            state.interactive_running += 1;
            return Some((task, Slot::Interactive));
        }
    }
    if state.background_running + 1 < workers {
        if let Some(task) = state.background.pop_front() {
            state.background_running += 1;
            return Some((task, Slot::Background));
        }
    }
    None
}
```

The existing tests `interactive_work_goes_first` and its neighbours bind
`take`'s second element as a `bool`. `Slot` is not one, so change those
assertions to `matches!(slot, Slot::Interactive)` (or `Slot::Background`) as
the compiler names them.

- [ ] **Step 5: Generalise the release guard**

Replace `BackgroundSlot` and its `Drop`:

```rust
/// Releases a worker's slot on drop, whether the task returned normally or
/// unwound through a panic — a straight-line decrement after the call would
/// never run for a panicking job, permanently shrinking the pool.
struct SlotGuard<'a> {
    shared: &'a Shared,
    slot: Slot,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().expect("the job queue is poisoned");
        match self.slot {
            Slot::Interactive => state.interactive_running -= 1,
            Slot::Background => state.background_running -= 1,
        }
        drop(state);
        // A freed slot may admit a task another worker is asleep on.
        self.shared.wake.notify_all();
    }
}
```

- [ ] **Step 6: Update the worker loop**

In `worker`, replace the destructuring and the guard construction:

```rust
let (task, slot) = loop {
    if let Some(taken) = take(&mut state, shared.workers) {
        break taken;
    }
    state = shared.wake.wait(state).expect("the job queue is poisoned");
};
drop(state);

let _slot = SlotGuard { shared: &shared, slot };

if !task.cancelled.load(Ordering::Relaxed) {
    lower_this_thread(matches!(slot, Slot::Background));
    let _wake = WakeOnEnd { shared: &shared };
    let outcome = catch_unwind(AssertUnwindSafe(|| (task.run)(&Blocking(()))));
    if let Err(panic) = outcome {
        log::error!("a job panicked: {}", panic_message(&panic));
        (task.on_failure)();
    }
}
```

The guard is now unconditional: interactive tasks occupy a slot too.

- [ ] **Step 7: Update `Pool::new`'s doc comment**

```rust
/// `workers` is clamped to at least two: each class's ceiling needs one
/// worker beyond the one it holds free.
```

- [ ] **Step 8: Run the test and the suite**

```sh
cargo test -p alacritree --bin alacritree jobs::tests::interactive_work_leaves_a_worker_for_background -- --exact
cargo test -p alacritree
```

Expected: the new test PASSES, everything else still passes.

- [ ] **Step 9: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/jobs.rs
git add alacritree/src/jobs.rs
git commit -m "$(cat <<'EOF'
perf(jobs): cap interactive work below the worker count

Only background work was gated, so enough interactive jobs held every
worker and the status refreshes, liveness probes and PR lookups behind
them stopped without anything on screen saying why.

Both classes now cap one below the worker count, so neither shuts the
other out.  Interactive keeps first refusal, and an over-ceiling
submission queues rather than being refused: dropping user-initiated
work silently is worse than making it wait.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Cancel a running job's child

Closes the second half of #32. `Job::drop` sets a flag the worker reads only before starting, so dropping the handle of a running job frees nothing.

**Files:**
- Modify: `alacritree/src/jobs.rs` (`Blocking`, `Task`, `Job`, `Pool::spawn`, `worker`, `on_this_thread`)

**Interfaces:**
- Consumes: `Slot`, `SlotGuard` from Task 1.
- Produces:
  - `struct Cancel { flag: AtomicBool, child: Mutex<Option<Child>> }` (private), with `fn cancel(&self)`.
  - `pub struct Blocking(Arc<Cancel>)`, whose constructor stays private to the module.
  - `pub fn Blocking::cancelled(&self) -> bool`
  - `pub fn Blocking::run_cancellable(&self, cmd: &mut Command) -> std::io::Result<Output>`
  - `Task.cancelled: Arc<AtomicBool>` becomes `Task.cancel: Arc<Cancel>`; `Job.cancelled` likewise becomes `Job.cancel: Arc<Cancel>`.

- [ ] **Step 1: Write the two failing tests**

Add to `mod tests` in `alacritree/src/jobs.rs`:

```rust
/// A command that outlives any test, so only a kill ends it.
///
/// `ping` rather than `timeout` on Windows: `timeout` refuses to run when
/// stdin is not a console and exits at once, which would let the test pass
/// without ever killing anything.
fn long_sleep() -> Command {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("ping");
        c.args(["-n", "31", "127.0.0.1"]);
        c
    } else {
        let mut c = Command::new("sleep");
        c.arg("30");
        c
    };
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

/// Dropping the handle of a job already waiting on a child must kill that
/// child and free the worker.  Checking the flag only before the task starts
/// leaves a worker parked for as long as the child runs.
#[test]
fn dropping_a_running_job_kills_its_child_and_frees_the_worker() {
    let pool = Pool::new(2);
    let (started_tx, started_rx) = mpsc::channel();
    let job = pool.spawn(Priority::Interactive, move |blocking| {
        let _ = started_tx.send(());
        let _ = blocking.run_cancellable(&mut long_sleep());
    });
    started_rx.recv_timeout(Duration::from_secs(5)).expect("the job never started");

    drop(job);

    let (ran_tx, ran_rx) = mpsc::channel();
    let next = pool.spawn(Priority::Interactive, move |_| {
        let _ = ran_tx.send(());
    });
    assert!(
        ran_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "the worker was still parked on the killed child"
    );
    drop(next);
}

/// The handle can drop between the spawn and the registration, so `cancel`
/// finds nothing to kill.  Registering must re-check the flag, or that child
/// runs to completion with nobody left to want it.
#[test]
fn a_cancel_racing_registration_still_kills_the_child() {
    let pool = Pool::new(2);
    let (started_tx, started_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();
    let job = pool.spawn(Priority::Interactive, move |blocking| {
        // The handshake has to come before the drop.  A flag set while the
        // task is still queued is caught by the pre-start check, the task is
        // skipped, `done_tx` drops unsent, and the assertion below reports a
        // disconnect instead of the behaviour under test.
        let _ = started_tx.send(());
        // Hold here until the handle has already been dropped, so the flag is
        // set before any child exists to register.
        let _ = gate_rx.recv();
        let result = blocking.run_cancellable(&mut long_sleep());
        let _ = done_tx.send(result.is_err());
    });
    started_rx.recv_timeout(Duration::from_secs(5)).expect("the job never started");

    drop(job);
    let _ = gate_tx.send(());

    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(5)),
        Ok(true),
        "the child outlived a cancel that landed before it was registered"
    );
}
```

Add `use std::process::{Command, Stdio};` to the test module if absent.

The existing `state_with` test helper constructs `Task` literals, so it needs
the field Step 5 adds. Give it `cancelled: Arc::new(Cancel::default())`.

- [ ] **Step 2: Run them and watch them fail**

```sh
cargo test -p alacritree --bin alacritree jobs::tests::dropping_a_running_job -- --exact
```

Expected: FAIL to compile, `no method named run_cancellable found for struct Blocking`. That is the right failure: the capability does not exist.

- [ ] **Step 3: Add the `Cancel` value**

Add above `Blocking` in `alacritree/src/jobs.rs`:

```rust
/// A job's cancellation state, shared by its handle, its queued task, and the
/// `Blocking` its worker runs with.
#[derive(Default)]
struct Cancel {
    flag: AtomicBool,
    /// The child this job opted into having killed, while one is running.
    child: Mutex<Option<Child>>,
}

impl Cancel {
    /// Set the flag, then end whatever child is registered.  Killing is what
    /// frees the worker: it is blocked in a wait the kernel returns from.
    fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.kill_registered();
    }

    fn kill_registered(&self) {
        if let Some(mut child) = self.child.lock().expect("the cancel slot is poisoned").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
```

Add `use std::process::{Child, Command, Output, Stdio};` and `use std::io;` to the file's imports. `Stdio` is used by the doc example only; drop it from the import if the compiler says it is unused.

- [ ] **Step 4: Give `Blocking` the Arc and its two methods**

Replace `pub struct Blocking(());` and add the impl:

```rust
/// Proof that the holder runs on a pool worker.  The constructor is private
/// to this module, so a blocking helper that takes one cannot be called from
/// the UI thread.
pub struct Blocking(Arc<Cancel>);

impl Blocking {
    /// Whether this job's handle has been dropped.  Check between steps: a
    /// job doing local work has no child registered for a cancel to kill, so
    /// nothing else would stop it.
    pub fn cancelled(&self) -> bool {
        self.0.flag.load(Ordering::Relaxed)
    }

    /// Run a child a cancel is allowed to kill, and return what it wrote.
    /// Registering is the opt-in: an unregistered child runs to completion
    /// whatever the caller does with the handle.
    ///
    /// The pipes are not drained until the child exits, so this suits a child
    /// whose output is bounded.  A child that fills a pipe would block on the
    /// write and never reach the exit this waits for.
    pub fn run_cancellable(&self, cmd: &mut Command) -> io::Result<Output> {
        let child = cmd.spawn()?;
        *self.0.child.lock().expect("the cancel slot is poisoned") = Some(child);
        // The handle can drop between the spawn above and the registration, in
        // which case `cancel` ran while there was nothing to kill.
        if self.cancelled() {
            self.0.kill_registered();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "job cancelled"));
        }
        loop {
            let mut slot = self.0.child.lock().expect("the cancel slot is poisoned");
            let exited = match slot.as_mut() {
                Some(child) => child.try_wait()?.is_some(),
                // The killer got here first and took it.
                None => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "job cancelled"));
                },
            };
            if exited {
                let child = slot.take().expect("observed present on this iteration");
                drop(slot);
                return child.wait_with_output();
            }
            drop(slot);
            std::thread::sleep(CHILD_POLL);
        }
    }
}

/// How often `run_cancellable` asks whether its child has exited.  The killer
/// and the waiter both need `&mut Child`, so they take turns on the mutex
/// rather than one blocking inside it.  Invisible against a fetch that runs
/// for seconds.
const CHILD_POLL: Duration = Duration::from_millis(25);
```

Add `use std::time::Duration;` to the file's imports.

- [ ] **Step 5: Thread the `Arc<Cancel>` through `Task`, `Job` and `spawn`**

In `Task`, replace `cancelled: Arc<AtomicBool>` with `cancel: Arc<Cancel>`. In `Job<T>`, replace `cancelled: Arc<AtomicBool>` with `cancel: Arc<Cancel>`. In `Pool::spawn`:

```rust
let cancel = Arc::new(Cancel::default());
let task = Task {
    cancel: Arc::clone(&cancel),
    run: Box::new(move |blocking| {
        let _ = tx.send(Ok(f(blocking)));
    }),
    on_failure: Box::new(move || {
        let _ = fail_tx.send(Err(JobFailed));
    }),
};
```

and return `Job { rx, cancel, failed: Cell::new(false) }`.

- [ ] **Step 6: Make `Job::drop` cancel for real**

```rust
impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
```

- [ ] **Step 7: Build each worker's `Blocking` from its task**

In `worker`, replace the flag read and the `Blocking` construction:

```rust
if !task.cancel.flag.load(Ordering::Relaxed) {
    lower_this_thread(matches!(slot, Slot::Background));
    let _wake = WakeOnEnd { shared: &shared };
    let blocking = Blocking(Arc::clone(&task.cancel));
    let outcome = catch_unwind(AssertUnwindSafe(|| (task.run)(&blocking)));
    if let Err(panic) = outcome {
        log::error!("a job panicked: {}", panic_message(&panic));
        (task.on_failure)();
    }
}
```

- [ ] **Step 8: Update `on_this_thread`**

```rust
pub fn on_this_thread<T>(f: impl FnOnce(&Blocking) -> T) -> T {
    // Nothing holds this `Cancel`, so `run_cancellable` here behaves exactly
    // like a plain run.
    f(&Blocking(Arc::new(Cancel::default())))
}
```

- [ ] **Step 9: Run the tests**

```sh
cargo test -p alacritree --bin alacritree jobs::tests -- --nocapture
cargo test -p alacritree
```

Expected: both new tests PASS, everything else still passes.

- [ ] **Step 10: Prove the gate still holds**

Temporarily add `let _ = jobs::Blocking(std::sync::Arc::new(Default::default()));` inside `AlacritreeApp::update` in `alacritree/src/app.rs`, run `cargo check -p alacritree`, and confirm `error[E0603]: tuple struct constructor 'Blocking' is private`. Remove the line. Do not commit it.

- [ ] **Step 11: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/jobs.rs
git add alacritree/src/jobs.rs
git commit -m "$(cat <<'EOF'
feat(jobs): let a dropped handle end the child its worker waits on

The cancel flag was read once, before a task started, so dropping the
handle of a running job freed nothing and its worker stayed parked for
as long as the child ran.

The handle, the task and the worker's token now share one value holding
the flag and the child a job opted into having killed.  Dropping the
handle kills that child, and the kernel returns from the wait, so the
worker comes back.  Registering re-checks the flag, since the handle can
drop between the spawn and the registration.

Killing is opt-in per child: an unregistered child runs to completion,
which is what a step that must not be interrupted needs.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Opt the fetch in and check the flag between steps

Makes cancellation reach `worktree::create`, the one job that blocks for an unbounded time.

**Files:**
- Modify: `alacritree/src/worktree.rs` (`create`, plus a new `run_git_cancellable`)

**Interfaces:**
- Consumes: `Blocking::run_cancellable`, `Blocking::cancelled` from Task 2.
- Produces: `fn run_git_cancellable(blocking: &jobs::Blocking, cwd: &Path, args: &[&str]) -> Result<(), String>` (private to `worktree.rs`).

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `alacritree/src/worktree.rs`:

```rust
/// `create` must stop between steps when its handle is gone.  Killing a
/// registered child only covers the steps that have one; the local steps
/// would otherwise run to completion for a worktree nobody is waiting for.
#[test]
fn create_stops_between_steps_once_cancelled() {
    let repo = tempfile::tempdir().expect("temp dir");
    let req = CreateRequest {
        project_root: repo.path().to_path_buf(),
        default_branch: Some("main".into()),
        branch: "topic".into(),
        base_dir: None,
    };
    let (tx, rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let job = jobs::pool().spawn(jobs::Priority::Interactive, move |blocking| {
        // Both halves of this handshake are load-bearing.  Without the
        // started signal, a flag set while the task is still queued hits the
        // pre-start check, the task is skipped, `tx` drops unsent, and the
        // assertion below reports a disconnect.  Without the gate, the task
        // can race past the first bail before the flag lands and fail on the
        // missing remote instead.
        let _ = started_tx.send(());
        let _ = gate_rx.recv();
        let _ = tx.send(create(&req, |_| {}, blocking));
    });
    started_rx.recv_timeout(Duration::from_secs(5)).expect("the job never started");
    drop(job);
    let _ = gate_tx.send(());
    let result = rx.recv_timeout(Duration::from_secs(10));
    match result {
        Ok(Err(msg)) => assert!(
            msg.contains("cancelled"),
            "create failed for the wrong reason: {msg}"
        ),
        Ok(Ok(path)) => panic!("create finished a worktree nobody was waiting for: {path:?}"),
        Err(e) => panic!("create never returned: {e}"),
    }
}
```

The temp dir is not a git repository, so `has_remote` returns false and `create` would normally fail with "no `origin` remote configured". The cancel check must come first, which is what this asserts. Step 4 puts the first `bail_if_cancelled!()` ahead of `has_remote`, so no repository fixture is needed to reach it.

- [ ] **Step 2: Run it and watch it fail**

```sh
cargo test -p alacritree --bin alacritree worktree::tests::create_stops_between_steps_once_cancelled -- --exact
```

Expected: FAIL with `create failed for the wrong reason: no 'origin' remote configured`. Cancellation is not consulted.

- [ ] **Step 3: Add the cancellable git runner**

Add beside `run_git` in `alacritree/src/worktree.rs`:

```rust
/// `run_git`, for a call a cancel is allowed to end.  Progress goes to a
/// pipe, where git suppresses it, so the output stays small enough that the
/// undrained pipes cannot fill.
#[allow(clippy::disallowed_methods)] // Running git is this function's job.
fn run_git_cancellable(
    blocking: &jobs::Blocking,
    cwd: &Path,
    args: &[&str],
) -> Result<(), String> {
    let output = blocking
        .run_cancellable(
            git_command(cwd).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()),
        )
        .map_err(|e| format!("failed to run git: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let msg = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
    Err(format!("git {}: {msg}", args.join(" ")))
}
```

`git_command` returns `Command` by value, so bind it to a local `let mut cmd = git_command(cwd);` first if the borrow checker objects to the chained form.

- [ ] **Step 4: Check the flag between steps and make the fetch cancellable**

In `create`, add a guard helper at the top and use it before each step:

```rust
pub fn create(
    req: &CreateRequest,
    mut on_step: impl FnMut(&str),
    blocking: &jobs::Blocking,
) -> Result<PathBuf, String> {
    let send = &mut on_step;
    // A cancel that lands between children has nothing to kill, so each step
    // asks before starting rather than running for a caller that is gone.
    macro_rules! bail_if_cancelled {
        () => {
            if blocking.cancelled() {
                return Err("worktree create cancelled".into());
            }
        };
    }

    bail_if_cancelled!();
    send("Syncing with remote…");
    if !has_remote(&req.project_root, "origin") {
        return Err("no `origin` remote configured".into());
    }

    // … resolve_base_branch unchanged …

    bail_if_cancelled!();
    send("Fetching latest changes…");
    run_git_cancellable(blocking, &req.project_root, &["fetch", "origin", &base])?;

    bail_if_cancelled!();
    send("Creating git worktree…");
    let target =
        pick_worktree_path(&req.project_root, &req.branch, req.base_dir.as_deref(), blocking)?;
    let target_arg = git_path_arg(&req.project_root, &target)?;
    run_git(&req.project_root, &["worktree", "add", &target_arg, "-b", &req.branch, &base_ref])?;

    bail_if_cancelled!();
    send("Copying LLM configurations…");
    // … remainder unchanged …
}
```

`git worktree add` stays on `run_git`. Killing it can leave a registered but incomplete entry in `.git/worktrees/`, and the local steps are fast enough that the worker returns anyway.

- [ ] **Step 5: Run the test and the suite**

```sh
cargo test -p alacritree --bin alacritree worktree::tests -- --nocapture
cargo test -p alacritree
```

Expected: the new test PASSES, everything else still passes.

- [ ] **Step 6: Run the audit and clippy**

```sh
python3 alacritree/tools/ui-thread-audit.py .
cargo clippy -p alacritree --all-targets -- -D clippy::disallowed_methods
```

Expected: `blocking leaves reachable from update: 0`, and clippy clean.

- [ ] **Step 7: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/worktree.rs
git add alacritree/src/worktree.rs
git commit -m "$(cat <<'EOF'
fix(worktree): end a create whose caller has gone

The fetch runs for as long as an unreachable remote takes to give up,
which is forever, and the steps around it kept going for a worktree
nobody was waiting for.

The fetch now runs as a child a cancel may kill, and each step asks
whether the caller is still there before starting.  `git worktree add`
stays uninterruptible: killing it leaves a registered but incomplete
entry, and the local steps finish fast enough that the worker returns
without it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: The IPC create deadline

The connection thread blocks on `rx.recv()` forever while both clients give up at 300 s, so a create against an unreachable remote parks a worker permanently and nobody who asked can see it.

**Files:**
- Modify: `alacritree/src/ipc.rs` (`create_worktree`)

**Interfaces:**
- Consumes: `Job::drop` killing a registered child, from Task 2.
- Produces: `const IPC_CREATE_BUDGET: Duration` in `ipc.rs`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `alacritree/src/ipc.rs`. The test drives the deadline logic directly rather than standing up a socket:

```rust
/// The deadline is absolute.  A per-message timeout resets on every progress
/// step, so a job that keeps reporting outlives the budget indefinitely: the
/// same parked worker, reached more slowly.
#[test]
fn a_dribbling_create_still_ends_at_the_budget() {
    let (tx, rx) = mpsc::channel::<wt::Progress>();
    let budget = Duration::from_millis(300);
    let step = Duration::from_millis(50);

    std::thread::spawn(move || {
        // Never sends `Done`; a real hung fetch reports and then stops.
        for _ in 0..100 {
            if tx.send(wt::Progress::Step("working".into())).is_err() {
                return;
            }
            std::thread::sleep(step);
        }
    });

    let started = Instant::now();
    let outcome = drain_create(&rx, budget);
    let elapsed = started.elapsed();

    assert!(outcome.is_err(), "a create that never finished reported success");
    assert!(
        elapsed < budget * 3,
        "the deadline reset on every step: took {elapsed:?} against a {budget:?} budget"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

```sh
cargo test -p alacritree --bin alacritree ipc::tests::a_dribbling_create_still_ends_at_the_budget -- --exact
```

Expected: FAIL to compile, `cannot find function drain_create`. The loop is inline in `create_worktree` and cannot be tested.

- [ ] **Step 3: Extract the drain loop with an absolute deadline**

Add to `alacritree/src/ipc.rs`:

```rust
/// How long this process will hold a pool worker for one create request.
/// The server's own limit, not a mirror of any client's: a client with a
/// different timeout changes nothing here.
const IPC_CREATE_BUDGET: Duration = Duration::from_secs(300);

/// Collect a create's progress until it finishes or the budget runs out.
///
/// The deadline is computed once.  A per-message timeout would reset on every
/// step, so a job that keeps reporting would hold its worker past any budget.
fn drain_create(
    rx: &Receiver<wt::Progress>,
    budget: Duration,
) -> Result<(PathBuf, Vec<String>), String> {
    let deadline = Instant::now() + budget;
    let mut steps = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(wt::Progress::Step(s)) => steps.push(s),
            Ok(wt::Progress::Done(Ok(path))) => return Ok((path, steps)),
            Ok(wt::Progress::Done(Err(e))) => return Err(e),
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!("worktree create exceeded {}s", budget.as_secs()));
            },
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the worktree create ended without reporting".into());
            },
        }
    }
}
```

Add `use std::sync::mpsc::{Receiver, RecvTimeoutError};` and `use std::time::{Duration, Instant};` to the file's imports if absent.

- [ ] **Step 4: Call it from `create_worktree`**

Replace the inline loop. The `Job` must drop on the timeout path, which is what kills the fetch:

```rust
let (rx, job) = wt::spawn_create(req, ctx.clone());
let outcome = drain_create(&rx, IPC_CREATE_BUDGET);
// Dropping on every path, including the deadline, is what ends the fetch and
// returns the worker.  Holding it would leave the pool one worker smaller
// with nothing on screen or in the reply saying so.
drop(job);
match outcome {
    Ok((path, steps)) => {
        let _ = call_app(IpcRequest::RefreshProject { root: project_root }, app_tx, ctx);
        // … existing success reply, using `path` and `steps` …
    },
    Err(e) => Err(e),
}
```

Preserve whatever the existing success arm returned; only the loop and the drop are new.

- [ ] **Step 5: Run the test and the suite**

```sh
cargo test -p alacritree --bin alacritree ipc::tests -- --nocapture
cargo test -p alacritree
```

Expected: the new test PASSES, everything else still passes.

- [ ] **Step 6: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/ipc.rs
git add alacritree/src/ipc.rs
git commit -m "$(cat <<'EOF'
fix(ipc): bound how long a create may hold a worker

The connection thread waited on the create's progress channel with no
limit while every client gave up after five minutes, so a create against
an unreachable remote parked a pool worker for the life of the process
and the caller who caused it had already been answered.

The thread now carries its own deadline, computed once, and drops the
job when it expires: the fetch dies and the worker returns.  Computed
once because a per-message timeout resets on every progress step, which
is the same parked worker reached more slowly.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Clamp `gh` concurrency to the pool's ceiling

Closes #33. `pr_status_concurrency` defaults to 8 while the pool admits at most `workers - 1` background tasks, so the setting exceeds its own ceiling on every machine and reads as a knob that raises what it can only lower.

**Files:**
- Modify: `alacritree/src/jobs.rs` (add `Pool::background_ceiling`)
- Modify: `alacritree/src/config.rs` (`Ui.pr_status_concurrency`, its doc comment, the resolver)
- Modify: `alacritree/src/pr_status.rs` (`PrCache.concurrency`, `set_concurrency`, `effective_cap`, remove `DEFAULT_CONCURRENCY`)
- Modify: `alacritree/src/app.rs:883,957` (pass the `Option` through)
- Modify: `schema/alacritree-config.json` (regenerated, not hand-edited)

**Interfaces:**
- Consumes: `State.interactive_running` from Task 1 (no direct use; the ceiling is `workers - 1` either way).
- Produces:
  - `pub fn Pool::background_ceiling(&self) -> usize`
  - `fn effective_cap(configured: Option<usize>, ceiling: usize) -> usize` in `pr_status.rs`
  - `PrCache::set_concurrency(&mut self, configured: Option<usize>)`
  - `Ui.pr_status_concurrency: Option<usize>`

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `alacritree/src/pr_status.rs`:

```rust
/// `gh` is the slowest thing the pool runs and the least urgent.  Letting it
/// take the last background slot puts the git status panel, which is what a
/// user reads to decide what to do next, behind a network call.
#[test]
fn gh_never_takes_the_last_background_slot() {
    // A four-worker pool admits three background tasks; an eight-worker one,
    // seven.
    assert_eq!(effective_cap(None, 3), 2);
    assert_eq!(effective_cap(None, 7), 6);
}

/// The setting lowers the cap and never raises it, which is what its doc
/// comment already claims.
#[test]
fn the_configured_cap_can_only_lower() {
    assert_eq!(effective_cap(Some(1), 7), 1);
    assert_eq!(effective_cap(Some(99), 7), 6);
}

/// A two-worker pool has a background ceiling of one, and one minus the
/// reservation is zero, which would admit no lookup at all.
#[test]
fn the_cap_never_reaches_zero() {
    assert_eq!(effective_cap(None, 1), 1);
    assert_eq!(effective_cap(Some(0), 7), 1);
}
```

- [ ] **Step 2: Run them and watch them fail**

```sh
cargo test -p alacritree --bin alacritree pr_status::tests::gh_never -- --exact
```

Expected: FAIL to compile, `cannot find function effective_cap`.

- [ ] **Step 3: Expose the pool's ceiling**

Add to `impl Pool` in `alacritree/src/jobs.rs`:

```rust
/// The most background tasks this pool runs at once.  Callers that keep
/// their own admission count clamp against this rather than inventing a
/// number that a differently sized pool would make wrong.
pub fn background_ceiling(&self) -> usize {
    self.shared.workers - 1
}
```

- [ ] **Step 4: Add `effective_cap` and rework `set_concurrency`**

In `alacritree/src/pr_status.rs`, delete `pub const DEFAULT_CONCURRENCY: usize = 8;` and add:

```rust
/// How many lookups may run at once: what the config asks for, never above
/// one below the pool's background ceiling.  Reserving that slot is the
/// pool's own trick one level down, and it keeps local background work a
/// worker on any pool size rather than by picking a number.
fn effective_cap(configured: Option<usize>, ceiling: usize) -> usize {
    configured.unwrap_or(usize::MAX).min(ceiling.saturating_sub(1)).max(1)
}
```

Change `PrCache::default` to `concurrency: effective_cap(None, jobs::pool().background_ceiling())`, and:

```rust
pub fn set_concurrency(&mut self, configured: Option<usize>) {
    self.concurrency = effective_cap(configured, jobs::pool().background_ceiling());
}
```

- [ ] **Step 5: Carry the `Option` through the config**

In `alacritree/src/config.rs`, change `pub pr_status_concurrency: usize` to `Option<usize>`, update its doc comment:

```rust
/// `[ui] pr_status_concurrency`: max `gh` lookups in flight at once.
/// Unset lets the pool decide, which is one below its own background
/// ceiling so a lookup can never take the last slot local work needs.
/// A value lowers that; nothing raises it, because the pool's ceiling
/// binds underneath either way.
pub pr_status_concurrency: Option<usize>,
```

Set the `Default` arm to `pr_status_concurrency: None`, and the resolver at
`config.rs:2220` to `pr_status_concurrency: self.ui.pr_status_concurrency`.
Mirror the wording on the `RawUi` field's doc comment.

Two things follow from that resolver line losing its `.unwrap_or(...).max(1)`:

- Delete `use crate::pr_status::DEFAULT_CONCURRENCY;` at `config.rs:27`. The
  constant is gone and nothing else in the file uses it.
- Two tests at `config.rs:3273` and `config.rs:3279` assert the old resolver.
  Rewrite `pr_status_concurrency_defaults_to_eight` to assert `None` when
  unset and `Some(4)` for `pr_status_concurrency = 4`, and rename it
  `pr_status_concurrency_is_unset_by_default`. Delete
  `pr_status_concurrency_clamps_to_one` outright: clamping now lives in
  `effective_cap`, which Step 1's test covers. Do not re-add a clamp to the
  resolver to keep it passing.

- [ ] **Step 6: Update the call site**

`alacritree/src/app.rs:883` and `:957` already pass the field straight through; they need no change once the type is `Option<usize>`. Run `cargo check -p alacritree` and fix whatever the compiler names.

- [ ] **Step 7: Regenerate the schema**

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
cargo test -p alacritree --test config_schema
```

Expected: the first regenerates `schema/alacritree-config.json`, the second passes against it.

- [ ] **Step 8: Run the suite**

```sh
cargo test -p alacritree
```

- [ ] **Step 9: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/jobs.rs alacritree/src/pr_status.rs alacritree/src/config.rs
git add alacritree/src/jobs.rs alacritree/src/pr_status.rs alacritree/src/config.rs alacritree/src/app.rs schema/alacritree-config.json
git commit -m "$(cat <<'EOF'
fix(pr-status): keep gh lookups off the last background slot

The setting defaulted to eight while the pool admits at most one below
its worker count, so it exceeded its own ceiling on every machine and
read as a knob that raises concurrency when it can only lower it.  The
excess queued, and the git status panel waited behind a network call for
no reason a user could infer.

The cap now comes from the pool: one below its background ceiling, so a
lookup can never take the slot local work needs, on any pool size.  The
config value lowers that and nothing raises it, which is what the doc
comment already claimed.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: An injectable clock for `PrCache`

Closes #37. Two TTL tests build a stale start with `checked_sub` and return early when it underflows, so they assert nothing on a machine up for less than the TTL, which is exactly what a fresh CI runner is.

**Files:**
- Modify: `alacritree/src/pr_status.rs` (`PrCache`, `Entry.queried_at`, `Pending.started`, `should_spawn`, tests)

**Interfaces:**
- Consumes: `effective_cap` from Task 5.
- Produces:
  - `PrCache.clock: Box<dyn Fn() -> Duration + Send>`, elapsed since the cache's own origin.
  - `PrCache::with_clock(clock: impl Fn() -> Duration + Send + 'static) -> Self`
  - `Entry.queried_at: Option<Duration>`, `Pending.started: Duration`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `alacritree/src/pr_status.rs`:

```rust
/// The TTL boundary itself, which `Instant` arithmetic could not reach: an
/// `Instant` cannot be constructed or advanced, so a test could only subtract
/// from now and hope the machine had been up long enough.
#[test]
fn the_ttl_boundary_is_exact() {
    let now = Arc::new(Mutex::new(Duration::ZERO));
    let reader = Arc::clone(&now);
    let mut cache = PrCache::with_clock(move || *reader.lock().expect("clock poisoned"));

    cache.entries.insert(
        PathBuf::from("/repo"),
        Entry { branch: Some("main".into()), queried_at: Some(Duration::ZERO), ..Entry::default() },
    );

    *now.lock().expect("clock poisoned") = TTL - Duration::from_nanos(1);
    assert!(!should_spawn(Some("main"), Some("main"), Some(Duration::ZERO), false, cache.now()));

    *now.lock().expect("clock poisoned") = TTL;
    assert!(should_spawn(Some("main"), Some("main"), Some(Duration::ZERO), false, cache.now()));
}
```

Match `should_spawn`'s real parameter list; the point is that its time arguments are `Duration` and the last one comes from the cache's clock.

`pr_status.rs` imports neither `Arc` nor `Mutex`, so add
`use std::sync::{Arc, Mutex};` to the test module.

- [ ] **Step 2: Run it and watch it fail**

```sh
cargo test -p alacritree --bin alacritree pr_status::tests::the_ttl_boundary_is_exact -- --exact
```

Expected: FAIL to compile, `no function or associated item named with_clock`.

- [ ] **Step 3: Give `PrCache` a clock**

```rust
pub struct PrCache {
    entries: HashMap<PathBuf, Entry>,
    in_flight: usize,
    concurrency: usize,
    generation: u64,
    /// Elapsed since this cache was built.  A `Duration` rather than an
    /// `Instant` because an `Instant` cannot be constructed or advanced, so
    /// nothing could set one to test a boundary against.
    clock: Box<dyn Fn() -> Duration + Send>,
}

impl PrCache {
    pub fn with_clock(clock: impl Fn() -> Duration + Send + 'static) -> Self {
        Self {
            entries: HashMap::new(),
            in_flight: 0,
            concurrency: effective_cap(None, jobs::pool().background_ceiling()),
            generation: 0,
            clock: Box::new(clock),
        }
    }

    fn now(&self) -> Duration {
        (self.clock)()
    }
}

impl Default for PrCache {
    fn default() -> Self {
        let origin = Instant::now();
        Self::with_clock(move || origin.elapsed())
    }
}
```

- [ ] **Step 4: Convert the stored timestamps**

Change `Entry.queried_at` to `Option<Duration>` and `Pending.started` to `Duration`. Replace every `Instant::now()` inside `PrCache`'s methods with `self.now()`. Update `should_spawn` so its time parameters are `Duration`, comparing `now.saturating_sub(queried_at) >= TTL`.

`self.now()` borrows `self` immutably, so calling it inside a loop over
`self.entries.values_mut()` is E0502. Hoist `let now = self.now();` above the
loop in `drain_completed`, `bank_pending` and `poll`, and use the local.

- [ ] **Step 5: Delete the underflow workaround**

Remove `stale_start()` and its `checked_sub`, and rewrite the two tests that called it to set the clock instead. Each becomes: build the cache with a settable clock, insert an entry with `queried_at: Some(Duration::ZERO)`, advance the clock past `TTL`, assert. Neither test returns early any more, so both assert on every machine.

- [ ] **Step 6: Run the suite**

```sh
cargo test -p alacritree --bin alacritree pr_status::tests -- --nocapture
cargo test -p alacritree
```

Expected: the boundary test passes, the two rewritten tests pass, everything else still passes.

- [ ] **Step 7: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/pr_status.rs
git add alacritree/src/pr_status.rs
git commit -m "$(cat <<'EOF'
test(pr-status): make the TTL tests assert whatever the uptime

Two tests built a stale start by subtracting the TTL from now.  An
`Instant` is monotonic since boot, so on a machine up for less than the
TTL that subtraction underflowed, the helper returned `None`, and both
tests returned early reporting success while measuring nothing.  A fresh
CI runner sits in exactly that window.

The cache now reads elapsed time through a closure a test can set, and
stores `Duration` rather than `Instant`.  The subtraction that could
underflow is gone, uptime stops being an input, and the boundary itself
is testable rather than only a point well past it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Build the batched PR query

First half of #44. Every worktree of a project asks the same repository the same question in its own `gh` process, and they all become due together when the TTL expires.

Task 7 builds the query and the parser as a pure module with no process
spawning. Task 8 rewires the cache to use them.

**Files:**
- Create: `alacritree/src/pr_query.rs` (query construction and response parsing, no process spawning)
- Modify: `alacritree/src/pr_status.rs` (extract `select_and_build` for both paths to share)
- Modify: `alacritree/src/main.rs` (declare the module)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces, in `pr_query.rs`:
  - `pub const CHUNK: usize = 100;`
  - `pub fn build(owner: &str, name: &str, branches: &[String]) -> String`, the GraphQL document.
  - `pub fn body(query: &str) -> String`, the query JSON-encoded as `{"query": …}`.
  - `pub fn parse(stdout: &[u8], branches: &[String], origin_owner: Option<&str>) -> HashMap<String, PrInfo>`, alias index back to branch name; a missing or errored alias is simply absent.

- [ ] **Step 1: Create the module, declare it, and write the failing tests**

Add `mod pr_query;` to `alacritree/src/main.rs`, in the existing alphabetical
run of module declarations, in the same step that creates the file. An
undeclared module is not compiled at all, so the tests below would report zero
tests run and exit 0 rather than failing.

Create `alacritree/src/pr_query.rs` with a test module:

```rust
#[test]
fn one_alias_per_branch() {
    let q = build("owner", "repo", &["main".into(), "topic".into()]);
    assert!(q.contains("b0: pullRequests(headRefName: \"main\""), "{q}");
    assert!(q.contains("b1: pullRequests(headRefName: \"topic\""), "{q}");
    assert!(q.contains("repository(owner: \"owner\", name: \"repo\")"), "{q}");
}

/// `gh api graphql --input -` reads a JSON body.  A bare query piped in comes
/// back as HTTP 502, which reads like a transient GitHub failure and is not.
#[test]
fn the_body_is_json_wrapped() {
    let body = body("query { x }");
    let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(v["query"], "query { x }");
}

/// One request answers for many branches, so an error on one alias must leave
/// the rest usable rather than losing a whole project's badges.
#[test]
fn a_failed_alias_leaves_the_others() {
    let stdout = br#"{"data":{"repository":{
        "b0":{"nodes":[{"number":7,"baseRefName":"master","url":"u","state":"OPEN",
                        "isDraft":false,"headRepositoryOwner":{"login":"me"}}]},
        "b1":null}},
        "errors":[{"message":"something went wrong"}]}"#;
    let found = parse(stdout, &["main".into(), "topic".into()], Some("me"));
    assert_eq!(found.get("main").map(|p| p.number), Some(7));
    assert!(!found.contains_key("topic"));
}

/// A head ref name matches across head repositories, so several PRs can come
/// back and the local origin's owner is what picks this checkout's own.
#[test]
fn the_origin_owner_breaks_a_tie() {
    let stdout = br#"{"data":{"repository":{"b0":{"nodes":[
        {"number":1,"baseRefName":"master","url":"a","state":"OPEN",
         "isDraft":false,"headRepositoryOwner":{"login":"someone-else"}},
        {"number":2,"baseRefName":"master","url":"b","state":"OPEN",
         "isDraft":false,"headRepositoryOwner":{"login":"me"}}]}}}}"#;
    let found = parse(stdout, &["main".into()], Some("me"));
    assert_eq!(found.get("main").map(|p| p.number), Some(2));
}
```

- [ ] **Step 2: Run them and watch them fail**

```sh
cargo test -p alacritree --bin alacritree pr_query -- --nocapture
```

Expected: FAIL to compile, the module is not declared and the functions do not exist.

- [ ] **Step 3: Write the query builder**

In `alacritree/src/pr_query.rs`:

```rust
//! Ask GitHub about many branches in one request.
//!
//! One `gh` process per worktree meant every worktree of a project asking the
//! same repository the same question, and all of them becoming due together
//! when the TTL expired.  Naming the exact head refs is what keeps the cost
//! proportional to branch count rather than to how many PRs the repository
//! has, which is what makes one request viable at all.

/// Aliases per request.  Measured: no error appeared at any size up to 398,
/// rate limit stays at one point through 100, and per-branch time flattens
/// around 50.  100 wins at the concurrency this pool actually leaves for
/// lookups, and is also under the ceiling a Windows command line would impose
/// if the query ever moved off stdin.
pub const CHUNK: usize = 100;

const FIELDS: &str = "number baseRefName url state isDraft headRepositoryOwner { login }";

/// `first: 5` because a head ref name matches across head repositories, so
/// several PRs can share one, and the owner tiebreak needs them all to choose
/// from.
pub fn build(owner: &str, name: &str, branches: &[String]) -> String {
    let mut q = format!("query {{ repository(owner: \"{owner}\", name: \"{name}\") {{");
    for (i, branch) in branches.iter().enumerate() {
        q.push_str(&format!(
            " b{i}: pullRequests(headRefName: \"{branch}\", states: [OPEN, MERGED, CLOSED], \
             first: 5, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ {FIELDS} }} }}"
        ));
    }
    q.push_str(" } }");
    q
}

/// `gh api graphql --input -` reads a JSON body, so the query is wrapped
/// rather than piped raw.
pub fn body(query: &str) -> String {
    serde_json::json!({ "query": query }).to_string()
}
```

Branch names reaching `build` have already passed `worktree::validate_branch_name`, which rejects whitespace and control characters, so they need no further escaping. Add a comment saying so.

- [ ] **Step 4: Write the parser**

```rust
/// Alias index back to branch, dropping any alias GitHub could not answer.
/// A partial response is normal here: one request covers a whole project, so
/// losing all of it because one branch failed would be worse than losing one.
pub fn parse(
    stdout: &[u8],
    branches: &[String],
    origin_owner: Option<&str>,
) -> HashMap<String, PrInfo> {
    let mut found = HashMap::new();
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(stdout) else {
        return found;
    };
    let Some(repo) = v.pointer("/data/repository") else {
        return found;
    };
    for (i, branch) in branches.iter().enumerate() {
        let Some(nodes) = repo.get(format!("b{i}")).and_then(|a| a.get("nodes")) else {
            continue;
        };
        let Some(list) = nodes.as_array() else { continue };
        if let Some(info) = crate::pr_status::select_and_build(list, origin_owner) {
            found.insert(branch.clone(), info);
        }
    }
    found
}
```

`select_and_build` is `pr_status::parse_gh_output`'s existing body from `select_pr` onward, made `pub(crate)` and taking `&[serde_json::Value]`. Extract it in this step rather than duplicating the field reads; `parse_gh_output` then calls it too, so the fallback path and the batched path agree by construction.

- [ ] **Step 5: Run the unit tests**

```sh
cargo test -p alacritree --bin alacritree pr_query -- --nocapture
cargo test -p alacritree
```

Expected: the four `pr_query` tests PASS, and the existing `pr_status` tests still pass through the extracted `select_and_build`.

- [ ] **Step 6: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/pr_query.rs alacritree/src/pr_status.rs
git add alacritree/src/pr_query.rs alacritree/src/pr_status.rs alacritree/src/main.rs
git commit -m "$(cat <<'MSG'
feat(pr-status): build a batched PR query for a whole repository

One aliased `pullRequests(headRefName:)` selection per branch answers a
whole project in one request, and the parser maps each alias back to its
branch.  A partial response keeps every alias that did answer, since one
failed branch would otherwise cost a project all of its badges.

Nothing calls this yet.  The cache still asks per worktree.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
MSG
)"
```

---

### Task 8: Rewire the cache to batch

Closes #44. `PrCache` tracks one job per entry, so it cannot hold a job whose
result covers many entries. This moves the job to the cache and leaves each
entry with a flag.

**Files:**
- Modify: `alacritree/src/pr_status.rs` (`Entry`, `Pending`, `poll`, `drain_completed`, `bank_pending`, `invalidate_all`, and the tests built on them)
- Modify: `alacritree/src/app.rs` (`drain_completed` gains a context parameter)

**Interfaces:**
- Consumes: `pr_query::{build, body, parse, CHUNK}` (Task 7), `effective_cap` (Task 5), `PrCache::now` (Task 6), `Blocking` (Task 2).
- Produces, all private to `pr_status.rs`:
  - `struct Member { path: PathBuf, branch: String }`
  - `type BatchResult = HashMap<PathBuf, Option<PrInfo>>`
  - `struct Batch { job: jobs::Job<BatchResult>, started: Duration, members: Vec<Member> }`
  - `struct Group { cwd: PathBuf, slug: Option<(String, String)>, members: Vec<Member> }`
  - `fn run_due(due: Vec<Member>, blocking: &jobs::Blocking) -> BatchResult`
  - `fn groups(due: Vec<Member>, blocking: &jobs::Blocking) -> Vec<Group>`
  - `fn query_group(group: &Group, blocking: &jobs::Blocking) -> HashMap<String, PrInfo>`
  - `fn origin_slug(path: &Path) -> Option<(String, String)>`

**Grouping runs on the worker, not on the frame.** `groups` opens each due
path with `git2` to read its `origin`. That is a blocking call, so a frame that
grouped before spawning would put one back on the UI thread and fail the audit.
One job therefore takes the whole frame's due list, groups it, and runs each
group's request in turn. Repositories are few (one per project, not one per
worktree), so the sequential round trips cost less than the machinery to
parallelise them would.

**What stays on the per-branch path.** A group whose `slug` is `None` runs
today's `query_gh` once per member instead of one GraphQL request. Three cases
land there, and all three must:

- A WSL worktree. `git2` cannot open a repository inside a distro, so nothing
  can read its `origin` to group it, and its `gh` runs as a script through the
  resident helper rather than as a `Command` a pipe can be attached to.
- A worktree whose `origin` is missing, unreadable, or not GitHub.
- Any group whose GraphQL request fails. GraphQL can need scopes `gh pr list`
  does not, so an install that works today can fail here.

Without this, every WSL badge would silently vanish.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/pr_status.rs`:

```rust
/// One result covers many entries, so the drain has to fan a single map out
/// across every path that contributed to it.
#[test]
fn one_banked_result_reaches_every_member() {
    let ctx = egui::Context::default();
    let mut cache = PrCache::new();
    let members = vec![
        Member { path: PathBuf::from("/repo/a"), branch: "topic-a".into() },
        Member { path: PathBuf::from("/repo/b"), branch: "topic-b".into() },
    ];
    let job = jobs::Pool::new(2).spawn(jobs::Priority::Background, |_| {
        HashMap::from([
            (PathBuf::from("/repo/a"), Some(PrInfo {
                number: 7,
                base_branch: "master".into(),
                url: "u".into(),
                state: PrState::Open,
            })),
            (PathBuf::from("/repo/b"), None),
        ])
    });
    cache.bank_batch(members, job);
    assert_eq!(cache.in_flight(), 1, "one request, not one per branch");

    for _ in 0..200 {
        cache.drain_completed(&ctx);
        if cache.in_flight() == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(cache.in_flight(), 0, "the request never reported");

    assert_eq!(cache.state(Path::new("/repo/a"), Some("topic-a")), Some(PrState::Open));
    // Asked about and absent from the answer means no PR, not "never asked":
    // the entry must be stamped, or it re-queries on the very next frame.
    assert_eq!(cache.state(Path::new("/repo/b"), Some("topic-b")), None);
    assert!(!cache.is_due(Path::new("/repo/b"), "topic-b"), "banked as no-PR, not left due");
}

/// A WSL worktree has no `origin` git2 can read and no `Command` to pipe a
/// query into.  Grouping must leave it on the per-branch path rather than
/// dropping it, or its badge disappears.
#[test]
fn an_ungroupable_path_still_gets_its_own_group() {
    let dir = tempfile::tempdir().expect("temp dir");
    let due = vec![Member { path: dir.path().to_path_buf(), branch: "topic".into() }];
    let out = jobs::on_this_thread(|blocking| groups(due, blocking));
    assert_eq!(out.len(), 1);
    assert!(out[0].slug.is_none(), "a repo with no readable origin cannot be grouped");
    assert_eq!(out[0].members.len(), 1);
}

/// Branches of one repository share a request; a chunk boundary splits them
/// into two rather than growing one request without limit.
#[test]
fn one_repository_chunks_at_the_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo = init_repo(dir.path());
    repo.remote("origin", "https://github.com/owner/repo.git").expect("remote");
    let due: Vec<Member> = (0..pr_query::CHUNK + 1)
        .map(|i| Member { path: dir.path().to_path_buf(), branch: format!("b{i}") })
        .collect();

    let out = jobs::on_this_thread(|blocking| groups(due, blocking));

    assert_eq!(out.len(), 2, "one chunk over the limit is two requests");
    assert!(out.iter().all(|g| g.slug == Some(("owner".into(), "repo".into()))));
    assert_eq!(out.iter().map(|g| g.members.len()).sum::<usize>(), pr_query::CHUNK + 1);
}
```

`init_repo` comes from `crate::test_util`; add it to the test module's imports.

- [ ] **Step 2: Run them and watch them fail**

```sh
cargo test -p alacritree --bin alacritree pr_status::tests::one_banked_result -- --exact
```

Expected: FAIL to compile, `no method named bank_batch found for struct PrCache`. `Member`, `groups` and `is_due` are missing too.

- [ ] **Step 3: Move the job from the entry to the cache**

Replace `Pending` with:

```rust
/// A worktree and the branch its badge is keyed to.  Carried through
/// grouping and back out through the drain, so a batched answer can find
/// every entry that asked for it.
#[derive(Debug, Clone, PartialEq)]
struct Member {
    path: PathBuf,
    branch: String,
}

/// One request in flight, and every entry waiting on it.  A job that never
/// reports would otherwise hold its concurrency slot forever: a panicked one
/// reports through `Job::failed` immediately, a merely slow one is backed off
/// once it has been in flight past the TTL.
struct Batch {
    job: jobs::Job<BatchResult>,
    started: Duration,
    members: Vec<Member>,
}

/// What one request reports back: an answer per worktree it covered.  Keyed by
/// path rather than by branch, because two repositories in one burst can hold
/// the same branch name.  `None` means the request covered that path and found
/// no PR, which is a real answer.
type BatchResult = HashMap<PathBuf, Option<PrInfo>>;
```

`LookupResult` loses its last use once Step 5 lands; delete it then.

In `Entry`, replace the `pending: Option<Pending>` field with:

```rust
    /// Set from the moment this entry joins the due list until its answer is
    /// banked.  `should_spawn` reads it to avoid asking twice for one badge,
    /// so it has to cover the queued frame as well as the running one.
    pending: bool,
```

Add to `PrCache`:

```rust
    /// Requests in flight.  `in_flight` counts these rather than branches:
    /// the cap exists to bound `gh` processes, and one batch is one process
    /// however many branches it names.
    batches: Vec<Batch>,
    /// Entries that asked for a lookup this frame, grouped and spawned by the
    /// next `drain_completed`.  Batching needs a whole frame's worth of due
    /// entries before it can group them, which one `poll` call cannot see.
    due: Vec<Member>,
```

Initialise both empty in `with_clock`.

- [ ] **Step 4: Make `poll` queue instead of spawn**

In `poll`, replace the `if spawn && may_spawn(...)` block that calls
`spawn_lookup` and `bank_pending`:

```rust
        if spawn {
            let entry = self.entries.entry(path.to_path_buf()).or_default();
            // Clear stale data immediately on branch switch so we don't show
            // a PR base that belongs to a different branch.
            if should_invalidate(entry.branch.as_deref(), Some(branch)) {
                entry.info = None;
            }
            entry.branch = Some(branch.to_string());
            entry.pending = true;
            self.due.push(Member { path: path.to_path_buf(), branch: branch.to_string() });
        }
```

Delete `spawn_lookup`: `spawn_due` in Step 6 replaces it. The concurrency cap
moves out of `poll` with it, because how many requests a due list becomes is
not known until it has been grouped.

`ctx` stops being read in `poll`. Drop the parameter and fix the call sites the
compiler names; `drain_completed` carries the context the spawn needs.

Add the accessor the test needs:

```rust
    #[cfg(test)]
    fn is_due(&self, path: &Path, branch: &str) -> bool {
        let now = self.now();
        self.entries.get(path).is_none_or(|e| {
            should_spawn(e.branch.as_deref(), Some(branch), e.queried_at, e.pending, now)
        })
    }
```

- [ ] **Step 5: Rewrite the drain**

Replace `drain_completed` and `bank_pending`:

```rust
    /// Bank every finished request and free its slot, then turn the frame's
    /// due list into new requests.  Runs once a frame ahead of every poll
    /// site rather than inside `poll`: an entry whose project collapsed
    /// mid-lookup is never polled again, and a slot it still held would never
    /// come back.
    pub fn drain_completed(&mut self, ctx: &egui::Context) {
        let now = self.now();
        let mut banked = false;
        let mut still_running = Vec::new();
        for mut batch in std::mem::take(&mut self.batches) {
            if let Some(found) = batch.job.poll() {
                for m in &batch.members {
                    self.settle(m, found.get(&m.path).cloned().flatten(), now);
                }
                banked = true;
            } else if batch.job.failed() || now.saturating_sub(batch.started) > TTL {
                // A request that never reports has no answer to bank, but its
                // members must still be stamped: leaving them due re-spawns a
                // `gh` process every frame for as long as the failure lasts.
                for m in &batch.members {
                    self.back_off(m, now);
                }
            } else {
                still_running.push(batch);
                continue;
            }
            self.in_flight = self.in_flight.saturating_sub(1);
        }
        self.batches = still_running;
        if banked {
            self.generation = self.generation.wrapping_add(1);
        }
        self.spawn_due(ctx);
    }

    /// Record one member's answer.  `None` means the request covered this
    /// branch and found no PR, which is a real answer and gets stamped.
    fn settle(&mut self, m: &Member, info: Option<PrInfo>, now: Duration) {
        let entry = self.entries.entry(m.path.clone()).or_default();
        entry.branch = Some(m.branch.clone());
        entry.info = info;
        // A refresh that arrived mid-request wants the *next* answer, so
        // leave the entry stale and let the next poll re-query.
        entry.queried_at = if entry.refresh_requested { None } else { Some(now) };
        entry.refresh_requested = false;
        entry.pending = false;
    }

    /// Stamp a member whose request produced nothing, keeping its previous
    /// answer on screen and holding it off for a TTL.
    fn back_off(&mut self, m: &Member, now: Duration) {
        let entry = self.entries.entry(m.path.clone()).or_default();
        entry.queried_at = Some(now);
        entry.refresh_requested = false;
        entry.pending = false;
    }

    /// Record a started request against every entry it covers.
    fn bank_batch(&mut self, members: Vec<Member>, job: jobs::Job<BatchResult>) {
        let started = self.now();
        for m in &members {
            let entry = self.entries.entry(m.path.clone()).or_default();
            entry.branch = Some(m.branch.clone());
            entry.pending = true;
        }
        self.batches.push(Batch { job, started, members });
        self.in_flight += 1;
    }
```

- [ ] **Step 6: Group the due list and spawn**

```rust
    /// Hand the frame's due list to one worker.  Grouping needs `git2` to read
    /// each path's `origin`, which is why nothing here inspects the list: the
    /// frame only decides whether there is room to ask.
    ///
    /// Over the cap, the list is dropped and every member's `pending` flag
    /// cleared, so they fall due again next frame rather than being lost.
    fn spawn_due(&mut self, ctx: &egui::Context) {
        let due = std::mem::take(&mut self.due);
        if due.is_empty() {
            return;
        }
        if !may_spawn(self.concurrency, self.in_flight) {
            for m in &due {
                if let Some(entry) = self.entries.get_mut(&m.path) {
                    entry.pending = false;
                }
            }
            return;
        }
        let members = due.clone();
        let ctx = ctx.clone();
        let job = jobs::pool().spawn(jobs::Priority::Background, move |blocking| {
            // Fires on a panicking unwind too, since it's a local: the drain
            // that frees this slot only runs on a frame, so an exit without a
            // repaint can stall polling for good.
            let _wake = RepaintOnDrop(ctx);
            run_due(due, blocking)
        });
        self.bank_batch(members, job);
    }
```

Then, at module level:

```rust
/// Group a whole burst and ask for each group in turn, reporting one answer
/// per worktree.  Runs on a worker: both the `git2` reads that grouping needs
/// and the requests themselves block.
fn run_due(due: Vec<Member>, blocking: &jobs::Blocking) -> BatchResult {
    let mut out = HashMap::new();
    for group in groups(due, blocking) {
        let found = query_group(&group, blocking);
        for m in &group.members {
            out.insert(m.path.clone(), found.get(&m.branch).cloned());
        }
    }
    out
}

/// What one request covers: the branches asked about, and one worktree inside
/// the repository to run `gh` from.  An absent `slug` means this group has no
/// batched form and runs the per-branch path instead.
struct Group {
    /// Any worktree of this repository; `gh` resolves the repo from its cwd.
    cwd: PathBuf,
    slug: Option<(String, String)>,
    members: Vec<Member>,
}

/// One request per repository, chunked, plus one per path that cannot be
/// grouped.  Reading `origin` costs a git2 open per due path, which is why
/// this runs on a worker rather than on the frame.
fn groups(due: Vec<Member>, blocking: &jobs::Blocking) -> Vec<Group> {
    let _ = blocking;
    let mut by_repo: HashMap<(String, String), Group> = HashMap::new();
    let mut ungrouped = Vec::new();
    for m in due {
        let slug = match wsl::classify(&m.path) {
            wsl::Location::Windows(p) => origin_slug(&p),
            // Nothing here can read a repository inside a distro, and its
            // `gh` runs as a script rather than a `Command`.
            wsl::Location::Wsl { .. } => None,
        };
        match slug {
            Some((owner, name)) => by_repo
                .entry((owner.clone(), name.clone()))
                .or_insert_with(|| Group {
                    cwd: m.path.clone(),
                    slug: Some((owner, name)),
                    members: Vec::new(),
                })
                .members
                .push(m),
            None => ungrouped.push(Group { cwd: m.path.clone(), slug: None, members: vec![m] }),
        }
    }
    by_repo
        .into_values()
        .flat_map(|g| {
            g.members
                .chunks(pr_query::CHUNK)
                .map(|c| Group { cwd: g.cwd.clone(), slug: g.slug.clone(), members: c.to_vec() })
                .collect::<Vec<_>>()
        })
        .chain(ungrouped)
        .collect()
}

/// Ask GitHub about a whole group in one request, falling back to the
/// per-branch path when there is no batched form or the batched one cannot
/// answer.  GraphQL can need scopes `gh pr list` does not, so an install that
/// works today can fail here, and a project's badges must not vanish when it
/// does.
fn query_group(group: &Group, blocking: &jobs::Blocking) -> HashMap<String, PrInfo> {
    let branches: Vec<String> = group.members.iter().map(|m| m.branch.clone()).collect();
    if let Some((owner, name)) = &group.slug {
        let query = pr_query::build(owner, name, &branches);
        if let Some(stdout) = run_graphql(&group.cwd, &query, blocking) {
            let parsed = pr_query::parse(&stdout, &branches, Some(owner));
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    group
        .members
        .iter()
        .filter_map(|m| query_gh(&m.path, &m.branch, blocking).map(|i| (m.branch.clone(), i)))
        .collect()
}

/// Run one GraphQL document through `gh`, returning its stdout.
///
/// The query goes in on stdin because `-f query=` puts it in argv, and a
/// Windows command line caps at 32,767 characters, which a full chunk of
/// aliases can exceed.  `--input -` reads a JSON body, so a bare query piped
/// in comes back as HTTP 502 rather than as an argument error.
///
/// Only ever called for a group with a slug, which means a native path: a WSL
/// group has no slug and never reaches here.
#[allow(clippy::disallowed_methods)] // Running `gh` is this function's job.
fn run_graphql(cwd: &Path, query: &str, _blocking: &jobs::Blocking) -> Option<Vec<u8>> {
    let mut child = Command::new("gh")
        .hide_console()
        .current_dir(cwd)
        .args(["api", "graphql", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take()?;
        stdin.write_all(pr_query::body(query).as_bytes()).ok()?;
    }
    let output = child.wait_with_output().ok()?;
    output.status.success().then_some(output.stdout)
}
```

The `stdin` scope ends before `wait_with_output`, which closes the pipe and lets `gh` see EOF. Without that the child waits for input that never ends.

- [ ] **Step 7: Widen the origin read to owner and repository**

`github_owner_from_url` returns only the owner; a batched query needs the
repository name too. Rename it `github_slug_from_url`, return
`Option<(String, String)>` from the same parse, and change its existing call
sites to take `.map(|(owner, _)| owner)`. Then:

```rust
/// The GitHub `(owner, repository)` of this worktree's `origin`, read straight
/// from the repository config.  `None` for a missing, unreadable or non-GitHub
/// remote, which is what puts a path on the per-branch path.
fn origin_slug(path: &Path) -> Option<(String, String)> {
    let repo = git2::Repository::open(path).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    github_slug_from_url(remote.url()?)
}
```

Delete `local_origin_owner` and its unused `_blocking` parameter rather than
leaving a wrapper; its one call site in `query_gh` becomes
`origin_slug(&p).map(|(owner, _)| owner)`.

- [ ] **Step 8: Update `invalidate_all` and the call site**

`invalidate_all` sets `refresh_requested` where a lookup is running. `pending`
is now a `bool`, so `if entry.pending` replaces `if entry.pending.is_some()`.
The rest is unchanged.

`drain_completed` gained a `ctx` parameter and `poll` lost one. Update both
call sites in `alacritree/src/app.rs`; `cargo check -p alacritree` names them.

- [ ] **Step 9: Rewrite the tests built on the old shape**

`spawn_stuck_job`, `insert_pending` and `insert_stuck_entry` all return or
store `jobs::Job<LookupResult>`, and no job returns a `LookupResult` any more.
Convert them:

- `spawn_stuck_job` returns `jobs::Job<BatchResult>` and its blocked closure
  returns `HashMap::new()`.
- `insert_pending(path, branch, job)` becomes
  `bank_batch(vec![Member { path, branch }], job)`.
- `insert_stuck_entry(cache, path, branch, started)` pushes a `Batch` with
  `started` straight onto `cache.batches` and bumps `in_flight`, since the
  point is to force the drain's TTL branch. `started` is a `Duration` after
  Task 6, so it no longer needs `stale_start`.
- Every `drain_completed()` call in the tests takes a context now. Use
  `egui::Context::default()`.

Work through whatever the compiler names; each is a mechanical shape change,
not a behaviour change. Do not weaken an assertion to make one pass.

- [ ] **Step 10: Run everything**

```sh
cargo test -p alacritree
python3 alacritree/tools/ui-thread-audit.py .
cargo clippy -p alacritree --all-targets -- -D clippy::disallowed_methods
```

Expected: all tests pass, `blocking leaves reachable from update: 0`, clippy clean.

- [ ] **Step 11: Verify the transport by hand**

```sh
printf '%s' '{"query":"query { viewer { login } }"}' | gh api graphql --input -
```

Expected: a `data.viewer.login` object, not `HTTP 502`. If it returns 502, the body is not reaching `gh` as JSON, which is the one failure mode that looks like a network problem and is not.

- [ ] **Step 12: Format and commit**

```sh
rustup run nightly rustfmt --edition 2024 alacritree/src/pr_status.rs
git add alacritree/src/pr_status.rs alacritree/src/app.rs
git commit -m "$(cat <<'MSG'
perf(pr-status): ask once per repository instead of once per worktree

Every worktree of a project ran its own `gh` process against the same
repository, filtered to a different branch, and they all fell due
together when the shared TTL expired.  The duplication peaked exactly
when the pool had fewest background slots to give.

One request now names every due branch of a repository as its own
aliased selection, so cost follows branch count rather than how many
PRs the repository holds.  The job moves from the entry to the cache,
since one answer covers many entries.

A worktree inside WSL, one with no readable GitHub `origin`, and any
group whose request fails all keep the per-branch path.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
MSG
)"
```

---

## Final verification

- [ ] `cargo test -p alacritree` passes.
- [ ] `python3 alacritree/tools/ui-thread-audit.py .` reports `blocking leaves reachable from update: 0`.
- [ ] `cargo clippy -p alacritree --all-targets -- -D clippy::disallowed_methods` is clean.
- [ ] `cargo check -p alacritree --all-targets` passes on Windows and on Linux.
- [ ] `cargo test -p alacritree --test config_schema` passes without `ALACRITREE_UPDATE_SCHEMA`.
- [ ] Every commit carries the `Co-Authored-By` trailer, no subject exceeds 72 characters, and none ends in a period.

## One spec item the pre-existing code already covers

The spec's fifth workstream lists "retry with backoff on failure". No task
implements it because `drain_completed` already does: a request that fails or
never reports stamps `queried_at` on every member, so the next attempt waits a
full TTL rather than re-spawning on the next frame. Task 8 Step 5 keeps that
behaviour under the new shape, in `back_off`. Do not add a second backoff.

## One spec test deliberately not written

The spec lists "`create` cancelled during the fetch returns `Err` and leaves no
worktree on disk", and says to drop it rather than build a fragile harness if it
needs a fake `git` on `PATH`. It does: it needs a repository with an origin that
hangs. Task 2's `dropping_a_running_job_kills_its_child_and_frees_the_worker` is
the pool-level cancel test the spec names as the substitute, and Task 3's
between-steps test covers the rest of `create`. Do not add the fetch test unless
a clean way to hang an origin appears.

## Known flake

`session::tests::a_pane_runs_its_child_without_a_console_host_handshake` asserts that `cmd /c echo ready` exits within 2 seconds, while the suite runs over a thousand tests in parallel. It fails in a full run and passes alone in well under its budget. It predates this branch and reproduces on branches carrying none of this work. Do not chase it; re-run it alone to confirm, and say so in the report.
