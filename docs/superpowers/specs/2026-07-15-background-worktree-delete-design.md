# Non-blocking worktree removal — design

Run worktree deletion (and prune) on a background thread so confirming the
delete modal never freezes the UI. While a deletion is in flight, its sidebar
row is faded and non-interactive with a spinner in place of the status icon.
Mirrors devkit's `issue end` background-removal design
(`devkit/docs/superpowers/specs/2026-07-07-issue-end-background-removals-design.md`).

## Decisions (confirmed with the user)

- **D1 (concurrency):** parallel, unbounded. Confirming N deletes runs N
  background threads at once. `branch -D` contention between concurrent
  deletes in the same repo degrades to a leftover branch (`delete_worktree`
  already ignores branch-deletion errors), never to a failure.
- **D2 (prune path):** the metadata-only prune (checkout dir already gone)
  goes through the same background path as a real removal. One code path; the
  row just fades briefly.

## Problem

`run_pending_delete` (`alacritree/src/app.rs:2924`) runs
`wt::delete_worktree` / `wt::prune_worktree` synchronously on the UI thread
after the modal confirm. `git worktree remove` `rm -rf`s the whole checkout —
multi-GB `node_modules` included — so the entire window freezes for the
duration. Worktree *creation* already has the right pattern: `spawn_create`
(`worktree.rs`) runs on a background thread and streams progress over an
`mpsc` channel polled each frame.

## Approach

Per-delete background thread reporting completion over an `mpsc` channel,
polled once per frame — the same shape as `pending_project_refresh`.

Alternatives rejected: a single worker thread with a queue (serializes
deletes, against D1); routing completion through the IPC `AppCall` channel
(that channel is for external requests, and per-path tracking would still be
needed for the fade).

## State

```rust
/// In-flight background deletions, keyed by worktree path. A worktree in
/// this map renders faded/non-interactive; results are adopted in
/// poll_worktree_deletes.
pending_worktree_deletes: HashMap<PathBuf, Receiver<Result<(), String>>>,
```

## Dispatch (`run_pending_delete`)

The pre-delete work stays on the UI thread, unchanged: kill sessions cwd'd in
the worktree, reset `current_workspace`, drop the `active_session` entry.
Then:

- Capture the **project root** (not `project_idx` — the index can go stale
  while the delete runs; IPC `remove_project` reorders `projects`).
- Spawn a thread running `wt::delete_worktree` or `wt::prune_worktree` per
  `req.prunable`, send the `Result` through the channel, and call
  `ctx.request_repaint()` so completion wakes the egui loop.
- Insert the receiver into `pending_worktree_deletes` keyed by
  `req.worktree_path`.

The modal closes immediately on confirm, as it does today.

## Poll (`poll_worktree_deletes`)

Called from `update` next to `poll_project_refreshes`. For each entry whose
receiver has a result:

- remove the map entry,
- on `Err`, set `last_error` (existing error bar),
- refresh the owning project **by root**, skipping silently if the project
  was removed meanwhile.

Success needs no message — the row vanishing on refresh is the feedback.
Failure un-fades the row (map entry gone, project refreshed) and it is fully
interactive again.

## Sidebar UI (`worktree_row`)

New `deleting: bool` parameter, true when the worktree's path is in the map:

- name color drops to `theme.text_muted`; status icon replaced by a small
  `egui::Spinner` (which requests repaint itself, keeping the animation
  alive),
- × and + buttons not drawn; hover highlight and hover-text suppressed,
- returned `WorktreeAction` is all-false — no activate/delete/spawn.

## Re-entry guards

Every path that would open the delete modal or activate a worktree checks
`pending_worktree_deletes` first and ignores the request for an in-flight
path. This covers both mouse and keyboard nav. The sidebar cursor may still
land on the row; Enter does nothing.

## Testing

`delete_worktree` / `prune_worktree` are already covered in `worktree.rs`.
The new code is thread plumbing plus egui rendering, so the testable surface
is thin: add a unit test only if a pure helper falls out (e.g. the re-entry
guard predicate). Manual verification:

- delete a worktree with a large `node_modules`; the UI stays responsive, the
  row fades with a spinner, other worktrees stay usable,
- confirm two deletes back-to-back; both spinners run concurrently,
- force a failure (e.g. hold a file lock in the checkout on Windows); the row
  returns to normal and the error bar shows the git message.

## Out of scope

- No IPC/MCP delete surface (none exists today).
- No concurrency cap (D1).
- No change to the dirty-check/force semantics, the confirm modal contents,
  or session-kill behavior at confirm time.
