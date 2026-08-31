# Prunable worktrees — design

Branch: `feat/prunable-worktrees` (worktree off `master`, PR upstream to
mathix420/alacritree). Local-only spec; PR description carries the context.

## Problem

Deleting a worktree's directory by hand leaves its git metadata behind
(git calls this a *prunable* worktree). alacritree still lists it in the
sidebar, and clicking it switches the workspace and tries to spawn a shell
with the dead path as cwd — on Windows that fails with raw
`os error 267` ("The directory name is invalid") and strands the user on a
broken, empty workspace. The row's `×` is broken too: it runs
`git worktree remove`, which refuses when the directory is gone
(`git worktree prune` is the sanctioned cleanup, but the CLI form prunes
*all* stale worktrees at once).

## Decisions (2026-07-12)

- Prunable worktrees stay **visible, marked**, with a prune affordance —
  not hidden, never auto-pruned (silent repo mutation could destroy
  metadata of a worktree on a temporarily unmounted drive).
- Prune dialog **asks about the branch** via a checkbox (default checked,
  matching the existing ×-deletes-worktree-and-branch semantics).
- Spawn guard **blocks the workspace switch** when the directory is
  missing and re-marks the row; no fallback-to-home, no dead workspace.
- Prune is implemented with **git2's per-worktree `Worktree::prune()`**,
  not a `git worktree prune` shell-out — targets exactly the row the user
  clicked.

## Design

### 1. Detection (`projects.rs`)

`Worktree` gains `prunable: bool`. `from_repo` marks each *linked*
worktree `prunable = !path.is_dir()`.

Directory existence — not git2's `is_prunable()` — is the signal, because
it is exactly what predicts spawn failure: a *locked* worktree with a
missing directory is not git-prunable but still cannot host a shell. The
main worktree and non-git pseudo-worktrees are never marked (if their
directory is gone the project itself is broken); the spawn guard still
covers them.

### 2. Sidebar UI (`app.rs::worktree_row`)

- Prunable rows render dimmed (`theme.text_muted`) with hover text
  "worktree directory is missing — × prunes it".
- Clicking the row does **not** activate: no workspace switch, no spawn.
- The `×` button remains, hover text "prune worktree".

### 3. Prune flow (`worktree.rs` + `app.rs`)

`×` on a prunable row opens the existing confirmation-dialog pattern with
prune wording:

- Title: ``Prune worktree `<name>`?``
- Body: the directory is already gone; only git's worktree metadata will
  be removed.
- Checkbox: ``Also delete branch `<branch>` `` — default **checked**;
  uncheck to keep the branch. (Live-worktree deletion keeps its current
  fixed behavior; the checkbox exists only in prune mode.)

Confirming calls a new `wt::prune_worktree(project_root, name, branch,
delete_branch)`:

1. `Repository::open(project_root)`, `find_worktree(name)`,
   `prune(WorktreePruneOptions)` — per-worktree, in-process, no console
   window on Windows.
2. If `delete_branch`, reuse the existing `git branch -D` shell-out;
   errors logged and ignored, same as `delete_worktree`.

Runs synchronously like the existing delete flow (prune only removes a
metadata directory). Afterwards: same cleanup as delete — drop the
`active_session` entry for that path, drop any sessions whose cwd is the
worktree, refresh the project.

### 4. Spawn guard (`app.rs`)

Marking happens at refresh time, so a directory can vanish between
refresh and click. Two layers:

- `activate_worktree` checks `path.is_dir()` first. If missing: do not
  switch workspace, set `last_error` to "worktree directory is missing —
  prune it from the sidebar", and refresh the owning project so the row
  picks up its marking.
- `spawn_session` checks the working directory before spawning and
  returns a readable error ("working directory no longer exists")
  instead of raw os error 267 — covers every other spawn path
  (new-instance binding, session tabs).

### 5. Error handling

Prune failure (locked worktree, directory reappeared, repo open error)
surfaces through the existing `last_error` toast. Nothing panics; the
sidebar stays usable.

### 6. Testing

First tests for the crate. Integration-style unit tests with temp repos
(git2 init + real `git worktree add` + `fs::remove_dir_all`):

- Discovery marks the missing-dir worktree `prunable`; live worktrees
  stay unmarked. (RED first: fails before the field exists.)
- `prune_worktree` removes the metadata — re-discovery no longer lists
  the worktree.
- Branch deleted when `delete_branch`, kept otherwise.

UI behavior (no-activate, dialog, guard message) is verified manually in
a release build: create a worktree, delete its directory, refresh →
dimmed row; click → no switch + message; × → prune dialog → metadata
gone; `git worktree add` on the same branch name works again.

## Out of scope

- Configurable worktree location (planned feature 3).
- Auto-refresh/file-watching of worktree dirs; marking updates on the
  existing refresh triggers.
- Prune affordances for the main worktree or non-git pseudo-worktrees.
