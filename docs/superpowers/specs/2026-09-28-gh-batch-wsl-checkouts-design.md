# Batch WSL checkouts in the PR refresh

Issue: alacritree/alacritree#374. Branch: `374-perf-gh-batch-wsl-checkouts-in-the-pr`, based on `master`.

## Problem

A PR refresh makes one serial `gh` call per WSL checkout. `PrCache::spawn_due` in `alacritree/src/pr_status.rs` fills `Head::remotes` only for Windows paths, so every WSL head reaches `groups_with` in `crates/alacritree_gh/src/lib.rs` with `remotes: None`, becomes its own ungrouped group, and takes the per-branch `query_gh` path: one `wsl.exe` plus `gh pr list`, about 0.7 s each. A WSL project with 53 worktrees made a 70-step refresh.

Repositories with no remote run `gh pr list` anyway and fail. A native checkout reports `gh failed (exit code: 1)`. A WSL checkout reports `Malformed`, because the script hands empty stdout to the JSON parser. Both count toward the status row's failure total.

Every burst also pays one `gh repo view` per repository, and a burst's groups run one after another.

## Goals

- A WSL project batches like a native one: a 53-worktree WSL project costs one remotes read plus two GraphQL requests once the resolve is cached.
- A checkout whose repository opened and lists no remotes starts no process, reports no step, and never counts as a failure. A repository that could not be read keeps today's per-branch path.
- `resolve_repo` runs once per repository per app run.
- A burst's groups run in parallel under a cap of 4, each step's timing covers its own request, and the status row's estimate accounts for the parallelism.

## Non-goals

- No config option. The change alters no behavior a user relies on, so it ships on.
- No change to the `VersionControl` or `RemoteForge` traits. `alacritree_vcs::Remotes`, a model type, gains one field.
- No invalidation of the resolve cache short of an app restart.
- Failures unrelated to remotes, such as `gh` missing inside a distro, still count as failures.

## Design

Forge-side changes live in `alacritree_gh`. Outside it:

- `alacritree_vcs::Remotes` gains `no_remotes`, and `alacritree_git::remotes` sets it.
- `alacritree_common::jobs` gains per-step start marks.
- `alacritree_forge` exports `PARALLEL_GROUPS`.
- `alacritree/src/activity.rs::estimate` divides by it.
- `alacritree/src/pr_status.rs` changes in a doc comment.

### `Remotes::no_remotes`

`alacritree_vcs::Remotes` gains `pub no_remotes: bool`, documented as "The repository opened and lists no remotes at all. `false` when it could not be read, so a caller skips a lookup only on evidence." `Default` leaves it `false`.

`alacritree_git::remotes::remotes` sets it from `repo.remotes()` being empty after `Repository::open` succeeds. An open failure returns `Remotes::default()` as today, so a reftable repository or a `safe.directory` refusal still reaches per-branch `gh pr list`. A clone whose only remote is `upstream` reads `origin_url: None, push_url: None, no_remotes: false` and also stays per-branch.

Struct literals in tests (`pr_status.rs`, `alacritree_gh`) take `..Default::default()`.

### Transport seam

`pull_requests_with` takes a private trait in place of its injected closures:

```rust
trait Transport: Sync {
    fn remotes(&self, distro: &str, heads: &[(&str, &str)]) -> Option<Vec<Option<Remotes>>>;
    fn resolve(&self, cwd: &Path, origin: &(String, String)) -> Option<(String, String)>;
    fn request(&self, cwd: &Path, query: &str) -> Option<Vec<u8>>;
    fn per_branch(&self, head: &Head, owner: Option<&str>) -> Result<Option<PrInfo>, ForgeError>;
}
```

`Gh<'a> { blocking: &'a Blocking }` is the real implementation. `resolve` and `request` dispatch on `wsl::classify(cwd)`. Tests use a recording fake. The trait is private, so `GhForge`'s public API does not move.

The WSL scripts and their parsers go in a new module, `crates/alacritree_gh/src/distro.rs`. `graphql::body` becomes `pub(crate)` so `Gh::request` can reuse it.

### Burst flow

`pull_requests_with(heads, blocking, transport)` runs these steps in order:

1. **Fill WSL remotes.** Heads with `remotes: None` whose path classifies as `Wsl` are bucketed by distro, preserving input order. Each distro gets one `transport.remotes(distro, &[(linux_path, branch)])` call.
2. **Answer remote-less heads.** A head whose remotes carry `no_remotes: true` is answered `Ok(None)` and leaves the burst: no group, no step, no process.
3. **Group.** `groups_with` keys groups by `(distro: Option<String>, owner, name)`, so a Windows clone and a WSL clone of one repository never share a `gh`. Native groups chunk at `graphql::CHUNK` (100), WSL groups at `distro::CHUNK` (50). Heads that stay ungrouped keep the per-branch path.
4. **Resolve.** Once per group key through the resolve cache.
5. **Query.** `set_steps` names every group, then groups run in parallel.

`blocking.cancelled()` is checked before each distro's remotes read and before each resolve, as well as before each group. A cancelled burst returns the answers it already holds, which step 2 may have produced, and starts no further process. Today's resolve loop has no such check. That gap is fixed along the way.

### WSL remotes read

`Gh::remotes` runs one `wsl::run_batch` script in the distro, with the pairs flattened into `$@` as `path branch path branch ...`. For each pair the script runs:

```sh
if cd "$p" 2>/dev/null && git rev-parse --git-dir >/dev/null 2>&1; then
  if [ -z "$(git remote)" ]; then
    printf '2\t\t\n'
  else
    o=$(git config --get remote.origin.url)
    r=origin
    for k in "branch.$b.pushRemote" remote.pushDefault "branch.$b.remote"; do
      v=$(git config --get "$k") || continue
      case "$v" in ''|.) continue ;; esac
      r=$v
      break
    done
    u=$(git config --get "remote.$r.url")
    printf '1\t%s\t%s\n' "$o" "$u"
  fi
else
  printf '0\t\t\n'
fi
```

The push-remote loop matches `alacritree_git::remotes::push_remote`: it walks the keys in git's order and skips a missing, empty or `.` value, so `branch.x.pushRemote = .` with `remote.pushDefault = fork` picks `fork`. The existing per-branch script in `query_gh` uses an `||` chain that stops at the first key that exists and picks `origin` there. It takes the same loop, so all three readers agree.

The loop lives once in `distro.rs` as a shell fragment that both scripts splice in. That keeps it a single source of truth.

`distro::parse_remotes(stdout, n)` reads one record per line:

- `2` yields `Some(Remotes { no_remotes: true, .. })`.
- `1` yields `Some(Remotes)` with an empty field read as `None` and `no_remotes: false`.
- `0` yields `None`, which leaves that head on the per-branch path.
- A record count other than `n`, a status field outside `0`, `1` and `2`, or a `run_batch` error makes the whole call `None`. Every head in that distro keeps `remotes: None`, which is today's behavior.

### WSL request and resolve

`Gh::request` for a WSL `cwd` runs `printf '%s' "$2" | "$1" api graphql --input -` with args `[gh, body]`, where `gh` comes from `tools::wsl_in_job` and `body` is `graphql::body(query)`. `run_batch` has no stdin, so the body rides in argv. The one-shot `wsl.exe` fallback caps argv at 32,767 characters. With Rust's Windows quoting, 50 aliases come to about 12k characters and 100 to about 24k before long branch names, which is why WSL groups chunk at 50.

`run_batch` returns stdout whenever there is any, whatever `gh`'s exit status, on both the one-shot and the helper path. `gh api graphql` exits 1 on a GraphQL error and still prints the JSON body, so a failed request reaches `graphql::parse`. `parse` returns `None` for a null `repository` or all-null aliases beside `errors`, and the group sweeps per-branch. Only a `run_batch` error answers `None` directly: `gh` missing, the distro refused, or helper `NoReply`.

The native path returns `None` on any non-zero exit, so a partial answer carrying `errors` sweeps per-branch natively and is kept on WSL. That divergence is accepted: `graphql.rs` already holds that keeping the aliases that answered beats losing the project. A comment on `Gh::request` says so.

`Gh::resolve` for a WSL `cwd` runs `cd "$1" && exec "$2" repo view --json nameWithOwner` and parses with the existing `parse_name_with_owner`.

### Per-branch WSL guard

The per-branch WSL script in `query_gh` answers `\n[]` and exits after its `cd` when `git rev-parse --git-dir` succeeds and `git remote` prints nothing. It only runs when the remotes read failed for that head, and it makes a remote-less repository read as "no PR" instead of `Malformed`. A folder git cannot read still runs `gh` and reports its failure, since an unreadable checkout is no evidence of a missing remote.

### Resolve cache

`ResolveCache` wraps `Mutex<HashMap<(Option<String>, (String, String)), (String, String)>>`, keyed by `(distro, origin slug)`. The origin slug is the key `groups_with` already groups on, so two spellings of one remote share an entry. Only successful resolves are stored. A `None` from missing auth or no network is retried on the next burst. Entries live until the app exits, so a `gh repo set-default` change takes effect after a restart.

`Cached<'a, T: Transport> { cache: &'a ResolveCache, inner: &'a T }` is a `Transport` decorator. Its `resolve` consults the cache and delegates everything else. `groups_with` passes the group's origin slug alongside its `cwd`. `Gh::resolve` ignores the slug, and `Cached` keys on it. Production holds one `static ResolveCache` and runs `pull_requests_with(heads, blocking, &Cached::new(&CACHE, &Gh { blocking }))`. Tests build their own `ResolveCache`, so no two tests share entries.

### Per-step timing

`jobs::Progress` gains `started: HashMap<String, Instant>`. `Blocking::step_started(label)` records the step's start. `step_done` measures `took` from that start when present and removes the entry, and otherwise measures from the shared `mark`, so sequential callers (`FakeForge`, existing tests) are unchanged. `set_steps` clears `started`. `Step`'s public fields do not change. The doc on `Step::took` becomes "From the step's own start, or else from the previous step's end."

### Parallel groups

`alacritree_forge` exports `pub const PARALLEL_GROUPS: usize = 4`. It lives in the trait crate because the app's estimate reads it and the app does not name backend crates for constants.

`pull_requests_with` runs groups in `std::thread::scope` with `min(PARALLEL_GROUPS, groups.len())` workers. GitHub's secondary rate limits penalize concurrent requests, which is why the cap is small. Workers pull from a `Mutex<vec::IntoIter<Group>>`. Each worker checks `blocking.cancelled()` before taking a group, then calls `step_started`, `query_group` and `step_done`, and merges its answers into a shared `Mutex<PullRequests>`. No lock is held across another.

Scoped threads do not inherit the pool worker's thread priority, which `jobs::worker` sets per thread through `lower_this_thread`. `Blocking` gains a private `background: bool`, set where the worker builds it, and `false` from `on_this_thread`. A new `Blocking::adopt_priority(&self)` calls `lower_this_thread(self.background)`, and each scoped worker calls it first. A background burst then stays below the git status work sharing the machine, whichever thread runs it.

`effective_cap`'s doc in `pr_status.rs` gains a sentence saying one burst job can run up to `PARALLEL_GROUPS` `gh` processes.

### Status row estimate

`activity.rs::estimate` sums the last timings of the pending steps. Its doc says that holds because groups run in sequence. It becomes `max(ceil(sum / PARALLEL_GROUPS), longest pending step)`, and the doc says why: up to `PARALLEL_GROUPS` steps run at once, and no burst finishes before its slowest step.

## Error handling

Every new failure path falls back to what the code does today:

| Failure | Result |
|---|---|
| Native repository fails to open (reftable, `safe.directory`) | `no_remotes: false`, per-branch as today |
| Clone whose only remote is not `origin` | `no_remotes: false`, per-branch as today |
| Remotes read errors or returns a malformed record set | Heads in that distro keep `remotes: None` and go per-branch |
| Helper `NoReply` on the remotes read | As above, after the helper's timeout. The helper then cools down, and each per-branch call pays a one-shot `wsl.exe`, which is today's cost |
| One head's `cd` or git read fails | That head goes per-branch |
| WSL GraphQL answer with a null repository or all aliases failed | `graphql::parse` returns `None`, the group sweeps per-branch |
| WSL GraphQL `run_batch` error | The group sweeps per-branch |
| `gh` missing inside the distro | Resolve fails, the group goes per-branch, every head fails `Wsl { Refused }` as today |
| WSL resolve fails | The group has no slug and goes per-branch. Nothing is cached |
| Cancel mid-burst | No further remotes read, resolve or group starts, and the answers already in hand are returned. A process already running finishes, since neither `run_batch` nor `gh` registers a child to kill |

## Testing

TDD through `pull_requests_with` with a fake `Transport`. Test 1 goes RED first, since it is the reported bug.

Tests 1 to 4 build WSL heads, which `wsl::classify` recognizes only from a UNC prefix that `std::path` parses on Windows. They are `#[cfg(windows)]`, like the existing classify tests, and CI runs them in its Windows job.

1. Three WSL heads in one distro sharing an origin cost one `remotes`, one `resolve` and one `request` call, and no `per_branch` call.
2. A head with `no_remotes: true`, native or from a WSL `2` record, answers `Ok(None)`, reports no step, and reaches no transport method past the remotes read. A native head with `Remotes::default()` still goes per-branch.
3. An `ok=0` record, a record-count mismatch, and a failed remotes read each leave the affected heads on `per_branch`.
4. WSL groups chunk at 50 and native groups at 100. A Windows head and a WSL head with the same origin form two groups.
5. Through `Cached` with a test-local `ResolveCache`, two bursts over one origin call `resolve` once, and a failed resolve is not cached.
6. Two groups whose `request` each signals arrival and waits, with a bounded timeout, for the other's signal both finish, which only happens when they run at once. The wait fails the test on timeout instead of hanging. Each step's `took` covers its own request.
7. `jobs`: `step_started` then `step_done` times from the step's own start. A step without `step_started` keeps the sequential timing. `set_steps` clears stale starts.
8. `distro::parse_remotes` unit tests cover empty fields, a `2` record, a trailing newline, a short count and a bad status field.
9. `alacritree_git::remotes`: a repository with no remotes sets `no_remotes`, one with only `upstream` does not, and an unopenable path does not.
10. `activity.rs::estimate`: eight pending 2 s steps estimate 4 s, and one 10 s step among short ones estimates at least 10 s.

11. The remotes script and the push-remote fragment run under a real `sh` against temporary git repositories, `#[cfg(unix)]` so ubuntu CI runs them. The cases: no remotes prints `2`, an unopenable path prints `0`, `pushRemote = .` with `pushDefault = fork` picks `fork`, and a branch with no push settings picks `origin`. This runs the exact script text WSL receives without needing a distro.
12. Cancel: a burst cancelled before it starts makes no `remotes` or `resolve` call. This extends `a_lookup_stops_between_groups_once_cancelled`.
13. `jobs`: `adopt_priority` on a background job's `Blocking` lowers the calling thread. `#[cfg(windows)]`, reading `GetThreadPriority`, if `lower_this_thread` is observable there. Otherwise the test is skipped and the plan says so.

## Commits

1. `refactor(gh): inject the gh transport as a trait`
2. `fix(gh): read the WSL push remote in git's order`
3. `feat(vcs): report a repository with no remotes`
4. `perf(gh): batch pull request lookups for WSL checkouts`
5. `fix(gh): skip the lookup for checkouts with no remote`
6. `perf(gh): cache the resolved repository per origin`
7. `fix(gh): stop a cancelled burst before each process`
8. `feat(jobs): time steps and set priority on any thread`
9. `perf(gh): run a burst's groups in parallel`, including the estimate change in `activity.rs`
