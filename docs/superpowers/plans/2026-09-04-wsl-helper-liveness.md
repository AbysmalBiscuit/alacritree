# WSL helper liveness implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** detect a WSL helper that has stopped answering within about 30 seconds and tear it down, without a wall-clock deadline that a loaded Windows machine would trip on a healthy helper.

**Architecture:** a waiter slices its wait into 5 second chunks and asks the helper's dispatcher to prove it is still reading, by sending a `PING` the dispatcher answers through the same writer a real reply travels through. The reader thread stamps a timestamp on every byte it reads, so liveness is "bytes arrived recently" rather than "this reply was on time". A slice whose own sleep overran declines to judge *and* restarts the observation window, so silence that accumulated while the waiter was descheduled never counts against the helper. Teardown gains the ability to kill the `wsl.exe` process rather than only closing its stdin. The client's two pipes are boxed behind `Read` and `Write` and its two periods are injectable, so the wiring that actually failed is driven by a fake helper in-process, in milliseconds, on any platform.

**Tech Stack:** Rust (std only: `std::sync::{Mutex, atomic}`, `std::sync::mpsc`, `std::process`), POSIX sh for the distro-side helper script.

**Spec:** `docs/superpowers/specs/2026-09-04-wsl-helper-liveness-design.md`. Read it before Task 1; it carries the evidence behind every constant chosen here.

**Issues:** [#56](https://github.com/AbysmalBiscuit/alacritree/issues/56) (Tasks 1-6, the resident transport) and [#58](https://github.com/AbysmalBiscuit/alacritree/issues/58) (Tasks 7-8, the panel that keeps showing a recovered error).

## Global Constraints

- Workspace MSRV is Rust 1.85, edition 2024. **Let-chains need 1.88 and are unavailable**: write nested `if let`, never `if let ... && ...`.
- The crate is deliberately synchronous. No async runtime, no tokio.
- `alacritree/` is the only crate this fork changes. The vendored `alacritty*` crates are read-only.
- The distro-side script is POSIX sh that must run under dash and busybox ash. `read -t` is not reliably available; a `sleep` loop is.
- **Never run stable `cargo fmt` in this checkout.** `rustfmt.toml` sets five nightly-only options (`format_strings`, `imports_granularity`, `reorder_impl_items`, `overflow_delimited_expr`, `condense_wildcard_suffixes`) that stable silently drops, so stable rustfmt rewrites code nightly leaves alone. Use `cargo +nightly fmt`.
- Test command in this checkout is `cargo nextest run -p alacritree`, not `cargo test`.
- Comments explain *why*, never *what*, and never reference PRs, issues, or the change that introduced them.
- Commits use [Conventional Commits](https://www.conventionalcommits.org/), imperative subject under 72 characters, and carry the trailer `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`.
- Everything here is Windows-only in effect: `wsl_helper::client` returns `None` on other targets. The code must still compile on Linux and macOS.

## Branch setup

The branch stacks on the newest open PR rather than on `master`. Read the tip fresh; it moves while specs sit unimplemented.

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
```

Take the entry with the highest `[n]` marker. Then:

```sh
devkit issue setup 56 --slug fix/wsl-helper-liveness
git -C ../alacritree-worktrees/fix/wsl-helper-liveness reset --hard origin/<that headRefName>
```

**Do not carry the uncommitted `wsl_helper.rs` change sitting in the main checkout's working tree.** It adds an `inspect_err` to `run` that calls `mark_down` on any `NoReply`, which tears the transport down over a legitimately slow job and kills that job with it. Task 5 supersedes it. The branch is cut from a pushed ref, so it will not be present; confirm with `git -C ../alacritree-worktrees/fix/wsl-helper-liveness status --short` before starting.

## File structure

Tasks 1-6 change one production file. Everything they touch lives in `alacritree/src/wsl_helper.rs`, which already owns the transport end to end: the shell script shipped into the distro, the frame parser, the client, and the registry. Splitting it would separate the wire protocol from the only code that speaks it.

| Region | Line today | Responsibility after this plan |
| --- | --- | --- |
| `HELPER_SCRIPT` dispatcher | `:178-231` | gains a `PING` branch, and a startup sweep of dead predecessors |
| `HelperClient` struct | `:350-357` | gains `child`, `started`, `last_bytes_at`, `timing`; `stdin` becomes a boxed `Write` |
| `HelperClient::spawn` | `:368-405` | stores the child instead of moving it into the reader thread, and boxes both pipes |
| `read_loop` | `:410-439` | takes a boxed `Read`, and stamps every successful read |
| `mark_down` | `:441-450` | kills the child before dropping stdin |
| `request` | `:464-493` | slices its wait, pings, and can tear down |
| free items | near `:325` | gains `Timing` and `wedged` |
| `mod tests` | `:673-941` | gains a fake-helper pipe pair, five unit tests and two `#[ignore]` live tests |

Tasks 7 and 8 reach two more files. `alacritree/src/git_status.rs` gains an age on the in-flight compute so a stuck one is visible, and `alacritree/src/wsl.rs` gains a deadline on `run_batch`'s one-shot fallback, which today waits forever. Both are independent of Tasks 1-6 and of each other.

**The fake helper is the point of the boxing.** The bug being fixed is a client-side control-flow defect: `request` returned `NoReply` without ever calling `mark_down`. Nothing in `request`, `read_loop`, `mark_down` or `ping` needs the far end to be `wsl.exe`, so once the pipes are trait objects and the two periods are injectable, the whole failure reproduces in-process in milliseconds and runs in CI, which has no WSL. The live `#[ignore]` tests stay as end-to-end proof, but they are not what guards this fix.

Line numbers are as of `c4db8a10` and will drift as tasks land. Anchor on the item name, not the number.

---

### Task 1: `Timing` and the `wedged` predicate

The whole liveness decision as a pure function, plus the two periods it reads, held in a struct so a test can drive the loop in milliseconds instead of half a minute. Nothing calls either yet.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs` (add beside `RESPAWN_COOLDOWN`, around `:328`)
- Test: `alacritree/src/wsl_helper.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `struct Timing { slice: Duration, silence_limit: Duration }` with `const DEFAULT: Timing`, and `fn wedged(timing: &Timing, asked: Duration, slept: Duration, silence: Duration) -> bool`.

The predicate takes two conditions, not three. The "already quiet before this request" case is handled by the caller in Task 5, which never lets `silence` exceed how long *this waiter* has been watching. Folding that into the predicate as a `since_sent` argument looked equivalent and is not: it condemns on silence the waiter never observed.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/wsl_helper.rs`:

```rust
#[test]
fn a_starved_slice_never_condemns_the_helper() {
    // The waiter asked for five seconds and the host handed it back twenty,
    // so the silence it measured under that is not evidence of anything.
    assert!(!wedged(
        &Timing::DEFAULT,
        Duration::from_secs(5),
        Duration::from_secs(20),
        Duration::from_secs(300),
    ));
}

#[test]
fn silence_past_the_limit_in_a_punctual_slice_condemns_the_helper() {
    assert!(wedged(
        &Timing::DEFAULT,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(31),
    ));
}

#[test]
fn silence_inside_the_limit_is_not_evidence() {
    assert!(!wedged(
        &Timing::DEFAULT,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(29),
    ));
}

#[test]
fn a_test_timing_scales_the_whole_decision_down() {
    // The loop under test has to run in milliseconds, so the limit the
    // predicate reads has to come from the struct rather than a constant.
    let fast = Timing { slice: Duration::from_millis(50), silence_limit: Duration::from_millis(300) };
    assert!(wedged(&fast, Duration::from_millis(50), Duration::from_millis(50), Duration::from_millis(301)));
    assert!(!wedged(&fast, Duration::from_millis(50), Duration::from_millis(50), Duration::from_millis(299)));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree -E 'test(/wsl_helper::tests::(a_starved|silence_past|silence_inside|a_test_timing)/)'`  (nextest takes one filter expression, not a list of names)

Expected: FAIL to compile, `cannot find function 'wedged' in this scope`.

- [ ] **Step 3: Write the implementation**

Add beside the existing timeout constants in `alacritree/src/wsl_helper.rs`:

```rust
/// The two periods the liveness decision reads.  A struct rather than
/// constants so a test can drive the wait loop in milliseconds; production
/// only ever uses `DEFAULT`.
struct Timing {
    /// How often a waiter pings and re-examines the transport.  Matches the
    /// period zed's remote client uses for the same job.
    slice: Duration,
    /// Six slices.  VS Code's equivalent tolerates four and AMQP two, both
    /// against peers that are not sharing vCPUs with the judge.
    silence_limit: Duration,
}

impl Timing {
    const DEFAULT: Self =
        Self { slice: Duration::from_secs(5), silence_limit: Duration::from_secs(30) };
}

/// Whether an expired slice is evidence the transport is dead.
///
/// `slept` past twice `asked` means the waiter's own sleep overran, so the
/// host was starved and nothing measured under it is evidence of anything.
/// `silence` is how long the caller has *observed* no bytes, which is not
/// the same as how old the last byte is: after a resume the last byte is
/// legitimately hours old with nobody watching.
fn wedged(timing: &Timing, asked: Duration, slept: Duration, silence: Duration) -> bool {
    slept <= asked * 2 && silence > timing.silence_limit
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree wsl_helper::`

Expected: PASS, including every pre-existing `wsl_helper` test.

`wedged` and `Timing` are not used yet, so `cargo check -p alacritree` will warn they are dead code. That is expected until Task 5; do not add `#[allow(dead_code)]` to silence it. CI runs clippy without `-D warnings`, so the warning does not break the build at any point in this stack.

- [ ] **Step 5: Commit**

```sh
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): add the transport liveness predicate

A slice whose own sleep overran says nothing about the far end, so the
rule that decides a helper is dead has to weigh the waiter's own lateness
alongside how long it has watched the pipe stay quiet.  Written as a pure
function over an injectable pair of periods, so the decision is testable
without a child process and the loop around it is testable in
milliseconds.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 2: Teardown can kill the helper process

`mark_down` today drops `ChildStdin` and hopes the far end notices the EOF. If the `wsl.exe` relay is what stopped talking, no EOF arrives, the reader thread blocks in `read` forever holding the `Child`, and nothing can end the process. It also hangs the thread doing the teardown: `request` holds the stdin mutex across `write_all`, so a blocked write parks `mark_down` behind it.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs` — `HelperClient` struct (`:350-357`), `spawn` (`:368-405`), `mark_down` (`:441-450`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: field `child: Mutex<Option<std::process::Child>>` on `HelperClient`, which Task 3's test constructor must initialise.

- [ ] **Step 1: Add the field**

In the `HelperClient` struct in `alacritree/src/wsl_helper.rs`, after `down`:

```rust
    /// Kept so a teardown can end a `wsl.exe` that stopped draining its
    /// pipes.  Dropping stdin only reaches a helper still listening for the
    /// EOF.
    child: Mutex<Option<std::process::Child>>,
```

- [ ] **Step 2: Store the child instead of moving it into the reader thread**

In `spawn`, add `child: Mutex::new(None),` to the `Arc::new(Self { ... })` literal, then replace the reader-thread block. It reads today:

```rust
        let spawned =
            std::thread::Builder::new().name(format!("wsl-helper-{distro}")).spawn(move || {
                reader.read_loop(stdout);
                // Stdin is closed by mark_down; reap so a dead helper never
                // lingers as a zombie in the process table.
                let _ = child.wait();
            });
```

Replace with:

```rust
        *lock(&client.child) = Some(child);
        let spawned =
            std::thread::Builder::new().name(format!("wsl-helper-{distro}")).spawn(move || {
                reader.read_loop(stdout);
                // Reap so a dead helper never lingers as a zombie in the
                // process table.  Taking it also releases the handle a
                // teardown would otherwise still be able to kill.
                let finished = lock(&reader.child).take();
                if let Some(mut child) = finished {
                    let _ = child.wait();
                }
            });
```

The `*lock(&client.child) = Some(child);` line must come after `child.stdin.take()` and `child.stdout.take()`, which both need `child` by mutable reference. Place it directly after the existing `let stdout = ...` line, and drop `mut` from `let mut child` only if the compiler asks.

**Take into a local before waiting.** Writing this as `if let Some(mut child) = lock(&reader.child).take()` compiles and is wrong: a temporary in an `if let` scrutinee lives to the end of the then-block, so the child mutex stays locked for the whole of `child.wait()`. Any concurrent `mark_down` then blocks on `lock(&self.child)` until `wsl.exe` exits, which is exactly the wait this task exists to remove.

- [ ] **Step 3: Kill before dropping stdin in `mark_down`**

In `mark_down`, insert before the existing `*lock(&self.stdin) = None;`:

```rust
        // Closing stdin cannot be the teardown: a writer parked inside
        // `write_all` holds the stdin lock until its write fails, and a
        // relay whose Linux side is already gone forwards no EOF at all.
        // Killing first bounds both.  The close below lands microseconds
        // later, so no ordering here gives the helper's EXIT trap a real
        // chance to run; Task 6's startup sweep is what reclaims the
        // directory this leaves behind.
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
```

Do not try to close stdin first for a graceful EOF. Closing `ChildStdin` completes `wsl.exe`'s pending `ReadFile` in a few microseconds, but the relay thread then has to be scheduled back to user mode and issue `shutdown()` on the hvsocket before `TerminateProcess` lands tens of microseconds later, and a terminate APC is delivered on exactly that kernel-to-user transition. Even winning that race only buys the VM a millisecond-scale sequence: hvsocket delivery, `init` scheduled, `sh` woken from `read`, then fork+exec `rm`. Two orders of magnitude against a window measured in tens of microseconds, and load only widens the gap.

- [ ] **Step 4: Verify the build and the whole suite**

Run: `cargo check -p alacritree && cargo nextest run -p alacritree`

Expected: clean build; every test passes. The suite is the gate here because `read_loop` calls `mark_down` on its own thread and then takes the child, so a wrong ordering deadlocks rather than misbehaving quietly.

- [ ] **Step 5: Verify against a real helper**

Run: `cargo nextest run -p alacritree wsl_helper::tests::helper_round_trips --run-ignored all`

Expected: PASS. That test spawns a real helper, exercises the full round trip and shuts it down, so it covers the spawn and reap restructure end to end.

- [ ] **Step 6: Prove that killing actually unblocks a blocked write**

This task's whole claim is that killing the child fails a `write_all` parked on a pipe the far end stopped draining. Nothing so far tests it, and Task 5's live wedge cannot: that wedge removes the completion FIFO while the dispatcher keeps draining stdin, so no write ever blocks. Whether a duplicate stdin handle held by a relay child keeps the pipe open past `TerminateProcess` is recorded in the spec as suspected and unverified. If it does, `mark_down` parks on `lock(&self.stdin)` behind the blocked writer and teardown hangs, which is worse than the bug this plan fixes.

Add to `mod tests`:

```rust
/// Killing the child is what frees a writer parked inside `write_all` on a
/// pipe nobody is draining.  Requires WSL and kills the shared helper for
/// the default distro, so run it on its own:
/// `cargo nextest run -p alacritree wsl_helper::tests::killing_a_helper --run-ignored all`
#[test]
#[ignore]
fn killing_a_helper_frees_a_writer_blocked_on_its_pipe() {
    let distro =
        crate::wsl::distros().into_iter().find(|d| d.is_default).expect("a default distro");
    let ready_by = Instant::now() + Duration::from_secs(120);
    let client = loop {
        if let Some(c) = client(&distro.name) {
            break c;
        }
        assert!(Instant::now() < ready_by, "helper never became ready");
        std::thread::sleep(Duration::from_millis(200));
    };

    // A job's parent is the backgrounded subshell and *its* parent is the
    // dispatcher, which is the process that has to stop reading stdin for a
    // write to block.  Field 4 of /proc/<pid>/stat is the ppid.
    let (exit, _) = client
        .run("kill -STOP \"$(awk '{print $4}' /proc/$PPID/stat)\"", &[])
        .expect("the stop request is answered before the dispatcher freezes");
    assert_eq!(exit, 0, "the dispatcher was never stopped");

    // Windows pipe buffers are about 64 KiB, so a few hundred KiB of
    // argument is certain to fill one and park the writer under the lock.
    let writer = client.clone();
    let blocked = std::thread::spawn(move || {
        let big = "x".repeat(200 * 1024);
        let _ = writer.run("printf ''", &[&big]);
    });

    std::thread::sleep(Duration::from_secs(1));
    let tore_down = std::thread::spawn(move || client.mark_down("blocked-write test"));

    let deadline = Instant::now() + Duration::from_secs(15);
    while !(blocked.is_finished() && tore_down.is_finished()) {
        assert!(Instant::now() < deadline, "kill did not free the blocked writer");
        std::thread::sleep(Duration::from_millis(100));
    }
    blocked.join().expect("writer thread");
    tore_down.join().expect("teardown thread");
}
```

Run: `cargo nextest run -p alacritree wsl_helper::tests::killing_a_helper --run-ignored all`

Expected: PASS in a couple of seconds.

**If it fails, stop and change the design before Task 3.** The fallback is a dedicated writer thread fed by an `mpsc` channel, so `request` hands off a line and never holds a lock across a pipe write at all. That removes the blocked-write hazard instead of relying on `kill` to clear it, and it changes what Tasks 3 to 5 build on, so it cannot be deferred. `SIGCONT` the stopped dispatcher (`kill -CONT`) or restart the app before re-running anything else against that distro.

- [ ] **Step 7: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/wsl_helper.rs
git commit -m "fix(wsl): let a teardown end the helper process

Marking a helper down closed its stdin and trusted it to notice.  When the
far end is what stopped reading, that EOF never lands: the reader thread
blocks forever holding the only handle to the process, and a teardown
racing a blocked write parks behind the stdin lock it will never get.

Keep the child on the client and kill it before touching that lock, so the
blocked write fails and releases it.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 3: Box the pipes, stamp every byte, and let a test stand in for the helper

Two changes that have to land together, because the second is what makes the rest of the plan testable. Liveness needs "bytes arrived recently", which no timestamp records today. And every method that failed reads and writes two pipes it has no reason to know are a `wsl.exe`'s: behind `Read` and `Write`, a test drives the whole client with a fake helper it fully controls.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs` — `HelperClient` struct, `spawn` (`:368-405`), `read_loop` (`:410-439`), plus two methods and a test-only constructor
- Test: `alacritree/src/wsl_helper.rs`, in `mod tests`

**Interfaces:**
- Consumes: `child: Mutex<Option<std::process::Child>>` from Task 2; `Timing` from Task 1.
- Produces: fields `started: Instant`, `last_bytes_at: AtomicU64`, `timing: Timing`; `stdin` retyped to `Mutex<Option<Box<dyn Write + Send>>>`; `read_loop` retyped to take `Box<dyn Read + Send>`; methods `fn stamp_bytes(&self)` and `fn silent_for(&self) -> Duration`; test-only `fn over(distro: &str, reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>, timing: Timing) -> Arc<Self>`, which starts the reader thread exactly as `spawn` does.

There is no `detached` constructor. A client with no pipes behind it can only test arithmetic; `over` costs the same and tests the wiring.

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
#[test]
fn a_client_that_has_never_read_reports_its_whole_life_as_silence() {
    // The helper end is bound rather than dropped: dropping it closes the
    // pipe, which the reader would correctly read as EOF and tear down.
    let (client, _helper) = FakeHelper::silent();
    std::thread::sleep(Duration::from_millis(20));
    // Never stamped past the hello, so the silence covers the sleep.  The
    // clock only moves forward, so this bound cannot invert under load.
    assert!(client.silent_for() >= Duration::from_millis(20));

    client.stamp_bytes();
    // A stamp at any point after construction leaves less silence behind it
    // than the client has been alive, whatever the scheduler does next.
    assert!(client.silent_for() < client.started.elapsed());
}
```

`FakeHelper` arrives in Step 3 below, alongside the constructor it needs.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p alacritree wsl_helper::tests::a_client_that_has_never_read`

Expected: FAIL to compile, `cannot find type 'FakeHelper' in this scope`.

- [ ] **Step 3: Retype the pipes, add the fields, the methods and the fake**

In the `HelperClient` struct, retype `stdin` and add the new fields after the `child` field from Task 2:

```rust
    /// Boxed rather than a `ChildStdin` so a test can stand a fake helper
    /// behind the same client the app uses.
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
```

```rust
    /// Monotonic base for `last_bytes_at`, which is stored as elapsed
    /// milliseconds so the read path stays lock-free.
    started: Instant,
    /// Milliseconds since `started` at the last successful read off the
    /// helper's stdout.  Bytes, not frames: a partially delivered frame is
    /// still proof the far end is producing output.
    last_bytes_at: AtomicU64,
    timing: Timing,
```

In `spawn`, box both pipes and fill the new fields. `*lock(&client.stdin) = child.stdin.take();` becomes:

```rust
        *lock(&client.stdin) =
            child.stdin.take().map(|w| Box::new(w) as Box<dyn Write + Send>);
```

and the `let stdout = ...` line becomes `let stdout = Box::new(child.stdout.take().expect("stdout piped above")) as Box<dyn Read + Send>;`. Add `started: Instant::now(),`, `last_bytes_at: AtomicU64::new(0),` and `timing: Timing::DEFAULT,` to the `Arc::new(Self { ... })` literal.

Change `read_loop`'s signature from `fn read_loop(&self, stdout: std::process::ChildStdout)` to `fn read_loop(&self, stdout: Box<dyn Read + Send>)`. `BufReader::new` takes it unchanged.

In `impl HelperClient`, beside `is_ready`:

```rust
    fn stamp_bytes(&self) {
        self.last_bytes_at.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// How long the helper has produced nothing at all.  `Relaxed` is
    /// enough because no other memory is published under the stamp, and a
    /// waiter that observes a stale value only defers judgment by one slice.
    fn silent_for(&self) -> Duration {
        let now = self.started.elapsed().as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.last_bytes_at.load(Ordering::Relaxed)))
    }

    /// A client over arbitrary pipes, so a test can be the helper.  Starts
    /// the reader thread the same way `spawn` does; there is no child to
    /// reap, so teardown finds `None` and skips the kill.
    #[cfg(test)]
    fn over(
        distro: &str,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        timing: Timing,
    ) -> Arc<Self> {
        let client = Arc::new(Self {
            distro: distro.to_string(),
            stdin: Mutex::new(Some(writer)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            capabilities: OnceLock::new(),
            down: AtomicBool::new(false),
            child: Mutex::new(None),
            started: Instant::now(),
            last_bytes_at: AtomicU64::new(0),
            timing,
        });
        let owner = client.clone();
        std::thread::spawn(move || owner.read_loop(reader));
        client
    }
```

Then the fake itself, in `mod tests`. A channel pair is enough: the client's `Write` end sends bytes to the test, and its `Read` end drains what the test sends back.

```rust
    /// One end of a pipe pair standing in for the helper's stdio.
    struct FakePipe {
        rx: mpsc::Receiver<Vec<u8>>,
        buf: std::collections::VecDeque<u8>,
    }

    impl Read for FakePipe {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            while self.buf.is_empty() {
                // A disconnected sender is the far end closing its pipe,
                // which `read_loop` reads as EOF exactly as it would from a
                // real one.
                let Ok(chunk) = self.rx.recv() else { return Ok(0) };
                self.buf.extend(chunk);
            }
            let n = out.len().min(self.buf.len());
            for slot in out.iter_mut().take(n) {
                *slot = self.buf.pop_front().expect("checked above");
            }
            Ok(n)
        }
    }

    struct FakeSink(mpsc::Sender<Vec<u8>>);

    impl Write for FakeSink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.send(data.to_vec());
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A client wired to a fake helper, plus the two ends the test drives it
    /// from: `to_client` writes what the helper says, `from_client` reads
    /// what the client sent.
    struct FakeHelper {
        to_client: mpsc::Sender<Vec<u8>>,
        from_client: mpsc::Receiver<Vec<u8>>,
    }

    impl FakeHelper {
        /// A helper that says hello and then never speaks again.
        fn silent() -> (Arc<HelperClient>, FakeHelper) {
            Self::with_timing(Timing::DEFAULT)
        }

        fn with_timing(timing: Timing) -> (Arc<HelperClient>, FakeHelper) {
            let (to_client, client_reads) = mpsc::channel();
            let (client_writes, from_client) = mpsc::channel();
            to_client.send(HELLO_LINE.as_bytes().to_vec()).expect("client not started yet");
            let client = HelperClient::over(
                "fake",
                Box::new(FakePipe { rx: client_reads, buf: Default::default() }),
                Box::new(FakeSink(client_writes)),
                timing,
            );
            let ready_by = Instant::now() + Duration::from_secs(5);
            while !client.is_ready() {
                assert!(Instant::now() < ready_by, "fake hello was never parsed");
                std::thread::sleep(Duration::from_millis(5));
            }
            (client, FakeHelper { to_client, from_client })
        }
    }
```

`HELLO_LINE` must be a hello the real `parse_hello` accepts, ending in `\n`. Read `parse_hello` and the `HELPER_SCRIPT`'s hello `printf` and copy the exact shape rather than inventing one; a hello the parser rejects makes every fake test fail at "fake hello was never parsed" with nothing else wrong.

`Read` and `Write` must be in scope in both the module and `mod tests`; add `use std::io::Read;` beside the existing `Write` import if the compiler asks.

In `read_loop`, the `Ok(n)` arm reads today:

```rust
                Ok(n) => match frames.push(&chunk[..n]) {
```

Change it to stamp first:

```rust
                Ok(n) => {
                    self.stamp_bytes();
                    match frames.push(&chunk[..n]) {
```

and close the extra brace after the existing `Err(e) => return self.mark_down(&e),` arm's closing `},` so the block balances. The arm becomes:

```rust
                Ok(n) => {
                    self.stamp_bytes();
                    match frames.push(&chunk[..n]) {
                        Ok(done) => {
                            for frame in done {
                                if let Some(tx) = lock(&self.pending).remove(&frame.id) {
                                    let _ = tx.send(frame);
                                }
                            }
                        },
                        Err(e) => return self.mark_down(&e),
                    }
                },
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p alacritree wsl_helper::`

Expected: PASS, all `wsl_helper` tests.

- [ ] **Step 5: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): record when the helper last produced output

A reply that is merely late and one that will never come are the same
event to a deadline, and on a loaded Windows box the first is common.  They
differ in whether any bytes arrive at all, which needs a timestamp the
transport did not keep.

Stamp on reads rather than on frames, so a half-delivered frame still
counts as the far end doing work.

Box both pipes behind Read and Write while the read path is open anyway.
Nothing in the client needs the far end to be a wsl.exe, and a fake one
puts the paths that go wrong under test on a machine with no WSL.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 4: A ping the dispatcher answers

The signal has to travel through the dispatcher. A keepalive emitted by a timer inside the helper would prove the writer, the FIFO and the relay while a dispatcher stuck in its inline `PROBE` branch reported health.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs` — `HELPER_SCRIPT` dispatcher (`:178-231`), plus one method
- Test: `alacritree/src/wsl_helper.rs`, in `mod tests`

**Interfaces:**
- Consumes: `HelperClient::over` and `FakeHelper` from Task 3.
- Produces: `fn ping(&self)` on `HelperClient`, and a `PING` request kind the dispatcher answers with a frame carrying reserved id `0`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn a_ping_reaches_the_far_end_as_a_reserved_id_zero_line() {
    let (client, helper) = FakeHelper::silent();
    client.ping();
    let sent = helper.from_client.recv_timeout(Duration::from_secs(5)).expect("a ping was sent");
    assert_eq!(sent, b"0\tPING\n");
}

#[test]
fn a_ping_with_nowhere_to_write_is_silently_skipped() {
    let (client, _helper) = FakeHelper::silent();
    // A torn-down client has no stdin.  A ping that cannot be sent is one
    // more slice of silence, which the wait loop already handles; it must
    // not panic and must not report anything new.
    client.mark_down("test");
    client.ping();
    assert!(client.is_down());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree -E 'test(/wsl_helper::tests::a_ping_/)'`

Expected: FAIL to compile, `no method named 'ping' found`.

- [ ] **Step 3: Add the dispatcher branch**

In `HELPER_SCRIPT`, add a branch after the `PROBE)` arm's closing `;;` and before the `esac`:

```sh
  PING) printf '0 0\n' >> "$t/done" & ;;
```

The trailing `&` is load-bearing and must not be dropped. A foreground append blocks the dispatcher inside `open()` once the writer subshell is dead, which would turn a dead writer into a dead dispatcher and break the stdin-EOF cleanup. Backgrounded, the dispatcher keeps reading; one blocked pinger accumulates per unanswered ping, and the EXIT trap's `kill 0` reaps them, exactly as it does for blocked `RUN` jobs today.

Reserved id `0` cannot collide: `next_id` starts at 1. The existing writer turns the completion into a `0\t0\t0\n` frame with no change to the writer, and `read_loop` drops a frame with no pending entry, so nothing routes it.

Add one line to the `HELPER_SCRIPT` doc comment's shape sentence so the branch is accounted for, changing "then the request dispatcher on stdin" to "then the request dispatcher on stdin, whose `PING` answers with an unrouted frame so a caller can tell a stalled dispatcher from a slow one".

- [ ] **Step 4: Add the method**

In `impl HelperClient`, after `silent_for`:

```rust
    /// Ask the dispatcher to prove it is still reading.  The reply is a
    /// frame nothing routes; its only effect is refreshing `last_bytes_at`.
    fn ping(&self) {
        // A held stdin lock is a reason to skip, never to wait: blocking
        // here would park the waiter inside the failure it came to detect,
        // and the thread holding the lock is itself proof of a live write.
        let Ok(mut guard) = self.stdin.try_lock() else { return };
        if let Some(stdin) = guard.as_mut() {
            let _ = stdin.write_all(b"0\tPING\n").and_then(|()| stdin.flush());
        }
    }
```

`ping` is not called yet, so expect an unused-method warning until Task 5. Do not silence it.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree wsl_helper::`

Expected: PASS.

- [ ] **Step 6: Verify the branch against a real helper**

Run: `cargo nextest run -p alacritree wsl_helper::tests::helper_round_trips --run-ignored all`

Expected: PASS. A malformed `PING` branch is a shell syntax error, which kills the helper before its hello and fails this test at "helper never became ready".

- [ ] **Step 7: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): answer a ping through the helper's dispatcher

The dispatcher runs the foreground probe inline, so it can stop reading
stdin while the writer subshell stays healthy.  Anything the helper emitted
on its own timer would keep arriving through that and report health.

A ping the dispatcher has to read and answer travels the path a real reply
travels, so the one component a timer cannot see is the one it proves.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 5: Slice the wait and tear down on silence

> **As implemented, this task's two test bodies below are wrong and were corrected during review.** `a_slow_job_over_a_healthy_pipe_is_never_torn_down` as written never sends a ping: the first message the answering thread receives is the `RUN` line, the `if` is false, control falls through to the job reply and returns, so the test measured about 4 ms and guarded only a single-slice round trip. The shipped version inverts the branch and `continue`s on any non-ping message, which takes about 276 ms across four real ping round trips. Separately, both tests as written still passed with the entire `watching_since` mechanism deleted, so a third test was added in which the client sits quiet past the silence limit *before* the request is sent; it was proven RED with the clamp removed. Its margin is 50 ms slice, 1 s limit, 1200 ms pre-sleep, which leaves about 950 ms of slack. The `slept > asked * 2` arithmetic was extracted as `fn starved(asked, slept) -> bool { slept > asked.saturating_mul(2) }`, called by both `wedged` and the loop's reset so the two sites cannot drift, and unit-tested directly. **Known residual:** `starved` guards the arithmetic and call-site agreement, not the loop's *placement* of the reset. No test drives a real scheduler-induced overrun, because doing so would mean controlling the waiter's own scheduling, which is the flake this project refuses.

Wires Tasks 1, 3 and 4 into `request`, and adds the only test that exercises what actually failed.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs` — `request` (`:464-493`)
- Test: `alacritree/src/wsl_helper.rs`, in `mod tests`

**Interfaces:**
- Consumes: `wedged` and `Timing` (Task 1); `silent_for`, `over`, `FakeHelper` (Task 3); `ping` (Task 4).
- Produces: no new public surface. `request` keeps its signature `fn request(&self, id: u64, line: String, timeout: Duration) -> Result<Frame, TransportError>`.

- [ ] **Step 1: Write the failing tests**

The first two run in CI on any platform and are what actually guard this fix. The third is the end-to-end proof and needs WSL.

Add to `mod tests`:

```rust
#[test]
fn a_helper_that_stops_answering_is_torn_down_rather_than_waited_out() {
    // Scaled down by two orders of magnitude: the decision is the same one
    // production makes, taken in a third of a second.
    let timing =
        Timing { slice: Duration::from_millis(50), silence_limit: Duration::from_millis(300) };
    let (client, helper) = FakeHelper::with_timing(timing);

    let started = Instant::now();
    let result = client.run("printf hi", &[]);

    assert!(matches!(result, Err(TransportError::NoReply(_))), "a silent helper answers nothing");
    assert!(client.is_down(), "a silent helper must be torn down, not merely reported");
    assert!(lock(&client.pending).is_empty(), "the waiter left its channel behind");
    assert!(
        started.elapsed() < RUN_TIMEOUT,
        "gave up on the run budget rather than on silence, after {:?}",
        started.elapsed()
    );

    // The waiter asked the dispatcher to prove it was reading, which is the
    // signal the old code had no way to send.
    let mut pings = 0;
    while let Ok(sent) = helper.from_client.try_recv() {
        if sent == b"0\tPING\n" {
            pings += 1;
        }
    }
    assert!(pings > 0, "no ping was ever sent");
}

#[test]
fn a_slow_job_over_a_healthy_pipe_is_never_torn_down() {
    // A silence limit far longer than the test's own runtime: a false
    // teardown here would need the reader thread starved for five seconds
    // inside a test that finishes in a fraction of one.
    let timing =
        Timing { slice: Duration::from_millis(50), silence_limit: Duration::from_secs(5) };
    let (client, helper) = FakeHelper::with_timing(timing);

    let answering = std::thread::spawn(move || {
        let mut slices = 0;
        while let Ok(sent) = helper.from_client.recv_timeout(Duration::from_secs(10)) {
            if sent == b"0\tPING\n" {
                slices += 1;
                let _ = helper.to_client.send(b"0\t0\t0\n".to_vec());
                if slices < 4 {
                    continue;
                }
            }
            // The job finishes after four answered pings: slow, but the
            // pipe was never quiet.
            let _ = helper.to_client.send(b"1\t0\t2\nhi".to_vec());
            return helper;
        }
        helper
    });

    let (exit, payload) = client.run("printf hi", &[]).expect("a slow job still answers");
    assert_eq!(exit, 0);
    assert_eq!(payload, b"hi");
    assert!(!client.is_down(), "a healthy pipe was torn down");
    assert!(lock(&client.pending).is_empty());
    let _ = answering.join();
}
```

The reply frames are written by hand here, so check them against `encode_run`'s id numbering and `FrameReader`'s header shape before running. `run` takes the id from `next_id`, which starts at 1, so the first job's reply is id `1`; `0\t0\t0\n` is the ping answer with a zero-length payload.

Then the live test:

```rust
/// A wedged helper is torn down rather than making every later caller pay
/// the full run budget.  Requires WSL, and it deliberately kills the shared
/// helper for the default distro, so run it on its own:
/// `cargo nextest run -p alacritree wsl_helper::tests::a_wedged_helper --run-ignored all`
#[test]
#[ignore]
fn a_wedged_helper_is_torn_down_once_it_stops_answering() {
    let distro =
        crate::wsl::distros().into_iter().find(|d| d.is_default).expect("a default distro");
    let ready_by = Instant::now() + Duration::from_secs(120);
    let client = loop {
        if let Some(c) = client(&distro.name) {
            break c;
        }
        assert!(Instant::now() < ready_by, "helper never became ready");
        std::thread::sleep(Duration::from_millis(200));
    };

    // A job's stdout is `$t/<id>.out`, so its own fd 1 names the directory
    // holding the completion fifo.  `$$` rather than `self`, because inside
    // a command substitution `/proc/self` is the substitution's own pipe.
    // Removing the fifo leaves the writer blocked on a deleted inode while
    // later completions land in a regular file nobody reads, which is the
    // wedge this test needs.  The removal is delayed so this request still
    // gets its own answer back.
    let (exit, _) = client
        .run(
            r#"d=$(readlink /proc/$$/fd/1); d=${d%/*}
[ -p "$d/done" ] || exit 1
( sleep 1; rm -f "$d/done" ) >/dev/null 2>&1 &"#,
            &[],
        )
        .expect("the wedge request is answered before the fifo goes");
    assert_eq!(exit, 0, "the job did not find the completion fifo");

    let down_by = Instant::now() + Duration::from_secs(90);
    while !client.is_down() {
        assert!(Instant::now() < down_by, "a wedged helper was never marked down");
        let _ = client.run("printf x", &[]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree -E 'test(/wsl_helper::tests::(a_helper_that_stops|a_slow_job_over)/)'`

Expected: `a_helper_that_stops_answering` FAILs after 60 seconds on `gave up on the run budget rather than on silence`, which is the bug stated as a runtime fact. `a_slow_job_over_a_healthy_pipe` PASSes already, because today's code never tears anything down; it is the guard that Step 3 does not overshoot.

Then the live one: `cargo nextest run -p alacritree wsl_helper::tests::a_wedged_helper --run-ignored all`

Expected: FAIL after about 120 seconds with `a wedged helper was never marked down`. The loop runs two full 60 second `run` calls before the deadline check trips, so budget for two minutes, not the 90 seconds the assertion names. Confirm it is that assertion and not `the job did not find the completion fifo`, which would mean the wedge never took and the test is measuring nothing.

- [ ] **Step 3: Replace the wait with a sliced loop**

In `request`, the tail reads today:

```rust
        match rx.recv_timeout(timeout) {
            Ok(frame) => Ok(frame),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(TransportError::NoReply(format!("no reply from the {} helper", self.distro)))
            },
        }
    }
```

Replace with:

```rust
        // Liveness is asked as "has anything arrived recently", not "was
        // this reply on time": a loaded host delivers late, a wedged helper
        // never delivers, and only the second is worth a teardown.
        let sent_at = Instant::now();
        let deadline = sent_at + timeout;
        // Silence only counts while somebody was awake to observe it.  A
        // slice that overran means the host was starved, and the quiet
        // underneath it says nothing about the far end, so the window
        // restarts rather than carrying that stretch forward.
        let mut watching_since = sent_at;
        loop {
            let slice_start = Instant::now();
            let remaining = deadline.saturating_duration_since(slice_start);
            if remaining.is_zero() {
                lock(&self.pending).remove(&id);
                return Err(TransportError::NoReply(format!(
                    "no reply from the {} helper",
                    self.distro
                )));
            }
            let asked = remaining.min(self.timing.slice);
            match rx.recv_timeout(asked) {
                Ok(frame) => return Ok(frame),
                // The sender is dropped by `mark_down`'s `pending.clear()`,
                // so a disconnect means someone already tore this down.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(TransportError::NoReply(format!(
                        "the {} helper went down while waiting",
                        self.distro
                    )));
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {},
            }
            let slept = slice_start.elapsed();
            let silence = self.silent_for().min(watching_since.elapsed());
            if slept > asked * 2 {
                watching_since = Instant::now();
            }
            if wedged(&self.timing, asked, slept, silence) {
                // The count is taken into a local first: passing
                // `lock(...).len()` as an argument keeps the guard alive
                // across the call, and `mark_down` takes the same mutex.
                let outstanding = {
                    let mut pending = lock(&self.pending);
                    pending.remove(&id);
                    pending.len()
                };
                self.mark_down(&format!(
                    "silent for {:.0}s with {outstanding} outstanding",
                    silence.as_secs_f64()
                ));
                return Err(TransportError::NoReply(format!(
                    "no reply from the {} helper",
                    self.distro
                )));
            }
            // After the judgment, so the next slice reads the answer to this
            // slice's question rather than to one sent moments ago.
            self.ping();
        }
    }
```

`RUN_TIMEOUT` stays at 60 seconds and stays a job budget: a slow `worktree add` keeps the helper up because pings keep being answered underneath it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree -E 'test(/wsl_helper::tests::(a_helper_that_stops|a_slow_job_over)/)'`

Expected: PASS, both, in well under a second each. `a_helper_that_stops_answering` should take roughly 350 milliseconds; anything near 60 seconds means the teardown branch is unreachable and the test is passing on the run budget.

Then the live one: `cargo nextest run -p alacritree wsl_helper::tests::a_wedged_helper --run-ignored all`

Expected: PASS in roughly 30 to 35 seconds after the wedge lands. Detection needs 30 seconds of observed silence, so the first `run` in the loop is the one that trips it.

- [ ] **Step 5: Run the whole suite**

Run: `cargo nextest run -p alacritree`

Expected: PASS. Then, separately, `cargo nextest run -p alacritree wsl_helper::tests::helper_round_trips --run-ignored all` to confirm a healthy helper is still never torn down: that test runs a deliberately slow job alongside a fast one and would now fail if a healthy pipe tripped the predicate.

- [ ] **Step 6: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/wsl_helper.rs
git commit -m "fix(wsl): tear down a helper that has gone silent

A missed reply was treated as a one-off, so a helper that stopped answering
while its pipe stayed open was never marked down and every later call paid
the full sixty second budget.  One did that for twenty-six hours.

Wait in slices instead, pinging the dispatcher between them and judging on
whether any bytes arrived rather than on whether this reply was punctual.
A slice whose own sleep overran declines to judge and restarts the
observation window, so a thread that was descheduled through a quiet
stretch cannot come back and condemn a healthy helper for it.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

---

### Task 6: A helper start sweeps what a teardown could not

Task 2 leaves the helper's temp directory and its FIFO behind on every teardown that ends the relay, and candidate 4 leaves them behind with no client involved at all. Nothing on the Windows side can reclaim either without paying a `wsl.exe` spawn against a VM that may be the thing that wedged. The distro can reclaim both for free at the only moment it is already running: the next helper's startup.

The full reasoning, the rejected alternatives and the experiment protocols live in `.superpowers/sdd/2026-09-04-wsl-helper-liveness/task-6-startup-sweep-design.md`. Read it before Step 1.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs`, `HELPER_SCRIPT` only (the GC loop near `:158-163` and the temp-dir setup at `:164-166`, plus the doc comment at `:142-144`)
- Test: `alacritree/src/wsl_helper.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from Tasks 1-5. It changes only the shell text and can be verified on its own.
- Produces: nothing the Rust side calls. The helper's temp directory changes name from `mktemp -d`'s random suffix to `$rt/helper-<dispatcher pid>`, which no Rust code reads.

- [ ] **Step 1: Settle what relay death actually delivers**

This decides whether the `trap 'exit' HUP` line below is a prompt fix or dead weight, and whether a wedged-but-alive dispatcher survives the sweep. Write `alacritree/examples/traprace.rs`:

```rust
//! Measures whether a helper's EXIT trap runs when its `wsl.exe` relay dies,
//! and whether closing stdin first changes the answer.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const SCRIPT: &str = "t=$(mktemp -d /tmp/traprace.XXXXXX); mkfifo \"$t/done\"; trap 'rm -rf \"$t\"' EXIT; printf '%s\\n' \"$t\"; ps -o pid,ppid,pgid,sid,tpgid,tty,comm -p $$ >&2; while read -r l; do :; done";

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "kill".into());
    let rounds: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let mut leaked = 0;
    for _ in 0..rounds {
        let mut child = Command::new("wsl.exe")
            .args(["-d", "kali-linux", "--exec", "sh", "-c", SCRIPT, "sh"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut dir = String::new();
        out.read_line(&mut dir).expect("temp dir line");
        let dir = dir.trim().to_string();

        match mode.as_str() {
            "close" => drop(child.stdin.take()),
            "kill" => {
                let _ = child.kill();
            }
            _ => {
                drop(child.stdin.take());
                let _ = child.kill();
            }
        }
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(500));

        let survived = Command::new("wsl.exe")
            .args(["-d", "kali-linux", "--exec", "test", "-d", &dir])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if survived {
            leaked += 1;
            let _ = Command::new("wsl.exe")
                .args(["-d", "kali-linux", "--exec", "rm", "-rf", &dir])
                .status();
        }
        let _ = std::io::stdout().flush();
    }
    println!("{mode}: {leaked}/{rounds} leaked");
}
```

Run each mode idle, then repeat under load:

```sh
cargo run -p alacritree --example traprace -- close 30
cargo run -p alacritree --example traprace -- kill 30
cargo run -p alacritree --example traprace -- both 30
```

Then edit `SCRIPT` to add `trap 'exit' HUP` immediately after the EXIT trap and rerun the `kill` mode.

Record all four numbers in your report. `kill` leaking and `both` leaking the same proves the ordering is ceremony, which is what Task 2 already assumes. `kill` leaking 0 would mean the leak never existed and the rest of this task is unnecessary: **stop and report that**, do not implement a sweep for a problem that is not there. `kill`-with-HUP-trap leaking 0 means the trap line earns its place.

- [ ] **Step 2: Confirm a transient O_RDWR open releases a blocked FIFO writer**

Removing the directory is not enough on its own: unlinking a FIFO does not wake a process blocked in `open(O_WRONLY)` on it, so an orphaned job subshell would outlive its own temp dir. Run in the distro:

```sh
wsl.exe -d kali-linux --exec sh -c 'd=$(mktemp -d); mkfifo "$d/done"; sh -c "echo x >> $d/done" & p=$!; sleep 0.2; exec 3<>"$d/done"; exec 3<&-; sleep 0.3; if kill -0 $p 2>/dev/null; then echo still-blocked; else echo released; fi; rm -rf "$d"'
```

Expected: `released`. Paste the output into your report. If it prints `still-blocked`, the sweep can still remove directories but cannot reap; say so and leave the FIFO open out of Step 4 rather than shipping a line that does nothing.

- [ ] **Step 3: Write the failing test**

The sweep is shell, so the test asserts on the shell text the binary ships. Add to `mod tests`:

```rust
#[test]
fn the_helper_script_reclaims_a_dead_predecessors_directory() {
    // The temp dir has to be named by dispatcher pid, or a start cannot tell
    // a dead predecessor's directory from a live sibling's.
    assert!(HELPER_SCRIPT.contains("t=$rt/helper-$$"));
    assert!(!HELPER_SCRIPT.contains("mktemp -d"));

    // Liveness comes from /proc, and the stale FIFO is opened before the
    // directory goes, so a job subshell parked in open(O_WRONLY) is released
    // rather than orphaned.
    let sweep = HELPER_SCRIPT
        .split_once("for d in \"$rt\"/helper-*")
        .expect("the startup sweep")
        .1;
    let body = sweep.split_once("done\n").expect("the sweep body").0;
    assert!(body.contains("[ -d \"/proc/$p\" ] && continue"));
    let fifo = body.find("exec 3<>").expect("the fifo release");
    let remove = body.find("rm -rf \"$d\"").expect("the directory removal");
    assert!(fifo < remove, "the FIFO must be opened before the directory is removed");
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run:

```sh
cargo nextest run -p alacritree -E 'test(the_helper_script_reclaims_a_dead_predecessors_directory)'
```

Expected: FAIL, on the first assertion, because the script still calls `mktemp -d`.

- [ ] **Step 5: Write the implementation**

Replace the temp-dir setup at `:164-166` and extend the GC loop at `:158-163` so the region reads:

```sh
mkdir -p "$rt" 2>/dev/null
for f in "$rt"/session-*.pid; do
  [ -e "$f" ] || continue
  p=$(cat "$f" 2>/dev/null)
  case $p in ''|*[!0-9]*) rm -f "$f"; continue;; esac
  [ -d "/proc/$p" ] || rm -f "$f"
done
for d in "$rt"/helper-*; do
  [ -d "$d" ] || continue
  p=${d##*helper-}
  case $p in ''|*[!0-9]*) continue;; esac
  [ -d "/proc/$p" ] && continue
  [ -p "$d/done" ] && { exec 3<>"$d/done"; exec 3<&-; }
  rm -rf "$d"
done
t=$rt/helper-$$
rm -rf "$t"
mkdir -m 700 "$t" || exit 1
mkfifo "$t/done" || exit 1
trap 'rm -rf "$t"; kill 0 2>/dev/null' EXIT
trap 'exit' HUP
```

Include the `trap 'exit' HUP` line only if Step 1 showed it changes the outcome. Drop the `exec 3<>` line only if Step 2 printed `still-blocked`.

`rm -rf "$t"` before `mkdir` is not redundant: a directory carrying our own pid is stale by definition, which is how pid reuse is handled. `mkdir -m 700` preserves the permissions `mktemp -d` gave.

Then rewrite the `HELPER_SCRIPT` doc comment at `:142-144` so it says a start reclaims the directories of dead predecessors, and why: a teardown that ends the relay never delivers the EOF the EXIT trap waits on.

- [ ] **Step 6: Run the test to verify it passes**

Run:

```sh
cargo nextest run -p alacritree -E 'test(the_helper_script_reclaims_a_dead_predecessors_directory)'
```

Expected: PASS.

- [ ] **Step 7: Verify against both shells**

The script must survive dash and busybox ash, and this task adds four constructs that differ between them: `mkdir -m`, `[ -p ]`, `exec 3<>`, and `trap ... HUP`. Install busybox in the distro, then run the shipped script text under each, drive one `RUN` and one `PROBE` through it, close stdin, and confirm the directory is gone:

```sh
wsl.exe -d kali-linux --exec sh -c 'command -v busybox >/dev/null || sudo apt-get install -y busybox'
cargo nextest run -p alacritree -E 'test(/wsl_helper::tests::/)' --run-ignored all
```

Paste both shells' output into your report. A failure here is a real blocker, not a portability nicety: busybox ash is what some distros ship as `/bin/sh`.

- [ ] **Step 8: Run the whole suite**

Run:

```sh
cargo nextest run -p alacritree
```

Expected: green.

- [ ] **Step 9: Format and commit**

```sh
cargo +nightly fmt -p alacritree
git -C ../alacritree-worktrees/fix/wsl-helper-liveness status --short
```

Restore every file you did not change, then:

```sh
git add alacritree/src/wsl_helper.rs
git commit
```

Subject: `fix(wsl): reclaim a dead helper's temp dir at the next start`.

---

### Task 7: Make a stuck compute visible

Issue #58. The git panel renders `status.error` out of `StatusCache` (`app.rs:4189`), and `poll` only spawns a refresh when `self.pending.is_none()`. A compute thread that never returns therefore pins `pending` forever and freezes the panel on whatever it last held.

Whether that is what #58 actually was is unsettled. Issue #56 alone explains a panel stuck on `no reply`: every compute burns the full 60 second budget, lands the same error, and the next one starts 1.5 seconds later, so the panel looks frozen while nothing is pinned at all. Telling the two apart needs a `git-status` that succeeded while the panel showed the error, and no such observation was recorded. This task exists to make the next occurrence attributable rather than inferred.

**Files:**
- Modify: `alacritree/src/git_status.rs` - `Pending` (`:161-166`), `spawn_compute` (`:224`), `StatusCache::poll` (`:194-221`)
- Test: `alacritree/src/git_status.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from Tasks 1-5. Independent of them and can be done first.
- Produces: fields `started: Instant` and `warned: bool` on `Pending`; `pub fn stalled_for(&self) -> Option<Duration>` on `StatusCache`, returning how long the in-flight compute has been running, or `None` when nothing is in flight.

- [ ] **Step 1: Write the failing test**

Add to `git_status.rs`'s `mod tests`:

```rust
#[test]
fn a_compute_that_never_answers_is_reported_as_stalled() {
    let mut cache = StatusCache::new(PathBuf::from("/nonexistent"));
    assert_eq!(cache.stalled_for(), None, "nothing in flight yet");

    // A sender held rather than dropped is a compute that has neither
    // answered nor died, which is the state that freezes the panel.
    let (tx, rx) = mpsc::channel();
    cache.pending = Some(Pending { hint: None, rx, started: Instant::now(), warned: false });
    std::thread::sleep(Duration::from_millis(20));

    let stalled = cache.stalled_for().expect("a held compute is in flight");
    assert!(stalled >= Duration::from_millis(20));
    drop(tx);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p alacritree git_status::tests::a_compute_that_never_answers`

Expected: FAIL to compile, `no method named 'stalled_for'` and `struct 'Pending' has no field named 'started'`.

- [ ] **Step 3: Write the implementation**

Add the field to `Pending`:

```rust
    /// When the compute was spawned, so a caller can tell a slow one from
    /// one that will never answer.
    started: Instant,
```

In `spawn_compute`, add `started: Instant::now(),` to the `Pending` literal it returns.

Add the method to `impl StatusCache`, beside `last`:

```rust
    /// How long the in-flight compute has been running, or `None` when
    /// nothing is in flight.  A compute that never returns pins `pending`,
    /// and `poll` will not spawn another while it does, so the panel keeps
    /// rendering whatever it last held.
    pub fn stalled_for(&self) -> Option<Duration> {
        self.pending.as_ref().map(|pending| pending.started.elapsed())
    }
```

In `poll`, after the drain block and before the staleness decision:

```rust
        // Nothing healthy takes this long: the resident transport caps a
        // request and the fallback is a single wsl.exe round trip.  Past it
        // the panel is frozen on a stale answer rather than waiting on a
        // slow one, and that difference is invisible from outside.
        if let Some(pending) = self.pending.as_mut() {
            if !pending.warned && pending.started.elapsed() > STALL_WARNING {
                pending.warned = true;
                log::warn!(
                    "git status for {} has been computing for {:.0}s; the panel is showing a \
                     stale result",
                    self.path.display(),
                    pending.started.elapsed().as_secs_f64()
                );
            }
        }
```

Add beside `REFRESH_INTERVAL`:

```rust
/// Long enough that no healthy compute reaches it, short enough that a
/// frozen panel is recorded while the process that froze it is still alive.
const STALL_WARNING: Duration = Duration::from_secs(120);
```

**Once per compute, not once per frame.** `poll` runs from the git sidebar's paint, and egui repaints on every terminal byte, so a build scrolling in a visible session drives it at monitor rate. A warning gated only on the threshold would emit thousands of identical lines a minute, into the persistent log when `[debug] persistent_logging` is on. Add the flag to `Pending` beside `started`:

```rust
    /// Set once the stall warning has been logged, so a frozen panel
    /// repainting at monitor rate records the freeze once rather than on
    /// every frame.
    warned: bool,
```

and `warned: false,` to the literal `spawn_compute` returns.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p alacritree git_status::`

Expected: PASS, including every pre-existing `git_status` test.

- [ ] **Step 5: Commit**

```sh
cargo +nightly fmt
git add alacritree/src/git_status.rs
git commit -m "feat(git): record a status compute that never answers

The panel renders the last completed status and refreshes only when
nothing is in flight, so a compute that never returns freezes it on a
stale answer.  A frozen panel and a merely slow one look identical from
outside and neither leaves a trace.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 8: Bound the one-shot fallback

`run_batch` falls back to a single `wsl.exe` round trip whenever the resident helper is unavailable, and calls `.output()`, which waits for the child to exit with no deadline. A `wsl.exe` that never exits pins the calling thread for the life of the process, and on the git panel that thread is the in-flight status compute. Tasks 1-5 close the other unbounded wait, inside `request`.

This is the likeliest cause of #58 rather than a proven one, and it is worth fixing either way: an unbounded wait on a thread that gates a panel is a hazard whatever froze the panel last time. The 60 second kill only ever aborts read-only scripts, since every caller of `run_batch` is `git_status`, `projects` discovery, `pr_status` or a `$HOME` lookup.

Worktree creation on WSL has the same unbounded shape at `worktree.rs:157`, going through `wsl::command(..).output()` directly rather than through `run_batch`. It is out of scope here because a killed `worktree add` leaves a half-created worktree, which needs its own decision about cleanup. Name it in the PR description so it is not mistaken for something this change covered.

**Files:**
- Modify: `alacritree/src/wsl.rs` - `run_batch` (`:317-342`)
- Test: `alacritree/src/wsl.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing. Independent of every other task.
- Produces: `const ONE_SHOT_TIMEOUT: Duration` in `wsl.rs`. `run_batch` keeps its signature `pub fn run_batch(distro: &str, script: &str, args: &[&str]) -> Result<Vec<u8>, String>`.

- [ ] **Step 1: Write the failing test**

Add to `wsl.rs`'s `mod tests`:

```rust
/// A one-shot that never exits must not pin its caller.  Requires WSL; run
/// manually:
/// `cargo nextest run -p alacritree wsl::tests::a_one_shot --run-ignored all`
#[test]
#[ignore]
fn a_one_shot_that_never_exits_gives_up_rather_than_hanging() {
    let distro = distros().into_iter().find(|d| d.is_default).expect("a default distro");
    // The resident helper would answer this on its own thread and never
    // reach the fallback, so it has to be off for the duration.
    crate::wsl_helper::set_enabled(false);

    let started = Instant::now();
    let result = run_batch(&distro.name, "sleep 3600", &[]);
    let waited = started.elapsed();

    crate::wsl_helper::set_enabled(true);
    assert!(result.is_err(), "a child that never exits is not a success");
    assert!(waited < ONE_SHOT_TIMEOUT * 2, "gave up only after {waited:?}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p alacritree wsl::tests::a_one_shot --run-ignored all`

Expected: FAIL to compile, `cannot find value 'ONE_SHOT_TIMEOUT'` and `cannot find type 'Instant' in this scope`. Adding the constant on its own would instead make the test hang for an hour, which is the bug stated as a runtime fact.

- [ ] **Step 3: Write the implementation**

`wsl.rs` imports no clock today (`:8-11` is `command_ext`, `std::path`, `std::process`, `std::sync::OnceLock`). Add:

```rust
use std::time::{Duration, Instant};
```

`Instant` is used only by the test; if the compiler warns it is unused outside `cfg(test)`, move it to a `use` inside `mod tests` rather than allowing it.

Add beside the other constants in `wsl.rs`:

```rust
/// The same budget the resident transport gives a request, for the same
/// reason: a cold WSL VM can take seconds to answer, and nothing healthy
/// takes longer.
const ONE_SHOT_TIMEOUT: Duration = Duration::from_secs(60);
```

`run_batch`'s fallback reads today:

```rust
    let output = command(distro, None)
        .arg("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run wsl.exe: {e}"))?;
```

Replace it with a spawn that keeps the `Child`, so the timeout has something to end:

```rust
    let mut child = command(distro, None)
        .arg("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run wsl.exe: {e}"))?;

    // `output()` waits for exit with no deadline, so a wsl.exe that never
    // exits pins this thread for the life of the process.  Draining on
    // workers and bounding the wait here mirrors how the ipc client bounds a
    // named-pipe request from its own side.  One thread per pipe, because a
    // child that fills whichever pipe is drained second blocks there while
    // the reader is still emptying the first.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut stdout = child.stdout.take().expect("stdout piped above");
    let mut stderr = child.stderr.take().expect("stderr piped above");
    let errors = std::thread::spawn(move || {
        let mut err = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr, &mut err);
        err
    });
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut out);
        let _ = tx.send(out);
    });

    let Ok(stdout_bytes) = rx.recv_timeout(ONE_SHOT_TIMEOUT) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("wsl.exe did not finish within {}s", ONE_SHOT_TIMEOUT.as_secs()));
    };
    let stderr_bytes = errors.join().unwrap_or_default();
    let status = child.wait().map_err(|e| format!("failed to wait on wsl.exe: {e}"))?;
```

Joining the stderr thread after stdout closes is safe: both pipes close when the child exits, so a drainer that has read stdout to EOF is waiting on a stderr that is already closing. On the timeout path the `kill` closes them, so neither thread outlives the call by more than the kill takes.

Then rewrite the two lines that consumed `output`. They read today:

```rust
    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() { "wsl.exe failed".to_string() } else { stderr });
    }
    Ok(output.stdout)
```

Change them to:

```rust
    if !status.success() && stdout_bytes.is_empty() {
        let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
        return Err(if stderr.is_empty() { "wsl.exe failed".to_string() } else { stderr });
    }
    Ok(stdout_bytes)
```

Each pipe has its own drainer and the child is never waited on until both are drained, so a script that writes more than a pipe buffer to either one cannot block against a reader still busy with the other.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p alacritree wsl::tests::a_one_shot --run-ignored all`

Expected: PASS in roughly 60 seconds rather than an hour.

- [ ] **Step 5: Run the whole suite**

Run: `cargo nextest run -p alacritree`

Expected: PASS. Then `cargo nextest run -p alacritree wsl::tests::run_batch_round_trips --run-ignored all`, which exercises the fallback's success path against a real distro and would catch a botched pipe drain.

- [ ] **Step 6: Format and commit**

```sh
cargo +nightly fmt
git add alacritree/src/wsl.rs
git commit -m "fix(wsl): give the one-shot fallback a deadline

The fallback waited on wsl.exe with no deadline, so a child that never
exits pinned its caller forever.  On the git panel that caller is the
in-flight status compute, and no new one is started while one is in
flight, freezing the panel on a stale answer for the life of the app.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

## Verification before opening the PR

- [ ] `cargo nextest run -p alacritree` passes, and `a_helper_that_stops_answering_is_torn_down_rather_than_waited_out` is among the tests it ran. That one test is the regression guard for the whole fix; a suite that skips it proves nothing here.
- [ ] `cargo nextest run -p alacritree -E 'test(/wsl_helper::tests::a_helper_that_stops/)'` finishes in under two seconds. Near sixty means it passed on the run budget rather than on the teardown, which is a false green.
- [ ] `cargo nextest run -p alacritree wsl_helper:: --run-ignored all` passes, all three live tests, run one at a time.
- [ ] `cargo +nightly fmt --check` is clean.
- [ ] `cargo check -p alacritree` has no warnings about `wedged`, `Timing` or `ping` being unused.
- [ ] `git diff origin/<base>` touches only `alacritree/src/wsl_helper.rs`, `alacritree/src/git_status.rs` and `alacritree/src/wsl.rs`.
- [ ] Build the release binary and use it against a real WSL worktree for a few minutes with the git panel open, confirming no `wsl helper for ...` warning appears in the log. A healthy helper being torn down would show up there, and nowhere else.

## Self-review notes

**Spec coverage.** Spec §1 is Task 4, §2 is Task 3, §3 is Tasks 1 and 5, §4 is Task 2, §5 is Task 5's step 3 plus the branch-setup warning about the uncommitted hunk, §6 is the `mark_down` message in Task 5. The spec's "what this deliberately does not do" section adds no tasks by construction. The spec's open questions are unresolved and none blocks implementation.

**Where the design can still change under you.** Task 2 step 6 is a gate, not a check. It tests the claim the whole teardown path rests on, that killing the child frees a writer blocked on its pipe. If it fails, Tasks 3 to 5 are built on a false premise and the fix is a dedicated writer thread instead. Do not carry a failure forward.

**Issue #58 coverage.** Task 7 makes a stalled compute observable and Task 8 removes the unbounded wait that is its likeliest cause. Neither proves the panel freeze end to end, and #56 on its own is a complete alternative explanation. Task 7 exists so the next occurrence is attributable rather than inferred, and Task 8 stands on its own merits regardless of which explanation is right.

**Known gap, carried deliberately.** Spec §5 records that probes can never reach the teardown branch, because `PROBE_TIMEOUT` is 2 seconds and a single 2 second slice cannot accumulate 30 seconds of observed silence. No task changes that. Detection therefore depends on a `RUN` caller, which `git_status`'s 1.5 second refresh supplies whenever a WSL worktree is on screen.

**One unbounded wait survives.** A `request` blocked in `write_all` under the stdin lock, with no other waiter alive to judge, is still unbounded: nobody runs the slice loop, and `ping` correctly declines to block on the same lock. Reaching it needs roughly 64 KiB of unanswered requests queued into a full pipe by callers who all left without judging. Narrow, but real, and recorded in the spec rather than left to be rediscovered.

**What each layer of test is for.** Task 1's cases cover the decision. Task 5's two fake-helper cases cover the wiring that actually broke, run in CI on any platform, and are the regression guard for this fix. The three live `#[ignore]` tests are end-to-end proof and cannot run in CI, which has no WSL.

An earlier draft of this plan left the wiring entirely to the live tests and recorded that as an acceptable gap. It was not: a green CI run would have said nothing about the bug being fixed. Boxing the pipes is what closed it, and it is why Task 3 does two things instead of one.
