# WSL helper liveness design

**Goal:** a WSL helper that stops answering is detected in about 30 seconds and torn down, on a machine loaded badly enough that a fixed wall-clock deadline would fire on a healthy one.

**Issue:** [#56](https://github.com/AbysmalBiscuit/alacritree/issues/56).

**Branch:** `fix/wsl-helper-liveness`. Cut from the open PR carrying the highest `[n]` marker, which was PR 206 (`fix-selecting-text-near-the-left-side-of-the`, marker `[4]`) when this was written. Several unimplemented specs also claim `[5]`; read the tip fresh at setup time rather than trusting that number.

**Platform:** Windows only. `wsl_helper::client` returns `None` on any other target (`wsl_helper.rs:537`), so the whole transport is inert elsewhere.

**Config:** no new key. `[wsl] resident_helper` (`config.rs:1693-1697`, default `true`) already turns the transport off wholesale, and nothing here changes behaviour while the helper is answering.

## Context

alacritree keeps one long-lived `sh` per WSL distro and pipes every git request through its stdio, rather than paying a `wsl.exe` spawn per call. The protocol is one tab-separated request line in, one `<id>\t<exit>\t<len>\n` frame plus `len` payload bytes out. Requests come in two kinds, `RUN` and `PROBE`.

On 2026-09-04 a helper went silent for roughly 26 hours. Its process was alive, its pipe was open, and it emitted no frames the whole time.

It is not a one-off. A later instance's log carries 46 `no reply from the kali-linux helper` errors between 14:58:52 and 15:00:50 and exactly one teardown, hours earlier at 10:32:28, whose reason was the ordinary `closed its pipe`. That string is only produced by `HelperClient::request`, so a resident helper was accepting writes and answering none of them for two minutes of wall clock while the client stayed `is_ready()` throughout. Many timeouts with no teardown is the signature.

`mark_down` (`wsl_helper.rs:441`) fires only on a write error or a reader-thread EOF, and neither happened. `request` (`wsl_helper.rs:464`) treats a `recv_timeout` expiry as a one-off: it removes the pending entry, returns `TransportError::NoReply`, and leaves the client `is_ready()`. Every later call rewrote to the same dead pipe and burned the full 60 s `RUN_TIMEOUT`. Only restarting the app recovered it.

### What it cost

Measured on the affected machine while the helper was wedged.

| Command | Path | Measured |
| --- | --- | --- |
| `project list`, `project rename` | in-memory app state | 0.13-0.24 s |
| `project refresh <root>` | worker thread to the helper | 10 s, then the IPC cap's "app busy or closed" |
| `git-status <wsl path>` | connection thread, no IPC cap in front | 62 s |
| the same git script, run directly inside the distro | git only | 40 ms |

The git work is not slow. The 62 s is `RUN_TIMEOUT` plus overhead, and `git-status` is the one command with no 10 s IPC deadline hiding it.

### How a shell script stops responding

Not by crashing. A crash is visible: the process exits, stdout EOFs, and `read_loop` already calls `mark_down` on that (`wsl_helper.rs:425`). What produces a 26-hour silence is a **blocking syscall that never returns**, which has no error, no exit status and no log line.

The helper is not one program. It is a dispatcher reading stdin, a writer subshell owning stdout, and one backgrounded subshell per in-flight `RUN`, coordinating through a FIFO. Any one of them can stop while the others keep running, and the FIFO is where a stop becomes permanent: opening a FIFO for write blocks in the kernel until some process opens it for read. Nothing times that out.

Reproduced in `kali-linux` against the real writer loop. Killing the writer subshell leaves the dispatcher reading requests and acknowledging them exactly as before, while every reply vanishes. That is the observed symptom, produced in one second.

### What went silent, and what did not

The process was killed during diagnosis before its state was captured, so the trigger is unknown. Five mechanisms produce the identical outside view of process alive, stdin accepted, zero frames, forever:

1. **A RUN job eats the request pipe.** Ruled out. The job redirects stdout and stderr but not stdin (`wsl_helper.rs:194`), so fd 0 looked inherited from the dispatcher. POSIX assigns `/dev/null` to an asynchronous list's stdin before any explicit redirection when job control is off, which dash and busybox ash both implement. Verified in `kali-linux`: the dispatcher reported a pipe while a backgrounded nested `sh -c` reported `/dev/null`. Adding `< /dev/null` would fix nothing.
2. **The writer subshell dies.** It owns stdout (`wsl_helper.rs:167-176`). If it exits, a job's `>> "$t/done"` open blocks forever waiting for a FIFO reader, and no frame is emitted again while the dispatcher keeps accepting requests. No trigger identified.
3. **The temp dir goes away.** `$t` comes from `mktemp -d`. Verified: after `rm -rf "$t"` the append fails with ENOENT rather than recreating anything, and removing the FIFO alone leaves `>>` creating a regular file nobody reads. Either way the completions stop reaching the writer. `systemd-tmpfiles` is not the trigger on the affected distro, which runs no systemd.
4. **The helper dies without its trap and the `wsl.exe` relay outlives it.** Confirmed. `wsl.exe` is a Windows process forwarding over an hvsocket into the VM, not the helper itself. On the affected machine a helper relay created at 12:32:58 was still running on Windows hours later, its temp dir `/tmp/tmp.kys0kREGWn` was still present and dated 12:32, and the distro held no dispatcher for it: the only live one had started at 16:52. The EXIT trap does `rm -rf "$t"`, so an orphaned temp dir means the Linux side was killed without running it. The relay kept the pipe open, so the client never saw an EOF. Nothing done to the shell script reaches this case, which is why a Windows-side detector is required.
5. **The dispatcher blocks.** `RUN` is backgrounded, but `PROBE` runs inline in the dispatcher's own loop and walks `/proc/[0-9]*/stat` (`wsl_helper.rs:198-230`). A dispatcher stuck anywhere in that branch stops reading stdin while the writer stays perfectly healthy. No trigger identified.

Because 2 through 5 have no identified trigger, hardening the script alone cannot be the whole answer. Candidate 5 is also why the liveness signal has to travel *through* the dispatcher rather than around it, which section 1 turns on.

### Why a deadline cannot be the signal

Windows under load deschedules threads badly enough to blow a wall-clock deadline on work that took milliseconds. A test in this repo was flaky on CI for that reason: `a_home_session_starts_in_the_configured_working_directory` spent a 10 s budget waiting for a child-exit event whose delivery, not whose work, outlasted it. That event crosses a ConPTY process boundary, so it evidences slow delivery under load rather than a thread inside one process being descheduled for 10 s. The weaker reading still rules out a deadline as the signal, because delivery is exactly what a liveness check measures.

A false teardown is not free. It drops the distro into `RESPAWN_COOLDOWN` (30 s, `wsl_helper.rs:328`), during which every git call falls back to a one-shot `wsl.exe` spawn at roughly 400 ms. That is more expensive than the pipe, at precisely the moment the machine is already loaded.

The distinguishing fact: a descheduled but healthy helper produces bytes late, while a wedged one produces none at all, for 26 hours. The design below asks the second question instead of the first.

### Prior art

Two mechanisms are worth copying and one is worth naming as a trap.

VS Code's remote socket protocol (`src/vs/base/parts/ipc/common/ipc.net.ts`) declares a timeout only when an unacked message is at least 20 s old (`TimeoutTime`, :300) **and** `lastReadTime`, stamped on every chunk in `acceptChunk` (:365), is at least 20 s stale (:1146-1147). Both checks are skipped when `LoadEstimator.hasHighLoad()` says so (:1154): a 1 s timer records when it actually fired, and if half of the last ten ticks were late the process declares itself starved and refuses to judge anyone (:742-786). That is the objection to deadlines, implemented in shipped code. SWIM's Lifeguard extension formalises the same instinct as a Local Health Multiplier.

zed's remote client sends a ping every 5 s unconditionally and answers it with a 5 s timeout, tearing down after five consecutive misses, with any inbound activity resetting the count (`crates/remote/src/remote_client.rs:160-162, 779-830`). AMQP and MQTT count any inbound frame as liveness and emit a dedicated one only while idle.

TCP keepalive is the trap. A wedged process's kernel ACKs happily, so transport-level liveness proves nothing about the application. `wsl.exe` alive with an open pipe is that exact non-signal, which is why the ping below is answered by the machinery that can actually wedge rather than by anything underneath it.

The accrual and gossip detectors (phi-accrual in Cassandra and Akka, SWIM's indirect probes) do not earn their complexity here. There is no population to compare against, and the failure is infinite silence, which any finite threshold catches; the threshold only tunes how long the first caller waits.

## 1. A solicited ping, answered through the dispatcher

The helper gains one dispatcher branch beside `RUN` and `PROBE`:

```sh
  PING) printf '0 0\n' >> "$t/done" & ;;
```

Id `0` is reserved: `next_id` starts at 1 (`wsl_helper.rs:374`), so no request can ever carry it. The existing writer turns the completion into a `0\t0\t0\n` frame with no change to the writer at all, and `read_loop` drops a frame with no pending entry (`wsl_helper.rs:430`), so nothing new routes it.

**The trailing `&` is load-bearing.** A foreground append blocks the dispatcher inside `open()` once the writer is dead, which would convert a dead writer into a dead dispatcher and break the stdin-EOF cleanup that candidate 2 still relies on. Verified in `kali-linux`: backgrounded, the dispatcher kept reading and answering across a `kill -9` of the writer; foreground, it stopped at the first ping. One blocked pinger accumulates per unanswered ping, the same way a blocked `RUN` job does today, and the EXIT trap's `kill 0` (`wsl_helper.rs:166`) reaps them.

The ping is solicited rather than emitted on a timer because a timer inside the helper proves only the writer, the FIFO and the relay. It travels the same path a reply travels, so a wedged dispatcher stops answering it, where an unsolicited keepalive would sail past candidate 5 reporting health.

Verified end to end against the real writer loop. Pinging twice with a payload request interleaved, then killing the writer and pinging again, produced exactly `0\t0\t0\n`, `2\t0\t5\nhello`, `0\t0\t0\n` and then nothing, while the dispatcher went on reading every later request.

## 2. The reader stamps every read

`HelperClient` gains two fields:

```rust
/// Monotonic base for `last_bytes_at`, which is stored as elapsed
/// milliseconds so the read path stays lock-free.
started: Instant,
/// Milliseconds since `started` at the last successful read off the
/// helper's stdout.  Bytes, not frames: a partially delivered frame is
/// still proof the far end is producing output.
last_bytes_at: AtomicU64,
```

Stamped in `read_loop`'s `Ok(n)` arm (`wsl_helper.rs:427`) before `frames.push`, and read as a duration:

```rust
fn stamp_bytes(&self) {
    self.last_bytes_at.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
}

fn silent_for(&self) -> Duration {
    let now = self.started.elapsed().as_millis() as u64;
    Duration::from_millis(now.saturating_sub(self.last_bytes_at.load(Ordering::Relaxed)))
}
```

`Relaxed` is correct and `Release`/`Acquire` would be cargo cult: no other memory is published under the stamp, and a reader that observes a stale value only delays a judgment by one slice. The `as_millis() as u64` cast overflows after roughly 5.8e8 years.

`last_bytes_at` begins at 0, so a client that has never read a byte reports its whole lifetime as silence, which is what section 3 wants.

## 3. The wait is sliced, and a starved slice does not judge

`request` keeps its signature and its `timeout` parameter. The single `recv_timeout(timeout)` becomes a loop over slices with the same total deadline.

The two periods live in a struct rather than in constants, so a test can drive the loop in milliseconds and exercise the teardown branch in CI:

```rust
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
```

The decision is a pure function, kept out of the wait so it can be tested without a process:

```rust
/// Whether an expired slice is evidence the transport is dead.
///
/// `slept` past twice `asked` means the waiter's own sleep overran, so the
/// host was starved and nothing measured under it is evidence of anything.
/// `silence` is how long the caller has *observed* no bytes, which is not
/// how old the last byte is: after a resume the last byte is legitimately
/// hours old with nobody watching.
fn wedged(timing: &Timing, asked: Duration, slept: Duration, silence: Duration) -> bool {
    slept <= asked * 2 && silence > timing.silence_limit
}
```

**Observed silence, not stamp age.** An earlier draft passed `since_sent` as a third condition to keep a helper that was quiet before the request from being condemned on arrival. That guard is per-request, and the silence it guards is cumulative, so it fails the case it matters for: under bursty load several slices overrun and decline, the machine frees up, the next slice is punctual, and it condemns on silence that accumulated while the WSL VM was starved alongside the waiter. The caller instead keeps a window that restarts whenever a slice overruns, and passes `min(silent_for(), window.elapsed())`. Silence then only counts while somebody was awake to see it, and the `since_sent` condition becomes redundant: 30 seconds of observed silence implies a request older than two slices.

The loop body:

```rust
let sent_at = Instant::now();
let deadline = sent_at + timeout;
let mut watching_since = sent_at;
loop {
    let slice_start = Instant::now();
    let remaining = deadline.saturating_duration_since(slice_start);
    if remaining.is_zero() {
        lock(&self.pending).remove(&id);
        return Err(TransportError::NoReply(format!("no reply from the {} helper", self.distro)));
    }
    let asked = remaining.min(self.timing.slice);
    match rx.recv_timeout(asked) {
        Ok(frame) => return Ok(frame),
        // The sender is dropped by `mark_down`'s `pending.clear()`, so a
        // disconnect means someone else already tore the transport down.
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(TransportError::NoReply(format!(
                "the {} helper went down while waiting",
                self.distro
            )));
        },
        Err(mpsc::RecvTimeoutError::Timeout) => {},
    }
    let slept = slice_start.elapsed();
    // Silence only counts while somebody was awake to observe it.  A slice
    // that overran means the host was starved, so the window restarts
    // rather than carrying that quiet stretch forward as evidence.
    let silence = self.silent_for().min(watching_since.elapsed());
    if slept > asked * 2 {
        watching_since = Instant::now();
    }
    if wedged(&self.timing, asked, slept, silence) {
        // The count is taken into a local first: passing `lock(...).len()`
        // as an argument keeps the guard alive across the call, and
        // `mark_down` takes the same mutex.
        let outstanding = {
            let mut pending = lock(&self.pending);
            pending.remove(&id);
            pending.len()
        };
        self.mark_down(&format!(
            "silent for {:.0}s with {outstanding} outstanding",
            silence.as_secs_f64()
        ));
        return Err(TransportError::NoReply(format!("no reply from the {} helper", self.distro)));
    }
    self.ping();
}
```

The ping goes after the judgment, so the next slice reads the answer to this slice's question rather than to one sent moments ago.

```rust
/// Ask the dispatcher to prove it is still reading.  The reply is a frame
/// nothing routes; its only effect is refreshing `last_bytes_at`.
fn ping(&self) {
    // A held stdin lock is a reason to skip, never to wait: blocking here
    // would park the waiter inside the failure it came to detect, and the
    // thread holding the lock is itself proof of an in-flight write.
    let Ok(mut guard) = self.stdin.try_lock() else { return };
    if let Some(stdin) = guard.as_mut() {
        let _ = stdin.write_all(b"0\tPING\n").and_then(|()| stdin.flush());
    }
}
```

A failed ping write is deliberately ignored. The existing write path already calls `mark_down` on a write error (`wsl_helper.rs:481-486`), and a ping that cannot be sent is one more slice of silence, which the same loop already handles.

## 4. Teardown can end the process, not just stop talking to it

`mark_down` today drops `ChildStdin` and clears `pending` (`wsl_helper.rs:441-450`). The `Child` is moved into the reader thread's closure (`wsl_helper.rs:394-403`), so nothing holds a killable handle. Under candidate 4 that makes teardown an abandonment: stdin EOF never reaches the dispatcher, stdout never EOFs, and the reader thread sits in `read` forever holding the child. One wedged `wsl.exe` and one leaked thread per teardown.

It also hangs the thread doing the teardown. `request` holds `lock(&self.stdin)` across `write_all` and `flush` (`wsl_helper.rs:470-479`). If the far end stops draining stdin, that write blocks holding the mutex, and `mark_down`'s `*lock(&self.stdin) = None` blocks behind it forever.

Both are fixed by keeping the child and killing it first. `HelperClient` gains:

```rust
/// Kept so a teardown can end a `wsl.exe` that stopped draining its pipes.
/// Dropping stdin only reaches a helper still listening for the EOF.
child: Mutex<Option<std::process::Child>>,
```

`spawn` stores the child here instead of moving it into the reader thread, and the thread reaps after `read_loop` returns:

```rust
reader.read_loop(stdout);
// Reap so a dead helper never lingers as a zombie in the process table.
let finished = lock(&reader.child).take();
if let Some(mut child) = finished {
    let _ = child.wait();
}
```

The take goes into a local first. Written as `if let Some(mut child) = lock(&reader.child).take()`, the guard temporary lives to the end of the then-block, so the child mutex stays held for the whole of `wait()` and a concurrent `mark_down` blocks on it until `wsl.exe` exits. That is the wait this section exists to remove.

`mark_down` kills before touching the stdin lock:

```rust
if let Some(child) = lock(&self.child).as_mut() {
    let _ = child.kill();
}
*lock(&self.stdin) = None;
```

Killing first is what unblocks a stuck `write_all`: the write fails, its holder releases the stdin mutex, and `mark_down` proceeds. The ordering is the fix, not an optimisation.

**No ordering here buys a graceful shutdown, and it is worth writing down why**, because the shape of the code invites the attempt. Closing `ChildStdin` first completes `wsl.exe`'s pending `ReadFile` with `STATUS_PIPE_BROKEN` within a few microseconds, so it looks like the dispatcher gets its EOF. It does not. The relay thread has to be scheduled back to user mode and issue `shutdown()` on the hvsocket before `TerminateProcess` lands tens of microseconds later, and a terminate APC is delivered on exactly that kernel-to-user transition. Even a thread that wins that race only buys the VM a millisecond-scale sequence: hvsocket delivery, `init` scheduled, `sh` woken from `read`, then fork+exec `rm` and `kill 0`. Two orders of magnitude separate the sides, and load only widens the gap. The relay's socket closes from process teardown microseconds later regardless, and the VM cannot tell the two apart at that spacing.

So a teardown leaves `$t` and its FIFO behind whenever the helper was still healthy, and section 7 reclaims them at the next helper's startup instead. That sweep is also the only thing that reaches candidate 4's orphan, which no teardown ordering can touch because the client never runs one.

`read_loop` reaches `mark_down` on its own thread and then takes the child, so the two accesses are sequential rather than racing. A teardown that finds `None` has nothing to kill because the process already exited.

**Load-bearing, and now verified:** whether terminating `wsl.exe` promptly closes the pipe handles rather than leaving a relay child holding duplicates. If a duplicate survived, the pipe would stay open, the blocked `write_all` would never fail, and `mark_down` would park on `lock(&self.stdin)` behind it, which is worse than the bug being fixed. The orphaned relay found on the affected machine is exactly the shape that would do it, so this was settled before anything was built on it: freeze the dispatcher with `kill -STOP`, fill the stdin pipe from one thread, call `mark_down` from another, and assert on the error the writer receives.

**Asserting on that error is the whole experiment, and the payload has to be large enough to reach it.** A test that discards the writer's `Result` proves only that teardown does not hang, which is also what happens when the write never blocked at all: the waiter parks in `recv_timeout` instead, and `mark_down`'s closing `pending.clear()` drops the sender and frees it in microseconds. Both paths return in the same fraction of a second. Measured against `kali-linux`, a few hundred KiB is the second path: 200 KiB of argument never parked the writer, because the Windows pipe buffer, the relay, the hvsocket and the Linux pipe absorb it between them. One MiB parks it. At that size the writer gets `NotWritten` with `os error 109` about one and a half seconds in, and removing the `kill()` hangs the test to its deadline instead. That pair is the evidence; either half alone is not.

Had the writer stayed parked with the `kill()` in place, the design would have changed to a dedicated writer thread fed by a channel, so `request` hands off a line and never holds a lock across a pipe write. That removes the hazard instead of relying on `kill` to clear it. It is written down here because it remains the fallback if the pipe behaviour ever differs on another Windows or WSL build.

## 5. `RUN_TIMEOUT` becomes a job budget only

It stays at 60 s and keeps returning `NoReply` without teardown, exactly as on `master`. Liveness and job duration become separate questions, which is the structural gain: a 90 s `worktree add` on a cold cache keeps the helper up because pings keep being answered underneath it, and a wedge is caught in 30 s even though the job budget is 60.

**This replaces the uncommitted change on `master`** (`wsl_helper.rs:498-509`), which makes `run` call `mark_down` on any `NoReply`. That change tears the transport down over a legitimately slow job and kills that job with it. Drop the hunk; do not carry both.

**Probes never tear the transport down.** `probe` passes `PROBE_TIMEOUT` (2 s), so the whole wait is one 2 s slice and the observation window never reaches the 30 s limit before `remaining` hits zero. Only `RUN` callers detect. With a shimmed session open and no git activity at all, the 1 Hz poller therefore probes forever without noticing a wedge. In practice `git_status`'s 1.5 s refresh puts a `RUN` on the pipe whenever a WSL worktree is on screen, so the gap is narrow; it is stated here because an earlier draft of this design claimed the opposite.

## 6. The teardown says enough to attribute the next one

`mark_down`'s existing `log::warn!` carries the reason string. The message built in section 3 carries the silence age and the outstanding count, so the next wedge leaves a record naming what the transport looked like when it was cut. The reason the 26-hour wedge is unattributed is that nothing recorded anything.

## 7. A helper start sweeps what a teardown could not

Section 4 leaves `$t` and its FIFO behind on every teardown that ends the relay, and candidate 4 leaves them behind with no client involved at all: the orphan found on the affected machine had a live relay, a temp dir dated hours earlier, and no dispatcher. Nothing on the Windows side can reclaim either without paying a `wsl.exe` spawn against a VM that may be the thing that wedged, which is the unbounded wait this design exists to remove. The distro can reclaim both for free at the only moment it is already running: the next helper's startup.

The temp dir is named by dispatcher pid rather than by `mktemp`, so liveness is readable from the name. That removes the pid-file write and, with it, the window in which a second alacritree instance on the same distro could sweep a newborn helper. A start then walks `$rt/helper-*`, skips any whose pid still has a `/proc` entry, and removes the rest, extending the pidfile GC that already runs there. `rm -rf "$t"` before `mkdir` covers pid reuse, since a directory carrying our own pid is stale by definition.

Removing the directory is not enough on its own. Unlinking a FIFO does not wake a process blocked in `open(O_WRONLY)` on it, so an orphaned job subshell would survive its own temp dir. Opening each stale FIFO O_RDWR and closing it immediately bumps the reader count and releases that writer, whose write then fails with `EPIPE`; the O_RDWR open never blocks on Linux, which is the same property the writer subshell already relies on. That is what turns the sweep from removing a directory into reaping a process.

One more line makes the whole thing moot at teardown time, and it is measured. Every `--exec` session gets a controlling pty the shell owns, so `init` closing the pty master on relay death SIGHUPs the foreground process group, and untrapped SIGHUP under dash terminates without running the EXIT trap. `trap 'exit' HUP` converts that into a normal exit, which does run it. Subshells reset non-ignored traps, so jobs and the writer still die by default action.

**The measurement, and the trap it nearly hid.** Thirty relay kills per configuration against `kali-linux`, counting surviving temp dirs. At idle, every configuration leaked nothing: closing stdin 0/30, killing 0/30, closing then killing 0/30. On that evidence the leak does not exist and none of this is needed. Under load the picture inverts: killing leaks 19/30, and closing stdin first before killing leaks 16/30, which is the ordering question settled empirically as well as by the timing argument. Adding `trap 'exit' HUP` takes it back to 0/30. So SIGHUP is what arrives, the trap closes the teardown case outright, and an idle-only experiment would have concluded the opposite of the truth.

That leaves the sweep covering what the trap cannot reach: candidate 4, where the Linux side dies under a still-live relay and no signal is delivered to a process that is already gone; a dispatcher killed outright, by the OOM killer or otherwise; and any launch path that does not give the shell a controlling pty, which would drop the trap silently. 0/30 measures one WSL build's pty teardown, not a guarantee.

**The sweep does not reclaim the orphan that motivated it.** It globs `$rt/helper-*`, and every directory the `mktemp -d` era leaked sits under `/tmp` behind a `tmp.` prefix, including the one found on the affected machine. The class is covered from this version forward; the existing instances are not. Both locations are tmpfs, so a VM restart clears them, which is why this is recorded rather than fixed.

Reclaim is bounded by "the next helper start in that distro", not by elapsed time. A machine that never starts another helper there keeps the directory until one launches that does.

## What this deliberately does not do

**No probe-miss counter.** A 2 s deadline is exactly the one load blows, and the poller only runs while a shimmed session is registered.

**No `< /dev/null` on the RUN job.** Candidate 1 is ruled out; adding the redirection would imply a fix that is not one.

**No unsolicited keepalive from the helper.** It cannot see candidate 5, and it costs a `sleep` loop and a reserved output file this design does not need.

**No phi-accrual or adaptive threshold.** One peer, one pipe, and a failure that is infinite silence. A fixed six-slice limit with a starvation guard covers it.

**No crash tracker.** VS Code stops auto-restarting after three crashes in five minutes. The loop that would guard already has gain below one: during cooldown no helper exists, so no further teardown is possible, and the registry admits one respawn per 30 s per distro whatever the false-positive rate. The worst steady state under permanent starvation is the pre-helper design. Write it if logs show repeated teardowns, not before.

**Nothing for a helper whose hello never arrives.** A login shell that hangs inside `sh -lc` leaves the client `Live` and never ready, with no cooldown, so every call falls back to one-shots silently and forever. Same family, different entry point, out of scope here.

**One unbounded wait survives.** A `request` blocked inside `write_all` under the stdin lock, with no other waiter alive, is judged by nobody: the slice loop is downstream of the write, and `ping` declines to block on the same lock by design. Reaching it takes roughly 64 KiB of unanswered requests filling the pipe, queued by callers who all returned without judging. It is narrow enough to leave, and written down here so the next person meets it as a known residual rather than a surprise.

**Nothing for the other unbounded `wsl.exe` waits.** `run_batch`'s one-shot fallback and `worktree.rs:157` both call `.output()` with no deadline, the same shape as the wait this design removes. The first is bounded as part of this work because it sits on the thread that gates the git panel. The second is left alone: killing a `worktree add` halfway leaves a partly created worktree, which needs its own decision about cleanup.

## Testing

`wedged` is pure, so the rules it encodes are testable without a process. A table covers the ordering that matters: a slice that overran twice its length returns `false` even when silence is past the limit; silence past the limit inside a punctual slice returns `true`; silence inside the limit returns `false`; and a scaled-down `Timing` moves all three, which is what lets the loop be tested in milliseconds.

`silent_for` gets a case asserting a freshly built client reports its whole lifetime as silence rather than zero.

`FrameReader` needs nothing new. `reassembles_frames_across_split_reads` (`wsl_helper.rs:722-739`) already delivers `9\t1\t0\n` byte-at-a-time and asserts an empty payload, which is the ping reply's shape on the wire.

**The wiring is tested in-process, not left to WSL.** The 26-hour wedge was a client-side control-flow defect: `request` returned `NoReply` and never called `mark_down`. Nothing in `request`, `read_loop`, `mark_down` or `ping` needs the far end to be a `wsl.exe`, so `stdin` is typed as `Mutex<Option<Box<dyn Write + Send>>>`, `read_loop` takes a `Box<dyn Read + Send>`, and a test-only `over(reader, writer, timing)` constructor starts the reader thread the way `spawn` does. A channel-backed pipe pair is about thirty lines, and two tests then cover the wiring on any platform:

- A helper that sends a valid hello and then answers nothing. With a 50 ms slice and a 300 ms limit against the real 60 s run budget, `run` must return `NoReply`, `is_down()` must hold, `pending` must be empty, and pings must have reached the fake. It finishes in about 350 ms; the only non-teardown exit is the 60 s deadline, so it fails only if every slice overruns for a full minute.
- A helper that answers every ping and delivers the job after four slices, with the limit set to 5 s while the test runs for a fraction of one. A false teardown here needs the reader thread starved for five seconds inside a two-hundred-millisecond test.

An earlier draft of this spec accepted that the unit tests would not have caught the original bug, and left the wiring entirely to an `#[ignore]` test. That was wrong: CI has no WSL, so a green run would have proved nothing about the fix. The live tests below stay as end-to-end proof, but they are not the guard.

The end-to-end test spawns a helper, wedges it, and asserts `is_down()` inside the limit. It needs a handle on machinery it did not create, which a `RUN` job can supply: the job learns `$t` from `readlink /proc/self/fd/1`, since its stdout is `$t/<id>.out`. From there, `rm "$t/done"` reproduces candidate 3, and scanning `/proc/*/fd` for the FIFO finds the writer to kill for candidate 2. Candidate 5 needs no such handle: a `PROBE` against a pidfile naming a process that never exits wedges the dispatcher directly.

## Open questions

1. What kills the helper's Linux side without letting its EXIT trap run? Candidate 4 is confirmed as far as the orphaned relay goes, but not as to what killed the dispatcher underneath it. Nothing in the design depends on the answer; the detector treats every candidate the same.
2. Does Rust's `Instant` on Windows include time spent suspended? It only affects wording. `Instant` is monotonic and `elapsed` saturates, so nothing inverts across a resume either way, and the observation window covers the case regardless: a slice spanning a suspend either overruns, and declines, or measures normally. Do not let a comment assert that the stamp reads hours-stale after a resume unless someone checks.
3. Can the reader thread stay starved for six consecutive slices while the waiter wakes punctually? That is the residual false positive the starvation guard does not cover, since the guard observes only the waiter. Both threads are in one process and Windows boosts a thread that has been ready and waiting for around four seconds, so the window is believed to be bounded well under 30 s. That is documented scheduler behaviour, not a measurement.
