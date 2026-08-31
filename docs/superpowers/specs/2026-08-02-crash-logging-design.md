# Crash logging

Date: 2026-08-02
Revised: 2026-08-02, three codex reviews against the code.

Record why alacritree died. Today it dies silently: `env_logger` writes to
stderr, and a release build carries `windows_subsystem = "windows"`, so a run
launched from a shortcut throws every log line away. There is no panic hook, no
log file, and no crash dump.

## Motivating evidence

Two distinct failure modes, both currently invisible.

**Silent exits, 2026-08-01 and overnight into 2026-08-02.** No Windows Error
Reporting event, no dump, while WER was demonstrably working — it wrote Godot
dumps at 07:41 and 07:42 on 2026-08-02. So these exits raised no unhandled
exception.

A plain Rust panic explains this exactly; see "How a panic actually exits". A
panic hook captures this case completely.

The other candidate is `main` returning `Err`: `main.rs:86` returns
`eframe::Result<()>`, so an eframe failure prints `Error: …` to the same
discarded stderr. `record_exit` captures that one.

**Aborts, 2026-07-25.** Thirteen WER events in one fault bucket: exception
`0xC0000409` (`STATUS_STACK_BUFFER_OVERRUN`, event name `BEX64`), faulting
module `alacritree.exe`, fault offset `0x6d6e11`. That code is how `__fastfail`
surfaces, which is Rust's `abort`. **The cause is unverified.** A panic crossing
the window procedure does not abort, so the obvious explanation is ruled out.
Remaining candidates: a panic while already unwinding (a panic in a `Drop`), a
double panic, or a genuine hard fault in foreign code. The offset can no longer
be symbolized; the crashing build is stamped 2026-07-25 21:13 and its PDB was
overwritten by the 2026-08-01 rebuild.

## Branching

**Branch off the newest open PR in the stack, never off `master`.** The base is
resolved at branch time, not read from this document — the stack grows while
specs sit unimplemented, and a stale base silently forks the chain.

```sh
gh pr list --repo mathix420/alacritree --state open \
  --json number,title,headRefName
```

Take the entry whose title carries the highest `[n]` marker; its `headRefName`
is the base and `n + 1` is this PR's marker.

- Base as of 2026-08-02: `origin/feat/sidebar-upstream-status` (PR #166, `[7]`).
  Recorded only so a drifting stack is visible — re-run the command and use what
  it returns.
- Branch name: `feat/crash-logging`, in a worktree under
  `../alacritree-worktrees/feat/crash-logging`, created with
  `git worktree add ../alacritree-worktrees/feat/crash-logging -b feat/crash-logging <base>`.
- PR title carries the next marker: `feat(logging): record why alacritree died
  [n+1]`.
- The PR opens against `mathix420/alacritree` `master`, matching every other PR
  in the stack, even though the branch descends from the previous PR rather than
  from `master`. It merges into `all-features` afterwards.

Note: the 2026-08-01 session-snapshot spec also claims `[7]`, off PR #165. Two
parallel sessions picked the same marker, so confirm the live numbering rather
than trusting either document.

## Scope

In:

- A per-process crash artifact — panics, the exit reason, and a session header,
  for the **GUI process only**. On by default, disableable.
- `alacritree crashes`, a read-only subcommand that concatenates the artifacts
  into the single view to read or hand over.
- `alacritree-<pid>.log` — the continuous log stream, teed from stderr. Off by
  default.
- One config key mirroring alacritty's `[debug]` section, plus one
  alacritree-only key.
- Copying `alacritree.pdb` in `install.local.ps1` so backtraces symbolize on the
  maintainer's own machine.

Out:

- **Crash capture for the CLI and MCP processes.** They exit or loop before
  config is ever loaded, so no gate can govern them; their stderr is attached to
  a real console or to the MCP client, which already sees a panic. See
  "Initialization order".
- **`debug.log_level`.** See "Why log_level is out of scope".
- In-process minidumps. Deferred; see "What this does not catch".
- WER `LocalDumps` registry configuration. Machine-local, needs administrator
  rights, and not something alacritree can ship — though it remains the right
  manual step for an abort, see "What this does not catch".
- Shipping the PDB anywhere but `install.local.ps1`. Not in release archives:
  `dist-workspace.toml`'s `include` list has no per-target scoping (see its own
  comment at line 31), so a 47 MB PDB would land in every Unix tarball as dead
  weight. Not in `alacritree install` (`cli/install.rs`) either — that is a
  shipped subcommand any user runs, and symbol delivery is a maintainer's
  debugging convenience, not a product feature. Keeping it out means no
  PDB-related code reaches the upstream binary at all.
- Any form of remote or automatic crash reporting.

## Why log_level is out of scope

An earlier draft honored upstream's `debug.log_level`. It cannot be implemented
at acceptable cost.

`env_logger` builds its filter inside `init()` and exposes no way to retarget or
re-filter afterwards (`logger.rs:498`, `logger.rs:520`). A late
`log::set_max_level` cannot widen a filter env_logger has already built. So
honoring a configured level requires `config::load()` to run *before* logger
init — but `config.rs` emits `log::warn!` from 15 sites during load
(`config.rs:244` through `config.rs:1719`), and those diagnostics are exactly
what a crash investigation wants. Reordering to gain a level knob would silence
the messages the knob exists to surface.

`RUST_LOG` remains the filter control, with the current `info` default
unchanged. Adding `log_level` later is possible via a buffering logger that
replays into env_logger; it is not worth that machinery now.

A second obstacle, recorded so a later attempt does not rediscover it: `log`'s
`serde` feature is not enabled (`Cargo.toml:22`), so `Option<LevelFilter>` will
not deserialize. It would need a string field parsed by hand, or the feature
turned on.

## Config

| Key | File | Default | Meaning |
| --- | --- | --- | --- |
| `debug.persistent_logging` | `alacritty.toml` | `false` | Write the continuous log to `alacritree-<pid>.log`. Upstream's name, upstream's default. |
| `debug.crash_log` | `alacritree.toml` | `true` | Write crash artifacts. alacritree-only. |

`debug.crash_log` is alacritree-only, so it belongs in `alacritree.toml` where
alacritty never parses it and cannot warn about an unknown key. It sits under
`[debug]` rather than `[ui]` or `[workspace]` because it is neither a UI nor a
worktree option, and grouping it beside `persistent_logging` is where a reader
will look.

alacritree has no `[debug]` section today — neither `Config` (`config.rs:28`)
nor `RawConfig` (`config.rs:979`). The implementation adds a `RawDebug` with
both fields, a `debug` field on `RawConfig`, a public counterpart on `Config`,
and the conversion in `RawConfig::into_config` (`config.rs:1412`, constructed at
`config.rs:1635`).

**Both raw fields are `Option<bool>`, resolved with `unwrap_or` in
`into_config`.** This is not cosmetic: a derived `Default` on a bare `bool`
yields `false`, which would silently invert `crash_log`'s intended default. The
codebase already has the correct pattern — `resident_helper` is `Option<bool>`
with `unwrap_or(true)` at `config.rs:1617`. `crash_log` follows it;
`persistent_logging` takes `unwrap_or(false)`. `Config::default` must agree with
`into_config` on both.

Two facts about the merge, stated precisely because an earlier draft
oversimplified them:

- Files merge in alacritty-then-alacritree order (`config.rs:911`), and the
  recursive merge takes `None => value` only for *absent* nested keys while
  existing ones recurse through `Some(existing)` (`config.rs:967`). A `[debug]`
  table present in both files therefore merges key by key rather than being
  replaced wholesale.
- `[debug]` already parses today and is silently ignored: `RawConfig` carries
  `#[serde(default)]` without `deny_unknown_fields` (`config.rs:977`). Adding
  `RawDebug` makes selected keys *observable*, it does not make a previously
  rejected table parseable. So no existing config becomes invalid.

## Storage

**Logs are machine-local state, not roaming config.** They live in a new
`log_dir()` helper, deliberately *not* `state::config_dir()`:

| Platform | Directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\alacritree`, falling back to `%APPDATA%` then home |
| Unix | `$XDG_STATE_HOME/alacritree`, falling back to `$HOME/.local/state/alacritree` |

`state::config_dir()` prefers `%APPDATA%` (`state.rs:80`), which is the roaming
profile. Putting a default-on log there is wrong three ways: a redirected or
UNC-backed roaming profile can block or fail during the synchronous panic hook;
a OneDrive-synced `%APPDATA%` copies crash data off-machine, which contradicts
this feature's "no remote reporting" positioning; and roaming a log to another
machine makes it meaningless. The fork already treats `%LOCALAPPDATA%` as the
home for machine-local data — `fonts.rs:257` puts its font cache there. On Unix,
`$XDG_STATE_HOME` is the specified location for logs; config is the wrong
basedir.

`log_dir()` returns `Option`, and nothing creates the directory, so crash
logging must `create_dir_all` itself. If it is `None` or creation fails, crash
logging disables itself and says so once on stderr. Files are created with
owner-only permissions (`0o600` on Unix; the inherited ACL on Windows already
restricts `%LOCALAPPDATA%`).

```
%LOCALAPPDATA%\alacritree\
  crash-20260801-190211-50916.log   one per GUI process, single writer
  alacritree-50916.log              continuous log, opt-in, pruned
```

`state.toml` stays in `state::config_dir()`. It is genuinely config-like and
moving it would break existing installs.

### Per-process artifacts

**No file is ever written by two processes.** There is no shared `crash.log` on
disk. The single view you read or hand over is *derived* by a read-only command,
not maintained as a file — see "Reading them".

An earlier draft tried to keep a shared `crash.log` fed by a reconciliation step
that claimed each per-process file by `rename` before appending. It does not
work, and the reasons are worth recording so it is not reattempted:

- A claim arbitrates one *pending file*, not the shared file. Two processes
  holding claims on two different files append to `crash.log` concurrently, so
  splicing was never actually prevented.
- `rename` + append + delete cannot be exactly-once. A crash mid-append leaves a
  prefix in `crash.log` and the claimed file still present; retrying appends the
  whole thing again. No marker records how many bytes committed, and append and
  delete cannot be one atomic operation.
- `fs::rename` *replaces* an existing target, so with pid reuse a claim could
  destroy an older artifact outright.

Each GUI process owns exactly one artifact for its lifetime:

```
crash-1785708131123456789-50916.log
       └ UTC epoch nanoseconds at start   └ pid
```

**The name must be a collision-proof identity, not a readable date.** A pid
alone is not unique — reuse would let a new session append to, truncate, or fail
to create a dead session's artifact. But a second-resolution local timestamp is
not enough either: pid reuse inside one second, a clock stepped backwards, and
the repeated hour at a DST fall-back can all reproduce a name. So the key is UTC
epoch nanoseconds, and the file is opened with **`create_new`**; on the
vanishingly unlikely `AlreadyExists`, a `-2`, `-3`, … suffix is tried. Without
`create_new` a collision silently truncates the authoritative record.

**One identity, used by both files, materialized only when a file is.**
`install` captures `start` and `pid` and touches the filesystem for nothing
else. The continuous log reuses them: `alacritree-<start>-<pid>[-<ordinal>].log`.
Every remaining `alacritree-<pid>.log` in this document is shorthand for that
name.

An earlier draft said the identity was "resolved at `install` by finding the
first ordinal that `create_new` accepts". That is not implementable: `install`
creates no file, and a launch with both gates off must create none either, so
there is nothing to probe with. Probing without creating is a TOCTOU race, and
reserving by creating violates the gates.

So the ordinal is resolved **at first creation, by whichever file is created
first**, and stored for the other to reuse. `create_new` is the allocator: on
`AlreadyExists`, bump the ordinal and retry. With both gates off nothing is
created and no ordinal is ever resolved.

`start`+`pid` is already collision-proof by construction, and the ordinal exists
only for debris. Two live processes cannot share a pid; a reused pid belongs to a
process that started later, so it cannot share a start nanosecond. The residual
case is a clock stepped backwards onto the exact nanosecond of an existing
artifact with the same pid — which `create_new` catches rather than truncates.
Because correlation between the two files is by `start`+`pid`, which both names
carry regardless, the two files are not required to agree on the ordinal; if the
second collides at the first's ordinal it takes its own and logs.

Ordering — for `alacritree crashes` and anything else that sorts artifacts — is
by parsed `start` descending, then `ordinal` ascending, then filename. The
secondary keys are not decoration: two artifacts produced by a `create_new`
retry share a `start` and a `pid` exactly, so `start` alone leaves their order,
and doctor's "newest", undefined. Wall-clock correction between runs can still
misorder them against real time; UTC removes the DST case and nothing defends
against a stepped clock beyond that.

**Write path.** `session_begin` creates the artifact with a header line naming
the version and start time. The panic hook appends `PANIC` blocks. `record_exit`
appends `exit ok` or `exit error: …`. Nothing else touches the file, ever.

```
2026-08-01 19:02:11 start v0.8.0 pid=50916
2026-08-01 22:04:33 PANIC thread=pty-3
  at terminal_view.rs:412
  index out of bounds: len is 80 but index is 80
  <backtrace>
2026-08-01 23:40:02 exit ok
```

**Retention is by age and liveness only. Contents never decide deletion.**
`record_exit` appends its marker and stops there. At startup, an artifact is
deleted if — and only if — its producer pid is dead *and* it is older than 30
days. Nothing reads an artifact to decide whether to keep it.

This is a deliberate simplification, and it is worth saying why, because two
earlier drafts tried to be cleverer and both were wrong.

Deleting "clean" artifacts requires classifying them, and classification is where
the bugs live. The first draft deleted inside `record_exit(Ok)`, which raced the
detached workers that outlive it. The second deferred deletion to the next
startup, which fixed that race and introduced a worse one: if `record_exit` holds
the recorder lock while a worker panics, the hook takes the `WouldBlock` path,
skips the record, and `record_exit` then writes `exit ok` — so the next startup
sees exactly `header + exit ok` and deletes the only trace of a real panic.
Concurrent pruners could also classify the same path, delete it, and have a
second pruner delete a *replacement* an identical-identity producer had just
created.

Every one of those failures is downstream of "read the file, then decide". So
the file is no longer read to decide. A clean launch leaves a header plus an
`exit ok`, about 100 bytes; ten launches a day for a month is roughly 30 KB
across 300 files, all of which age out. That is a much better trade than a
classifier that can erase a crash report.

Startup cleanup therefore touches only the filename and `stat`: parse the
identity, check liveness, check age, delete. A `NotFound` from a concurrent
pruner is success, not an error. No artifact is ever opened.

**Why stat-then-delete is safe here, stated as an invariant because it is not
obvious.** The concern is the classic one: pruner A stats a path, pruner B
deletes it, a producer recreates that same path, and A's delete then destroys the
replacement. It cannot happen, because **identities are never reused**:

- A path is only deleted when its producer pid is dead *and* the file is over 30
  days old. Recreating that exact path needs the same `start` nanosecond, pid,
  and ordinal — so a new process would need both to be handed the dead pid and to
  have started at a nanosecond over 30 days in the past. A stepped clock is the
  only route, and `create_new` refuses rather than truncates.
- The one legitimate recreator is `ensure_artifact` in the *owning* process,
  which by definition has a live pid and is therefore never a prune candidate.

So `NotFound` as success is sufficient, and no claim-verify protocol is needed.
If this invariant is ever broken — by making identities reusable, or by pruning
live producers — the race returns and the deletion must verify identity first.

A draft deleted header-only artifacts inside `record_exit(Ok)`. That races the
threads that outlive it. Alacritree never joins its workers: the PTY thread's
`JoinHandle` is discarded (`session.rs:1191`, upstream `event_loop.rs:205`) and
`Session::drop` only sends `Msg::Shutdown` without joining (`session.rs:1449`);
the IPC listener and its per-connection threads are detached likewise
(`ipc.rs:244`, `ipc.rs:252`). So this order is reachable:

```
record_exit locks → sees no panic → deletes → unlocks
                                              → PTY hook locks → recreates a PANIC-only file
```

leaving an artifact with a panic and no header. Deferring cleanup to a startup
that runs after the producer is provably dead removes the race rather than
narrowing it, and deletes a state machine along with it.

**What an artifact means.**

| Contents | Meaning |
| --- | --- |
| header, then `exit ok`, nothing else | a clean run — the common case |
| `PANIC` block, then `exit ok` | the app recovered and shut down normally |
| `PANIC` block, no exit line | the panic took the process down |
| `exit error:` | `run_native` returned `Err` |
| `panic records skipped: N` | at least N panics occurred that could not be written; treat as a panic artifact |
| any record *after* `exit ok` | a detached worker outlived the exit; treat as a panic artifact |
| header only, no exit line, pid dead | died without panicking — hard fault, `taskkill`, shutdown, power loss |
| header only, no exit line, pid live | a window running right now |
| truncated, or a record with no header | writing was interrupted; **indeterminate** |

The last row exists because `write_all` is not one atomic OS write: it can
commit a prefix and then fail, and a process killed mid-write can split even a
UTF-8 code point. Readers must treat a malformed artifact as indeterminate —
never as clean, and never as a live process.

### Reading them

`alacritree crashes` concatenates every artifact, newest first, to stdout. That
is the single thing to read or redirect — `alacritree crashes > crashes.txt` —
and it is strictly read-only, so it can run from any process at any time without
the coordination that sank the shared-file design.

It is handled locally in `cli::run` beside `Doctor` and `Install`
(`cli/mod.rs:181`, `cli/mod.rs:244`), not turned into an `IpcRequest`: it reads
files, it does not need a running instance.

The contract, because "concatenate the files" is not implementable without it:

- **Order** is by parsed `start` descending, then `ordinal` ascending, then
  filename — the same three keys defined under "Per-process artifacts", not
  `start` alone.
- **Separator** is a line naming the artifact, so a merged dump stays
  attributable: `==> crash-<start>-<pid>.log <==`.
- **Byte-copy**, never `read_to_string`. A truncated artifact may not be valid
  UTF-8, and the one thing the reader must not do is refuse to show a damaged
  crash record.
- **Missing directory** is empty success, exit 0. Nothing has crashed.
- **A file that vanishes mid-read** — pruned by a concurrently starting
  instance — is skipped silently. Any other read error goes to stderr and makes
  the exit code nonzero, after the remaining artifacts are still printed.
- **`--json`** is a global flag (`cli/mod.rs:31`), so it applies here: an array
  of objects carrying the filename, parsed start, pid, and the raw contents as a
  lossy-decoded string. Raw concatenation is not JSON and must not be emitted
  under that flag.

`alacritree doctor` reports a summary — how many artifacts exist, the newest,
and how many look like hard faults — with the same human and JSON renderers it
already uses for everything else (`doctor.rs:84`).

Its status must be chosen deliberately, because only `Fail` exits nonzero
(`doctor.rs:873`) and doctor is a health check, not a crash browser:

| Condition | Status |
| --- | --- |
| no artifacts, or only running/clean ones | `Ok` |
| any artifact with a `PANIC` block, a `panic records skipped:` line, a record after `exit ok`, an `exit error:`, or an unexplained death | `Warn` |
| any indeterminate artifact — truncated, headerless, or over the size cap | `Warn`, counted separately from the crash count |
| the log directory is unresolvable or unwritable | `Fail` |

Past crashes are `Warn`, never `Fail`: a crash last week must not make
`alacritree doctor` exit nonzero in someone's script. `Fail` is reserved for
crash logging being *broken right now*. A malformed artifact counts as
indeterminate — reported, never folded into the clean or the live count.

Doctor reads artifact contents, unlike `prune`. It bounds that read: it inspects
at most the first and last few kilobytes of each file, and an artifact larger
than a fixed cap is reported as indeterminate rather than parsed. Without a
bound, one oversized malformed file would be read in full on every invocation.

Neither command writes to the log directory. A CLI subcommand mutating it would
reintroduce the side effect the initialization order exists to prevent.

### Bounding growth

Growth is bounded at the source, because a panicking process does not
necessarily die: a panicking PTY thread leaves the app running by design, and
the IPC listener spawns a fresh thread per connection (`ipc.rs:244`), so one
repeatable worker defect could otherwise append a backtrace per request
indefinitely. Two limits, applied to each artifact:

- **Consecutive identical panics collapse.** A repeat from the same location as
  the previous record increments a count instead of writing a new block, flushed
  as `… ×N` when a different event follows or at exit.
- **Twenty panic records per process**, after which one
  `panic records suppressed after 20` line is written and no further panic is
  recorded by that process.

Worst case is about 160 KB per process, and a clean launch leaves about 100
bytes. With 30-day pruning the directory is bounded by launch rate and incident
rate over a fixed window, not by uptime.

### alacritree-\<pid\>.log

One file per process, named like upstream's `Alacritty-<pid>.log`. Per-process
rather than one shared file with startup rotation: rotation renames a file
another live process is still writing, splitting one session's output across two
names. Per-process files remove the race rather than guarding it.

Pruned at startup, but **liveness first, age second**: a file is deleted only if
its pid is not a running process *and* it was last modified more than 7 days ago.

Age alone is not safe. A window can sit open and idle for a week without
emitting a line, leaving its mtime stale while the process is alive. Rust opens
files with `FILE_SHARE_DELETE` on Windows (`std/sys/fs/windows.rs:206`), so the
delete succeeds against the open handle; the live process then keeps writing
into an unlinked file that no path reaches — worse than not pruning, because
diagnostics appear to be recorded and are unreachable.

Liveness needs one implementation per platform, and an earlier draft's "neither
needs a new dependency" was wrong on two of the three:

| Platform | Check | Cost |
| --- | --- | --- |
| Windows | `OpenProcess` | adds the `Win32_System_Threading` feature to the existing `windows-sys` (`Cargo.toml:83`), which does not currently enable it |
| Linux | `/proc/<pid>` exists | nothing — there is no direct `libc` dependency on Linux, and the fork already reads `/proc` directly for agent detection |
| macOS | `libc::kill(pid, 0)` | nothing — `libc` is already a macOS-target dependency (`Cargo.toml:111`) |

Pid reuse can make a dead pid look live, which only defers a deletion — the safe
direction. It cannot cause a wrong *identity*, because the artifact name carries
the start value as well.

## Privacy

Not a design constraint here, recorded so a later reader does not re-derive it.

A symbolized backtrace embeds the absolute checkout path, and therefore a
username. That only affects locally built binaries, whose PDB the developer
built on their own machine — releases never carry one, and a CI build's paths
are the container's. Panic payloads are arbitrary strings and may carry worktree
or branch names. The log directory is machine-local and never roamed or synced.

No redaction is attempted. Redacting a backtrace is guesswork, and a diagnostic
you cannot trust is worse than one you read before sending.

## Architecture

Six pieces, each independently testable: the crash recorder, the log tee, the
`crashes` subcommand, the doctor summary, the config wiring, and the install
script. The first two carry the design weight and are described below; the rest
are wiring already specified in "Config" and "Reading them".

### `crash_log.rs`

- `install(dir: &Path, version: &str)` — `create_dir_all(dir)`, records the
  directory, installs the panic hook chaining to the hook returned by
  `panic::take_hook()` so stderr still gets the default message. **Creates no
  file**; the pending file is created lazily by the first write.
- `set_enabled(bool)` — sets the gate from config once it is known. An
  `AtomicBool` defaulting to `true`, so a panic between `install` and config load
  is still recorded.
- `session_begin(version: &str)` — creates this process's artifact with a header
  line. Called after the gate is set, so a launch with `crash_log = false`
  normally creates nothing.
- `record_exit(&eframe::Result<()>)` — appends `exit ok` or `exit error: …`,
  preceded by `panic records skipped: N` when the skip counter below is nonzero.
  Deletes nothing.
- `prune()` — at startup, deletes artifacts whose producer pid is dead and which
  are older than 30 days. Filename and `stat` only; opens nothing.
- `append(event: &str)` — one `write_all` of a fully built `String` to this
  process's own artifact, then `flush`.
- `ensure_artifact()` — the single idempotent initializer. Creates the artifact
  with its header if absent, returns the handle otherwise. `session_begin`, the
  early hook, and any write after the file has been removed all go through it,
  so no path can produce a headerless artifact.

`ensure_artifact` exists because the header has three possible authors. A panic
during config load must create the file before `session_begin` ever runs, and a
file deleted underneath a running process must be recreated. Left to `append`,
both cases would write a `PANIC` block into a file with no header — which the
reader is obliged to call indeterminate, discarding information it actually has.

**Write failures are swallowed, once.** A failed open, write, or flush sets an
`AtomicBool` that disables further artifact writes and emits one direct stderr
line. The hook must never propagate an error or panic: a panic raised inside a
panic hook aborts the process immediately, converting a diagnosable crash into
an unexplained one. A partial write is left as-is on disk and read back as the
indeterminate row of the artifact table.

The hook writes only when a target directory has been recorded *and* the gate is
on. Both are unset by default, which is what makes the hook inert in unit tests
that never opt in.

**One process is not one thread.** The IPC listener spawns a thread per
connection (`ipc.rs:244`) and every session has its own PTY thread, so two
threads can panic concurrently and splice even a process-private file. `append`
and the suppression counters are therefore guarded by a `Mutex`.

**The hook uses `try_lock`, never `lock`.** A blocking lock deadlocks against
itself: if a thread panics *while already holding* the mutex, the hook runs
before unwinding releases the guard, so its own `lock()` waits forever on a
mutex that is not yet poisoned. `PoisonError::into_inner` does not help, because
poisoning only happens once unwinding completes. So:

- `Ok(guard)` — write the record.
- `Err(Poisoned)` — recover with `into_inner` and write; an earlier panic must
  not silence this one.
- `Err(WouldBlock)` — increment a lock-free `AtomicUsize` skip counter, write one
  minimal line to stderr, and skip the record.

Losing a concurrent record is strictly better than converting a recoverable
panic into a permanent hang, which is a worse failure than the one being
diagnosed.

The skip counter matters because `record_exit` is itself a lock holder: a worker
panicking while `record_exit` runs is exactly when `WouldBlock` happens. So
`record_exit` reads the counter — it already holds the lock — and writes
`panic records skipped: N` ahead of its own marker.

**That marker is explicitly best-effort, not a guarantee.** `record_exit` can
read the counter, and a panic can then take the `WouldBlock` path and increment
it before the lock is released, so the marker can undercount or be absent
entirely. Closing that would need the exit protocol to re-check under the same
lock the panicking thread is failing to acquire, which is circular. The stderr
line is the backstop, and the cost of an undercount is now only a slightly less
informative artifact — retention no longer reads contents, so nothing is deleted
because of it. An earlier draft promised the marker left the artifact "honest
about what it is missing"; that promise was unimplementable and is withdrawn.

This is the ceiling on what the hook may do. It formats, locks, appends, and
flushes. It does not rename, read other files, or delete anything. A panic
raised *inside* a panic hook aborts the process immediately
(`std/panicking.rs:798`), so every operation the hook performs is one more way
to convert a recoverable panic into an unexplained abort — which is the exact
failure mode this feature exists to diagnose.

Two constraints shape `append`, and both belong in comments:

- **No long-lived handle.** If a panic ever does reach an abort, abort skips
  destructors. Every event must be on its way to disk before the hook returns,
  so the file is opened and closed per event. `flush` guarantees the bytes left
  this process; it is not a power-loss durability claim.
- **One write per event**, for the interleaving reason above.

The hook captures:

- `std::backtrace::Backtrace::force_capture()` — stable since 1.65, ignores both
  backtrace environment variables. `std::env::set_var` is `unsafe` in edition
  2024, and while std documents Windows env mutation as sound even when
  threaded, the cross-platform answer is to not touch the environment at all.
- `panic_info.location()`,
- the payload, via `downcast_ref::<&str>` then `downcast_ref::<String>`,
- `std::thread::current().name()`, so a PTY thread that panics without killing
  the app is still attributed.

No native eframe, egui, or winit code installs a panic hook — eframe's is
web-only (`eframe/src/lib.rs:164`) — so there is nothing to clobber, and the
chained previous hook is the std default.

### `logging.rs`

- `Tee` implements `io::Write`, forwarding to stderr and to a shared
  `Arc<Mutex<Option<File>>>`. The `Arc` is load-bearing: `Target::Pipe` takes
  `Box<dyn Write + Send + 'static>` (`target.rs:11`) and *moves* the writer into
  env_logger, so `main` can only fill the sink later through a handle it owns.
- **`write` mirrors only the prefix stderr accepted.** If stderr returns
  `n < buf.len()`, only `buf[..n]` goes to the file and `n` is returned;
  env_logger's `write_all` then retries the suffix, which must not be written
  twice. Writing the whole buffer and returning `n` duplicates it in the file.
- **Diagnostics go to stderr directly, never through a `log::*` macro.**
  env_logger holds its own pipe mutex across `write_all` and `flush`
  (`buffer.rs:92`); logging from inside `Tee::write` re-enters that mutex and
  deadlocks. A failing file sink is set to `None` after one direct stderr note,
  degrading to today's behavior.
- Otherwise the outer-then-inner lock order is redundant but not deadlock-prone.
- Startup pruning as described above.

### `install.local.ps1`

Add `alacritree.pdb` to `$Payload` (line 25). The release profile sets
`debug = 1`, so line tables exist; Windows backtrace symbolization reads the PDB
beside the exe. Without it a captured backtrace is a list of addresses.

The script's staging, rename-aside, and sweep logic is driven entirely by
`$Payload` and is name-agnostic (lines 52, 96, 110), and `stale_exe.rs:26`'s
sweep would recognize an `alacritree.pdb.stale-*` name. A running process does
not lock its PDB — the loader maps the PE image, not the symbol database — so
the rename-aside path will rarely trigger for it. `build.rs:55` searches only
`.exe` names and is unaffected.

This script is the only place the PDB is copied. No Rust code changes for it, so
nothing about symbol delivery reaches the upstream binary.

### `main.rs`

Capture the `run_native` result, hand it to `record_exit`, return it unchanged.

## Initialization order

The hook is installed on the **GUI path only**, after `cli::run` has declined to
handle the invocation.

An earlier draft installed it before `cli::run`, which was wrong in three ways:
every subcommand exits before `config::load()` (`main.rs:97`, `cli/mod.rs:181`),
so `crash_log = false` could never govern them; `alacritree mcp` is a long-lived
stdin loop (`mcp.rs:23`) that would have written crash records no config could
disable; and `create_dir_all` would have run on every `--help`, `doctor`, and
IPC command. The claim that those paths were "untouched" was false.

CLI and MCP processes keep today's behavior: a panic prints to stderr, which on
those paths is a real console (`attach_parent_console`) or the MCP client's pipe.
Capturing them is a separate decision with its own gate and lifecycle.

The order in `main`, with existing steps marked:

1. `harden_dll_search_path()` — existing.
2. `env_logger` init with the `Tee` target, file sink empty — existing position,
   existing filter, so nothing about CLI output changes.
3. `attach_parent_console()` — existing. Anything reported before this is
   invisible to a Windows CLI user, which is why nothing is reported earlier.
4. `cli::run`; `Some(code)` exits here — existing.
5. `crash_log::install(&log_dir, version)`. The gate defaults to enabled, so a
   panic in any later step is recorded even though config has not been read.
6. `config::load()`.
7. `crash_log::set_enabled(config.debug.crash_log)`, then
   `crash_log::session_begin(version)` and `crash_log::prune()`. Deferring file
   creation past the gate is what keeps a launch with `crash_log = false` from
   leaving an *artifact* behind. The log directory itself is still created by
   `install` in step 5, before the gate is known — an empty directory, and the
   narrowest thing that still lets a config-load panic be recorded.

   **One documented exception:** the gate defaults to enabled, so a panic in
   step 5 or 6 — before the preference is known — does create an artifact even
   when the config would have said no. That is the deliberate trade for catching
   a crash during config loading, and it is the only case where `crash_log =
   false` leaves a file.
8. If `persistent_logging`, prune old logs, open `alacritree-<pid>.log`, fill the
   sink. Lines logged before this point went to stderr only — acceptable, since a
   panic in steps 5–7 is caught by the artifact regardless.
9. `run_native`, then `record_exit`.

The asymmetry in 5–7 is deliberate: the hook is armed as early as the GUI path
allows and silenced late, while routine bookkeeping waits for consent. A crash
during config loading is the one case where writing against a not-yet-known
preference is the right trade.

## How a panic actually exits

`eframe` calls `App::update` (`epi_integration.rs:281`). Winit catches the
callback's unwind (`runner.rs:170`) and resumes it *after* `DispatchMessageW`
returns (`event_loop.rs:419`). The unwind therefore propagates out of
`run_native` without eframe ever reaching `app.return_result` (`run.rs:321`),
straight out of `main`, and Rust's runtime turns the main-thread panic into exit
code 101 (`rt.rs:175`).

Consequences the design depends on:

- **The panic hook is the only thing that records a panic.** It runs before the
  panic runtime, so it fires regardless. `record_exit` is bypassed entirely.
- `record_exit` covers exactly two paths: `run_native` returning `Ok`, and
  returning `Err`.
- The CLI's `process::exit` (`main.rs:97`) also bypasses `record_exit`, which is
  correct — that path never opens a window and never wrote a `start` marker.

## Testing

The crate is binary-only — no `lib.rs`, no `tests/`, and `[[bin]]` is the sole
target (`Cargo.toml:148`) — so `config`, `state`, and the new modules are private
to `main.rs`. Integration tests cannot call them. That splits the plan in two.

### In-crate unit tests

The hook is inert until a target directory is recorded, so an unrelated
`#[should_panic]` elsewhere in the binary writes nothing even while our hook is
installed. That is what makes these safe under the normal harness rather than
any ordering guarantee.

1. Record a temp directory, run `catch_unwind(|| panic!("boom"))`, assert the
   artifact contains the payload, the source location, and the thread name. This
   is the RED test: before the hook exists the file is never created, so it fails
   on the missing file rather than on a formatting detail.
2. Panic a *named* spawned thread; the record names that thread and the process
   survives.
3. Two events append in order; a file deleted between them is recreated.
4. `install` creates a log directory that does not yet exist; a panic on that
   first launch is still recorded.
5. Twenty-one panics produce twenty records plus one suppression line; two
   identical consecutive panics collapse to one record with a count.
6. Concurrent panics from several named threads produce whole, unspliced records
   — one per panic, each with its own thread name.
7. A `Mutex` poisoned by an earlier panic does not stop the next panic from
   being recorded, **and** a panic raised by a thread already holding the lock
   is skipped with a stderr note instead of hanging. The second half is the one
   that matters: a blocking `lock()` would deadlock here, and the test must fail
   by timing out against that implementation rather than passing.
8. A `WouldBlock` skip increments the counter, and a `record_exit` that runs
   afterwards writes `panic records skipped: N` ahead of its marker. The test
   asserts the best-effort contract, not a guarantee: a skip racing
   `record_exit`'s read may be absent from the artifact, and that is not a
   failure.
9. `record_exit` never deletes: `Ok` on a header-only artifact leaves it on disk
   with `exit ok` appended, and a `PANIC` record written *after* `record_exit`
   still lands in the same file.
10. `ensure_artifact` is idempotent and header-correct from all three callers: a
    panic before `session_begin` produces a file with exactly one header,
    `session_begin` afterwards does not write a second, and a file deleted
    mid-session is recreated with one.
11. Startup cleanup deletes by age and liveness alone: a dead pid's artifact over
    30 days goes regardless of contents, a fresh one stays regardless of
    contents, and a live pid's is never touched. A path that vanishes between
    listing and deletion is success, not an error.
12. Two pruners running concurrently over the same directory both complete
    without error, and neither deletes an artifact created after it listed.
13. Two sessions colliding on both pid and start value: `create_new` forces the
    ordinal path, both artifacts exist, neither truncates the other, and
    `crashes` orders them deterministically by ordinal.
14. Identity allocation: with both gates off nothing is created and no ordinal is
    resolved; with either gate on the first file created resolves the ordinal and
    the second reuses it; both names carry the same `start` and `pid` so the two
    correlate regardless.
15. A truncated artifact — bytes cut mid-record, including mid-UTF-8 — is
    reported as indeterminate by doctor and still printed by `crashes`. An
    artifact over doctor's size cap is indeterminate without being read whole.
16. Continuous-log pruning honors the 7-day rule under liveness: old plus dead
    is deleted, old plus live is spared.
17. A pid reused by a *different* executable reads as live and only defers
    deletion.
18. Doctor's status mapping: no artifacts is `Ok`, a past `PANIC` is `Warn` and
    exits 0, an unwritable log directory is `Fail` and exits 1.
19. `Tee`: writes reach both sinks with a `Vec<u8>` standing in for stderr; a
    short stderr write mirrors only the accepted prefix and does not duplicate
    on retry; an erroring sink is dropped without failing the stderr write;
    filling the sink after construction takes effect.
20. `record_exit(Err)` writes `exit error:` with the message.
21. A write failure disables further artifact writes and notes it once, without
    propagating an error out of the hook.
22. Config: `crash_log` defaults to `true`, `persistent_logging` to `false`,
    `alacritree.toml` overrides `alacritty.toml` for `crash_log`, and a
    `[debug]` table in both files merges key by key.

Tests that install a hook restore the previous one on the way out. `take_hook`
installs the default in place of what it removes (`panicking.rs:182`) rather
than leaving a slot, so restoration is an explicit save-and-set, and these tests
are grouped so they do not race each other.

### Subprocess tests

The defect that motivated the initialization-order rewrite would pass every unit
test above, because it is about which *process* installs the hook. So one
integration test binary under `alacritree/tests/` spawns the real executable via
`env!("CARGO_BIN_EXE_alacritree")`, with `HOME`/`LOCALAPPDATA` pointed at a temp
directory:

23. `alacritree --help` and `alacritree doctor` exit without creating the log
    directory or any artifact.
24. `alacritree crashes` through the real binary: correct order and separators
    over seeded artifacts, empty success against a missing directory, and a
    valid array under `--json` in both flag positions the global parser accepts.
25. A GUI launch that panics while holding the artifact lock exits rather than
    hanging — a timeout is the failure, which is the only way to catch a
    blocking-`lock()` regression from outside the process. This needs an
    explicit stimulus, since the shipped binary offers no way to provoke it: a
    `#[cfg(debug_assertions)]` hidden argument that takes the lock and panics.
    Without it the test cannot be written, and a stimulus that ships in release
    builds is not acceptable.

This is the regression guard for "the CLI is untouched" — the one claim in this
spec that was previously asserted and false.

## What this does not catch

An access violation, a stack overflow, or any hard fault with no Rust panic
behind it. The artifact stays silent for those, because the panic hook never runs.
Given that the 2026-07-25 aborts are unexplained, this gap is real rather than
theoretical.

The gap has two halves, and they need different tools.

**Ordinary exceptions** — access violations and the like — are catchable in
process with `SetUnhandledExceptionFilter` plus `MiniDumpWriteDump` from
`dbghelp`. That remains the deferred branch, and it depends on this one for the
directory handling.

**Fail-fast aborts are not.** Rust's Windows `abort` uses `__fastfail`, which
terminates *without running any in-process exception handlers*
(`std/sys/pal/windows/mod.rs:256`). No `SetUnhandledExceptionFilter` will ever
see it. So the deferred work would not have captured 2026-07-25's `0xC0000409`,
which is precisely the mode it was meant to explain. The only mechanisms that
capture a fail-fast are out of process: WER `LocalDumps` or an attached
debugger. `LocalDumps` stays out of scope as a design element — it is a
machine-local registry setting, not something alacritree can ship — but it is
the right manual step on a machine that is reproducing an abort.

One consolation: if those aborts follow a Rust panic — a panic while already
unwinding, or a double panic, both live candidates — then the hook fires for the
*first* panic and the artifact records it before anything reaches `__fastfail`.
This feature may explain 2026-07-25 after all, just not by capturing the abort
itself.
