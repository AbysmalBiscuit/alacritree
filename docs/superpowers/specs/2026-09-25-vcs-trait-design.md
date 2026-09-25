# Version control trait: design for git, jj and Mercurial

Issue: #134. Split out of #133. Builds on `2026-09-23-integration-crates-pilot-design.md`, whose crate model, ambassador dispatch and thiserror rules apply here unchanged.

This spec designs the trait and the refactor that moves today's git code behind it. It does not implement jj or Mercurial. It does record what each of them needs from the trait, checked against jj 0.45.1 and Mercurial 7.2.4, so the trait is shaped for them now rather than reshaped when they arrive.

## Goals

- git code lives in its own crate, and the app cannot reach git except through the trait. The app's `Cargo.toml` loses its `git2` dependency outside `[dev-dependencies]`, which is what makes the compiler enforce it.
- The trait fits jj and Mercurial without git vocabulary leaking into their answers. A jj working-copy commit is never reported as "unstaged".
- The refactor changes no behavior for git users. Every UI string, IPC reply, CLI output, config key and `state.toml` key stays byte-identical.

## Where git leaks today

The issue lists the model's leaks. The inventory for this spec found these on top of them, and corrects one claim.

- `git2` is used in production outside the git modules. `pr_status.rs:768-790` reads `branch.<b>.pushRemote`, `remote.pushDefault` and the `origin` URL through git2, and `worktree.rs:668` prunes through git2.
- `tasks/facts.rs:25-100` shells out to `git worktree list --porcelain` and `rev-parse --show-toplevel` on its own and parses `branch refs/heads/` itself, independent of `projects.rs`.
- `app/modals.rs:1313-1318` decides whether a delete was refused for unsaved work by matching git's English stderr ("contains modified or untracked files, use --force", "is dirty, use --force").
- `app.rs:1102-1108` treats `project.default_branch.is_some()` as "this project is a repository". A git repository whose default branch cannot be detected falls through as a non-repository there. The trait fixes this by giving the project an explicit backend.
- `worktree_liveness.rs` reads `.git` and `HEAD` as plain files every probe interval, which is git's on-disk layout.
- The diff pane runs `git -c core.pager=<pager> diff <args>` (`diff_viewer.rs:47-61`, `app/git_panel.rs:553-616`), and tuicr's built-in templates pass `{base}...HEAD`, a git revspec.
- `Worktree.branch` holds either a branch name or a 7-character OID, and the flag telling them apart is thrown away at `projects.rs:328`. PR lookup and the taskwarrior node read the field without knowing which one they got.
- `validate_branch_name` enforces git-check-ref-format rules.

## The capability question

The issue left three options open. This spec takes the first, optional capabilities, and expresses it as data rather than as a second trait.

- **Lowest common denominator** throws away the Staged section git users have today. That is a regression for Arnaud's workflow, which the project's rules forbid.
- **Git-shaped trait** puts a lie in two of three backends. jj's working copy is a commit and Mercurial commits from the working directory, so "unstaged" is wrong in both, and the wrong word reaches IPC replies.
- **Optional capabilities** keeps git's Staged section and lets the others leave it out.

An optional sub-trait (`trait Staging`) does not fit the dispatch rule. The app dispatches through one `#[derive(Delegate)]` enum, and ambassador has no way to ask whether a variant also implements `Staging`. So the capability lives in the answer. `Status.staged` is `Option<Vec<FileChange>>`, and `None` means the backend has no staging area. The panel draws the Staged section only when it is `Some`. The same rule covers the other gaps. `Checkout.upstream` is already optional, and a backend that cannot read its head cheaply returns `None` from its probe.

There is no `Capabilities` struct. Every place that would read one already holds a status or a checkout record, and a flag that could disagree with the data is one more thing to keep in sync.

## Vocabulary

| Term | git | jj | Mercurial |
|---|---|---|---|
| Checkout | worktree | workspace | share |
| Main checkout | the non-linked worktree | the workspace at the project root | the repository without `.hg/sharedpath` |
| Head | `HEAD` | `@` and its nearest bookmark | `.` and its active bookmark or named branch |
| Name | branch | bookmark | bookmark, else named branch |
| Trunk | default branch (origin/HEAD, well-known names) | `trunk()` | the `default` branch, resolved through a bookmark or remote name |
| Working changes | index vs working tree | `@` vs its parents | `hg status` |
| Staged changes | HEAD vs index | none | none |
| Base diff | `merge-base(base, HEAD)..HEAD` | `fork_point(trunk() \| @)..@` | `ancestor(., base)..` |

"Checkout" is the word the checkout hooks crate already uses. The hooks crate's event struct `Checkout<'a>` is renamed `CheckoutEvent<'a>` in this refactor so the record type below can take the name. The rename is mechanical but reaches the hooks crate, every hook implementation (`CommandHook`, `FakeHook`, `DopplerHook`) and the app's create, delete and first-open call sites.

User-facing strings keep "worktree" for git. A backend that needs its own noun ("workspace") gets it when that backend lands, not now.

## Crate graph

```
alacritree (app)
 ├─ alacritree_git
 │   └─ alacritree_vcs
 │       └─ alacritree_common
 ├─ alacritree_vcs
 ├─ alacritree_checkout_hooks
 └─ alacritree_common
```

- `alacritree_vcs` is the integration-type crate. It holds the trait, the models, `VcsError` and `FakeVcs` behind `test-support`.
- `alacritree_git` is the backend crate. It holds everything that knows git exists, both transports (git2 natively, batched `sh` in a WSL distro), and `RawGit`.
- Later, `alacritree_jj` and `alacritree_hg` sit next to it.

## alacritree_vcs

### Models

```rust
/// One repository as discovery found it.
pub struct Repository {
    /// Display name of the trunk, e.g. `main`. `None` when nothing resolves.
    pub trunk: Option<String>,
    /// The main checkout first.
    pub checkouts: Vec<Checkout>,
    /// The distro's `$HOME` for a WSL repository. Rides the discovery round
    /// trip because a second wsl.exe call would cost ~400 ms.
    pub home: Option<String>,
}

pub struct Checkout {
    /// What the backend accepts back to remove this checkout: a git
    /// worktree's admin name, a jj workspace name. `main` for the main one.
    pub name: String,
    pub path: PathBuf,
    pub head: Head,
    pub is_main: bool,
    /// The directory is gone but the backend still records the checkout.
    pub gone: bool,
    pub upstream: Option<Upstream>,
}

pub struct Head {
    /// The branch or bookmark, when one applies. What a PR's head ref and
    /// the taskwarrior node use.
    pub name: Option<String>,
    /// A short revision id: 7 hex digits of a git OID, a jj change id.
    pub revision: Option<String>,
}

impl Head {
    /// `name`, else `revision`. What the sidebar, `$branch` and the IPC
    /// `branch` field show, matching today's single field.
    pub fn label(&self) -> Option<&str>;
}

/// Moved unchanged from `upstream.rs`.
pub enum Upstream {
    Level { upstream: String },
    Diverged { upstream: String, ahead: usize, behind: usize },
    Gone { upstream: String },
    Untracked,
}

pub struct Status {
    pub head: Head,
    pub trunk: Option<String>,
    /// The revision the base diff ran against, e.g. `refs/remotes/origin/main`.
    /// `None` when no base resolved and `base_diff` is empty.
    pub base: Option<String>,
    /// `None`: the backend has no staging area.
    pub staged: Option<Vec<FileChange>>,
    pub working: Vec<FileChange>,
    pub base_diff: Vec<DiffStat>,
}

pub struct FileChange { pub path: String, pub kind: ChangeKind }
pub enum ChangeKind { Added, Modified, Deleted, Renamed, Untracked, Conflicted }
pub struct DiffStat { pub path: String, pub additions: usize, pub deletions: usize }

/// What removing a checkout would discard. `staged` is 0 for a backend with
/// no staging area.
pub struct Dirty { pub staged: usize, pub modified: usize, pub untracked: usize }

pub enum Liveness { Present, Missing, Unknown }

/// One filesystem probe: no process, no library open.
pub struct Probe {
    pub liveness: Liveness,
    /// An opaque token that changes when the head moves, e.g. the contents
    /// of git's `HEAD`. `None` when the backend cannot read it cheaply;
    /// the sidebar then relies on periodic discovery.
    pub head: Option<String>,
}

/// Which checkout and head a directory belongs to, for `alacritree hook`.
pub struct Located { pub main: PathBuf, pub checkout: PathBuf, pub head: Head }

pub struct CreateCheckout {
    pub main: PathBuf,
    /// Chosen by the app from `[workspace]`; the backend creates it.
    pub target: PathBuf,
    /// The new branch or bookmark.
    pub name: String,
    /// A cached trunk detection, used when the live query cannot answer.
    pub trunk_hint: Option<String>,
}

pub struct RemoveCheckout {
    pub main: PathBuf,
    pub checkout: Checkout,
    /// Remove even with unsaved work. Ignored for a gone checkout.
    pub force: bool,
    /// Also delete `checkout.head.name`.
    pub delete_name: bool,
}

pub enum DiffScope { Staged, Working, Base { base: String } }

pub struct DiffTarget {
    pub scope: DiffScope,
    /// One file, or the whole scope when `None`.
    pub file: Option<String>,
    /// The file is untracked, which git diffs against `/dev/null`.
    pub untracked: bool,
}

/// Where a head's name is pushed, for the forge's PR lookup.
pub struct PushTarget { pub remote_url: String, pub name: String }
```

`Worktree.prunable` becomes `Checkout.gone`. The meaning is unchanged: the directory is gone but the backend still lists it.

`ChangeKind::glyph` and `label` stay in the app. They are how alacritree draws a change, and Mercurial's own `!` means "missing", which alacritree draws as `D`.

### The trait

```rust
#[ambassador::delegatable_trait]
pub trait VersionControl {
    /// Whether `root` is this backend's repository: a marker check at the
    /// root (`.git`, `.jj`, `.hg`). Never walks upward, matching today's
    /// `Repository::open` rather than `discover`.
    fn claims(&self, root: &Path) -> bool;

    fn discover(&self, root: &Path, upstream: bool, blocking: &Blocking)
        -> Result<Repository, VcsError>;

    /// Must not record history. A backend whose every command writes (jj
    /// snapshots the working copy) reads without snapshotting here.
    fn status(&self, checkout: &Path, base_hint: Option<&str>, blocking: &Blocking)
        -> Result<Status, VcsError>;

    /// Cheaper than `status`: no base diff.
    fn dirty(&self, checkout: &Path, blocking: &Blocking) -> Result<Dirty, VcsError>;

    /// Runs on the probe worker for rows the sidebar draws.
    fn probe(&self, checkout: &Path) -> Probe;

    fn locate(&self, dir: &Path, blocking: &Blocking) -> Option<Located>;

    /// Names the base picker offers, locals first.
    fn names(&self, checkout: &Path, blocking: &Blocking) -> Result<Vec<String>, VcsError>;

    fn validate_name(&self, name: &str) -> Result<(), VcsError>;

    /// Resolves the base, fetches it, creates `req.target` on a new name.
    /// Reports each step as it starts.
    fn create_checkout(&self, req: &CreateCheckout, on_step: &mut dyn FnMut(&str), blocking: &Blocking)
        -> Result<(), VcsError>;

    /// A live checkout is removed; a gone one is forgotten.
    fn remove_checkout(&self, req: &RemoveCheckout, blocking: &Blocking) -> Result<(), VcsError>;

    /// Arguments after the program that print `target` as a unified diff,
    /// with `pager` wired in when given.
    fn diff_args(&self, target: &DiffTarget, pager: Option<&str>) -> Vec<String>;

    fn push_target(&self, checkout: &Path, name: &str, blocking: &Blocking) -> Option<PushTarget>;
}
```

The real signatures name every type by absolute path and the crate declares `extern crate self as alacritree_vcs;`, as the hooks crate does.

`on_step` is `&mut dyn FnMut` rather than a generic so ambassador has no generic method to forward. `create` in the app already takes `impl FnMut`, and it passes `&mut on_step` through.

The backend value holds its resolved config (program paths), so `Vcs::Git(GitBackend)` is cheap to clone and each `Project` keeps its own.

### Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("could not run {program}")]
    Spawn { program: String, #[source] source: std::io::Error },
    #[error("{command}: {stderr}")]
    Failed { command: String, stderr: String },
    /// Removal refused because it would discard work. Replaces matching
    /// git's stderr in the delete modal.
    #[error("{message}")]
    Unsaved { message: String },
    #[error("no `{remote}` remote configured")]
    NoRemote { remote: String },
    #[error("could not determine base branch (tried: {})", tried.join(", "))]
    NoBase { tried: Vec<String> },
    #[error("{0}")]
    InvalidName(String),
    #[error("{0}")]
    Unreachable(String),
    #[error("{what} cancelled")]
    Cancelled { what: &'static str },
    #[error("{context}")]
    Backend { context: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
}
```

Messages match today's strings where the UI shows them. `Backend` carries a git2 error without `alacritree_vcs` depending on git2.

### FakeVcs

Behind `test-support`, as `FakeHook` is. It returns a scripted `Repository` and `Status`, records create and remove requests, and can be built with `staged: None`. Its main job is the test that no git crate can write, the panel and the delete dialog with a backend that has no staging area.

## alacritree_git

Moves in from `alacritree/src/`, with git2 and the batched scripts both inside.

| From | What moves |
|---|---|
| `default_branch.rs` | all of it, private to the crate |
| `upstream.rs` | the parsers and `map_from_repo`; `Upstream` goes to `alacritree_vcs` |
| `git_status.rs` | `compute`, both transports, the porcelain and numstat parsers, `dirty_counts` |
| `projects.rs` | `from_repo`, `discover_wsl`, `discover_batch`, `parse_worktree_list_z`, `current_branch`, `branch_from_admin_head` |
| `worktree.rs` | `git_command`, `run_git*`, `has_remote`, `list_branches`, `resolve_base_branch`, `query_origin_head`, `validate_branch_name`, the `worktree add/remove`, `branch -D` and prune calls |
| `worktree_liveness.rs` | `probe`, `probe_checkout`, `head_branch` |
| `diff_viewer.rs` | `diff_args` and the `core.pager` wiring |
| `pr_status.rs` | `read_remotes`, `push_remote` |
| `tasks/facts.rs` | the `worktree list` and `rev-parse` parsing |
| `test_util.rs` | `init_repo`, `add_worktree`, behind `test-support` |

`remove_checkout` on a gone checkout does what `prune_worktree` does today, git2's per-worktree prune with default options, so a directory that came back is refused rather than swept.

`RawGit` (`path`, `wsl_path`) moves here and replaces the `raw_tool_table!` line for git, the same move the pilot made for doppler. `Tool::Git` stays in `alacritree_common`'s table.

## App changes

### Dispatch and detection

```rust
#[derive(Debug, Clone, Delegate)]
#[delegate(VersionControl)]
pub(crate) enum Vcs {
    Git(GitBackend),
}

/// Backends in claim order. Built once from config.
pub(crate) fn backends(integrations: &IntegrationsConfig) -> Vec<Vcs>;

/// The first backend that claims `root`, or `None` for a plain directory.
pub(crate) fn detect(backends: &[Vcs], root: &Path) -> Option<Vcs>;
```

`Project` gains `vcs: Option<Vcs>` and renames `default_branch` to `trunk`. `worktrees: Vec<Worktree>` becomes `checkouts: Vec<Checkout>`, and `projects::Worktree` goes away. `Project::discover` becomes `detect` then `vcs.discover`, and a project with `vcs: None` gets today's placeholder checkout. `project_and_worktree` gates on `vcs.is_some()` instead of `default_branch.is_some()`.

### What stays in the app

- `StatusCache`, `LivenessCache` and all polling cadence. They hold the project's `Vcs` and call the trait.
- `Project`'s user state (`label`, `expanded`, `shell_override`) and `Project::apply`.
- `project_json` and every other wire format.
- The worktree create flow around the backend call. That is picking the target path from `[workspace]`, copying LLM configs, the Claude bell, checkout hooks, progress over `mpsc`, `spawn_create` and `spawn_delete`.
- The diff pane's program resolution, WSL wrapping and PTY session. Only the diff arguments come from the backend.
- The PR cache. It asks `push_target` instead of reading git config.

### The git panel

- Staged section only when `status.staged` is `Some`. For git it always is, so nothing changes on screen.
- With `staged: None`, the working section is titled "Changes" instead of "Unstaged", and `ReviewStaged` reports itself unavailable the way a Direct viewer with an empty template already does.
- `GitSection::Unstaged` becomes `Working` internally, and `Branch` becomes `Base`. Row keys (`staged:`, `worktree:`, `branch:`, `section:*`) are compared across frames, so they keep their spellings.
- `DirtyCounts` becomes `vcs::Dirty`. The delete modal matches `VcsError::Unsaved` rather than stderr text.

### Wire formats

No change in this refactor. `git_status` keeps its name and its `branch`, `default_branch`, `staged`, `unstaged` and `diff_vs_default_branch` fields. `list_projects` keeps `default_branch` and `worktrees[].branch`, now filled from `trunk` and `head.label()`. `alacritree git-status` and `alacritree worktree create` keep their names. How these grow for a second backend is an open question below.

### Lints

`git2` moves to `[dev-dependencies]` in `alacritree/Cargo.toml`, which is what stops production code reaching around the trait. The tests that build repositories switch to `alacritree_git`'s `test-support` helpers, and once they have, the dev-dependency goes too.

## What jj and Mercurial need from this

Recorded so each backend's own spec starts from facts. Versions checked were jj 0.45.1 (docs and source at tag v0.45.1) and Mercurial 7.2.4.

Both should be driven through their CLIs with templates. jj-lib's FAQ says it "is not a stable API", it needs Rust 1.89 against this workspace's 1.85, and it does not read user config, so a `trunk()` alias that `jj git clone` writes into `%APPDATA%\jj\repos\<hash>\config.toml` would be invisible to it. Mercurial has no usable Rust library. `hg-core` on crates.io is a 2019 placeholder, and `hg help scripting` recommends `HGPLAIN=1` with the CLI.

### jj

- **Checkouts.** `jj workspace list -T 'name ++ " " ++ root ++ "\n"'` lists every workspace from a central registry. `root` is missing for workspaces made before 0.38 or whose directory moved. `jj workspace forget` leaves the directory on disk, so removal is forget, then delete.
- **Stale workspaces.** A workspace whose `@` another workspace rewrote is stale, and `jj st` there exits 1 until `jj workspace update-stale`. `status` must report this as an error the panel can show, not as an empty tree.
- **Status must not snapshot.** Every jj command, even `jj log`, snapshots the working copy and records an operation when files changed. Polling every 1.5 s would fill the op log. `--ignore-working-copy` avoids it at the cost of a stale view until the user's next jj command. jj's FAQ recommends that flag for watch loops and suggests watching `.jj/repo/op_heads/heads` as the refresh signal, which fits `Probe.head`.
- **Head.** jj has no current bookmark. `name` is the nearest bookmark, `heads(::@ & bookmarks())`, and `revision` is `change_id.shortest(8)`.
- **Base diff.** `jj diff --from 'fork_point(trunk() | @)' --to @`. The shorter `-r 'trunk()..@'` fails with "Cannot diff revsets with gaps" once trunk is merged into the line. Line counts need `--stat` or `--git` parsing, since the diff template has no per-file stats.
- **Upstream.** Tracked remote bookmarks expose `tracking_ahead_count` and `tracking_behind_count`, counted from the remote's side, so local ahead is the remote ref's behind. Colocated repos list a pseudo-remote `@git` to filter out.
- **Colocation** has been jj's default since 0.34. A colocated repo has both `.jj` and `.git`. git sees a detached HEAD at `@-` with `@`'s changes unstaged, and `git worktree list` does not show jj workspaces. A git backend claiming such a repo shows a misleading tree, so claim order matters.

### Mercurial

- **Checkouts.** `share` is bundled but disabled by default. A share records its source in `.hg/sharedpath`, and the source records nothing, so a repository's shares cannot be listed from the repository. Removal is deleting the directory.
- **Status.** `hg status -Tjson`. `!` (missing) maps to `Deleted`, `?` to `Untracked`. An unresolved merge shows files as `M`, and conflicts come from `hg resolve -l -Tjson`.
- **Head.** The active bookmark, else the named branch. `.hg/bookmarks.current` and `.hg/branch` are plain files, which gives a cheap `Probe.head`.
- **Base.** The `default` symbol is the tipmost head of the named branch, so a feature commit on an anonymous head becomes `default` and the diff comes out empty. The base must resolve through a bookmark or a `remotenames` name, as `ancestor(., <base>)`.
- **Upstream.** `incoming` and `outgoing` contact the remote. The bundled but experimental `remotenames` extension records remote bookmarks locally at pull time, which is the only offline source for ahead and behind.
- **Cost.** Mercurial is Python with no Rust acceleration on Windows. `hg help scripting` recommends a command server (`hg serve --cmdserver pipe`) for many calls in short order. Startup cost on Windows was not measured.

### The base diff contract

git's base diff runs to `HEAD` and leaves uncommitted work out. jj's `@` is a commit, so its base diff includes the working copy. The contract is "the checkout's line of work since it forked from the base, up to the head", and each backend defines head the way its own tools do: `HEAD` for git, `@` for jj, `.` for Mercurial. So jj's base diff includes uncommitted edits and git's and Mercurial's do not.

## Tests

| Crate | Coverage |
|---|---|
| `alacritree_vcs` | `Head::label` prefers the name. `FakeVcs` returns what it was scripted with and records requests. |
| `alacritree_git` | The moved tests move with their code. New tests cover these cases. `claims` is true only at a repository root. A detached `HEAD` yields `name: None` with a revision. `remove_checkout` on a gone checkout prunes only that worktree and refuses one whose directory came back. A dirty `worktree remove` returns `Unsaved` rather than `Failed`. `diff_args` produces today's argv for every target. |
| App | `Vcs` delegates across crates. A project whose default branch cannot be detected still counts as a repository for the tasks scope. The panel model and delete dialog with `FakeVcs { staged: None }` show no Staged section, title the working section "Changes", and count dirty work. Create and delete orchestration run hooks in order around a `FakeVcs`. |
| Existing | `git_status`, `list_projects` and `project_json` replies are byte-identical for a git repository, checked by a golden test captured before the refactor. `steady_state` passes with `Checkout` in place of `Worktree`. |

## Order of work

Each step builds, passes the suite, and changes nothing a user sees.

1. Golden tests for the `git_status`, `list_projects` and CLI text output, captured on today's code.
2. `alacritree_vcs` with the models. The app's own types become the crate's, `Worktree` becomes `Checkout`, and the hooks event becomes `CheckoutEvent`.
3. The trait, `VcsError` and `FakeVcs`. A cross-crate `Delegate` derive compiles in the app before anything uses it.
4. `alacritree_git` with `default_branch`, `upstream` and status compute. `StatusCache` calls the trait.
5. Discovery, `claims`, `probe` and the `Project.vcs` field. `project_and_worktree` gates on it.
6. Create and remove. The delete modal switches to `VcsError::Unsaved`.
7. `diff_args`, `push_target` and `locate`. `pr_status.rs` and `tasks/facts.rs` stop naming git.
8. The panel's `Option` staged path, with its `FakeVcs` tests.
9. `git2` moves to dev-dependencies, then out.

## Risks

- **Golden tests are the only proof of "no change".** The wire formats are built from the new models, so a field that moves (`branch` from `head.label()`) could change quietly without them. Step 1 exists for this.
- **Probe cost.** `probe` runs through the enum on the probe worker. The pilot's benchmark found enum dispatch across crates inlines like a local match, so this adds no cost, but `steady_state` has to keep passing.
- **Prune in WSL.** `prune_worktree` opens a `\\wsl.localhost\` path with git2 and has no in-distro path. The refactor keeps that as is. Whether it works today was not checked.

## Unresolved questions

1. **Capability approach.** This spec takes optional capabilities, expressed as `Option` fields rather than a sub-trait. Confirm, since the issue records it as undecided.
2. **Enabling a second backend.** A jj or Mercurial backend that claims repositories automatically changes what an existing user sees in a colocated jj+git repository. The recommendation is `[integrations.jj] enabled` and `[integrations.hg] enabled`, defaulting to false, with enabled backends claiming before git. Is claim order enough, or should a project be able to pin its backend in `state.toml`?
3. **Listing Mercurial shares.** The repository does not record them. Either alacritree records the shares it creates in `state.toml` and the backend checks each still points back, or the sidebar shows only the main checkout for Mercurial. Recording them is the recommendation, but it makes `discover` take a list of known paths.
4. **Wire names for a second backend.** Keep `git_status` with `staged`/`unstaged` fields and omit `staged` for jj, or add a `vcs_status` request with `working` and a `vcs` field and keep `git_status` as an alias? The recommendation is to add `vcs_status` when the second backend lands and leave `git_status` untouched.
5. **jj refresh cadence.** Poll with `--ignore-working-copy` and snapshot when `op_heads` changes, or snapshot on a slower cadence and accept op-log entries? This belongs to the jj spec, but it decides whether the trait needs a separate `refresh` method.
6. **Direct diff viewer templates.** tuicr's built-in and custom templates pass `{base}...HEAD`, a git revspec. The options are per-backend template tables under `[integrations.diff_viewer.custom]`, or a `{range}` placeholder the backend fills in. Whether tuicr itself reads jj or Mercurial repositories was not checked.
7. **Mercurial process cost.** A command-server client per repository, or one process per call like git's WSL path? This needs a measurement on Windows first.
8. **jj head label.** Show the nearest bookmark alone, or "bookmark +N" with the distance to `@`? The latter needs a `distance` field on `Head`.
