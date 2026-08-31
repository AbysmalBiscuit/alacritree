# WSL Discovery Reports Authority Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop a transient `wsl.exe` failure from silently collapsing a WSL project's worktree list to a single pseudo-worktree.

**Architecture:** `Project::discover` returns `Discovered { project, authoritative }` instead of a bare `Project`. A caller that already holds a worktree list keeps it when the answer is not authoritative. The three exits of `discover_wsl` are classified by a pure function so all of them are reachable in a test without a WSL host.

**Tech Stack:** Rust 2024 (MSRV 1.85), git2. No new dependencies.

## Why this is its own branch

Two reasons, and the second is the one that forces it:

1. It is a live bug on `master` today, independent of any sidebar work. A flaky WSL round trip blanks the sidebar for a WSL project right now.
2. It is a **prerequisite** for the sidebar focus reconciler
   (`2026-07-23-sidebar-focus-preservation.md`), which infers row removal from
   absence. Without this fix a transient WSL hiccup is byte-identical to "every
   worktree in this project was deleted", and the reconciler would fire a bogus
   slide — under `"follow"`, navigating the terminal off the user's work.

Same failure shape as `17f95c23 fix(wsl): keep last probe when helper is briefly down`, which fixed the foreground-TUI probe cache: a transport error collapsed into the same value as a definitive negative answer. That one shipped as its own small commit; so does this.

## Global Constraints

- Branch from **`master`**, not from `feat/sidebar-search-actions`. This is independently mergeable and should not carry the sidebar branch's commits into review.
- Only the `alacritree/` crate is edited. `alacritty*/` crates are vendored and read-only.
- Conventional Commits, imperative mood, subject ≤50 chars including the type prefix (72 is the hard limit).
- Comments explain *why*, never restate *what*. No PR/task references, no change-relative phrasing.
- `cargo fmt` is enforced. Run it before committing.
- Do not commit anything under `docs/superpowers/`.

## File Structure

| File | Responsibility |
| --- | --- |
| `alacritree/src/projects.rs` | Add `Discovered` and `WslAnswer`; `discover` reports authority; add `apply`; `refresh` delegates to it. |
| `alacritree/src/app.rs` | Three `discover` call sites unwrap `.project`; `poll_project_refreshes` adopts through `apply`; the refresh channel carries `Discovered`. |
| `alacritree/src/cli/offline.rs` | Two `discover` call sites unwrap `.project`. |

---

### Task 0: Create the worktree

- [x] **Step 1: Branch from master**

```bash
cd C:/Users/Lev/Git/github/alacritree
git fetch origin master
git worktree add -b fix/wsl-discovery-authoritative \
    ../alacritree-worktrees/fix/wsl-discovery-authoritative master
```

- [x] **Step 2: Confirm the tree builds before touching it**

Run: `cargo test -p alacritree` from the new worktree.
Expected: PASS. If `master` is red, stop and say so — a red baseline makes the RED step in Task 1 meaningless.

---

### Task 1: Discovery reports whether its answer is authoritative

A failed `wsl.exe` round trip currently returns `Project::placeholder`, which holds one pseudo-worktree. `refresh` and `poll_project_refreshes` copy that over the real worktree list, so a transient failure is indistinguishable from "every worktree was deleted".

`discover_wsl` has three exits and only the middle one is authoritative, so the classification is extracted into a pure function rather than left implicit in the control flow — otherwise the only testable part is `apply`, which is not the part that was wrong.

**Files:**
- Modify: `alacritree/src/projects.rs:37-52`, `:75-128`, `:181-185`
- Modify: `alacritree/src/app.rs:354`, `:573`, `:749`, `:767-780`, `:1192`, `:1211`
- Modify: `alacritree/src/cli/offline.rs:84`, `:119`
- Test: `alacritree/src/projects.rs` (existing `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `projects::Discovered { project: Project, authoritative: bool }`; `projects::WslAnswer` and `fn classify_wsl_answer(batch: Result<&str, ()>, is_repo: bool, worktrees_parsed: usize) -> WslAnswer`; `Project::discover(root: PathBuf) -> Discovered`; `Project::apply(&mut self, found: Discovered)`; `Project::refresh(&mut self)` keeps its signature.

- [x] **Step 1: Write the failing tests**

Add to the `mod tests` block in `alacritree/src/projects.rs`:

```rust
#[test]
fn refresh_keeps_worktrees_when_discovery_is_not_authoritative() {
    let mut project = Project::placeholder(PathBuf::from("/nonexistent-root"));
    project.default_branch = Some("develop".to_string());
    project.worktrees = vec![
        Worktree {
            name: "main".to_string(),
            path: PathBuf::from("/nonexistent-root"),
            branch: None,
            is_main: true,
            prunable: false,
        },
        Worktree {
            name: "feature".to_string(),
            path: PathBuf::from("/nonexistent-root-feature"),
            branch: Some("feature".to_string()),
            is_main: false,
            prunable: false,
        },
    ];

    let before = project.worktrees.clone();
    project.apply(Discovered {
        project: Project::placeholder(project.root.clone()),
        authoritative: false,
    });

    assert_eq!(project.worktrees.len(), before.len());
    assert_eq!(project.worktrees[1].name, "feature");
    assert_eq!(
        project.default_branch.as_deref(),
        Some("develop"),
        "an unreachable backend must not erase the known default branch either"
    );
}

#[test]
fn apply_adopts_an_authoritative_result() {
    let mut project = Project::placeholder(PathBuf::from("/root"));
    let mut fresh = Project::placeholder(PathBuf::from("/root"));
    fresh.worktrees.clear();
    fresh.default_branch = Some("main".to_string());

    project.apply(Discovered { project: fresh, authoritative: true });

    assert!(project.worktrees.is_empty());
    assert_eq!(project.default_branch.as_deref(), Some("main"));
}

#[test]
fn only_a_reachable_distro_gives_an_authoritative_answer() {
    // The round trip failed: the tree is unknown, not empty.
    assert_eq!(classify_wsl_answer(Err(()), false, 0), WslAnswer::Unreachable);
    // The distro answered "this is not a repository" — that is the truth.
    assert_eq!(classify_wsl_answer(Ok("no"), false, 0), WslAnswer::NotARepo);
    // A repository always has at least its main checkout, so parsing none of
    // them means the round trip came back malformed.
    assert_eq!(classify_wsl_answer(Ok("yes"), true, 0), WslAnswer::Unreachable);
    assert_eq!(classify_wsl_answer(Ok("yes"), true, 2), WslAnswer::Repo);
}

#[test]
fn a_non_git_windows_root_is_authoritative() {
    let dir = tempfile::tempdir().unwrap();
    let found = Project::discover(dir.path().to_path_buf());
    assert!(found.authoritative, "a directory that is genuinely not a repo is the truth");
    assert_eq!(found.project.worktrees.len(), 1);
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree projects::tests -- --nocapture`
Expected: FAIL to compile — `cannot find struct 'Discovered'`, `no method named 'apply'`, `cannot find function 'classify_wsl_answer'`.

- [x] **Step 3: Add `Discovered`, the classifier, and rewrite the entry points**

In `alacritree/src/projects.rs`, add above `impl Project`:

```rust
/// A discovery result and whether it can be trusted to replace an existing
/// worktree list.  A backend that could not be reached returns a placeholder
/// standing in for an unknown tree, which must never overwrite what the
/// caller already knows.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub project: Project,
    pub authoritative: bool,
}

impl Discovered {
    fn found(project: Project) -> Self {
        Self { project, authoritative: true }
    }

    fn unavailable(project: Project) -> Self {
        Self { project, authoritative: false }
    }
}

/// What an in-distro discovery round trip established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WslAnswer {
    Repo,
    NotARepo,
    /// The distro could not be reached, or answered malformed — the tree is
    /// unknown rather than empty.
    Unreachable,
}

/// Decide what a discovery round trip established, kept separate from running
/// it so all three outcomes are reachable without a WSL host.
fn classify_wsl_answer(batch: Result<&str, ()>, is_repo: bool, worktrees_parsed: usize) -> WslAnswer {
    match batch {
        Err(()) => WslAnswer::Unreachable,
        Ok(_) if !is_repo => WslAnswer::NotARepo,
        // A repository always has at least its main checkout.
        Ok(_) if worktrees_parsed == 0 => WslAnswer::Unreachable,
        Ok(_) => WslAnswer::Repo,
    }
}
```

Replace `pub fn discover` (lines 37-52):

```rust
    /// Classify the root and discover through the owning backend: in-distro
    /// git for WSL paths, git2 for Windows paths, and a pseudo-worktree
    /// placeholder when the root is not a repository.
    pub fn discover(root: PathBuf) -> Discovered {
        let name = display_name(&root);
        match wsl::classify(&root) {
            wsl::Location::Wsl { distro, linux_path } => {
                Self::discover_wsl(root, name, &distro, &linux_path)
            },
            // A directory that is not a repository is a fact, not a failure.
            wsl::Location::Windows(_) => match Repository::open(&root) {
                Ok(repo) => Discovered::found(Self::from_repo(root, name, &repo)),
                Err(_) => Discovered::found(Self::placeholder(root)),
            },
        }
    }
```

Change `discover_wsl` to return `Discovered`, routing each exit through the classifier. The `run_batch` error arm logs before discarding the error, since `classify_wsl_answer` only needs to know that it failed:

```rust
    fn discover_wsl(root: PathBuf, name: String, distro: &str, linux_path: &str) -> Discovered {
        let batch = wsl::run_batch(distro, DISCOVER_SCRIPT, &[linux_path]).map_err(|e| {
            log::warn!("WSL discovery failed for {}: {e}", root.display());
        });
        let stdout = match batch {
            Ok(s) => s,
            Err(()) => {
                return match classify_wsl_answer(Err(()), false, 0) {
                    WslAnswer::Unreachable => Discovered::unavailable(Self::placeholder(root)),
                    _ => unreachable!("a failed round trip is always Unreachable"),
                };
            },
        };
```

Keep the existing `text(0)` and worktree parsing, then replace the two remaining early exits and the final construction with one classification:

```rust
        match classify_wsl_answer(Ok(&stdout), text(0) == "yes", worktrees.len()) {
            WslAnswer::NotARepo => Discovered::found(Self::placeholder(root)),
            WslAnswer::Unreachable => Discovered::unavailable(Self::placeholder(root)),
            WslAnswer::Repo => Discovered::found(Project { /* the existing construction */ }),
        }
```

Replace `refresh` (lines 181-185) with:

```rust
    pub fn refresh(&mut self) {
        let found = Project::discover(self.root.clone());
        self.apply(found);
    }

    /// Adopt a discovery result.  A non-authoritative result leaves the
    /// worktree list and default branch alone: an unreachable backend must not
    /// read as deletion.  `expanded`, `shell_override`, and `label` are user
    /// state and are never touched either way.
    pub fn apply(&mut self, found: Discovered) {
        if !found.authoritative {
            return;
        }
        self.worktrees = found.project.worktrees;
        self.default_branch = found.project.default_branch;
    }
```

- [x] **Step 4: Update every caller**

`alacritree/src/app.rs:573` — `wsl::Location::Windows(_) => Project::discover(root).project,`

`alacritree/src/app.rs:1192` — `wsl::Location::Windows(_) => self.projects.push(Project::discover(path.clone()).project),`

`alacritree/src/app.rs:1211` — `self.projects.push(Project::discover(path.clone()).project);`

`alacritree/src/app.rs:767-780` — `poll_project_refreshes` adopts through `apply`:

```rust
    /// Adopt completed background discoveries.  Only worktrees and the
    /// default branch are copied — `expanded`, the shell override, and the
    /// label are user state that survives refreshes (mirrors
    /// `Project::refresh`).
    fn poll_project_refreshes(&mut self) {
        let projects = &mut self.projects;
        self.pending_project_refresh.retain(|root, rx| match rx.try_recv() {
            Ok(found) => {
                if let Some(project) = projects.iter_mut().find(|p| p.root == *root) {
                    project.apply(found);
                }
                false
            },
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
    }
```

The worker at `app.rs:749` already sends whatever `discover` returns, so it now sends a `Discovered`. Change the channel type on the `pending_project_refresh` field declaration (`app.rs:354`) from `Receiver<Project>` to `Receiver<Discovered>` and add `Discovered` to the `crate::projects::` import list.

`alacritree/src/cli/offline.rs:84` — append `.project`.
`alacritree/src/cli/offline.rs:119` — `let mut project = Project::discover(p.root).project;`

In `projects.rs`'s own `mod tests`, lines 376, 390, 402, 413 each call `Project::discover(repo_dir)` — append `.project` to all four.

- [x] **Step 5: Run the full suite**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS, including the four new tests.

- [x] **Step 6: Prove the fix was real (RED for the right reason)**

Temporarily change `apply` to drop its `if !found.authoritative { return; }` guard.

Run: `cargo test -p alacritree projects::tests::refresh_keeps_worktrees_when_discovery_is_not_authoritative -- --exact`
Expected: FAIL — `assertion (left == right) failed: left: 1, right: 2`.

The full test name matters: `--exact` against the `refresh_keeps_worktrees` prefix matches nothing and reports success having run zero tests.

Restore the guard and re-run. Expected: PASS. Record both outcomes.

- [x] **Step 7: Commit**

```bash
git add alacritree/src/projects.rs alacritree/src/app.rs alacritree/src/cli/offline.rs
git commit -m "fix(projects): keep worktrees when unreachable"
```

---

## Verification before opening the PR

- [x] `cargo fmt --check` clean (stable rustfmt; note master is not fmt-clean under either toolchain)
- [x] `cargo clippy -p alacritree --all-targets` no new warnings
- [x] `cargo test -p alacritree` green
- [ ] Manual WSL pass in the isolated verification lab: add a project on a WSL path, confirm its worktrees list, then make `wsl.exe` fail (rename it on `PATH`, or stop the distro) and force a refresh. The worktree list must stay intact rather than collapsing to one row. Restore `wsl.exe` and confirm a later refresh still adopts real changes.
- [x] `git log --oneline master..HEAD` shows one commit, touching nothing under `docs/superpowers/`

## Unresolved questions

1. **Should a non-authoritative refresh surface anything to the user?** Right now it is silent apart from the `log::warn!`, so a WSL project can quietly show a stale worktree list for as long as the distro is down. A sidebar indicator is out of scope here, but "stale and silent" is a deliberate choice worth recording rather than an oversight.
