# Non-blocking UI thread implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** No handler reachable from `AlacritreeApp::update` waits on a subprocess or a repository walk, and the compiler rejects new ones.

**Architecture:** Add one process-wide job pool with two priorities and cancel-on-drop handles. Move the existing bare `thread::spawn` sites onto it, then convert each blocking handler the audit found. Close the class by giving the blocking helpers a `Blocking` token whose constructor is private to the pool, so a call from `update` fails to build.

**Tech Stack:** Rust 2024 (MSRV 1.85), std threads and channels, `windows-sys 0.59` (`Win32_System_Threading`, already enabled), egui/eframe, `git2`, `ast-grep` plus Python 3 for the audit tool.

## Global constraints

- All new code lives in `alacritree/`. The vendored `alacritty*` crates are read-only.
- `cargo fmt` is enforced; the repo formats with **nightly** rustfmt because `rustfmt.toml` declares unstable options. Run `rustup run nightly rustfmt --edition 2024 <files>` or `cargo +nightly fmt`.
- Unit tests live in-module under `#[cfg(test)]`. Run with `cargo test -p alacritree`.
- Conventional Commits, imperative subject, under ~72 chars.
- Every commit carries the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Comments explain *why*, never restate *what*. No task references, no PR meta, no change-relative phrasing.
- Mirror upstream alacritty where it has already solved something.
- Branch: `perf/nonblocking-ui`, based on `origin/fix/pty-packet-storm`. Worktree: `C:\Users\Lev\Git\github\alacritree-worktrees\perf\nonblocking-ui`.
- Do not "fix" `StatusCache::poll`, `PrCache`, or `refresh_project`. They already background correctly; Task 2 only changes *which* executor they use.

## Source of truth

Issue #22 on `AbysmalBiscuit/alacritree` holds the findings. Regenerate them at any time:

```sh
python3 alacritree/tools/ui-thread-audit.py .
```

The file:line references in this plan go stale; the script does not.

## File structure

| File | Responsibility |
| --- | --- |
| `alacritree/src/jobs.rs` (create) | The pool, `Priority`, `Job<T>` handle, the `Blocking` token, the thread-priority shim. Self-contained and egui-free so it unit-tests without a window. |
| `alacritree/src/main.rs` (modify) | Declare `mod jobs;`. |
| `alacritree/src/git_status.rs` (modify) | `spawn_compute` submits to the pool; add `DirtyCounts::from_status`. |
| `alacritree/src/pr_status.rs` (modify) | `gh` lookups submit to the pool at background priority. |
| `alacritree/src/worktree.rs` (modify) | `spawn_delete` submits to the pool; `list_branches` gains a background entry point. |
| `alacritree/src/app.rs` (modify) | Every handler conversion: delete dialog, branch picker, doppler sync, file drop, image paste. |
| `alacritree/src/clipboard_image.rs` (modify) | `store` stops sweeping; the sweep becomes its own entry point. |
| `alacritree/src/links.rs` (modify) | `open` submits the spawn to the pool. |
| `alacritree/src/doppler.rs` (modify) | `mirror_scopes` takes the token. |
| `clippy.toml` (create, repo root) | `disallowed-methods` backstop for inline primitives that take no token. |

---

### Task 1: The job pool

**Files:**
- Create: `alacritree/src/jobs.rs`
- Modify: `alacritree/src/main.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `jobs::Priority::{Interactive, Background}`
  - `jobs::Blocking` (opaque; no public constructor)
  - `jobs::Job<T>` with `fn poll(&self) -> Option<T>`, cancels on drop
  - `jobs::Pool` with `fn new(workers: usize) -> Pool` and `fn spawn<T, F>(&self, priority: Priority, f: F) -> Job<T> where F: FnOnce(&Blocking) -> T + Send + 'static, T: Send + 'static`
  - `jobs::pool() -> &'static Pool`, the process-wide instance
  - `jobs::on_this_thread<T>(f: impl FnOnce(&Blocking) -> T) -> T`, the one sanctioned way to block on the calling thread

Two priorities, not three. `Interactive` means something on screen shows a pending state until the job lands; `Background` means nobody is looking. A third tier has no distinct consumer today.

One worker is always held free for `Interactive` work. Without that reservation a click can queue behind a pool full of git walks, which is the exact failure the pool exists to prevent.

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/jobs.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn state_with(interactive: usize, background: usize) -> State {
        let mut state = State::default();
        for _ in 0..interactive {
            state.interactive.push_back(Task { cancelled: Arc::new(AtomicBool::new(false)), run: Box::new(|_| {}) });
        }
        for _ in 0..background {
            state.background.push_back(Task { cancelled: Arc::new(AtomicBool::new(false)), run: Box::new(|_| {}) });
        }
        state
    }

    #[test]
    fn interactive_work_goes_first() {
        let mut state = state_with(1, 1);
        let (_, was_background) = take(&mut state, 4).expect("a runnable task");
        assert!(!was_background);
        assert_eq!(state.background.len(), 1, "the background task is still queued");
    }

    #[test]
    fn a_worker_stays_free_for_interactive_work() {
        let mut state = state_with(0, 3);
        assert!(take(&mut state, 2).is_some(), "the first background task runs");
        assert!(take(&mut state, 2).is_none(), "the second would leave no worker for a click");
        assert_eq!(state.background.len(), 2);
    }

    #[test]
    fn a_finished_background_task_frees_its_slot() {
        let mut state = state_with(0, 2);
        take(&mut state, 2).expect("the first background task runs");
        state.background_running -= 1;
        assert!(take(&mut state, 2).is_some(), "the freed slot admits the next one");
    }

    #[test]
    fn a_result_reaches_the_handle() {
        let pool = Pool::new(2);
        let job = pool.spawn(Priority::Interactive, |_| 7_u32);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = job.poll() {
                assert_eq!(value, 7);
                return;
            }
            assert!(Instant::now() < deadline, "the job never reported");
            std::thread::yield_now();
        }
    }

    #[test]
    fn the_calling_thread_can_take_a_token_explicitly() {
        assert_eq!(on_this_thread(|_| 3_u8), 3);
    }

    #[test]
    fn dropping_the_handle_cancels_work_that_has_not_started() {
        // `Pool::new` floors the worker count at two, which leaves exactly one
        // background slot.  Holding that slot busy keeps the second submission
        // queued until after its handle drops.
        let pool = Pool::new(1);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let blocker = pool.spawn(Priority::Background, move |_| {
            let _ = release_rx.recv();
        });
        let (ran_tx, ran_rx) = mpsc::channel::<()>();
        drop(pool.spawn(Priority::Background, move |_| {
            let _ = ran_tx.send(());
        }));
        let _ = release_tx.send(());
        drop(blocker);
        assert!(
            ran_rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "a cancelled task must not run"
        );
    }
}
```

Note the one-worker pool in the last test: `take` refuses background work when `workers` is 1, so give `Pool::new` a floor of 2 and let the test rely on that, or the blocker never starts. Step 3 clamps it.

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo test -p alacritree jobs
```

Expected: FAIL to compile, `cannot find type State in this scope`.

- [ ] **Step 3: Write the implementation**

Prepend to `alacritree/src/jobs.rs`:

```rust
//! One pool for every piece of work that must not run on the UI thread.
//!
//! A handler that gathers its content synchronously cannot draw until the
//! gathering returns, which under CPU load is seconds.  Work goes here
//! instead, and the `Blocking` token makes that structural: the helpers that
//! block take one, only a pool worker is handed one, so calling such a helper
//! from `update` does not compile.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::Instant;

/// Whether anything on screen is waiting for the job.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    /// A pending state is showing until this lands.
    Interactive,
    /// Housekeeping nobody is looking at: status polls, PR lookups, liveness.
    Background,
}

/// Proof that the holder runs on a pool worker.  The constructor is private to
/// this module, so a blocking helper that takes one cannot be called from the
/// UI thread.
pub struct Blocking(());

struct Task {
    cancelled: Arc<AtomicBool>,
    run: Box<dyn FnOnce(&Blocking) + Send>,
}

#[derive(Default)]
struct State {
    interactive: VecDeque<Task>,
    background: VecDeque<Task>,
    background_running: usize,
}

/// The next task this worker may run, and whether it occupies a background
/// slot.  Background work is capped one below the worker count so a click
/// never queues behind a pool full of git walks.
fn take(state: &mut State, workers: usize) -> Option<(Task, bool)> {
    if let Some(task) = state.interactive.pop_front() {
        return Some((task, false));
    }
    if state.background_running + 1 < workers {
        if let Some(task) = state.background.pop_front() {
            state.background_running += 1;
            return Some((task, true));
        }
    }
    None
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    workers: usize,
}

pub struct Pool {
    shared: Arc<Shared>,
}

/// A submitted job.  Dropping the handle cancels the work if it has not
/// started, so a status scan for a workspace the user has left stops costing
/// a core.
pub struct Job<T> {
    rx: mpsc::Receiver<T>,
    cancelled: Arc<AtomicBool>,
}

impl<T> Job<T> {
    /// The result if it has landed.  Never blocks.
    pub fn poll(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Pool {
    /// `workers` is clamped to at least two: the background reservation needs
    /// one worker beyond the one it holds free.
    pub fn new(workers: usize) -> Self {
        let workers = workers.max(2);
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            workers,
        });
        for _ in 0..workers {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || worker(shared));
        }
        Self { shared }
    }

    pub fn spawn<T, F>(&self, priority: Priority, f: F) -> Job<T>
    where
        F: FnOnce(&Blocking) -> T + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let task = Task {
            cancelled: Arc::clone(&cancelled),
            run: Box::new(move |blocking| {
                let _ = tx.send(f(blocking));
            }),
        };
        let mut state = self.shared.state.lock().expect("the job queue is poisoned");
        match priority {
            Priority::Interactive => state.interactive.push_back(task),
            Priority::Background => state.background.push_back(task),
        }
        drop(state);
        self.shared.wake.notify_one();
        Job { rx, cancelled }
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let mut state = shared.state.lock().expect("the job queue is poisoned");
        let (task, was_background) = loop {
            if let Some(taken) = take(&mut state, shared.workers) {
                break taken;
            }
            state = shared.wake.wait(state).expect("the job queue is poisoned");
        };
        drop(state);

        if !task.cancelled.load(Ordering::Relaxed) {
            lower_this_thread(was_background);
            (task.run)(&Blocking(()));
        }

        if was_background {
            let mut state = shared.state.lock().expect("the job queue is poisoned");
            state.background_running -= 1;
            drop(state);
            // A freed slot may admit a task another worker is asleep on.
            shared.wake.notify_all();
        }
    }
}

/// Housekeeping should yield to the UI thread when the CPU is contended, and a
/// worker outlives one job, so the class is set per job rather than at spawn.
#[cfg(windows)]
fn lower_this_thread(background: bool) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_NORMAL,
    };
    let level = if background { THREAD_PRIORITY_BELOW_NORMAL } else { THREAD_PRIORITY_NORMAL };
    unsafe { SetThreadPriority(GetCurrentThread(), level) };
}

#[cfg(not(windows))]
fn lower_this_thread(_background: bool) {}

/// Block on the calling thread, deliberately.  The CLI and the IPC connection
/// threads have no window and nothing to paint, so blocking is correct there.
/// A named entry point rather than a public constructor, so the exception is
/// one reviewable call instead of a habit.
pub fn on_this_thread<T>(f: impl FnOnce(&Blocking) -> T) -> T {
    f(&Blocking(()))
}

/// The process-wide pool.  Sized for work that waits on subprocesses and the
/// filesystem rather than work that saturates a core, so a wide pool would only
/// multiply concurrent git walks.
pub fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 4));
        Pool::new(workers)
    })
}
```

- [ ] **Step 4: Declare the module**

In `alacritree/src/main.rs`, add `mod jobs;` in the existing alphabetical run of `mod` declarations.

- [ ] **Step 5: Run the tests to verify they pass**

```sh
cargo test -p alacritree jobs
```

Expected: 5 passed.

- [ ] **Step 6: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/jobs.rs alacritree/src/main.rs
git commit -m "feat(jobs): add a priority pool for work off the UI thread"
```

---

### Task 2: Move the existing background work onto the pool

**Files:**
- Modify: `alacritree/src/git_status.rs`, `alacritree/src/pr_status.rs`, `alacritree/src/worktree.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `jobs::{pool, Priority, Job, Blocking}` from Task 1.
- Produces: nothing new. Callers keep their existing shapes; only the executor changes.

Migrate before adding new users, so the pool is exercised by code whose behavior is already known. Five sites spawn threads today; the audit script lists them:

```sh
ast-grep -p 'thread::spawn($$$)' -l rust alacritree/src
```

Each keeps its `try_recv` drain. `Job<T>::poll` replaces `Receiver::try_recv` and adds cancel-on-drop for free.

- [ ] **Step 1: Write the failing test**

In `alacritree/src/git_status.rs`, inside `mod tests`:

```rust
#[test]
fn a_status_poll_reports_without_blocking_its_caller() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let repo = git2::Repository::init(dir.path()).expect("a repo");
    drop(repo);

    let ctx = egui::Context::default();
    let mut cache = StatusCache::new(dir.path().to_path_buf());
    // The first poll has nothing banked and must return anyway.
    let started = Instant::now();
    let _ = cache.poll(None, &ctx);
    assert!(started.elapsed() < Duration::from_millis(50), "poll blocked its caller");

    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.last().branch.is_none() && Instant::now() < deadline {
        let _ = cache.poll(None, &ctx);
        std::thread::yield_now();
    }
    assert!(cache.last().branch.is_some(), "the background compute never landed");
}
```

- [ ] **Step 2: Run it to verify it fails**

```sh
cargo test -p alacritree git_status::tests::a_status_poll_reports_without_blocking_its_caller
```

Expected: PASS today, because `spawn_compute` already backgrounds. That is the point: it is a characterization test that must keep passing across the executor swap. Record the pass, then proceed.

- [ ] **Step 3: Swap `git_status::spawn_compute` onto the pool**

Replace the `Pending` struct's `rx: Receiver<GitStatus>` with `job: jobs::Job<GitStatus>` and rewrite:

```rust
fn spawn_compute(path: PathBuf, hint: Option<String>, ctx: egui::Context) -> Pending {
    let worker_hint = hint.clone();
    let job = jobs::pool().spawn(jobs::Priority::Background, move |blocking| {
        let status = compute(&path, worker_hint.as_deref(), blocking);
        ctx.request_repaint();
        status
    });
    Pending { hint, job }
}
```

`compute` gains a `_blocking: &jobs::Blocking` parameter. Its other callers are `cli/offline.rs` and the IPC connection thread, neither of which is the UI thread; wrap those two call sites in `jobs::on_this_thread(|blocking| compute(&path, hint, blocking))` now rather than deferring them.

In `StatusCache::poll`, `pending.rx.try_recv()` becomes `pending.job.poll()`:

```rust
if let Some(pending) = &self.pending {
    if let Some(status) = pending.job.poll() {
        self.last = status;
        self.last_refreshed = Some(Instant::now());
        self.last_hint = pending.hint.clone();
        self.pending = None;
    }
}
```

- [ ] **Step 4: Swap the other four sites the same way**

- `pr_status.rs`: the `gh` lookup, `Priority::Background`. Its `drain_completed` `try_recv` match arms become `Job::poll`, and the `Disconnected` arm folds into "nothing yet" because a cancelled job simply never reports; keep the TTL backoff that arm existed to trigger by treating a job whose handle has been held past the TTL as stale.
- `worktree.rs::spawn_delete`: `Priority::Interactive`. A sidebar row shows a spinner until it lands.
- `app.rs::refresh_project`: `Priority::Background`.
- `app.rs` WSL delta discovery: `Priority::Background`.

- [ ] **Step 5: Verify nothing regressed**

```sh
cargo test -p alacritree
python3 alacritree/tools/ui-thread-audit.py .
```

Expected: tests pass. The audit still reports its findings; the count must not *grow*. If a finding appears that was not there before, a migration accidentally moved work onto the calling thread.

- [ ] **Step 6: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/git_status.rs alacritree/src/pr_status.rs alacritree/src/worktree.rs alacritree/src/app.rs
git commit -m "refactor(jobs): run the existing background work on the pool"
```

---

### Task 3: The delete prompt opens from the cache

**Files:**
- Modify: `alacritree/src/git_status.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `jobs::{pool, Priority}`, `git_status::{GitStatus, DirtyCounts, ChangeKind}`.
- Produces: `DirtyCounts::from_status(&GitStatus) -> DirtyCounts`.

This is issue #24. The blocking call is not only misplaced, it is redundant: `StatusCache` already holds `staged` and `unstaged` with a `ChangeKind` per entry, and `DirtyCounts` is three counts derived from exactly that.

The counts also stop deciding `--force`. Attempt the removal unforced and let git refuse; git is then the authority on whether work would be lost, rather than a number read seconds earlier while the user reads the dialog. Both stale directions fail safe: stale-clean on a dirty tree gets a refusal and a retry, stale-dirty on a clean tree passes `--force` to a removal where it makes no difference.

- [ ] **Step 1: Write the failing test**

In `alacritree/src/git_status.rs`, inside `mod tests`:

```rust
#[test]
fn dirty_counts_come_from_a_status_the_panel_already_has() {
    let status = GitStatus {
        branch: Some("main".into()),
        default_branch: None,
        default_branch_resolved: None,
        staged: vec![FileChange { path: "a".into(), kind: ChangeKind::Added }],
        unstaged: vec![
            FileChange { path: "b".into(), kind: ChangeKind::Modified },
            FileChange { path: "c".into(), kind: ChangeKind::Untracked },
            FileChange { path: "d".into(), kind: ChangeKind::Untracked },
        ],
        branch_diff: Vec::new(),
        error: None,
    };
    let counts = DirtyCounts::from_status(&status);
    assert_eq!(counts.staged, 1);
    assert_eq!(counts.modified, 1);
    assert_eq!(counts.untracked, 2);
    assert!(counts.is_dirty());
}
```

- [ ] **Step 2: Run it to verify it fails**

```sh
cargo test -p alacritree dirty_counts_come_from_a_status
```

Expected: FAIL, `no function or associated item named from_status`.

- [ ] **Step 3: Implement `from_status`**

In `alacritree/src/git_status.rs`, in `impl DirtyCounts`:

```rust
/// Derive the delete modal's counts from a status the git panel already
/// polled, so opening the dialog costs no repository walk.
pub fn from_status(status: &GitStatus) -> Self {
    let untracked =
        status.unstaged.iter().filter(|c| c.kind == ChangeKind::Untracked).count();
    Self {
        staged: status.staged.len(),
        modified: status.unstaged.len() - untracked,
        untracked,
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

```sh
cargo test -p alacritree dirty_counts_come_from_a_status
```

Expected: PASS.

- [ ] **Step 5: Make the counts optional and non-blocking**

In `alacritree/src/app.rs`, change `DeleteRequest`:

```rust
struct DeleteRequest {
    project_idx: usize,
    worktree_path: PathBuf,
    worktree_name: String,
    branch: Option<String>,
    /// `None` until a count lands.  The cache answers for a worktree the git
    /// panel has shown; one never selected has to wait for the job.
    dirty: Option<DirtyCounts>,
    /// Fills `dirty` when the cache was cold.
    dirty_job: Option<jobs::Job<DirtyCounts>>,
    prunable: bool,
    delete_branch: bool,
}
```

In `request_worktree_delete`, replace the synchronous `git_status::dirty_counts(&wt.path)` arm:

```rust
let (dirty, dirty_job) = if prunable {
    // A missing dir has nothing to be dirty.
    (Some(DirtyCounts::default()), None)
} else if let Some(cache) = self.git_status.get(&wt.path) {
    (Some(DirtyCounts::from_status(cache.last())), None)
} else {
    let path = wt.path.clone();
    let job = jobs::pool()
        .spawn(jobs::Priority::Interactive, move |blocking| {
            git_status::dirty_counts(&path, blocking)
        });
    (None, Some(job))
};
```

`git_status::dirty_counts` gains a `_blocking: &jobs::Blocking` parameter.

- [ ] **Step 6: Adopt the late count and render the pending state**

In `show_delete_dialog`, before drawing, adopt whatever landed:

```rust
if let Some(req) = self.pending_delete.as_mut() {
    if let Some(counts) = req.dirty_job.as_ref().and_then(jobs::Job::poll) {
        req.dirty = Some(counts);
        req.dirty_job = None;
    }
}
```

Where the dialog prints the counts, `None` renders `checking…` instead of a number. The confirm control is never disabled by a missing count, which is the whole point.

- [ ] **Step 7: Let git decide `--force`**

In `run_pending_delete`, the `DeleteJob::Remove` arm becomes `force: false`.

Add to `poll_pending_deletes`: when the result is `Err(message)` and the message names a dirty or untracked tree, open a second confirm carrying the same request with `force: true` rather than surfacing a plain error. Match on the substrings git actually emits:

```rust
/// `git worktree remove` refuses a tree with work in it, and that refusal is
/// the authority on whether removing would lose anything.  The wording is
/// git's, so match the stable fragments rather than the whole sentence.
fn refused_for_unsaved_work(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("contains modified or untracked files")
        || message.contains("is dirty")
}
```

- [ ] **Step 8: Verify**

```sh
cargo test -p alacritree
cargo run -p alacritree
```

By hand, with a build running to load the machine: click the delete affordance on a worktree the git panel has never shown. The prompt must appear immediately, with `checking…` where the counts go, and fill in a moment later.

- [ ] **Step 9: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/git_status.rs alacritree/src/app.rs
git commit -m "fix(sidebar): open the delete prompt without a status walk"
```

---

### Task 4: The base-branch picker opens empty

**Files:**
- Modify: `alacritree/src/app.rs`, `alacritree/src/worktree.rs`

**Interfaces:**
- Consumes: `jobs::{pool, Priority, Job, Blocking}`, `worktree::list_branches`.
- Produces: nothing new.

`open_base_branch_picker` calls `list_branches`, which spawns `git for-each-ref` and waits, before it sets the state that draws the picker. Same shape as Task 3, simpler: there is no cache to read, so the picker opens with an empty list and a pending row.

- [ ] **Step 1: Add the pending field**

`BaseBranchPicker` already carries `branches: Result<Vec<String>, String>`, where `Err` is what git said when listing failed. Wrap it rather than replacing it, so a failure still reaches the user:

```rust
struct BaseBranchPicker {
    worktree: PathBuf,
    query: String,
    /// `None` until the listing lands; the picker opens before git answers.
    /// `Err` is what git said when listing failed (not a repo, WSL down…).
    branches: Option<Result<Vec<String>, String>>,
    branches_job: Option<jobs::Job<Result<Vec<String>, String>>>,
    /// Auto-detected base shown on the "Auto" row.
    detected: Option<String>,
    cursor: usize,
}
```

- [ ] **Step 2: Submit instead of calling**

In `open_base_branch_picker`, replace `let branches = crate::worktree::list_branches(&worktree);` with:

```rust
let job = jobs::pool().spawn(jobs::Priority::Interactive, move |blocking| {
    crate::worktree::list_branches(&worktree, blocking)
});
```

and build the picker with `branches: None, branches_job: Some(job)`.

`worktree::list_branches` gains a `_blocking: &jobs::Blocking` parameter.

- [ ] **Step 3: Adopt in the draw path**

In `show_base_branch_picker`, before drawing rows:

```rust
if let Some(picker) = self.base_branch_picker.as_mut() {
    if let Some(branches) = picker.branches_job.as_ref().and_then(jobs::Job::poll) {
        picker.branches = Some(branches);
        picker.branches_job = None;
    }
}
```

Render `None` as a single non-selectable `loading branches…` row. The existing filter box stays live; it just matches nothing until the list arrives.

- [ ] **Step 4: Verify**

```sh
cargo test -p alacritree
cargo run -p alacritree
```

By hand: open the picker under load. It must appear at once.

- [ ] **Step 5: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/app.rs alacritree/src/worktree.rs
git commit -m "fix(sidebar): open the branch picker before git answers"
```

---

### Task 5: Doppler scope sync leaves the session-spawn path

**Files:**
- Modify: `alacritree/src/app.rs`, `alacritree/src/doppler.rs`

**Interfaces:**
- Consumes: `jobs::{pool, Priority, Blocking}`.
- Produces: nothing new.

`spawn_session_with_shell` calls `sync_doppler_scopes`, which spawns `doppler` and waits, before the PTY exists. That is the "tab appears but the shell does not start" symptom.

**Read the open question below before implementing.** The current ordering is load-bearing: the comment says the sync runs before the PTY so the shell cannot race `doppler run` against the scope write. Backgrounding does not remove the constraint, it moves who waits.

- [ ] **Step 1: Submit the mirror instead of calling it**

In `sync_doppler_scopes`, replace the blocking tail:

```rust
let worktree_for_log = worktree.clone();
// Handle deliberately dropped: nothing reads the result, and the guard set
// above already stops a second pass for this worktree.
drop(jobs::pool().spawn(jobs::Priority::Background, move |blocking| {
    let linked = doppler::mirror_scopes(&main_checkout, &worktree, blocking);
    if linked > 0 {
        log::info!("linked {linked} doppler scope(s) into {}", worktree_for_log.display());
    }
}));
```

Dropping a `Job` cancels work that has not started, so this must instead hold the handle for the sync to be guaranteed to run. Store it on `AlacritreeApp` in a `Vec<jobs::Job<()>>` drained each frame, the same shape `pending_deletes` uses:

```rust
/// Submitted syncs, held so dropping the handle does not cancel them, and
/// drained once a frame.
doppler_syncs: Vec<jobs::Job<()>>,
```

Drain in `update`: `self.doppler_syncs.retain(|job| job.poll().is_none());`

- [ ] **Step 2: Take the token**

`doppler::mirror_scopes` and the `doppler::run` beneath it each gain a `_blocking: &jobs::Blocking` parameter.

- [ ] **Step 3: Verify**

```sh
cargo test -p alacritree
cargo run -p alacritree
```

By hand, under load, in a worktree created outside alacritree: open a new session. The shell must start without waiting on `doppler`.

- [ ] **Step 4: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/app.rs alacritree/src/doppler.rs
git commit -m "fix(session): stop the doppler sync delaying the shell"
```

---

### Task 6: File drop discovers natively off-thread

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `AlacritreeApp::refresh_project`, `projects::Project::placeholder`.
- Produces: nothing new.

`add_project_off_thread` sends WSL roots to a worker and runs `Project::discover` in place for native ones, on the reasoning that native roots "spawn nothing and are cheap enough to discover in place". Discovery opens the repository, lists worktrees, opens each one, and detects the default branch. The correct pattern already exists in the same function.

- [ ] **Step 1: Give the native arm the placeholder path**

```rust
fn add_project_off_thread(&mut self, ctx: &Context, path: PathBuf) {
    if self.projects.iter().any(|p| p.root == path) {
        return;
    }
    // Both arms go in as a placeholder and discover on a worker: a native
    // root spawns no subprocess, but it still opens the repository once per
    // worktree, which is not free on a loaded machine.
    self.projects.push(Project::placeholder(path.clone()));
    let idx = self.projects.len() - 1;
    self.refresh_project(ctx, idx);
    self.persist_project(&path);
}
```

Delete the now-unused `wsl::classify` match and update the doc comment: it currently explains a split that no longer exists.

- [ ] **Step 2: Verify**

```sh
cargo test -p alacritree
cargo run -p alacritree
```

By hand: drop a large native repository onto the window. The row must appear immediately and fill in its worktrees a beat later, the way a WSL root already does.

- [ ] **Step 3: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/app.rs
git commit -m "fix(projects): discover a dropped native root on a worker"
```

---

### Task 7: The paste sweep and the link spawn leave the UI thread

**Files:**
- Modify: `alacritree/src/clipboard_image.rs`, `alacritree/src/links.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `jobs::{pool, Priority, Blocking}`.
- Produces: `clipboard_image::sweep(dir: &Path, keep: usize, in_use: &Path, _: &Blocking)`, the sweep `store` no longer performs.

These two are smaller than the dialogs and need different treatment, so do not background them wholesale.

**Image paste cannot move as a unit.** `store` returns a path that is then pasted into the PTY, so the file must exist before those bytes are written and must not reorder against later keystrokes. Only `apply_cap`, the cache sweep, is safe to move: it runs after the path is decided and nothing reads its result.

**Link open** already uses `.spawn()` without waiting, so the cost is `CreateProcess` alone. Moving it costs nothing and removes the last process spawn from the paint path.

- [ ] **Step 1: Write the failing test**

In `alacritree/src/clipboard_image.rs`, inside `mod tests`:

```rust
#[test]
fn storing_does_not_sweep() {
    let dir = tempfile::tempdir().expect("a temp dir");
    // `store` names a file by hashing its bytes and never parses them, so
    // distinct payloads are all this needs to land under distinct names.
    for byte in 0..4_u8 {
        store(dir.path(), &[byte], Some(1)).expect("store");
    }
    let count = std::fs::read_dir(dir.path()).expect("read dir").count();
    assert_eq!(count, 4, "store must leave the cap to the sweep");
}

#[test]
fn sweeping_applies_the_cap() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut last = PathBuf::new();
    for byte in 0..4_u8 {
        last = store(dir.path(), &[byte], Some(1)).expect("store");
    }
    jobs::on_this_thread(|blocking| sweep(dir.path(), 1, &last, blocking));
    let count = std::fs::read_dir(dir.path()).expect("read dir").count();
    assert_eq!(count, 1, "the sweep keeps the cap");
}
```

- [ ] **Step 2: Run it to verify it fails**

```sh
cargo test -p alacritree storing_does_not_sweep
```

Expected: FAIL, only one file remains because `store` still calls `apply_cap`.

- [ ] **Step 3: Split the sweep out**

In `clipboard_image.rs`, drop the `apply_cap` call from `store` and expose it:

```rust
/// Trim the managed directory to its cap.  Separate from `store` because the
/// stored path is pasted into the terminal the moment it exists, while the
/// sweep is housekeeping nothing reads.
pub fn sweep(dir: &Path, keep: usize, in_use: &Path, _: &jobs::Blocking) {
    apply_cap(dir, keep, in_use);
}
```

- [ ] **Step 4: Run it to verify it passes**

```sh
cargo test -p alacritree storing_does_not_sweep
```

Expected: PASS.

- [ ] **Step 5: Submit the sweep from the paste handler**

In `store_clipboard_image` in `app.rs`, after `store` returns the path, submit the sweep at `Priority::Background` and hold the handle in a `Vec<jobs::Job<()>>` drained each frame, as in Task 5.

- [ ] **Step 6: Submit the link spawn**

In `links.rs`, `open` submits instead of spawning inline:

```rust
/// Hand the URI to the OS handler.  Submitted rather than spawned inline:
/// `CreateProcess` is not free on a loaded machine, and this runs from the
/// grid's click handler.
pub fn open(uri: &str) -> jobs::Job<()> {
    let uri = uri.to_owned();
    jobs::pool().spawn(jobs::Priority::Interactive, move |blocking| {
        if let Err(err) = spawn(&uri, blocking) {
            log::warn!("failed to open link {uri:?}: {err}");
        }
    })
}
```

`handle_selection` holds the returned handle in the same drained vector. `spawn` gains the token parameter on all three platform arms.

- [ ] **Step 7: Verify**

```sh
cargo test -p alacritree
```

- [ ] **Step 8: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/clipboard_image.rs alacritree/src/links.rs alacritree/src/app.rs
git commit -m "fix(paste): sweep the image cache off the UI thread"
```

---

### Task 8: Close the class

**Files:**
- Modify: `alacritree/src/git_status.rs`, `alacritree/src/worktree.rs`, `alacritree/src/wsl.rs`, `alacritree/src/projects.rs`, `alacritree/src/pr_status.rs`, `alacritree/src/doppler.rs`, `alacritree/src/cli/*.rs`, `alacritree/src/ipc.rs`
- Create: `clippy.toml` (repo root)

**Interfaces:**
- Consumes: `jobs::Blocking`.
- Produces: a build that rejects a blocking helper called from the UI thread.

Every helper the audit named now takes `_blocking: &jobs::Blocking`. The UI thread cannot construct one, so the call does not compile. This works in a release build, which matters because that is the daily driver.

The CLI and IPC paths block legitimately: they are not the UI thread and nothing is waiting to paint. They need a token. Give the CLI one entry point that opens the door explicitly:

```rust
/// The CLI has no window and nothing to paint, so blocking is correct there.
/// A separate entry point rather than a public constructor, so the exception
/// is one reviewable place instead of a habit.
pub fn on_this_thread<T>(f: impl FnOnce(&Blocking) -> T) -> T {
    f(&Blocking(()))
}
```

- [ ] **Step 1: Add `on_this_thread` to `jobs.rs` with its test**

```rust
#[test]
fn the_cli_entry_point_hands_out_a_token() {
    assert_eq!(on_this_thread(|_| 3_u8), 3);
}
```

- [ ] **Step 2: Thread the token through every remaining helper**

Work from the audit's own list so nothing is missed:

```sh
python3 alacritree/tools/ui-thread-audit.py .
```

Each leaf it names gets the parameter, each caller either receives one or is converted to submit a job. Wrap the CLI and IPC connection-thread call sites in `jobs::on_this_thread`.

- [ ] **Step 3: Split `wsl::distros` so the sidebar never reaches `wsl.exe`**

#22 files this one as noted-rather-than-broken, because the `OnceLock` makes it a first-frame cost at worst. The token forces the issue anyway: `show_project_sidebar` calls `distros()`, and after this task `cli_distros` needs a token that the UI thread cannot produce.

The split follows what the two sources actually cost. Reading the `Lxss` registry key does not block meaningfully; only the CLI fallback spawns `wsl.exe`. So the UI thread keeps the registry path and hands the fallback to a job:

```rust
/// Registry first, and the CLI fallback on a worker: `wsl.exe` costs hundreds
/// of milliseconds warm and seconds while the distro VM boots, and this is
/// reached from a per-frame sidebar path.  Until the fallback lands, callers
/// see an empty list, the same answer they get on a machine with no distros.
#[cfg(windows)]
pub fn distros() -> Vec<WslDistro> {
    static DISTROS: OnceLock<Vec<WslDistro>> = OnceLock::new();
    if let Some(list) = DISTROS.get() {
        return list.clone();
    }
    match registry_distros() {
        Some(list) if !list.is_empty() => DISTROS.get_or_init(|| list).clone(),
        _ => Vec::new(),
    }
}

/// Fill the cache from `wsl.exe` when the registry came up empty.  Submitted
/// once at startup rather than from a draw path.
#[cfg(windows)]
pub fn prime_distros_from_cli(_: &jobs::Blocking) { /* set DISTROS from cli_distros() */ }
```

`AlacritreeApp::new` submits `prime_distros_from_cli` at `Priority::Background`, holding the handle in the same drained vector Task 5 introduced. A `OnceLock` cannot be filled twice, so make the cache a `Mutex<Option<Vec<WslDistro>>>` or an `OnceLock` written only by the primer, whichever reads more plainly.

- [ ] **Step 4: Run the audit to verify it is empty**

```sh
python3 alacritree/tools/ui-thread-audit.py .
```

Expected: `blocking leaves reachable from update: 0`.

If a leaf remains, it is either a real one to convert or a resolution artifact. Check it by hand before dismissing it; the script over-approximates by design.

- [ ] **Step 5: Add the clippy backstop**

The token guards the helpers. It cannot guard someone writing `Command::new("git").output()` inline in a handler, because no helper is involved. Create `clippy.toml` at the repo root:

```toml
disallowed-methods = [
  { path = "std::process::Command::output", reason = "waiting on a process can hold the UI thread for an unknown duration; submit a job with alacritree::jobs" },
  { path = "std::process::Command::status", reason = "waiting on a process can hold the UI thread for an unknown duration; submit a job with alacritree::jobs" },
  { path = "std::process::Command::spawn", reason = "spawning a process can hold the UI thread for an unknown duration; submit a job with alacritree::jobs" },
]
```

Add `#[allow(clippy::disallowed_methods)]` at the handful of sites inside `wsl.rs`, `worktree.rs`, `doppler.rs`, `links.rs` and `pr_status.rs` whose job is to run a process, each with a one-line reason.

- [ ] **Step 6: Verify**

```sh
cargo clippy -p alacritree --all-targets
cargo test -p alacritree
```

Expected: clean.

- [ ] **Step 7: Prove the gate bites**

Temporarily add `git_status::dirty_counts(&path)` to `AlacritreeApp::update`, run `cargo check -p alacritree`, confirm it fails on the missing token argument, then remove it. Do not commit this.

- [ ] **Step 8: Format and commit**

```sh
cargo +nightly fmt
git add -A
git commit -m "feat(jobs): make a blocking call from the UI thread fail to build"
```

---

## Open questions

1. **The doppler race.** The sync currently runs before the PTY so the shell cannot race `doppler run` against the scope write. Backgrounding it opens a window: a brand-new shell in a worktree created outside alacritree could run a doppler command before the scopes land. The window is milliseconds to seconds, once per worktree per process, and the failure is a retryable `You must specify a project` error rather than data loss. Task 5 accepts that trade. Reject it and the alternatives are worse: gating the first PTY write on the sync reintroduces the wait where the user sees it, and syncing at discovery time fires `doppler` once per worktree for every project in the sidebar. Confirm before implementing Task 5.

2. **Config gating.** `AGENTS.local.md` requires new UX features to ship behind a flag, default off. These are bug fixes rather than features, and flagging them would mean keeping the blocking path alive forever. The visible change is that a dialog appears at once and fills in, rather than appearing late and complete. This plan adds no flag. Say if you want one.

3. **The `StatusCache` interval.** Not a threading defect: `poll` never blocks its caller, and it runs for the visible worktree only. The open question is cost, an untracked-recursive git2 walk every 1.5 s against a tree a build may be writing into. Task 2 lowers its thread priority, which may be enough. Whether the interval itself needs raising, or the walk needs narrowing, wants a measurement rather than a guess, and is out of scope here.

4. **`available_parallelism` on a job-object-capped process.** `perf/focus-priority` puts sessions in job objects. That caps priority class, not thread count, so pool sizing is unaffected. Worth re-checking once both branches are in `all-features`.

5. **Audit coverage of CPU work.** `encode_png` runs synchronously on the UI thread during an image paste, and the audit does not flag it because it performs no IO. Whether that is worth moving depends on the image size people actually paste. Not in this plan.
