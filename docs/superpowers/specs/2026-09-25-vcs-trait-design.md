# Version control trait: design for git, jj and Mercurial

Issue: #134. Split out of #133. Builds on `2026-09-23-integration-crates-pilot-design.md`, whose crate model, ambassador dispatch and thiserror rules apply here unchanged.

This spec designs the trait and the refactor that moves today's git code behind it. It does not implement jj or Mercurial. It does record what each of them needs from the trait, checked against jj 0.45.1 and Mercurial 7.2.4, so the trait is shaped for them now rather than reshaped when they arrive.

## Goals

- git code lives in its own crate, and the app cannot reach git except through the trait. The app's `Cargo.toml` loses its `git2` dependency outside `[dev-dependencies]`, which is what makes the compiler enforce it.
- The trait fits jj and Mercurial without git vocabulary leaking into their answers. A jj working-copy commit is never reported as "unstaged".
- The refactor changes no behavior for git users. Every UI string, IPC reply, CLI output, config key and `state.toml` key stays byte-identical.

## Where git leaks today

The issue lists the model's leaks. The inventory for this spec found these on top of them, and corrects one claim.

- `git2` is used in production outside the git modules. `alacritree_gh`'s `read_remotes` and `push_remote` read `branch.<b>.pushRemote`, `remote.pushDefault` and the `origin` URL through git2, and `worktree.rs`'s `prune_worktree` prunes through git2.
- `tasks/facts.rs` shells out to `git worktree list --porcelain` and `rev-parse --show-toplevel` on its own and parses `branch refs/heads/` itself, independent of `projects.rs`.
- `refused_for_unsaved_work` in `app/modals.rs` decides whether a delete was refused for unsaved work by matching git's English stderr ("contains modified or untracked files, use --force", "is dirty, use --force").
- `project_and_worktree` in `app.rs` treats `project.default_branch.is_some()` as "this project is a repository". A git repository whose default branch cannot be detected falls through as a non-repository there. The trait fixes this by giving the project an explicit backend.
- `worktree_liveness.rs` reads `.git` and `HEAD` as plain files every probe interval, which is git's on-disk layout.
- The diff pane runs `git -c core.pager=<pager> diff <args>`, built by `alacritree_diff_viewer`'s `diff_args` and `target_git_args`, and tuicr's built-in templates pass `{base}...HEAD`, a git revspec.
- `Worktree.branch` holds either a branch name or a 7-character OID, and `projects.rs` throws away the flag telling them apart. PR lookup and the taskwarrior node read the field without knowing which one they got.
- `validate_branch_name` enforces git-check-ref-format rules.

## The capability question

The issue left three options open. This spec takes the first, optional capabilities, and expresses it as data rather than as a second trait. Lev confirmed the choice.

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
    pub upstream: Option<UpstreamState>,
}

pub struct Head {
    /// The branch or bookmark, when one applies. What a PR's head ref and
    /// the taskwarrior node use.
    pub name: Option<String>,
    /// A short revision id: 7 hex digits of a git OID, a jj change id.
    pub revision: Option<String>,
    /// Commits from `name` to the head. `None` when the head sits on `name`,
    /// which is always true for a git branch. jj's nearest bookmark can be
    /// several commits behind `@`.
    pub distance: Option<u32>,
}

impl Head {
    /// `name`, else `revision`. What the sidebar, `$branch` and the IPC
    /// `branch` field show, matching today's single field.
    pub fn label(&self) -> Option<&str>;
}

/// Moved unchanged from `upstream.rs`, keeping its name.
pub enum UpstreamState {
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
    /// What `Head::label` would read from disk right now, compared with the
    /// discovered checkout's label to catch a switch made outside the app.
    /// `None` when it cannot be read cheaply, and the sidebar then waits for
    /// discovery.
    pub head: Option<String>,
}

/// Which checkout and head a directory belongs to, for `alacritree hook`.
pub struct Located { pub main: PathBuf, pub checkout: PathBuf, pub head: Head }

/// What `create_checkout` made.
pub struct Created {
    /// The repository will not list this checkout again, so the app records
    /// it in `state.toml` and passes it back to `discover`. Mercurial shares
    /// set this; git worktrees and jj workspaces do not.
    pub record: bool,
}

/// The backends, for the `state.toml` pin, menus, icons and wire fields.
#[derive(strum::EnumString, strum::IntoStaticStr, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub enum VcsKind { Git, Jj, Hg }

/// The base a new checkout starts from, as `prepare_checkout` resolved it.
pub struct Base {
    /// What steps and errors call it, e.g. `main`.
    pub name: String,
    /// What the backend starts the checkout from, e.g. `origin/main`.
    pub revision: String,
}

pub struct CreateCheckout {
    pub main: PathBuf,
    /// Chosen by the app from `[workspace]`; the backend creates it.
    pub target: PathBuf,
    /// The new branch or bookmark.
    pub name: String,
    pub base: Base,
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

/// The repository a PR targets and where the branch is pushed, for the
/// forge's PR lookup.
pub struct Remotes { pub origin_url: Option<String>, pub push_url: Option<String> }
```

`Worktree.prunable` becomes `Checkout.gone`. The meaning is unchanged: the directory is gone but the backend still lists it.

`ChangeKind::glyph` and `label` stay in the app. They are how alacritree draws a change, and Mercurial's own `!` means "missing", which alacritree draws as `D`.

### The trait

```rust
#[ambassador::delegatable_trait]
pub trait VersionControl {
    /// Whether `root` looks like this backend's repository. A cheap
    /// pre-check that never walks upward and may answer true when unsure,
    /// as git does for WSL paths. Discovery does not call it: it calls
    /// `discover` on each enabled backend in turn, and `NotARepository`
    /// moves on to the next. `claims` answers the questions that come
    /// before a discovery, such as which backends the chooser offers.
    fn claims(&self, root: &Path) -> bool;

    fn kind(&self) -> VcsKind;

    /// Makes `root` a new repository of this backend, after which `claims`
    /// is true for it.
    fn init(&self, root: &Path, blocking: &Blocking) -> Result<(), VcsError>;

    /// `recorded` holds the checkouts the app recorded because `Created.record`
    /// asked it to. The backend keeps the ones that still belong to `root` and
    /// drops the rest. git ignores it.
    fn discover(&self, root: &Path, recorded: &[PathBuf], upstream: bool, blocking: &Blocking)
        -> Result<Repository, VcsError>;

    /// Must not record history. A backend whose every command writes (jj
    /// snapshots the working copy) reads without snapshotting here.
    fn status(&self, checkout: &Path, base_hint: Option<&str>, blocking: &Blocking)
        -> Result<Status, VcsError>;

    /// Brings the backend's record of the working copy up to date, which may
    /// write history. Runs only on a user's explicit refresh. A no-op for git
    /// and Mercurial, whose status reads the files directly.
    fn snapshot(&self, checkout: &Path, blocking: &Blocking) -> Result<(), VcsError>;

    /// Cheaper than `status`: no base diff.
    fn dirty(&self, checkout: &Path, blocking: &Blocking) -> Result<Dirty, VcsError>;

    /// Runs on the probe worker for rows the sidebar draws.
    fn probe(&self, checkout: &Path) -> Probe;

    fn locate(&self, dir: &Path, blocking: &Blocking) -> Option<Located>;

    /// Names the base picker offers, locals first.
    fn names(&self, checkout: &Path, blocking: &Blocking) -> Result<Vec<String>, VcsError>;

    fn validate_name(&self, name: &str) -> Result<(), VcsError>;

    /// Resolves the base and fetches it, reporting each step as it starts.
    /// The last report names the step `create_checkout` runs under, since
    /// the app picks the target path inside that step, between the calls.
    fn prepare_checkout(&self, main: &Path, trunk_hint: Option<&str>, on_step: &mut dyn FnMut(&str),
        blocking: &Blocking) -> Result<Base, VcsError>;

    /// Creates `req.target` on a new name, starting from `req.base`.
    fn create_checkout(&self, req: &CreateCheckout, blocking: &Blocking) -> Result<Created, VcsError>;

    /// A live checkout is removed; a gone one is forgotten.
    fn remove_checkout(&self, req: &RemoveCheckout, blocking: &Blocking) -> Result<(), VcsError>;

    /// Arguments after the program that print `target` as a unified diff,
    /// with `pager` wired in when given.
    fn diff_args(&self, target: &DiffTarget) -> Vec<String>;

    /// The `{range}` a Direct diff viewer template receives for a base
    /// review: `<base>...HEAD` for git.
    fn review_range(&self, base: &str) -> String;

    fn remotes(&self, checkout: &Path, name: &str) -> Remotes;
}
```

The real signatures name every type by absolute path and the crate declares `extern crate self as alacritree_vcs;`, as the hooks crate does.

`on_step` is `&mut dyn FnMut` rather than a generic so ambassador has no generic method to forward. `create` in the app already takes `impl FnMut`, and it passes `&mut on_step` through.

Creation is two calls so the app picks the target path at the moment it does today. The origin check, the base and the fetch all come first, and their errors take precedence. Picking the path creates the project's worktree directory and, for a WSL repository, asks the distro for `$HOME`, so moving it earlier would leave an empty directory behind a failed origin check and report a path error ahead of a missing remote.

The backend value holds its resolved config (program paths), so `Vcs::Git(GitBackend)` is cheap to clone and each `Project` keeps its own.

### Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("failed to run {program}: {source}")]
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
    /// A path the backend cannot hand to its tool, e.g. one outside the
    /// repository's WSL distro.
    #[error("{0}")]
    BadPath(String),
    #[error("{} is not a repository", .0.display())]
    NotARepository(PathBuf),
    #[error("{0}")]
    Unreachable(String),
    #[error("{what} cancelled")]
    Cancelled { what: &'static str },
    #[error("{context}")]
    Backend { context: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
}
```

Messages match today's strings where the UI shows them. `Backend` carries a git2 error without `alacritree_vcs` depending on git2. Its `context` is whatever text the UI showed for that error before, which for a status failure is git2's full `Display`, class and code suffix included.

git decides `Unsaved` the way the delete modal does today, by reading git's English reason after the quoted path. That match, and its guard against a path that spells out the phrase, move into `alacritree_git` unchanged. The child's locale is left alone, so a git that speaks another language reports `Failed` with its own text, as it does now.

### FakeVcs

Behind `test-support`, as `FakeHook` is. It returns a scripted `Repository` and `Status`, records create and remove requests, and can be built with `staged: None`. Its main job is the test that no git crate can write, the panel and the delete dialog with a backend that has no staging area.

## alacritree_git

Moves in from `alacritree/src/`, with git2 and the batched scripts both inside.

| From | What moves |
|---|---|
| `default_branch.rs` | all of it, private to the crate |
| `upstream.rs` | the parsers and `map_from_repo`; `UpstreamState` goes to `alacritree_vcs` |
| `git_status.rs` | `compute`, both transports, the porcelain and numstat parsers, `dirty_counts` |
| `projects.rs` | `from_repo`, `discover_wsl`, `discover_batch`, `parse_worktree_list_z`, `current_branch`, `branch_from_admin_head` |
| `worktree.rs` | `git_command`, `run_git*`, `has_remote`, `list_branches`, `resolve_base_branch`, `query_origin_head`, `validate_branch_name`, the `worktree add/remove`, `branch -D` and prune calls |
| `worktree_liveness.rs` | `probe`, `probe_checkout`, `head_branch` |
| `alacritree_diff_viewer` | the diff argv builders. The `core.pager` wiring stays there, since its WSL script resolves the pager inside the distro |
| `alacritree_gh` | `push_remote` and the URL half of `read_remotes`. gh fills `alacritree_forge::Head.remotes` from the app for native paths and keeps its WSL shell lookup |
| `tasks/facts.rs` | the `worktree list` and `rev-parse` parsing |
| `test_util.rs` | `init_repo`, `add_worktree`, behind `test-support` |

`remove_checkout` on a gone checkout does what `prune_worktree` does today, git2's per-worktree prune with default options, so a directory that came back is refused rather than swept.

`RawGit` (`path`, `wsl_path`) moves here and replaces the `raw_tool_table!` line for git, the same move the pilot made for doppler. `Tool::Git` stays in `alacritree_common`'s table. It gains the two keys every backend has, described under "Choosing a backend": `enabled`, true for git, and `show_icon`, false for git.

## App changes

### Dispatch and detection

```rust
#[derive(Debug, Clone, Delegate)]
#[delegate(VersionControl)]
pub(crate) enum Vcs {
    Git(GitBackend),
}

/// The enabled backends in priority order: git, jj, Mercurial. Built once
/// from config.
pub(crate) fn backends(integrations: &IntegrationsConfig) -> Vec<Vcs>;

/// The pinned backend when it is enabled and claims `root`, else the first
/// enabled backend that claims it, else `None` for a plain directory. A
/// claim may open the repository, so callers run it off the UI thread.
pub(crate) fn detect(backends: &[Vcs], root: &Path, pin: Option<VcsKind>) -> Option<Vcs>;
```

`Project` gains `vcs: Option<Vcs>` and renames `default_branch` to `trunk`. `worktrees: Vec<Worktree>` becomes `checkouts: Vec<Checkout>`, and `projects::Worktree` goes away. `Project::discover` calls `discover` on the pinned backend, then on each enabled backend in priority order, and the first that does not answer `NotARepository` owns the project. It skips `claims`, because startup discovers every native project on the UI thread and a pre-check would open each repository twice. A project with `vcs: None` gets today's placeholder checkout. `project_and_worktree` gates on `vcs.is_some()` instead of `default_branch.is_some()`.

### Choosing a backend

Only one backend is active in a project. Each backend's config section has an `enabled` key. Git's defaults to true and every other backend's to false, so a user who changes nothing gets today's behavior. Setting `[integrations.git] enabled = false` makes every git repository a plain directory, with no git panel, no worktree rows and no PR lookup.

Git takes priority. A colocated jj repository has both `.git` and `.jj`, so with jj enabled it still opens as git until the user picks jj for that project. That pick is a per-project pin in `state.toml`, written as `vcs = "jj"` next to the existing `shell` key and held in `Project.vcs_override: Option<VcsKind>`, which `Project::apply` preserves like `shell_override`. A pin whose backend is disabled or no longer claims the root is ignored and kept, so re-enabling the backend brings it back. `cli/offline.rs` reads the pin too, so the CLI and the window agree.

A project can also be a folder no backend claims, since adding a project never required a repository. The user picks its backend the same way. Picking a backend that does not claim the root opens a confirm dialog offering to initialize one there, through the trait's `init`: `git init`, `jj git init` or `hg init`, run through the WSL transport for a WSL path. Confirming initializes and pins the choice. The pin matters because `jj git init` makes a colocated repository with a `.git`, which git's priority would otherwise take. Cancelling changes nothing and writes no pin.

The pin is set three ways.

- **The project's context menu** gains a "Version control" section below "Open in", built like `shell_override_menu`. It lists "Auto" and every enabled backend, marks the current choice, and labels a backend that does not claim the root "Initialize <backend> repository". The section is shown when the root is a plain folder, when two or more enabled backends claim it, or when a pin exists. A git repository in a git-only setup does not show it, since there is nothing to choose.
- **A `ChooseVcs` action**, "Choose the version control for a project" in the palette, opens a picker with the same choices for the project under the sidebar cursor, else the current workspace's project. It is modeled on `SetBaseBranch`. It has no default binding.
- **An `OpenContextMenu` action** opens the context menu of the sidebar row under the cursor and drives it with the arrow keys, Enter and Esc. This reaches every row's menu, not only the version control section. It is valid only while the projects sidebar has focus, joining the actions `is_projects_filter_scoped` lists. Its default binding is Ctrl+M. Ctrl+M is the carriage return byte in a terminal, and the focus scope is what keeps it from shadowing Enter in the shell.

egui 0.31.1 cannot open a `context_menu` from code. `MenuRoot::context_interaction` in its `menu.rs` creates one only when the row is hovered and secondary-clicked, and the menu root it stores is private to egui. So the menu's entries become data, built by one function per row kind. The right-click path draws them with `context_menu`, and the keyboard path draws the same list in a popup anchored to the row with its own cursor. There is one list of entries and two ways to draw it.

The project row can show its backend's icon, drawn left of the name. `[ui.icons]` gains `vcs_git`, `vcs_jj` and `vcs_hg`, whose defaults join the baked glyph set through `baked_glyphs!`. Each backend's `show_icon` key decides whether its projects draw it. The default is false for git, which keeps today's row, and true for every other backend, since seeing jj or Mercurial there is the signal that the project is not using git.

`[integrations.git] enabled`, `show_icon` and `vcs_git` land in this refactor, with defaults that change nothing on screen. `OpenContextMenu` does not depend on the trait, so it ships as its own change and keeps the refactor free of new behavior. The "Version control" section, `ChooseVcs`, `init` and the pin ship together as one change after the refactor. They are useful with git alone, because a plain-folder project can be initialized as a git repository from the sidebar.

### What stays in the app

- `StatusCache`, `LivenessCache` and all polling cadence. They hold the project's `Vcs` and call the trait.
- `Project`'s user state (`label`, `expanded`, `shell_override`) and `Project::apply`.
- `project_json` and every other wire format.
- The worktree create flow around the backend calls. That is picking the target path from `[workspace]` between `prepare_checkout` and `create_checkout`, copying LLM configs, the Claude bell, checkout hooks, progress over `mpsc`, `spawn_create` and `spawn_delete`.
- The diff pane's program resolution, WSL wrapping and PTY session. Only the diff arguments come from the backend.
- The PR cache. It asks `remotes` for native heads, so gh no longer opens repositories.

### The git panel

- Staged section only when `status.staged` is `Some`. For git it always is, so nothing changes on screen.
- With `staged: None`, the working section is titled "Changes" instead of "Unstaged", and `ReviewStaged` reports itself unavailable the way a Direct viewer with an empty template already does.
- `GitSection::Unstaged` becomes `Working` internally, and `Branch` becomes `Base`. Row keys (`staged:`, `worktree:`, `branch:`, `section:*`) are compared across frames, so they keep their spellings.
- `DirtyCounts` becomes `vcs::Dirty`. The delete modal matches `VcsError::Unsaved` rather than stderr text.
- `StatusCache` exposes the live `Head`, not a string. The sidebar's label and the focus model read `label()`, and the PR lookup for the active workspace reads `name`, so a detached active workspace sends no revision id to the forge.
- The diff viewer needs a backend only to build a launch. The panel passes the project's `Vcs`, else the first enabled backend, and with neither it opens no viewer.

### Wire formats

No change in this refactor. `git_status` keeps its name and its `branch`, `default_branch`, `staged`, `unstaged` and `diff_vs_default_branch` fields. `list_projects` keeps `default_branch` and `worktrees[].branch`, now filled from `trunk` and `head.label()`. `alacritree git-status` and `alacritree worktree create` keep their names.

The second backend adds a `vcs_status` request, with an `alacritree vcs-status` CLI command and MCP tool to match. Its reply names the backend in a `vcs` field and carries `head`, `trunk`, `base`, `working` and `base_diff`, plus `staged` only when the backend has a staging area. `git_status` stays as it is and keeps answering for git projects, so existing scripts and agents keep working. `list_projects` gains a `vcs` field per project at the same time, an addition existing readers ignore.

### Row templates

`[ui] worktree_name` gains `$distance`, rendered as `+N` from `Head.distance` and absent when the head sits on its name. A jj user who wants the distance writes `${branch:$name} ${distance:}`. Git never sets it, so no git row changes. It reads the discovered `Checkout` the row already holds, which the existing discovery cadence refreshes. No hover state or new cache is needed.

### Diff viewer templates

Direct viewer templates gain a `{range}` placeholder, filled from `review_range`. The built-in tuicr templates switch from `{base}...HEAD` to `{range}`, which renders the same string for git. `{base}` keeps working in custom templates, so no user's config breaks.

### Lints

`git2` moves to `[dev-dependencies]` in `alacritree/Cargo.toml`, which is what stops production code reaching around the trait. The tests that build repositories switch to `alacritree_git`'s `test-support` helpers, and once they have, the dev-dependency goes too.

## What jj and Mercurial need from this

Recorded so each backend's own spec starts from facts. Versions checked were jj 0.45.1 (docs and source at tag v0.45.1) and Mercurial 7.2.4.

Claims about jj and Mercurial below were checked against their docs and source at those versions, except where a line says it was not.

Both should be driven through their CLIs with templates. jj-lib's docs say it "is not a stable API", and it needs Rust 1.97.1 as of jj 0.45, far past this workspace's 1.85. It also does not read user config (not re-checked against the source), so a `trunk()` alias that `jj git clone` writes into `%APPDATA%\jj\repos\<hash>\config.toml` would be invisible to it. Mercurial has no usable Rust library. `hg-core` on crates.io is a 2019 placeholder, and `hg help scripting` recommends `HGPLAIN=1` with the CLI.

### jj

- **Checkouts.** `jj workspace list -T 'name ++ " " ++ root ++ "\n"'` lists every workspace from a central registry. `root` is missing for workspaces made before 0.38 or whose directory moved. `jj workspace forget` leaves the directory on disk, so removal is forget, then delete.
- **Stale workspaces.** A workspace whose `@` another workspace rewrote is stale, and `jj st` there exits 1 until `jj workspace update-stale`. `status` must report this as an error the panel can show, not as an empty tree.
- **Status must not snapshot.** Every jj command, even `jj log`, snapshots the working copy and records an operation when files changed. Polling every 1.5 s would fill the op log. `--ignore-working-copy` avoids it at the cost of a stale view until the user's next jj command. jj's FAQ recommends that flag for watch loops and suggests watching `.jj/repo/op_heads/heads` as the refresh signal, which fits `Probe.head`.
- **Refresh cadence.** Decided. `status` always passes `--ignore-working-copy`, and `Probe.head` is a token read from `.jj/repo/op_heads/heads`, so the panel re-reads whenever any jj command runs. `snapshot` runs a jj command without the flag, and the app calls it only on the user's explicit refresh. Edits made in an editor therefore appear after the user's next jj command or a manual refresh, never from polling. alacritree never writes to the operation log on its own initiative, including on focus changes.
- **Head.** jj has no current bookmark. `name` is the nearest bookmark, `heads(::@ & bookmarks())`, and `revision` is `change_id.shortest(8)`.
- **Base diff.** `jj diff --from 'fork_point(trunk() | @)' --to @`. The shorter `-r 'trunk()..@'` fails with "Cannot diff revsets with gaps" once trunk is merged into the line (from memory, not re-checked). Line counts need `--stat` or `--git` parsing, since the diff template has no per-file stats.
- **Upstream.** Tracked remote bookmarks expose `tracking_ahead_count` and `tracking_behind_count`, counted from the remote's side, so local ahead is the remote ref's behind. Colocated repos list a pseudo-remote `@git` to filter out.
- **Colocation** has been jj's default since 0.34. A colocated repo has both `.jj` and `.git`. git sees a detached HEAD at `@-` with `@`'s changes unstaged, and `git worktree list` does not show jj workspaces. A git backend claiming such a repo shows a misleading tree. Git still takes priority there, and the jj user pins the project to jj once, as "Choosing a backend" describes.

### Mercurial

- **Checkouts.** `share` is bundled but disabled by default (from memory, as is `remotenames` being bundled, below). A share records its source in `.hg/sharedpath`, and the source records nothing, so a repository's shares cannot be listed from the repository. Removal is deleting the directory. Decided: `create_checkout` returns `Created { record: true }`, the app writes the path into the project's `checkouts` list in `state.toml`, and `discover` keeps each recorded path whose `.hg/sharedpath` still points at the root. The list is written only when it is non-empty, so git users' `state.toml` does not change. Shares made outside alacritree do not appear.
- **Status.** `hg status -Tjson`. `!` (missing) maps to `Deleted`, `?` to `Untracked`. An unresolved merge shows files as `M`, and conflicts come from `hg resolve -l -Tjson`.
- **Head.** The active bookmark, else the named branch. `.hg/bookmarks.current` and `.hg/branch` are plain files, which gives a cheap `Probe.head`.
- **Base.** The `default` symbol is the tipmost head of the named branch, so a feature commit on an anonymous head becomes `default` and the diff comes out empty. The base must resolve through a bookmark or a `remotenames` name, as `ancestor(., <base>)`.
- **Upstream.** `incoming` and `outgoing` contact the remote. The bundled but experimental `remotenames` extension records remote bookmarks locally at pull time, which is the only offline source for ahead and behind.
- **Cost.** Decided: one command server (`hg serve --cmdserver pipe`) per repository, started on first use and kept alive, the way `wsl_helper` keeps one `sh` per distro. Mercurial is Python with no Rust acceleration on Windows, and process startup dominates every call. A benchmark on 2026-09-25 (Windows 11, Mercurial 7.2.4 from PyPI on Python 3.14, a 3000-file repository, 30 runs, medians) measured the following. A refresh here means status, head and base diff stat together.

  | Measure | One process per call | Command server |
  |---|---|---|
  | `hg status -Tjson` | 474 ms | 16 ms |
  | head (`hg log -r .`) | 480 ms | 3 ms |
  | `hg diff --stat` against the base | 607 ms | 55 ms |
  | a refresh | 1474 ms | 72 ms |
  | server start | | 532 ms, once |

  `git status --porcelain=v2` on the same files took 238 ms on the same machine. The server read a file edited between two calls correctly, so it does not serve a stale status. The PyPI install's `hg.exe` launcher failed with "failed to load Python DLL python314.dll", and running the `hg` script through the venv's `python.exe` worked, so the backend's program setting has to accept an interpreter plus script. To rerun, install Mercurial into a venv and run `2026-09-25-vcs-trait-hg-bench.py`, next to this spec, with that venv's Python. It builds its own repositories in its own directory.

### The base diff contract

git's base diff runs to `HEAD` and leaves uncommitted work out. jj's `@` is a commit, so its base diff includes the working copy. The contract is "the checkout's line of work since it forked from the base, up to the head", and each backend defines head the way its own tools do: `HEAD` for git, `@` for jj, `.` for Mercurial. So jj's base diff includes uncommitted edits and git's and Mercurial's do not.

## Tests

| Crate | Coverage |
|---|---|
| `alacritree_vcs` | `Head::label` prefers the name. `FakeVcs` returns what it was scripted with and records requests. |
| `alacritree_git` | The moved tests move with their code. New tests cover these cases. `claims` accepts a bare repository root and rejects a folder with no repository. A detached `HEAD` yields `name: None` with a revision. `remove_checkout` on a gone checkout prunes only that worktree and refuses one whose directory came back. A dirty `worktree remove` returns `Unsaved` rather than `Failed`, and a path that spells out git's phrase does not. `prepare_checkout` reports today's first four step strings in order. `diff_args` produces today's argv for every target. |
| App | `Vcs` delegates across crates. `detect` prefers git over a second claimant, honors an enabled pin and ignores a pin whose backend is disabled. A project with a pin round-trips `vcs` through `state.toml`, and one without writes no key. `$distance` renders `+N` and is absent for git. A project whose default branch cannot be detected still counts as a repository for the tasks scope. The panel model and delete dialog with `FakeVcs { staged: None }` show no Staged section, title the working section "Changes", and count dirty work. Create and delete orchestration run hooks in order around a `FakeVcs`, and create picks the target path only after `prepare_checkout` succeeds. The PR lookup for a detached active workspace queries nothing. Picking a backend for a plain folder calls `init` only after the confirm, pins it, and writes nothing when cancelled. |
| Existing | `git_status`, `list_projects` and `project_json` replies are byte-identical for a git repository, checked by a golden test captured before the refactor. `steady_state` passes with `Checkout` in place of `Worktree`. |

## Order of work

Each step builds, passes the suite, and changes nothing a user sees.

1. Golden tests for the `git_status`, `list_projects` and CLI text output, captured on today's code.
2. `alacritree_vcs` with the models. The app's own types become the crate's, `Worktree` becomes `Checkout`, and the hooks event becomes `CheckoutEvent`.
3. The trait, `VcsError` and `FakeVcs`. A cross-crate `Delegate` derive compiles in the app before anything uses it.
4. `alacritree_git` with `default_branch`, `upstream` and status compute. `StatusCache` calls the trait.
5. Discovery, `claims`, `probe` and the `Project.vcs` field. `project_and_worktree` gates on it.
6. Create through `prepare_checkout` and `create_checkout`, then remove. The delete modal switches to `VcsError::Unsaved`.
7. `diff_args`, `remotes` and `locate`. `alacritree_gh` and `tasks/facts.rs` stop reading git.
8. The panel's `Option` staged path, with its `FakeVcs` tests.
9. `[integrations.git] enabled` and `show_icon`, the `vcs_git` icon, `$distance` and `{range}`, each with a default that renders today's output.
10. `git2` moves to dev-dependencies, then out.

`OpenContextMenu` is a separate change that can land before or after these steps.

## Risks

- **Golden tests are the only proof of "no change".** The wire formats are built from the new models, so a field that moves (`branch` from `head.label()`) could change quietly without them. Step 1 exists for this.
- **Probe cost.** `probe` runs through the enum on the probe worker. The pilot's benchmark found enum dispatch across crates inlines like a local match, so this adds no cost, but `steady_state` has to keep passing.
- **Prune in WSL.** `prune_worktree` opens a `\\wsl.localhost\` path with git2 and has no in-distro path. The refactor keeps that as is. Whether it works today was not checked.
- **tuicr outside git.** `{range}` gives a backend a place to put its range, but whether tuicr reads a jj or Mercurial repository at all was not checked. If it does not, the Direct viewer is git-only and the panel hides the review buttons for other backends.

## Decisions

Lev answered the questions on 2026-09-25. Each answer is written into the section it affects.

- Optional capabilities as data. See "The capability question".
- `enabled` per backend, git on and the rest off, git first, one backend per project, a per-project pin, a "Version control" menu section, `ChooseVcs`, `OpenContextMenu` and a per-backend icon. See "Choosing a backend".
- A plain-folder project can pick a backend, and picking one offers to initialize a repository. See "Choosing a backend".
- Mercurial shares are recorded in `state.toml`. See Mercurial's "Checkouts".
- jj polls with `--ignore-working-copy` and snapshots only on an explicit refresh, never on its own initiative. See jj's "Refresh cadence".
- A `vcs_status` request lands with the second backend, and `git_status` stays. See "Wire formats".
- Direct viewer templates gain `{range}`. See "Diff viewer templates".
- Mercurial runs through a command server, chosen from a benchmark. See Mercurial's "Cost".
- jj's distance to its bookmark is a `$distance` row template variable. See "Row templates".
- Creation is `prepare_checkout` then `create_checkout`, so the target path is picked where it is today. See "The trait".
- `StatusCache` exposes the live `Head`, and the PR lookup reads its `name`. See "The git panel".

## Unresolved questions

None.
