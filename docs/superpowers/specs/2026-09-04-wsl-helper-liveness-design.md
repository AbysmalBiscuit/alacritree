# WSL helper liveness design

**Goal:** a WSL helper that stops answering is detected in about 30 seconds and torn down, on a machine loaded badly enough that a fixed wall-clock deadline would fire on a healthy one.

**Issue:** [#56](https://github.com/AbysmalBiscuit/alacritree/issues/56).

**Branch:** `fix/wsl-helper-liveness`. Cut from the open PR carrying the highest `[n]` marker, which was PR 206 (`fix-selecting-text-near-the-left-side-of-the`, marker `[4]`) when this was written. Several unimplemented specs also claim `[5]`; read the tip fresh at setup time rather than trusting that number.

**Platform:** Windows only. `wsl_helper::client` returns `None` on any other target (`wsl_helper.rs:537`), so the whole transport is inert elsewhere.

**Config:** no new key. `[wsl] resident_helper` (`config.rs:1693-1697`, default `true`) already turns the transport off wholesale, and nothing here changes behaviour while the helper is answering.

## Context

alacritree keeps one long-lived `sh` per WSL distro and pipes every git request through its stdio, rather than paying a `wsl.exe` spawn per call. The protocol is one tab-separated request line in, one `<id>\t<exit>\t<len>\n` frame plus `len` payload bytes out. Requests come in two kinds, `RUN` and `PROBE`.

On 2026-09-04 a helper went silent for roughly 26 hours. Its process was alive, its pipe was open, and it emitted no frames the whole time.

`mark_down` (`wsl_helper.rs:441`) fires only on a write error or a reader-thread EOF, and neither happened. `request` (`wsl_helper.rs:464`) treats a `recv_timeout` expiry as a one-off: it removes the pending entry, returns `TransportError::NoReply`, and leaves the client `is_ready()`. Every later call rewrote to the same dead pipe and burned the full 60 s `RUN_TIMEOUT`. Only restarting the app recovered it.

### What it cost

Measured on the affected machine while the helper was wedged.

| Command | Path | Measured |
| --- | --- | --- |
| `project list`, `project rename` | in-memory app state | 0.13–0.24 s |
| `project refresh <root>` | worker thread to the helper | 10 s, then the IPC cap's "app busy or closed" |
| `git-status <wsl path>` | connection thread, no IPC cap in front | 62 s |
| the same git script, run directly inside the distro | git only | 40 ms |

The git work is not slow. The 62 s is `RUN_TIMEOUT` plus overhead, and `git-status` is the one command with no 10 s IPC deadline hiding it.

### What went silent, and what did not

The process was killed during diagnosis before its state was captured, so the trigger is unknown. Four mechanisms produce the identical outside view of process alive, stdin accepted, zero frames, forever:

1. **A RUN job eats the request pipe.** Ruled out. The job redirects stdout and stderr but not stdin (`wsl_helper.rs:194`), so fd 0 looked inherited from the dispatcher. POSIX assigns `/dev/null` to an asynchronous list's stdin before any explicit redirection when job control is off, which dash and busybox ash both implement. Verified in `kali-linux`: the dispatcher reported `pipe:[…]` while a backgrounded nested `sh -c` reported `/dev/null`. Adding `< /dev/null` would fix nothing.
2. **The writer subshell dies.** It owns stdout (`wsl_helper.rs:167-176`). If it exits, a job's `>> "$t/done"` open blocks forever waiting for a FIFO reader, and no frame is emitted again while the dispatcher keeps accepting requests. No trigger identified.
3. **The temp dir is swept.** `>> "$t/done"` then recreates the FIFO as a regular file nobody reads, while the writer stays blocked on the deleted inode. `systemd-tmpfiles` is not the trigger on the affected distro, which runs no systemd.
4. **The `wsl.exe` relay goes quiet.** `wsl.exe` is a Windows process forwarding over an hvsocket into the VM, not the helper itself. A 26-hour wedge spans a night, and sleep/resume is the obvious suspect. Unverified, and it is the reason a Windows-side detector is required: nothing done to the shell script reaches this case.

Because 2, 3 and 4 have no identified trigger, hardening the script alone cannot be the whole answer.

### Why a deadline cannot be the signal

Windows under load deschedules threads badly enough to blow a wall-clock deadline on work that took milliseconds. A test in this repo was flaky on CI for exactly that: `a_home_session_starts_in_the_configured_working_directory` spent a 10 s budget waiting for a child-exit event whose delivery, not whose work, outlasted it. Any liveness rule of the form "did the reply arrive within T" inherits that false-positive rate.

A false teardown is not free. It drops the distro into `RESPAWN_COOLDOWN` (30 s, `wsl_helper.rs:328`), during which every git call falls back to a one-shot `wsl.exe` spawn at roughly 400 ms. That is more expensive than the pipe, at precisely the moment the machine is already loaded.

The distinguishing fact: a descheduled but healthy helper produces bytes late, while a wedged one produces none at all, for 26 hours. The design below asks the second question instead of the first.

### Prior art

Two mechanisms are worth copying and one is worth naming as a trap.

VS Code's remote socket protocol (`src/vs/base/parts/ipc/common/ipc.net.ts`) declares a timeout only when an unacked message is at least 20 s old **and** `lastReadTime`, stamped on every chunk in `acceptChunk`, is at least 20 s stale. Both checks are skipped when `LoadEstimator.hasHighLoad()` says so: a 1 s timer records when it actually fired, and if half of the last ten ticks were late the process declares itself starved and refuses to judge anyone. That is the objection to deadlines, implemented in shipped code. SWIM's Lifeguard extension formalises the same instinct as a Local Health Multiplier.

AMQP, MQTT, zed's remote client and VS Code all converge on counting any inbound traffic as liveness, so a dedicated frame is sent only while idle.

TCP keepalive is the trap. A wedged process's kernel ACKs happily, so transport-level liveness proves nothing about the application. `wsl.exe` alive with an open pipe is that exact non-signal, which is why the keepalive below is emitted by the machinery that can actually wedge rather than by anything underneath it.

The accrual and gossip detectors (φ-accrual in Cassandra and Akka, SWIM's indirect probes) do not earn their complexity here. There is no population to compare against, and the failure is infinite silence, which any finite threshold catches; the threshold only tunes how long the first caller waits.

## 1. The helper emits an unsolicited keepalive

A ticker beside the writer subshell, inserted after the writer subshell at `wsl_helper.rs:167-176`:

```sh
( while sleep 5; do printf '0 0\n' >> "$t/done"; done ) &
```

Id `0` is reserved: `next_id` starts at 1 (`wsl_helper.rs:374`), so no request can ever carry it.

The existing writer turns that into a `0\t0\t0\n` frame with no change to the writer at all. It has no `$t/0.out`, so `wc -c` fails and `n` falls back to 0, and `cat` and `rm -f` are already error-suppressed. The shell prints a diagnostic about the failed `<` redirection, which goes to stderr; the helper is spawned with `stderr(Stdio::null())` (`wsl_helper.rs:385`), so it is discarded and never reaches the frame stream.

Verified against the real writer loop in `kali-linux`. Feeding it `7 0` with a payload, two keepalives, then `8 0` with a payload produced exactly:

```
7\t0\t12\nreal payload0\t0\t0\n0\t0\t0\n8\t0\t6\nsecond
```

**The ticker must not create `$t/0.out`.** Creating it first looks tidier and introduces a race: the writer's `rm -f` for one keepalive can land after the ticker's create for the next, and the diagnostic reappears anyway. Reproduced in the same harness. Owning no file is what makes the reserved id safe to reuse forever.

**On the Rust side nothing routes it.** `read_loop` drops a frame with no pending entry (`wsl_helper.rs:430`), and `FrameReader::push` already handles `len` 0 by taking an empty payload slice. Both paths are unchanged.

The keepalive travels the exact path a reply travels, which is what makes it prove the writer, the FIFO and the relay together. A dead writer leaves the ticker's `>>` blocking on a FIFO with no reader; a swept `$t` turns it into a write to a regular file nobody reads; a dead relay delivers nothing. Every candidate mechanism converges on silence.

The EXIT trap's `kill 0` (`wsl_helper.rs:166`) already takes the ticker down with the rest of the process group.

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
    self.last_bytes_at.store(self.started.elapsed().as_millis() as u64, Ordering::Release);
}

fn silent_for(&self) -> Duration {
    let now = self.started.elapsed().as_millis() as u64;
    Duration::from_millis(now.saturating_sub(self.last_bytes_at.load(Ordering::Acquire)))
}
```

`started` is also the initial value's meaning: `last_bytes_at` begins at 0, so a client that never read a byte reports its whole lifetime as silence, which is correct.

## 3. The wait is sliced, and a starved slice does not judge

`request` keeps its signature and its `timeout` parameter. The single `recv_timeout(timeout)` becomes a loop over slices, with the same total deadline.

```rust
/// How often a waiter re-examines the transport.  Shorter than the
/// keepalive period, so a healthy helper refreshes the stamp between two
/// consecutive examinations.
const WAIT_SLICE: Duration = Duration::from_secs(5);
/// Six keepalive periods.  VS Code's equivalent tolerates four and AMQP
/// two, both against peers that are not sharing vCPUs with the judge.
const SILENCE_LIMIT: Duration = Duration::from_secs(30);
```

The decision at each slice expiry is a pure function, kept separate from the wait so it can be tested without a process:

```rust
/// What a slice expiry means for the transport.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// The waiter's own sleep overran, so the host was starved and the
    /// silence measurement is not evidence of anything.
    Starved,
    /// No bytes at all for long enough that no healthy helper explains it.
    Silent,
    /// Nothing decided; keep waiting out the caller's budget.
    Waiting,
}

fn verdict(asked: Duration, slept: Duration, silence: Duration) -> Verdict {
    if slept > asked * 2 {
        Verdict::Starved
    } else if silence > SILENCE_LIMIT {
        Verdict::Silent
    } else {
        Verdict::Waiting
    }
}
```

The loop body:

```rust
let sent_at = Instant::now();
let deadline = sent_at + timeout;
loop {
    let slice_start = Instant::now();
    let remaining = deadline.saturating_duration_since(slice_start);
    if remaining.is_zero() {
        lock(&self.pending).remove(&id);
        return Err(TransportError::NoReply(format!("no reply from the {} helper", self.distro)));
    }
    let asked = remaining.min(WAIT_SLICE);
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
    // Silence is measured from the later of the last byte and the moment
    // this request was sent.  After a laptop resume the last keepalive is
    // hours old and means "unknown", not "dead"; a healthy helper answers
    // that with a byte inside one keepalive period.
    let silence = self.silent_for().min(sent_at.elapsed());
    if verdict(asked, slice_start.elapsed(), silence) == Verdict::Silent {
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
}
```

`Verdict::Starved` and `Verdict::Waiting` both fall through to the next slice, and they differ only in why. Keeping them distinct is what makes the unit tests able to assert that a starved slice declined to judge rather than merely failing to trip the limit.

The pending entry is removed before `mark_down` so the count in the message reflects the other waiters, not this one.

## 4. `RUN_TIMEOUT` becomes a job budget only

It stays at 60 s and keeps returning `NoReply` without teardown, exactly as on `master`. Liveness and job duration become separate questions, which is the structural gain: a 90 s `worktree add` on a cold cache keeps the helper up because keepalives keep flowing underneath it, and a wedge is caught in 30 s even though the job budget is 60.

**This replaces the uncommitted change on `master`** (`wsl_helper.rs:498-509`), which makes `run` call `mark_down` on any `NoReply`. That change tears the transport down over a legitimately slow job and kills that job with it. Drop the hunk; do not carry both.

`probe`'s 2 s `PROBE_TIMEOUT` is likewise unchanged, and the poller keeps discarding probe errors (`wsl_helper.rs:659`). The probe still contributes to liveness, because it reaches the silence rule through `request` like any other caller. What it must not do is count its own missed 2 s deadlines: that deadline is exactly the one load blows, and the poller only runs while a shimmed session is registered.

## 5. The teardown says enough to attribute the next one

`mark_down`'s existing `log::warn!` carries the reason string. The message built in section 3 carries the silence age and the outstanding count, so the next wedge leaves a record naming what the transport looked like when it was cut. The reason the 26-hour wedge is unattributed is that nothing recorded anything.

## What this deliberately does not do

**No probe-miss counter.** Rejected in section 4.

**No `< /dev/null` on the RUN job.** Candidate 1 is ruled out; adding the redirection would imply a fix that is not one.

**No φ-accrual or adaptive threshold.** One peer, one pipe, and a failure that is infinite silence. A fixed six-period limit with a starvation guard covers it.

**No crash tracker.** VS Code stops auto-restarting after three crashes in five minutes. The equivalent here is about ten lines, and the loop it guards already has gain below one: during cooldown no helper exists, so no further teardown is possible, and the registry admits one respawn per 30 s per distro whatever the false-positive rate. The worst steady state under permanent starvation is the pre-helper design. Write it if logs show repeated teardowns, not before.

## Testing

The decision logic is pure, so the parts worth testing need no child process.

`verdict` gets a table: a slice that overran its own length twice returns `Starved` even when silence exceeds the limit, which is the ordering that matters; silence past the limit inside a punctual slice returns `Silent`; silence under the limit returns `Waiting`.

`FrameReader::push` gets a case for a zero-length frame arriving between two payload-carrying frames, asserting all three come out and the buffer is drained. That is the keepalive's shape on the wire, and the existing tests never exercise `len` 0.

`silent_for` gets a case asserting a freshly built client reports its whole lifetime as silence rather than zero.

An end-to-end test that spawns a real helper, kills its writer subshell, and asserts `is_down()` within the limit is worth having and cannot run in CI, which has no WSL. Mark it `#[ignore]` and name the distro requirement in a doc comment.

`ChildStdin` has no public constructor, which is why the earlier plan to test this needed a teardown-policy parameter. Splitting `verdict` out removes that need: the branch that used to be reachable only through a real child is now a function call.

## Open questions

1. Is the relay actually the wedge? Section "What went silent" leans on it to argue that script-only fixes are insufficient, and it is unverified. Capturing a wedged helper's state before killing it would settle it. Nothing in the design changes if it turns out to be candidate 2 instead.
2. Does 5 s of keepalive hold the WSL VM awake in a way the resident helper does not already? The helper is a live process either way, so the VM is pinned regardless, but a measurement of idle power with and without the ticker would retire the question rather than assume it.
