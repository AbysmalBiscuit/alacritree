# Prunable Worktrees Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stale git worktrees (directory deleted by hand, metadata left behind) show up dimmed in the sidebar, refuse to spawn shells, and can be pruned — with an optional branch delete — from the existing × affordance, instead of stranding the user on a dead workspace with `os error 267`.

**Architecture:** Detection happens at discovery time in `projects.rs` (a `prunable` flag on `Worktree`, keyed on directory existence). The sidebar row in `app.rs` renders prunable rows dimmed and non-activatable; its × routes into the existing delete-dialog flow, which branches into a new git2-based `prune_worktree` in `worktree.rs`. A two-layer spawn guard (`activate_worktree` + `session.rs`) covers the refresh race.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe 0.31, git2 0.20, tempfile (new dev-dependency for tests).

**Spec:** `docs/superpowers/specs/2026-07-12-prunable-worktrees-design.md`

## Global Constraints

- Only the `alacritree/` crate changes; `alacritty*` crates are vendored and read-only.
- Workspace MSRV 1.85, edition 2024; `cargo fmt` is enforced (`rustfmt.toml`).
- Comments explain *why*, not *what*; never narrate the change ("now we", "this PR").
- Conventional Commits, imperative subject ≤ 50 chars incl. type prefix, lowercase after colon.
- `docs/superpowers/` is in `.git/info/exclude` — never commit spec or plan files.
- Test command: `cargo test -p alacritree`. Type-check loop: `cargo check -p alacritree`.
- Work happens in a dedicated worktree for branch `feat/prunable-worktrees` off `master` (create at execution start via superpowers:using-git-worktrees). All `git add` paths below are relative to that worktree root.
- Tests that call `git branch -D` need the `git` binary on PATH (present on dev machines).

---

### Task 1: Mark prunable worktrees during discovery

**Files:**
- Modify: `alacritree/Cargo.toml` (add dev-dependency)
- Modify: `alacritree/src/main.rs:3-20` (declare test_util module)
- Create: `alacritree/src/test_util.rs`
- Modify: `alacritree/src/projects.rs`
- Test: `alacritree/src/projects.rs` (`#[cfg(test)] mod tests` — the crate is bin-only, so unit tests live in-file; integration `tests/` dirs can't import from a binary crate)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `projects::Worktree` gains field `pub prunable: bool` (Task 4 reads it in `worktree_row`, Task 5 in the delete-request plumbing).
  - `test_util::init_repo(dir: &Path) -> git2::Repository` and `test_util::add_worktree(repo: &git2::Repository, name: &str) -> PathBuf` (Task 2's tests reuse both).
  - Prunable worktrees keep their `branch: Option<String>` populated (read from the git admin area since the checkout is gone) — Task 5's branch checkbox depends on this.

- [ ] **Step 1: Add the tempfile dev-dependency**

In `alacritree/Cargo.toml`, after the `[dependencies]` section's last entry (`notify-rust = "4"`) and before `[target.'cfg(unix)'.dependencies]`, add:

```toml
[dev-dependencies]
# Throwaway repos for worktree discovery/prune tests.
tempfile = "3"
```

- [ ] **Step 2: Create the shared test fixture module**

Create `alacritree/src/test_util.rs`:

```rust
//! Shared fixtures for tests that need a real repository with worktrees.

use std::path::{Path, PathBuf};

use git2::Repository;

/// Initialize a repository with one empty commit so worktrees can be added.
pub fn init_repo(dir: &Path) -> Repository {
    std::fs::create_dir_all(dir).unwrap();
    let repo = Repository::init(dir).unwrap();
    {
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        let tree_id = {
            let mut index = repo.index().unwrap();
            index.write_tree().unwrap()
        };
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    }
    repo
}

/// Add a linked worktree named `name` (git2 also creates a branch `name`).
/// Returns the worktree's checkout path, a sibling of the repo directory.
pub fn add_worktree(repo: &Repository, name: &str) -> PathBuf {
    let path = repo.workdir().unwrap().parent().unwrap().join(format!("wt-{name}"));
    repo.worktree(name, &path, None).unwrap();
    path
}
```

In `alacritree/src/main.rs`, add to the module list (alphabetical, after `mod terminal_view;`):

```rust
#[cfg(test)]
mod test_util;
```

- [ ] **Step 3: Write the failing tests**

At the bottom of `alacritree/src/projects.rs`, append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{add_worktree, init_repo};

    #[test]
    fn live_worktree_is_not_prunable() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        add_worktree(&repo, "feature");

        let project = Project::discover(repo_dir);
        let wt = project.worktrees.iter().find(|w| w.name == "feature").unwrap();
        assert!(!wt.prunable);
        assert_eq!(wt.branch.as_deref(), Some("feature"));
    }

    #[test]
    fn missing_dir_marks_worktree_prunable_and_keeps_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        let wt_path = add_worktree(&repo, "feature");
        std::fs::remove_dir_all(&wt_path).unwrap();

        let project = Project::discover(repo_dir);
        let wt = project.worktrees.iter().find(|w| w.name == "feature").unwrap();
        assert!(wt.prunable);
        assert_eq!(wt.branch.as_deref(), Some("feature"));
    }

    #[test]
    fn main_worktree_is_never_prunable() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);

        let project = Project::discover(repo_dir);
        assert!(project.worktrees[0].is_main);
        assert!(!project.worktrees[0].prunable);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p alacritree prunable`
Expected: compilation FAILS with `no field 'prunable' on type ...Worktree` (the RED state — the field doesn't exist yet).

- [ ] **Step 5: Implement detection**

In `alacritree/src/projects.rs`:

Add the field to the struct:

```rust
#[derive(Debug, Clone)]
pub struct Worktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
    /// The checkout directory is gone but git's worktree metadata remains
    /// (`git worktree list` still shows it as prunable). Such a row cannot
    /// host a shell and only offers cleanup.
    pub prunable: bool,
}
```

In `Project::discover`'s non-git fallback, add `prunable: false` to the pseudo-worktree literal:

```rust
            Err(_) => Project {
                worktrees: vec![Worktree {
                    name: name.clone(),
                    path: root.clone(),
                    branch: None,
                    is_main: true,
                    prunable: false,
                }],
```

In `from_repo`, the main worktree gets `prunable: false`:

```rust
        worktrees.push(Worktree {
            name: "main".to_string(),
            path: main_path.clone(),
            branch: current_branch(repo),
            is_main: true,
            prunable: false,
        });
```

The linked-worktree loop becomes:

```rust
        if let Ok(names) = repo.worktrees() {
            for name in names.iter().flatten() {
                if let Ok(wt) = repo.find_worktree(name) {
                    let path = wt.path().to_path_buf();
                    let branch = Repository::open(&path)
                        .ok()
                        .and_then(|wt_repo| current_branch(&wt_repo))
                        .or_else(|| branch_from_admin_head(repo, name));
                    worktrees.push(Worktree {
                        name: name.to_string(),
                        // Directory existence, not git2's `is_prunable`, is
                        // the signal: a *locked* worktree with a missing dir
                        // is not git-prunable but still can't host a shell.
                        prunable: !path.is_dir(),
                        path,
                        branch,
                        is_main: false,
                    });
                }
            }
        }
```

Add the helper next to `current_branch`:

```rust
/// A prunable worktree's checkout is gone, so its HEAD can't be read via
/// `Repository::open`. Git still records it in the main repo's admin area
/// (`.git/worktrees/<name>/HEAD`) — parse the symref line from there.
fn branch_from_admin_head(repo: &Repository, worktree_name: &str) -> Option<String> {
    let head = repo.path().join("worktrees").join(worktree_name).join("HEAD");
    let contents = std::fs::read_to_string(head).ok()?;
    contents.trim().strip_prefix("ref: refs/heads/").map(str::to_string)
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p alacritree prunable`
Expected: 3 passed (`live_worktree_is_not_prunable`, `missing_dir_marks_worktree_prunable_and_keeps_branch`, `main_worktree_is_never_prunable`).

Then run: `cargo check -p alacritree`
Expected: no errors (nothing else constructs `Worktree`, but confirm).

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/Cargo.toml alacritree/src/main.rs alacritree/src/test_util.rs alacritree/src/projects.rs Cargo.lock
git commit -m "feat(projects): detect prunable worktrees"
```

---

### Task 2: Per-worktree prune in worktree.rs

**Files:**
- Modify: `alacritree/src/worktree.rs`
- Test: `alacritree/src/worktree.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `test_util::{init_repo, add_worktree}` from Task 1; existing `run_git(cwd, args)` already in `worktree.rs`.
- Produces: `pub fn prune_worktree(project_root: &Path, worktree_name: &str, branch: Option<&str>, delete_branch: bool) -> Result<(), String>` — Task 5 calls it from `run_pending_delete` as `wt::prune_worktree(...)`.

- [ ] **Step 1: Write the failing tests**

At the bottom of `alacritree/src/worktree.rs`, append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{add_worktree, init_repo};

    #[test]
    fn prune_removes_stale_metadata_and_keeps_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        let wt_path = add_worktree(&repo, "stale");
        std::fs::remove_dir_all(&wt_path).unwrap();

        prune_worktree(&repo_dir, "stale", Some("stale"), false).unwrap();

        assert!(repo.find_worktree("stale").is_err());
        assert!(repo.find_branch("stale", git2::BranchType::Local).is_ok());
    }

    #[test]
    fn prune_deletes_branch_when_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        let wt_path = add_worktree(&repo, "stale");
        std::fs::remove_dir_all(&wt_path).unwrap();

        prune_worktree(&repo_dir, "stale", Some("stale"), true).unwrap();

        assert!(repo.find_worktree("stale").is_err());
        assert!(repo.find_branch("stale", git2::BranchType::Local).is_err());
    }

    #[test]
    fn prune_refuses_a_live_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        add_worktree(&repo, "live");

        assert!(prune_worktree(&repo_dir, "live", Some("live"), false).is_err());
        assert!(repo.find_worktree("live").is_ok());
        assert!(repo.find_branch("live", git2::BranchType::Local).is_ok());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree prune_`
Expected: compilation FAILS with `cannot find function 'prune_worktree' in this scope`.

- [ ] **Step 3: Implement prune_worktree**

In `alacritree/src/worktree.rs`, directly below `delete_worktree`, add:

```rust
/// Remove the git metadata of a worktree whose checkout directory is gone
/// (git calls these *prunable*). Uses git2's per-worktree prune rather than
/// shelling out to `git worktree prune`, which would sweep every stale
/// worktree in the repo instead of just the one the user asked about.
pub fn prune_worktree(
    project_root: &Path,
    worktree_name: &str,
    branch: Option<&str>,
    delete_branch: bool,
) -> Result<(), String> {
    let repo = git2::Repository::open(project_root)
        .map_err(|e| format!("failed to open repository: {}", e.message()))?;
    let wt = repo
        .find_worktree(worktree_name)
        .map_err(|e| format!("failed to find worktree `{worktree_name}`: {}", e.message()))?;
    // Default prune options refuse valid or locked worktrees — exactly the
    // safety we want if the directory reappeared since discovery; the error
    // surfaces to the caller.
    wt.prune(None).map_err(|e| format!("failed to prune: {}", e.message()))?;
    if delete_branch {
        if let Some(branch) = branch {
            // Branch may already be gone — ignore errors, as delete_worktree does.
            let _ = run_git(project_root, &["branch", "-D", branch]);
        }
    }
    Ok(())
}
```

(`worktree.rs` has no `use git2::...` import today; the fully-qualified `git2::Repository::open` path above needs none.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree prune_`
Expected: 3 passed.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/worktree.rs
git commit -m "feat(worktree): add per-worktree prune"
```

---

### Task 3: Spawn guard for vanished working directories

**Files:**
- Modify: `alacritree/src/session.rs` (guard inside `spawn_with`, covers both `Session::spawn` and `Session::spawn_command`)
- Modify: `alacritree/src/app.rs:308-311` (`activate_worktree`)
- Test: `alacritree/src/session.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Worktree.prunable` is *not* needed here — the guard is pure path checking, covering rows not yet re-marked.
- Produces: `Session::spawn`/`spawn_command` now fail with `io::ErrorKind::NotFound` and message `working directory no longer exists: <path>` when the cwd is gone. Existing `failed to spawn shell: {e}` call sites in `app.rs` display it unchanged.

- [ ] **Step 1: Write the failing tests**

At the bottom of `alacritree/src/session.rs`, append:

```rust
#[cfg(test)]
mod tests {
    use super::ensure_working_directory;

    #[test]
    fn missing_dir_is_a_readable_error() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        let err = ensure_working_directory(Some(&gone)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(err.to_string().contains("no longer exists"));
    }

    #[test]
    fn none_and_existing_dirs_pass() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ensure_working_directory(None).is_ok());
        assert!(ensure_working_directory(Some(tmp.path())).is_ok());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree working_directory`
Expected: compilation FAILS with `cannot find function 'ensure_working_directory'`.

- [ ] **Step 3: Implement the guard**

In `alacritree/src/session.rs`, add a free function near `spawn_with` (module level, alongside the other helpers; it needs `std::path::Path`, already imported via `PathBuf` — if only `PathBuf` is imported, extend the `use` to `use std::path::{Path, PathBuf};`):

```rust
/// A vanished cwd would otherwise surface as the PTY backend's raw error
/// (`os error 267`, "The directory name is invalid", on Windows) — reject it
/// up front with a message the error toast can show as-is.
fn ensure_working_directory(dir: Option<&Path>) -> std::io::Result<()> {
    match dir {
        Some(d) if !d.is_dir() => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("working directory no longer exists: {}", d.display()),
        )),
        _ => Ok(()),
    }
}
```

Then make it the first statement of `spawn_with` (session.rs:281, before the shell/title logic):

```rust
    fn spawn_with(
        ctx: egui::Context,
        config: &Config,
        working_directory: Option<PathBuf>,
        size: TermSize,
        cell_size: (f32, f32),
        shell: Option<Shell>,
        // ... existing params unchanged
    ) -> std::io::Result<Self> {
        ensure_working_directory(working_directory.as_deref())?;
        // ... existing body unchanged
```

(Match the real parameter list when editing — only the added first line changes.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree working_directory`
Expected: 2 passed.

- [ ] **Step 5: Add the activate-time guard in app.rs**

Replace `activate_worktree` (app.rs:308-311):

```rust
    fn activate_worktree(&mut self, ctx: &Context, path: &Path) {
        // The dir can vanish between discovery marking the row live and the
        // click. Switching first would strand the user on a dead workspace
        // with a failed spawn — stay put and let the sidebar re-mark the row.
        if !path.is_dir() {
            self.last_error =
                Some("worktree directory is missing — prune it from the sidebar".to_string());
            if let Some(idx) =
                self.projects.iter().position(|p| p.worktrees.iter().any(|w| w.path == path))
            {
                self.projects[idx].refresh();
            }
            return;
        }
        self.current_workspace = Some(path.to_path_buf());
        self.ensure_active_session(ctx);
    }
```

- [ ] **Step 6: Verify the whole crate still builds and tests pass**

Run: `cargo test -p alacritree`
Expected: all tests pass (Tasks 1-3 suites), no compile errors.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "fix(session): guard spawn against missing cwd"
```

---

### Task 4: Dim prunable rows and block activation

**Files:**
- Modify: `alacritree/src/app.rs:1555-1631` (`worktree_row`)

**Interfaces:**
- Consumes: `Worktree.prunable` from Task 1; `theme.text_muted` (existing).
- Produces: prunable rows return `WorktreeAction { activate: false, .. }` on click; delete via × still works and is routed by Task 5.

No unit test — `worktree_row` is egui immediate-mode UI; behavior is covered by the manual verification in Task 6. (The crate has no UI test harness; introducing one is out of scope.)

- [ ] **Step 1: Implement the rendering changes**

In `worktree_row` (app.rs:1555):

Replace the two style lines at the top of the frame closure (app.rs:1574-1575):

```rust
            let default_icon = if wt.is_main { "●" } else { "○" };
            let name_color = if wt.prunable {
                theme.text_muted
            } else if is_active {
                theme.text
            } else {
                theme.text_dim
            };
```

Replace the × hover text (app.rs:1594-1595):

```rust
                    if !wt.is_main {
                        let hover =
                            if wt.prunable { "prune worktree" } else { "delete worktree and branch" };
                        let btn = icon_button(ui, "×", theme.text_muted, theme).on_hover_text(hover);
```

After the `.interact(egui::Sense::click())` chain that produces `resp` (app.rs:1605), add a hover hint for prunable rows:

```rust
    let resp = if wt.prunable {
        resp.on_hover_text("worktree directory is missing — × prunes it")
    } else {
        resp
    };
```

Change the return (app.rs:1630) so clicking a prunable row never activates:

```rust
    WorktreeAction { activate: resp.clicked() && !delete_clicked && !wt.prunable, delete: delete_clicked }
```

- [ ] **Step 2: Verify it builds and existing tests pass**

Run: `cargo test -p alacritree`
Expected: all tests pass, no warnings about unused fields.

- [ ] **Step 3: Format and commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): dim prunable rows, block spawn"
```

---

### Task 5: Prune dialog with branch checkbox

**Files:**
- Modify: `alacritree/src/app.rs:134-140` (`DeleteRequest`), `app.rs:849-857` (× call site), `app.rs:1725-1789` (`show_delete_dialog`), `app.rs:1791-1812` (`run_pending_delete`)

**Interfaces:**
- Consumes: `Worktree.prunable` (Task 1), `wt::prune_worktree` (Task 2), `DirtyCounts::default()` (existing `git_status.rs`).
- Produces: end-to-end prune flow; no new public surface.

No new unit test — the dialog is egui UI over the already-tested `prune_worktree`; covered by manual verification in Task 6.

- [ ] **Step 1: Extend DeleteRequest**

Replace the struct (app.rs:134-140):

```rust
struct DeleteRequest {
    project_idx: usize,
    worktree_path: PathBuf,
    worktree_name: String,
    branch: Option<String>,
    dirty: DirtyCounts,
    /// The checkout dir is already gone; confirm prunes metadata instead of
    /// removing a directory.
    prunable: bool,
    /// Checkbox state for the prune dialog's "also delete branch".
    delete_branch: bool,
}
```

- [ ] **Step 2: Fill the new fields at the × call site**

Replace the `DeleteRequest` literal (app.rs:849-857):

```rust
                                if action.delete {
                                    delete_request.set(Some(DeleteRequest {
                                        project_idx: idx,
                                        worktree_path: wt.path.clone(),
                                        worktree_name: wt.name.clone(),
                                        branch: wt.branch.clone(),
                                        // A missing dir has nothing to be dirty;
                                        // skip the status probe.
                                        dirty: if wt.prunable {
                                            DirtyCounts::default()
                                        } else {
                                            git_status::dirty_counts(&wt.path)
                                        },
                                        prunable: wt.prunable,
                                        delete_branch: true,
                                    }));
                                }
```

- [ ] **Step 3: Branch the dialog wording and add the checkbox**

In `show_delete_dialog` (app.rs:1725), replace from the `let Some(req) = ...` line through the `let warning = ...` line with (note `as_mut` — the checkbox needs to write back):

```rust
        let Some(req) = self.pending_delete.as_mut() else {
            return;
        };
        let (title, detail, verb) = if req.prunable {
            (
                format!("Prune worktree `{}`?", req.worktree_name),
                "The worktree directory is already gone; this removes git's leftover metadata."
                    .to_string(),
                "Prune",
            )
        } else {
            (
                format!("Delete worktree `{}`?", req.worktree_name),
                match &req.branch {
                    Some(b) => format!("Removes the worktree directory and deletes branch `{b}`."),
                    None => "Removes the worktree directory.".to_string(),
                },
                "Delete",
            )
        };
        let warning = dirty_warning(&req.dirty);
```

Inside the modal closure, after the `detail` label and the optional warning label (app.rs:1751-1754), insert the checkbox:

```rust
                if req.prunable {
                    if let Some(b) = req.branch.clone() {
                        ui.checkbox(
                            &mut req.delete_branch,
                            RichText::new(format!("Also delete branch `{b}`"))
                                .color(theme.text_muted)
                                .small(),
                        );
                    }
                }
```

Update the keyboard-hint label (app.rs:1758) and confirm-button label (app.rs:1764) to use `verb`:

```rust
                        RichText::new(format!("Enter to {} · Esc to cancel", verb.to_lowercase()))
```

```rust
                        let delete = ui.add(
                            egui::Button::new(RichText::new(verb).color(danger)).frame(false),
                        );
```

(`theme` is already copied to a local before the borrow; `danger` likewise — the `req` mutable borrow ends with the modal closure, before `self.run_pending_delete()` is reached, so the borrow checker is satisfied.)

- [ ] **Step 4: Branch run_pending_delete**

Replace the delete call (app.rs:1805-1810):

```rust
        let force = req.dirty.is_dirty();
        let result = if req.prunable {
            wt::prune_worktree(
                &project_root,
                &req.worktree_name,
                req.branch.as_deref(),
                req.delete_branch,
            )
        } else {
            wt::delete_worktree(&project_root, &req.worktree_path, req.branch.as_deref(), force)
        };
        if let Err(e) = result {
            let action = if req.prunable { "prune" } else { "delete" };
            self.last_error = Some(format!("{action} failed: {e}"));
        }
```

(The session/workspace cleanup above it — `sessions.retain`, `current_workspace` reset, `active_session.remove` — stays exactly as is; it is correct for both paths. The trailing `self.projects[req.project_idx].refresh();` also stays.)

- [ ] **Step 5: Verify build and tests**

Run: `cargo test -p alacritree`
Expected: all tests pass.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): prune worktrees from delete dialog"
```

---

### Task 6: Full-suite check and manual verification

**Files:** none (verification only).

**Interfaces:** none.

- [ ] **Step 1: Full test + fmt gate**

```bash
cargo fmt
cargo test -p alacritree
cargo build -p alacritree --release
```

Expected: fmt makes no changes, all tests pass, release build succeeds.

- [ ] **Step 2: Manual verification (release build)**

Using the release binary in a scratch repo:

1. Add a project, create a worktree from the sidebar `+`, confirm it activates normally.
2. Outside the app, delete the worktree's directory (`Remove-Item -Recurse -Force <path>`).
3. Click the sidebar refresh icon → the row renders dimmed; hovering shows "worktree directory is missing — × prunes it".
4. Click the row → nothing happens (no workspace switch, no error 267).
5. Click × → dialog says "Prune worktree `<name>`?" with the checked "Also delete branch" checkbox; press Enter → row disappears; `git worktree list` in the repo no longer shows it; branch is gone.
6. Repeat 1-2, then prune with the checkbox *unchecked* → branch survives (`git branch` lists it).
7. Race check: with the row still marked live (no refresh after deleting the dir), click it → error toast "worktree directory is missing — prune it from the sidebar", workspace does not switch, row re-renders dimmed.

Record any deviation as a bug before proceeding.

- [ ] **Step 3: Update the local feature list**

In `docs/specs/planned_features.md` (local-only, not committed), note under the Windows findings item: prunable-worktrees implemented on `feat/prunable-worktrees` with date and test count.

---

## Execution notes

- Branch: `feat/prunable-worktrees`, worktree off `master` (create via superpowers:using-git-worktrees before Task 1).
- Don't push or open a PR without being asked; the PR description carries the spec context when requested (specs stay uncommitted).
