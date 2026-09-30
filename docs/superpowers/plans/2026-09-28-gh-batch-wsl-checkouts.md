# Batch WSL checkouts in the PR refresh: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A PR refresh batches WSL checkouts per repository, skips repositories with no remotes, caches the repository resolve, and runs a burst's groups in parallel.

**Architecture:** `alacritree_gh` reads WSL remotes itself, one `run_batch` script per distro, then groups WSL heads like native ones and sends GraphQL through the distro's `gh`. A private `Transport` trait is the seam every test drives. `alacritree_vcs::Remotes` gains `no_remotes` so a lookup is skipped only on evidence. `jobs::Blocking` gains per-step start marks and priority adoption so scoped worker threads time and schedule correctly.

**Tech Stack:** Rust 2024 workspace, `git2`, `serde_json`, `std::thread::scope`, `windows-sys`, nextest.

**Spec:** `C:\Users\Lev\Git\github\alacritree\docs\superpowers\specs\2026-09-28-gh-batch-wsl-checkouts-design.md`. Read it before starting any task. It holds the reasoning this plan does not repeat.

**Worktree:** `C:\Users\Lev\Git\github\alacritree-worktrees\374-perf-gh-batch-wsl-checkouts-in-the-pr`. Every path below is relative to it.

## Global constraints

- No config option. The feature ships on.
- `VersionControl` and `RemoteForge` trait signatures do not change.
- `PARALLEL_GROUPS = 4`, exported from `alacritree_forge`.
- WSL GraphQL chunk: `distro::CHUNK = 50`. Native stays `graphql::CHUNK = 100`.
- Remotes record status: `0` unreadable, `1` has remotes, `2` opened with no remotes.
- Every new failure path falls back to today's per-branch lookup (spec, "Error handling").
- Errors stay `thiserror` types. No new error variants are needed.
- Comments follow `AGENTS.md` and `AGENTS.local.md`: why, not what, no em dashes, no change narration. Keep module `//!` headers accurate.
- Claim each file before editing: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`.
- Targeted test runs use `cargo nextest run --locked -p <crate> <filter>` (the same argv as `devrun task test`, plus a filter). Full runs use `devrun task test`, `devrun task clippy` and `devrun task fmt`.
- Commit with `devrun task commit --arg commit_subject='<subject>' --arg files=<a>,<b> --arg coauthors='Claude Opus 5.5 <noreply@anthropic.com>'`, plus `--arg commit_body=...` when the change needs context.
- Tests that build WSL heads use paths like `\\wsl.localhost\Ubuntu\home\me\r\a` and are `#[cfg(windows)]`, since only Windows `std::path` parses the UNC prefix `wsl::classify` needs. Tests that run a script under a real `sh` are `#[cfg(unix)]`.

## Review focus

Failure modes the spec implies that no spec test pins. Each gets a test in the task named.

1. A GraphQL body with quotes and backslashes (from branch names) must reach `gh` byte for byte through the WSL request script's `printf '%s' "$2" |`. Task 4.
2. A WSL checkout path containing a space must still produce a `1` or `2` record, not `0`. Task 4.
3. A mixed burst (native, WSL, remote-less, unreadable) must answer every input head exactly once, with no head dropped or duplicated. Task 5.
4. The per-branch WSL script in a remote-less repository must print `\n[]` without ever running `gh`. Task 5.
5. A WSL head with an origin but no push settings must get `push_url` equal to origin's URL, as native does. Task 4.

---

### Task 1: Inject the gh transport as a trait

A pure refactor. Behavior is unchanged, and the existing tests are the proof.

**Files:**
- Modify: `crates/alacritree_gh/src/lib.rs`

**Interfaces:**
- Produces:
  - `trait Transport: Sync` (private) with
    - `fn resolve(&self, cwd: &Path, origin: &(String, String)) -> Option<(String, String)>`
    - `fn request(&self, cwd: &Path, query: &str) -> Option<Vec<u8>>`
    - `fn per_branch(&self, head: &Head, owner: Option<&str>) -> Result<Option<PrInfo>, ForgeError>`
  - `struct Gh<'a> { blocking: &'a Blocking }` implementing it with today's `resolve_repo`, `graphql::run` and `query_gh`. `Gh::resolve` ignores `origin`.
  - `fn pull_requests_with(heads: Vec<Head>, blocking: &Blocking, transport: &impl Transport) -> PullRequests`
  - `fn groups_with(due: Vec<Head>, resolve: impl Fn(&Path, &(String, String)) -> Option<(String, String)>) -> Vec<Group>`. The second argument is the origin slug the group was keyed on.
  - `query_group(group, transport: &impl Transport) -> PullRequests`
  - In `mod tests`: `struct Fns<R, Q, P> { resolve: R, request: Q, per_branch: P }`, implementing `Transport` by calling its closures (each `Fn(..) + Sync`). Task 4 adds a `remotes` field to it.

- [ ] **Step 1: Change the signatures above and migrate every existing test to `Fns` and the two-argument resolve closure (`|_| ...` becomes `|_, _| ...`).**
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: every test that passed before passes, and none is removed.
- [ ] **Step 3: Run `devrun task clippy`.** Expected: clean.
- [ ] **Step 4: Commit** `refactor(gh): inject the gh transport as a trait` with `crates/alacritree_gh/src/lib.rs`.

### Task 2: Read the WSL push remote in git's order

**Files:**
- Create: `crates/alacritree_gh/src/distro.rs` (module header: WSL-side scripts and their parsers for the gh forge)
- Modify: `crates/alacritree_gh/src/lib.rs` (`mod distro;`, and the per-branch script in `query_gh`)

**Interfaces:**
- Produces: `distro::PUSH_REMOTE: &str`, a shell fragment. It expects `$b` to hold the branch and the working directory to be the checkout, and it leaves the chosen remote name in `$r`. Body, as in the spec:
  ```sh
  r=origin
  for k in "branch.$b.pushRemote" remote.pushDefault "branch.$b.remote"; do
    v=$(git config --get "$k") || continue
    case "$v" in ''|.) continue ;; esac
    r=$v
    break
  done
  ```
- Produces: `distro::per_branch_script() -> String`, the `query_gh` WSL script with the fragment spliced in after `cd "$1" || exit 1` and `b=$3`. `$1..$5` keep today's meaning.

- [ ] **Step 1: Write the failing tests** in `distro.rs`, `#[cfg(unix)]`. Each one creates a temporary repository with the `git` CLI (`git init`, `git remote add`, `git config`), then runs `sh -c 'cd "$1" && b=$2 && <PUSH_REMOTE> && printf %s "$r"' sh <repo> <branch>`:
  - `push_remote_skips_a_dot_and_takes_push_default`: `branch.main.pushRemote = .`, `remote.pushDefault = fork`. Asserts `fork`.
  - `push_remote_prefers_the_branch_push_remote`: `branch.main.pushRemote = fork`, `remote.pushDefault = other`. Asserts `fork`.
  - `push_remote_defaults_to_origin`: no settings. Asserts `origin`.
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh push_remote`.** Expected: FAIL to compile, `PUSH_REMOTE` not found. On Windows these tests are compiled out, so this RED shows only on unix or in CI.
- [ ] **Step 3: Add `PUSH_REMOTE` and `per_branch_script()`, and have `query_gh` use `per_branch_script()`.**
- [ ] **Step 4: Run the filter again.** Expected: PASS (unix). Then run `cargo nextest run --locked -p alacritree_gh`. Expected: PASS.
- [ ] **Step 5: Commit** `fix(gh): read the WSL push remote in git's order` with both files. Body: the `||` chain stopped at the first key that existed, so `pushRemote = .` read as `origin` where git and the native reader take `remote.pushDefault`.

### Task 3: Report a repository with no remotes

**Files:**
- Modify: `crates/alacritree_vcs/src/model.rs:227-233` (`Remotes`)
- Modify: `crates/alacritree_git/src/remotes.rs` (`remotes()` and its tests)
- Modify: struct literals of `Remotes` in `alacritree/src/pr_status.rs` tests and `crates/alacritree_gh/src/lib.rs` tests (`..Default::default()`)

**Interfaces:**
- Produces: `pub no_remotes: bool` on `alacritree_vcs::Remotes`. Doc: "The repository opened and lists no remotes at all. `false` when it could not be read, so a caller skips a lookup only on evidence."

- [ ] **Step 1: Write the failing tests** in `remotes.rs`:
  - `a_repository_without_remotes_says_so`: `init_repo`, no remotes. Asserts `no_remotes == true`, both URLs `None`.
  - `a_clone_with_only_a_non_origin_remote_is_not_remote_less`: `add_remote(repo, "upstream", ...)`. Asserts `no_remotes == false`, both URLs `None`.
  - `an_unreadable_path_is_not_remote_less`: a temporary directory that is not a repository. Asserts `remotes(...) == Remotes::default()`.
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_git remote`.** Expected: FAIL to compile, no field `no_remotes`.
- [ ] **Step 3: Add the field, and set it in `remotes()` from `repo.remotes()` being empty after a successful open.** Fix the struct literals the compiler names.
- [ ] **Step 4: Run `cargo nextest run --locked -p alacritree_git remote`, then `devrun task check`.** Expected: PASS, and the workspace builds.
- [ ] **Step 5: Commit** `feat(vcs): report a repository with no remotes` with the changed files.

### Task 4: Batch pull request lookups for WSL checkouts

**Files:**
- Modify: `crates/alacritree_gh/src/distro.rs`
- Modify: `crates/alacritree_gh/src/lib.rs` (`Transport`, `Gh`, `pull_requests_with`, `groups_with`, `Group`)
- Modify: `crates/alacritree_gh/src/graphql.rs` (`body` becomes `pub(crate)`; the `run` doc no longer says WSL never reaches it through a slug)

**Interfaces:**
- Consumes: `distro::PUSH_REMOTE` (Task 2), `Remotes::no_remotes` (Task 3).
- Produces:
  - `Transport::remotes(&self, distro: &str, heads: &[(&str, &str)]) -> Option<Vec<Option<Remotes>>>`, with the pairs being `(linux_path, branch)`.
  - `distro::CHUNK: usize = 50`
  - `distro::REMOTES_SCRIPT` (or a `fn remotes_script() -> String` if splicing needs it): the spec's script, taking `$@` as `path branch ...` pairs and printing one `status\torigin\tpush\n` record per pair.
  - `distro::parse_remotes(stdout: &[u8], n: usize) -> Option<Vec<Option<Remotes>>>`, with the spec's rules for `0`, `1`, `2`, count mismatch and bad status.
  - `distro::request(distro: &str, gh: &str, body: &str, blocking: &Blocking) -> Option<Vec<u8>>`, which runs `printf '%s' "$2" | "$1" api graphql --input -`. `Ok(stdout)` becomes `Some`, a `BatchError` becomes `None`. Its doc states the partial-answer divergence from native (spec, "WSL request and resolve").
  - `distro::resolve(distro: &str, gh: &str, linux_path: &str, blocking: &Blocking) -> Option<(String, String)>`, which runs `cd "$1" && exec "$2" repo view --json nameWithOwner` and parses with `parse_name_with_owner`.
  - `Group` gains `distro: Option<String>`. The `by_repo` key becomes `(Option<String>, String, String)`. Chunking uses `distro::CHUNK` when `distro.is_some()`.
  - `Fns` in tests gains a `remotes` field.

- [ ] **Step 1: Write the failing tests.**
  - `lib.rs`, `#[cfg(windows)]`: `wsl_heads_sharing_an_origin_share_one_request`. Three `\\wsl.localhost\Ubuntu\home\me\r\{a,b,c}` heads with `remotes: None`. The fake `remotes` returns three `1` records with origin `https://github.com/o/r.git`. Asserts one `remotes` call (distro `Ubuntu`, paths `/home/me/r/a`...), one `resolve`, one `request`, and zero `per_branch`.
  - `lib.rs`, `#[cfg(windows)]`: `an_unreadable_wsl_record_goes_per_branch`, `a_short_record_set_sends_the_distro_per_branch`, `a_failed_remotes_read_sends_the_distro_per_branch`. Each asserts `per_branch` ran for the affected heads only.
  - `lib.rs`, `#[cfg(windows)]`: `wsl_groups_chunk_at_fifty`: 51 WSL heads, one origin, 2 groups. `a_windows_and_a_wsl_clone_of_one_repo_group_apart`: 2 groups.
  - `distro.rs`, platform-free: `parse_remotes` with empty fields, a `2` record (`no_remotes: true`), a trailing newline, a short count (`None`), and a status `7` (`None`).
  - `distro.rs`, `#[cfg(unix)]`, run under `sh` against temporary repositories: `the_remotes_script_reports_each_checkout` (repo with origin, repo with none (`2`), a non-repository (`0`), in that order) and `a_checkout_without_push_settings_pushes_to_origin` (push field equals the origin URL, Review focus 5), using a directory name with a space for one repo (Review focus 2).
  - `distro.rs`, `#[cfg(unix)]`: `the_request_script_pipes_the_body_verbatim`. `$1` is a stub script that `cat`s stdin to a file. The body holds `"` and `\`. Asserts the file equals the body byte for byte (Review focus 1).
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: FAIL to compile on the missing items. On Windows, once they exist as stubs, `wsl_heads_sharing_an_origin_share_one_request` fails with 3 `per_branch` calls, which is the reported bug.
- [ ] **Step 3: Implement the interfaces.** In `pull_requests_with`, before grouping: bucket `remotes: None` WSL heads by distro in input order, call `transport.remotes`, and assign the results back. `Gh::remotes` calls `wsl::run_batch` with `REMOTES_SCRIPT`, then `parse_remotes`. `Gh::request` and `Gh::resolve` dispatch on `wsl::classify(cwd)`, getting `gh` from `tools::wsl_in_job(Tool::Gh, &distro, blocking)` on the WSL side.
- [ ] **Step 4: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: PASS. Then run `devrun task clippy`.
- [ ] **Step 5: Commit** `perf(gh): batch pull request lookups for WSL checkouts` with `distro.rs`, `lib.rs` and `graphql.rs`. Body: WSL heads arrived without remotes and each cost a serial `wsl.exe` plus `gh pr list`, so a 53-worktree project made 53 calls. They now cost one remotes read per distro and one GraphQL request per 50 branches.

### Task 5: Skip the lookup for checkouts with no remote

**Files:**
- Modify: `crates/alacritree_gh/src/lib.rs` (`pull_requests_with`)
- Modify: `crates/alacritree_gh/src/distro.rs` (`per_branch_script`)

**Interfaces:**
- Consumes: `Remotes::no_remotes` (Task 3), the record `2` path (Task 4).

- [ ] **Step 1: Write the failing tests.**
  - `a_remote_less_head_is_answered_without_a_lookup` (platform-free): a native head with `Remotes { no_remotes: true, .. }`. Under `jobs::recorded`, asserts `Ok(None)` for it, no steps declared, and a `Fns` whose `resolve`, `request` and `per_branch` all panic.
  - `an_unread_head_still_looks_up` (platform-free): `Remotes::default()`. Asserts `per_branch` ran once.
  - `#[cfg(windows)]` `every_head_in_a_mixed_burst_is_answered_once`: one native grouped head, one WSL grouped head, one native remote-less head, one native `Remotes::default()` head. Asserts the answer's keys equal the input paths exactly (Review focus 3).
  - `distro.rs`, `#[cfg(unix)]`: `the_per_branch_script_answers_no_pr_without_remotes`. Runs `per_branch_script()` under `sh` in a repository with no remotes, with `$2` set to a stub that writes a marker file. Asserts stdout is `\n[]` and the marker file does not exist (Review focus 4).
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: `a_remote_less_head_is_answered_without_a_lookup` fails on the `per_branch` panic, and the script test fails on stdout.
- [ ] **Step 3: Implement.** After the remotes fill, move heads with `no_remotes` into the output as `Ok(None)` before grouping. Add `[ -n "$(git remote)" ] || { printf '\n[]'; exit 0; }` to `per_branch_script` after the `cd`.
- [ ] **Step 4: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: PASS.
- [ ] **Step 5: Commit** `fix(gh): skip the lookup for checkouts with no remote`.

### Task 6: Cache the resolved repository per origin

**Files:**
- Modify: `crates/alacritree_gh/src/lib.rs`

**Interfaces:**
- Produces:
  - `struct ResolveCache(Mutex<HashMap<(Option<String>, (String, String)), (String, String)>>)` with `const fn new() -> Self`
  - `struct Cached<'a, T: Transport> { cache: &'a ResolveCache, inner: &'a T }`, implementing `Transport`. Its `resolve` keys on `(distro of cwd, origin)`, stores only `Some`, and delegates everything else.
  - `static RESOLVED: ResolveCache`. `GhForge::pull_requests` passes `&Cached { cache: &RESOLVED, inner: &Gh { blocking } }`.

- [ ] **Step 1: Write the failing tests**, each with its own `ResolveCache::new()`:
  - `a_second_burst_reuses_the_resolved_repository`: two `pull_requests_with` calls over one native origin through `Cached`. Asserts the inner `resolve` count is 1.
  - `a_failed_resolve_is_asked_again`: the inner `resolve` returns `None`. After two bursts the count is 2.
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh resolve`.** Expected: FAIL to compile, `ResolveCache` not found.
- [ ] **Step 3: Implement the interfaces.** Use `wsl::classify(cwd)` for the distro half of the key.
- [ ] **Step 4: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: PASS.
- [ ] **Step 5: Commit** `perf(gh): cache the resolved repository per origin`.

### Task 7: Stop a cancelled burst before each process

**Files:**
- Modify: `crates/alacritree_gh/src/lib.rs` (`pull_requests_with`, `groups_with`)

**Interfaces:**
- Changes: `groups_with(due, blocking: &Blocking, resolve) -> Option<Vec<Group>>`, returning `None` when `blocking.cancelled()` turns true before a resolve. `pull_requests_with` then returns the answers it already holds, including Task 5's remote-less ones.

- [ ] **Step 1: Write the failing tests**, using the pool-and-drop pattern of `a_lookup_stops_between_groups_once_cancelled`:
  - `a_cancelled_burst_resolves_nothing` (platform-free): native heads with a GitHub origin. Asserts the `resolve` count is 0 and the result is empty.
  - `#[cfg(windows)]` `a_cancelled_burst_reads_no_remotes`: WSL heads. Asserts the `remotes` count is 0.
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh cancelled`.** Expected: `a_cancelled_burst_resolves_nothing` fails with count 1.
- [ ] **Step 3: Check `blocking.cancelled()` before each distro's remotes read and before each resolve.**
- [ ] **Step 4: Run `cargo nextest run --locked -p alacritree_gh`.** Expected: PASS, including `a_lookup_stops_between_groups_once_cancelled`.
- [ ] **Step 5: Commit** `fix(gh): stop a cancelled burst before each process`.

### Task 8: Time steps and set priority on any thread

**Files:**
- Modify: `crates/alacritree_common/src/jobs.rs` (`Progress`, `Step::took` doc, `Blocking`, `set_steps`, `step_done`, `worker`, `on_this_thread`, `recorded`)

**Interfaces:**
- Produces:
  - `Blocking::step_started(&self, label: &str)`
  - `Blocking::adopt_priority(&self)`, which calls `lower_this_thread(self.background)`
  - A private `background: bool` on `Blocking`, set in `worker` from `matches!(slot, Slot::Background)` and `false` in `on_this_thread` and `recorded`
  - `Progress.started: HashMap<String, Instant>`, cleared by `set_steps`. `step_done` removes the step's entry and times from it when present, otherwise from `mark`.

- [ ] **Step 1: Write the failing tests** in `jobs.rs`:
  - `a_started_step_is_timed_from_its_own_start`: under `recorded`, `set_steps(["a","b"])`, `step_started("a")`, `step_started("b")`, sleep 50 ms, `step_done("a")`, sleep 50 ms, `step_done("b")`. Asserts `b.took >= 100ms`. Timing from the shared mark would give about 50 ms. Only lower bounds are asserted, so a slow machine cannot flake it.
  - `set_steps_forgets_earlier_starts`: `step_started("a")`, sleep 50 ms, `set_steps(["a"])`, `step_done("a")`. Asserts `took < 50ms`.
  - The existing `recorded_returns_what_the_closure_reported` and sequential-timing tests stay unchanged.
  - `#[cfg(windows)]` `adopt_priority_lowers_a_background_thread`: spawn a `Priority::Background` job on `Pool::new(2)` that spawns a `std::thread::scope` thread, which calls `adopt_priority` and returns `GetThreadPriority(GetCurrentThread())`. Asserts it equals `THREAD_PRIORITY_BELOW_NORMAL`. Add the `Win32_System_Threading` feature to the `windows-sys` dependency if `GetThreadPriority` needs it.
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_common jobs`.** Expected: FAIL to compile, no `step_started` or `adopt_priority`.
- [ ] **Step 3: Implement the interfaces.**
- [ ] **Step 4: Run `cargo nextest run --locked -p alacritree_common`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(jobs): time steps and set priority on any thread`.

### Task 9: Run a burst's groups in parallel

**Files:**
- Modify: `crates/alacritree_forge/src/lib.rs` (`pub const PARALLEL_GROUPS: usize = 4`, doc giving the rate-limit reason)
- Modify: `crates/alacritree_gh/src/lib.rs` (`pull_requests_with`)
- Modify: `alacritree/src/activity.rs:197-207` (`estimate` and its doc) and its test `the_estimate_sums_the_pending_steps_last_durations`
- Modify: `alacritree/src/pr_status.rs:24-37` (`effective_cap` doc: one burst job runs up to `PARALLEL_GROUPS` `gh` processes)

**Interfaces:**
- Consumes: `Blocking::step_started`, `Blocking::adopt_priority` (Task 8), and the cancel checks (Task 7).
- Produces: `alacritree_forge::PARALLEL_GROUPS`. `estimate` returns `max(ceil(sum / PARALLEL_GROUPS), longest pending step)`.

- [ ] **Step 1: Write the failing tests.**
  - `alacritree_gh`: `groups_run_at_once`. Two native groups whose `request` sends on a channel and then `recv_timeout(5s)` on the other group's signal, panicking on timeout. Asserts both groups answered. Also `each_step_is_timed_by_its_own_request`: one group's `request` sleeps 200 ms and the other's 20 ms, both starting together. Asserts the long step's `took >= 200ms`. Timing from the shared mark would give about 180 ms, measured from the short step's end.
  - `activity.rs`: rename and update the existing test to `the_estimate_spreads_pending_steps_across_parallel_groups`. Pending steps of 3 s and 4 s give `~4s`, not `~7s`. Add `eight_two_second_steps_estimate_four_seconds` and `the_estimate_never_undercuts_the_slowest_step` (one 10 s step among three 1 s steps gives `~10s`).
- [ ] **Step 2: Run `cargo nextest run --locked -p alacritree_gh groups_run_at_once` and `cargo nextest run --locked -p alacritree estimate`.** Expected: `groups_run_at_once` panics on the timeout, and the estimate tests fail on `~7s`.
- [ ] **Step 3: Implement.** `std::thread::scope` with `min(PARALLEL_GROUPS, groups.len())` workers pulling from `Mutex<vec::IntoIter<Group>>`. Each worker calls `adopt_priority`, then loops: check `cancelled()`, take a group, `step_started`, `query_group`, `step_done`, and extend a `Mutex<PullRequests>`. No lock is held across another. Update the `pull_requests_with` doc and the `GhForge::pull_requests` doc ("ask for each group in turn" is no longer true).
- [ ] **Step 4: Run `devrun task test`, `devrun task clippy` and `devrun task fmt`.** Expected: all pass, and fmt leaves no diff.
- [ ] **Step 5: Commit** `perf(gh): run a burst's groups in parallel` with the four files.

### Final verification

- [ ] `devrun task test`, `devrun task clippy` and `devrun task fmt` are clean on the tip.
- [ ] `git -C <worktree> log --oneline origin/master..` shows the nine commits in order.
- [ ] On Windows with a WSL distro, a manual check: add a WSL project with many worktrees and a remote-less project, then trigger a PR refresh. The status row counts about one step per repository chunk, and the remote-less project reports no failure.
