# Pool accounting design

**Goal:** the job pool's admission limits stop lying, a cancelled job gives its
worker back, no request can park a worker forever, and PR lookups stop spending
one subprocess per worktree.

**Issues:** [#32](https://github.com/AbysmalBiscuit/alacritree/issues/32),
[#33](https://github.com/AbysmalBiscuit/alacritree/issues/33),
[#37](https://github.com/AbysmalBiscuit/alacritree/issues/37) and
[#44](https://github.com/AbysmalBiscuit/alacritree/issues/44), all sub-issues of
[#22](https://github.com/AbysmalBiscuit/alacritree/issues/22).

**Branch:** `perf/pool-accounting`, cut from `perf/nonblocking-ui`. It is the
first branch of the stack described in #22, so every later grouping rebases onto
it.

## Context

`perf/nonblocking-ui` introduced `alacritree/src/jobs.rs`: a fixed pool of
worker threads, two priority classes, and a `Blocking` token whose constructor
is private to the module so a blocking helper cannot be called from the UI
thread. That branch established the invariant. This one fixes the accounting
underneath it.

Three properties of the pool as it stands:

- `take()` pops from the interactive queue unconditionally. Only `Background` is
  gated, admitted while `background_running + 1 < workers`, so its ceiling is
  `workers - 1`. Interactive tasks occupy no slot and have no ceiling, so enough
  of them hold every worker.
- `Job::drop` sets a `cancelled` flag that `worker` reads exactly once, before
  the task starts. Dropping the handle of a running job frees nothing.
- `Pool::new` clamps workers to at least two; `pool()` sizes the singleton
  `available_parallelism().map_or(4, |n| n.get().clamp(4, 8))`, chosen for
  IO-bound work rather than derived from core count.

`worktree::spawn_create` runs at `Interactive` and shells out to an untimed
`git fetch`. Its two callers are the create modal, which holds the `Job` in
`CreateState::Running`, and the IPC connection thread serving `CreateWorktree`.
Closing the modal already drops the handle, and today that does nothing.

`PrCache` submits one `Background` job per worktree path, each running its own
`gh pr list --head <branch>` process. Its `pr_status_concurrency` default of 8
exceeds the pool's background ceiling on every machine.

## 1. Interactive admission ceiling

`State` gains `interactive_running` beside `background_running`, and `take()`
gates both classes identically:

```rust
if state.interactive_running + 1 < workers {
    if let Some(task) = state.interactive.pop_front() { /* … */ }
}
if state.background_running + 1 < workers {
    if let Some(task) = state.background.pop_front() { /* … */ }
}
None
```

Interactive keeps first refusal, so a click never queues behind a git walk. What
changes is that a worker whose interactive class is at ceiling falls through to
background rather than taking another create.

An over-ceiling submission queues. It is not refused. Refusing would change
`spawn`'s return type and force every call site to handle a case that only fires
under parallel MCP-driven creates, and dropping user-initiated work silently is
worse than making it wait.

`BackgroundSlot` generalises into one guard parameterised by class, keeping the
release-on-unwind behaviour that stops a panicking job from permanently
shrinking the pool.

Both ceilings are `workers - 1`, so each class is guaranteed a worker the other
cannot take. They deliberately do not sum to the worker count: the workers are
still the real limit, and the ceilings exist only to prevent a shutout.

## 2. Job cancellation

### What ports from zed, and what does not

zed's `Task<T>` is `#[must_use]` and cancels on drop
(`crates/scheduler/src/executor.rs`). For work blocked in a subprocess it adds
`kill_on_drop(true)` on the child (`crates/git/src/repository.rs`,
`crates/git/src/blame.rs`), so dropping the task drops the future, which drops
the `Child`, which kills the process.

The async half does not port. zed cancels at await points and this pool runs
`FnOnce` on std threads with none. Killing the child does port, and works better
than expected there: the worker is blocked in a wait on the child, so the kernel
is the yield point and no cooperative polling is needed.

zed puts no timeout on `git fetch`, deliberately
(`crates/git/src/repository.rs`, in `run_askpass_command`):

> Git can legitimately run long without prompting (e.g. large fetches, hooks),
> so completion is determined by the process itself.

That reasoning holds here. A large fetch over a slow link and a hung fetch are
indistinguishable by duration, so any timeout eventually cancels someone's
legitimate clone. This design has no timeout.

### Mechanism

The cancel flag and the child a job is waiting on move into one shared value:

```rust
struct Cancel {
    flag: AtomicBool,
    child: Mutex<Option<Child>>,
}
```

`Task`, `Job` and `Blocking` each hold an `Arc<Cancel>`. `Blocking` stops being
a `()` newtype and wraps that Arc; its constructor stays private to the module,
so the compile-time gate that `perf/nonblocking-ui` established is unchanged.

`Blocking` gains the opt-in:

```rust
/// Run a child a cancel is allowed to kill.  Registering is the opt-in: an
/// unregistered child runs to completion whatever the caller does with the
/// handle.
pub fn run_cancellable(&self, cmd: &mut Command) -> io::Result<Output>

/// Whether this job's handle has been dropped.  For work between children,
/// where there is nothing registered to kill.
pub fn cancelled(&self) -> bool
```

It spawns, registers the child, then re-checks the flag before waiting. That
double-check closes the race where `Job::drop` fires between the spawn and the
registration and its kill finds nothing to kill.

`Job::drop` sets the flag, then locks and kills whatever is registered.

The killer needs `&mut Child` and so does the waiter, so they take turns on the
mutex: the waiter loops on `try_wait` with a short sleep, and a killer that got
there first leaves `None` behind, which the waiter reads as cancelled. Poll
every 25 ms: invisible against a fetch that runs for seconds, and confined to
the one call that opts in.

The alternative is a platform seam that kills by pid, `TerminateProcess` and
`libc::kill` behind a cfg the way `focus_priority` does it. Rejected: it costs
unsafe code on two platforms to remove a latency nobody can perceive.

### Which children opt in

Cancellation safety is not uniform across `worktree::create`. It checks the
remote, resolves the base branch, fetches, runs `git worktree add`, then copies
local files. Killing during the fetch is safe, because git writes to a temporary
pack and updates refs at the end. Killing during `worktree add` can leave a
registered but incomplete entry in `.git/worktrees/` that needs a prune.

So the fetch registers and nothing else does:

```rust
send("Fetching latest changes…");
run_git_cancellable(blocking, &req.project_root, &["fetch", "origin", &base])?;

send("Creating git worktree…");
run_git(&req.project_root, &["worktree", "add", …])?;   // not killable
```

A cancel during the fetch kills the child, the error propagates through
`create`, and the worker is free. A cancel after it is a no-op, and the
remaining steps are local and fast, so the worker comes back either way. No
half-registered worktree is reachable.

Killing a child is only half of it. `create` also runs `pick_worktree_path` and
the config copies, which take `blocking` and do local work with no child
registered, so a flag set during one of those frees nothing until it returns.
Those steps are fast, so it is a delay rather than a leak. `create` checks
`blocking.cancelled()` between steps and returns early when it is set, which
costs one branch and makes cancellation honest rather than approximately
honest.

### Callers outside the pool

`jobs::on_this_thread` builds a `Cancel` nothing holds, so `run_cancellable`
there behaves exactly like a plain run. The CLI path does not change.

### The IPC deadline

The GUI modal owns its `Job` and drops it when the user closes the modal, so
cancellation has a trigger there. The IPC connection thread has none.
`create_worktree` blocks on `rx.recv()` until the job finishes, holding the
handle alive, while both clients that reach it cap their own side at 300 s and
give up. Against an unreachable remote the caller gets its error and the worker
stays parked on the fetch for good. A few parallel MCP creates and the pool is
gone, with nothing visible to whoever asked.

The connection thread takes its own deadline:

```rust
let deadline = Instant::now() + IPC_CREATE_BUDGET;
loop {
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        // …
        Err(RecvTimeoutError::Timeout) => break Err("worktree create timed out"),
    }
}
```

Absolute, computed once. A per-message `recv_timeout` resets on every
`Progress::Step`, so a job that dribbles steps outlives the bound indefinitely:
the same leak, slower.

`IPC_CREATE_BUDGET` is the server's limit on how long it will hold a worker for
one request, not a mirror of the client's timeout. Naming it that way is what
keeps the two numbers from having to track each other, because a future client
with a different bound changes nothing here.

Dropping the handle on timeout kills the registered fetch and frees the worker.
The error travels back over a socket the client has usually already abandoned,
which costs nothing.

Detecting client disconnect is the more precise trigger and is not worth its
machinery. The thread is parked on the progress channel rather than the socket,
and noticing a dead named pipe on Windows needs a watcher thread plus a way to
interrupt the `recv`. The deadline is the ownership boundary either way, so
disconnect detection would sit on top of it rather than replace it.

Two races, both pre-existing in shape. The client normally times out first, so
its message is what the user sees rather than the server's. And inside the skew
the server can finish a create the client already reported as failed, which is
what any one-shot request and reply does when a client vanishes.

## 3. The `gh` concurrency cap

`Pool` exposes the number it already computes:

```rust
/// The most background tasks this pool will run at once.
pub fn background_ceiling(&self) -> usize   // workers - 1
```

The resolved `Ui.pr_status_concurrency` becomes `Option<usize>`, carrying the
user's intent rather than a number the pool overrides anyway. `RawUi` already
holds an `Option` for merge purposes; this stops the resolver collapsing it.

```rust
fn effective_cap(configured: Option<usize>, ceiling: usize) -> usize {
    configured.unwrap_or(usize::MAX).min(ceiling.saturating_sub(1)).max(1)
}
```

Unset means the pool decides. Set means at most that, still clamped. No value a
user can write raises the cap, which is what the doc comment already claims and
the code does not currently honour.

Reserving one slot below the ceiling is the pool's own trick one level down: it
guarantees local background work a worker by construction rather than by
choosing a literal. A fixed default cannot do that, because any number safe on
the four-worker floor is pointlessly small on an eight-worker machine.

`DEFAULT_CONCURRENCY` is removed. The doc comment on the `Raw` struct loses
"Defaults to 8" and gains what unset means. Regenerate the published schema:

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

## 4. The `pr_status` clock

The diagnosis in #37 is right and its prescription stops short. Injecting
`Instant::now()` does not help, because `Instant` cannot be constructed or
advanced, so a test still cannot build a stale one and `checked_sub` survives.

Stop storing `Instant`. `PrCache` captures one origin at construction and stores
every timestamp as elapsed `Duration` from it:

```rust
struct PrCache {
    clock: Box<dyn Fn() -> Duration + Send>,   // elapsed since this cache's origin
    // entry.queried_at: Option<Duration>
}
```

Production passes a closure over a captured `Instant`. Tests pass a closure over
a `Cell<Duration>` they set directly. `stale_start()`, `checked_sub` and the two
early returns disappear along with the reason they existed, and a machine's
uptime stops being an input to whether a test asserts anything.

The TTL boundary becomes testable, which is what #37 asks for at the end.

`StatusCache`'s 1.5 s throttle has the same `Instant` shape but no test
asserting on it, so it is left alone rather than widening the diff. It is the
obvious next place if the same bug appears there.

## 5. Batched PR lookups (#44)

`PrCache` runs one `gh pr list --head <branch> --state all --limit 100` process
per worktree path. Worktrees of one project all ask the same repository the same
question, in separate processes, and they all become due together when the TTL
expires. That is worst exactly when the cap from section 3 binds.

alacritree knows the exact branch list, so it never needs to fetch and filter.

### The query

One aliased `pullRequests(headRefName:)` per branch, in one request:

```graphql
query { repository(owner: "…", name: "…") {
  b0: pullRequests(headRefName: "perf/pool-accounting", states: [OPEN, MERGED, CLOSED],
                   first: 5, orderBy: {field: CREATED_AT, direction: DESC})
      { nodes { number baseRefName url state isDraft headRepositoryOwner { login } } }
  b1: pullRequests(headRefName: "…", …) { nodes { … } }
} }
```

No pagination and no client-side filtering, so the `--limit 100` overflow the
current code hedges against stops existing. The fields match what
`PR_JSON_FIELDS` already requests, so `parse_gh_output`'s owner tiebreak carries
over: `headRefName` still matches across forks, so the tiebreak is still needed
to prefer this checkout's own PR.

### Transport

`gh api graphql`, not direct HTTP.

devkit POSTs GraphQL with a resolved token and falls back to `gh api graphql`
(`crates/devkit-issue/src/prs.rs`, `fetch_graphql`). alacritree has neither a
token path nor an HTTP client, and adding both to fetch PR badges is a bad
trade. One `gh` process for N branches is the whole win.

The query goes to `gh` as a JSON body on stdin, not on the command line:

```sh
printf '%s' '{"query":"…"}' | gh api graphql --input -
```

`--input -` reads a body, so a bare query string piped in returns HTTP 502,
which looks like a transient GitHub failure and is not. Build the query, wrap it
as `{"query": …}`, write that to the child's stdin.

Stdin rather than `-f query=` because `-f` puts the query in argv and Windows
caps a command line at 32,767 characters. At roughly 205 characters per alias
plus the branch name, that ceiling lands near 100 aliases for long branch names.
Stdin removes it, verified on Windows `gh` 2.98.0.

The per-branch `gh pr list` path stays as the fallback rather than being
deleted. GraphQL can require scopes that `pr list` does not, so a `gh` install
that works today could fail on the batched path. That mirrors devkit's
HTTP-then-`gh` shape one layer down.

On WSL the batched query rides the resident helper, the same way the per-branch
query does now.

### What changes in `PrCache`

Entries stay keyed by worktree path. At spawn time the due entries are grouped
by repository slug, and one job goes out per group instead of one per path.
`in_flight` counts groups, so `effective_cap` caps groups, which is the useful
meaning.

Grouping by repository groups by WSL location for free, since worktrees of one
project share a location. The repository slug comes from the same git2 read of
`origin` that `local_origin_owner` already does, hoisted to once per group.

The reduction is the worktrees-per-project ratio: one `gh` process per project
per TTL window, instead of one per worktree.

### What devkit teaches beyond the query

devkit pages its search at 25 with this reason
(`crates/devkit-issue/src/prs.rs`):

> GitHub to resolve ~90k nodes before it can answer, which times out (HTTP 504)
> on a repo with many open PRs.

Its query is a search over `author:@me`, so it pays for repository size where
this one pays for branch count. Measurement confirms the difference: at four
aliases, a repository with 4,582 open PRs answered in 0.51 s and one with 4
answered in 0.49 s. devkit's failure does not reproduce here, and naming exact
head refs is why.

Chunk at **100 aliases**. Nothing forces a smaller number: no 502 or 504 appeared
at any size up to 398, no partial-`data` response appeared, and rate limit cost
stays at 1 point through 100 aliases against a 5,000/hour budget. 100 is chosen
on latency, where per-branch time flattens around 50 and chunks from 50 to 150
are indistinguishable at six concurrent requests, but 100 wins consistently at
two: 2.58 s for 398 branches, against 3.30 s at 75 and 3.01 s at 125. Two is the
concurrency an eight-worker machine actually has after the reservation in
section 3.

`first: 5` earns its place. Across 40 measured branches the batched query picked
the same PR as `gh pr view` every time, and one head ref did match more than one
PR.

Also worth taking: retry with backoff on failure, and accept partial responses.
Partial matters more here than it does in devkit, because one failed request
loses a whole project's badges instead of one branch's, so a per-alias error has
to leave the other aliases' data usable.

Keep the transport split from a pure parse, which `parse_gh_output` already
does.

### Not a config gate

The same badges by a cheaper route. No new UX, so nothing to put behind a flag.

## Testing

### Pool

Against a hand-built `Pool::new(4)`, not the process singleton.

**The interactive ceiling holds a slot.** Submit four interactive jobs that
block on a channel, submit one background job, assert the background job runs
before any interactive job is released. Fails today.

**A cancel kills a registered child.** Run a long-sleeping child through
`run_cancellable`, drop the `Job`, assert the call returns and the worker takes
the next task within a bound. Without the fix the worker is held for the sleep's
full duration, so it fails for the right reason.

**A cancel racing registration.** Set the flag before the child is registered,
assert the child does not outlive the call. This covers the double-check, which
is the part that would silently regress.

### `pr_status`

`effective_cap` is a free function over two numbers, so its table is cheap:
unset yields `ceiling - 1`, a configured value below that wins, a configured
value above it is clamped, and a two-worker pool yields 1 rather than 0.

The two existing TTL tests lose their early return and assert on every machine.
One new test pins the boundary: at exactly `TTL` the entry is stale, at one tick
less it is not.

Batching gets a pure test over a built query and a canned response: N branches
produce N aliases, a response with a per-alias error still yields the other
aliases' PRs, and the owner tiebreak still prefers this checkout's own PR.

### `worktree` and IPC

`create` returns early when `blocking.cancelled()` is set between steps, rather
than running the remaining local work.

The IPC create ends at its deadline and frees the worker. The test that matters
is the one that separates an absolute deadline from a resetting one: a job
emitting a progress step every second must still end at the budget, not run
forever. A per-message `recv_timeout` passes every other test here and fails
that one.

`create` cancelled during the fetch returns `Err` and leaves no worktree on
disk. This needs a repository fixture and an origin that hangs. If it turns out
to need a fake `git` on `PATH`, drop it and rely on the pool-level cancel test
rather than building a fragile harness.

Nothing here re-tests what already passes. The UI-thread audit and the clippy
gate carry over from the base branch unchanged, and the audit must still report
zero blocking leaves.

## Deliberately not done

- **No timeout on the fetch**, which contradicts #32's title. The issue asked
  for one because a hung fetch holds a worker forever. Cancellation solves that
  without having to guess a duration, and zed's reasoning above says any
  duration guessed here eventually cancels a legitimate large fetch. Close #32
  noting the substitution rather than leaving the title's promise unexplained.
- **No cancellation of git2 walks.** libgit2 has cancellation callbacks, but
  nothing here is blocked long enough in one to justify the reach.
- **No third priority class or per-consumer budget.** Reserving one background
  slot achieves what a separate `gh` budget would, for one consumer.
- **`StatusCache` keeps its `Instant`.** Same shape as the `pr_status` bug, no
  test asserting on it, out of scope.

## Open questions

1. `IPC_CREATE_BUDGET`'s value. 300 s matches what the clients already use
   (`cli/mod.rs:315`, `mcp.rs:117`), so the server's error almost never reaches
   a caller. A shorter server budget would deliver a real message instead of the
   client's own timeout, at the cost of two numbers that drift. Left at 300 s
   unless the message matters.
2. Nothing further. Chunk size was measured and is recorded in section 5; the
   handoff that produced it is `gh_batch_measurement.local.md`.
