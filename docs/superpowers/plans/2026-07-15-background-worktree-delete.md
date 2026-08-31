# Non-Blocking Worktree Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Worktree deletion runs on a background thread; the sidebar row fades with a spinner and is non-interactive until it finishes.

**Architecture:** Mirror `spawn_create` (`worktree.rs`): a `wt::spawn_remove` helper runs `delete_worktree`/`prune_worktree` on a thread and reports over an `mpsc` channel. The app tracks in-flight removals in `pending_worktree_deletes: HashMap<PathBuf, Receiver<Result<(), String>>>`, polled once per frame like `pending_project_refresh`. `worktree_row` gains a `deleting: bool` that fades the row, swaps the status icon for `egui::Spinner`, hides the ×/+ buttons, and returns no actions.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, `std::sync::mpsc`, git CLI + git2 (both already used by `worktree.rs`).

**Spec:** `docs/superpowers/specs/2026-07-15-background-worktree-delete-design.md`

## Global Constraints

- Only the `alacritree/` crate changes; vendored `alacritty*` crates are read-only.
- Both the real-removal and prune paths go through the background path (spec D2).
- Concurrency is unbounded — one thread per confirmed removal (spec D1).
- Anything producing events on a background thread must call `ctx.request_repaint()` or the UI appears to hang (project CLAUDE.md).
- `cargo fmt` before every commit (rustfmt is enforced).
- Conventional Commits, imperative subject ≤72 chars, no trailing period.
- Comments explain *why*, never narrate the change; no PR/task references.
- **Do not commit the spec or this plan file** — `docs/superpowers/` is excluded via `.git/info/exclude` and must stay untracked (specs must never reach an upstream PR). Stage files individually; never `git add -A`.

---

### Task 1: `wt::spawn_remove` — background removal helper

**Files:**
- Modify: `alacritree/src/worktree.rs` (add `RemoveRequest` + `spawn_remove` after `spawn_create`, which ends at line 72; add tests inside the existing `mod tests`, after `prune_refuses_a_live_worktree`)

**Interfaces:**
- Consumes: existing `delete_worktree`, `prune_worktree` (same file), `crate::test_util::{init_repo, add_worktree}` (tests).
- Produces (Task 2 depends on these exact names):

```rust
pub struct RemoveRequest {
    pub project_root: PathBuf,
    pub worktree_path: PathBuf,
    pub worktree_name: String,
    pub branch: Option<String>,
    pub prunable: bool,
    pub delete_branch: bool,
    pub force: bool,
}

pub fn spawn_remove(req: RemoveRequest, ctx: egui::Context) -> Receiver<Result<(), String>>
```

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `alacritree/src/worktree.rs` (after `prune_refuses_a_live_worktree`):

```rust
    #[test]
    fn spawn_remove_deletes_worktree_in_background() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        let wt_path = add_worktree(&repo, "doomed");

        let rx = spawn_remove(
            RemoveRequest {
                project_root: repo_dir.clone(),
                worktree_path: wt_path.clone(),
                worktree_name: "doomed".into(),
                branch: Some("doomed".into()),
                prunable: false,
                delete_branch: true,
                force: false,
            },
            egui::Context::default(),
        );

        let result = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(result, Ok(()));
        assert!(!wt_path.exists());
        assert!(repo.find_worktree("doomed").is_err());
    }

    #[test]
    fn spawn_remove_routes_prunable_to_prune() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let repo = init_repo(&repo_dir);
        let wt_path = add_worktree(&repo, "stale");
        std::fs::remove_dir_all(&wt_path).unwrap();

        let rx = spawn_remove(
            RemoveRequest {
                project_root: repo_dir.clone(),
                worktree_path: wt_path,
                worktree_name: "stale".into(),
                branch: Some("stale".into()),
                prunable: true,
                delete_branch: true,
                force: false,
            },
            egui::Context::default(),
        );

        let result = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(result, Ok(()));
        assert!(repo.find_worktree("stale").is_err());
        assert!(repo.find_branch("stale", git2::BranchType::Local).is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree spawn_remove`
Expected: compile error — `cannot find struct RemoveRequest` / `cannot find function spawn_remove`.

- [ ] **Step 3: Implement `RemoveRequest` + `spawn_remove`**

Insert in `alacritree/src/worktree.rs` directly after `spawn_create` (after line 72), before the `create` function:

```rust
pub struct RemoveRequest {
    pub project_root: PathBuf,
    pub worktree_path: PathBuf,
    pub worktree_name: String,
    pub branch: Option<String>,
    /// The checkout dir is already gone; prune metadata instead of removing
    /// a directory.
    pub prunable: bool,
    /// Prune path only: also delete the branch. The removal path always
    /// deletes the branch when one is known.
    pub delete_branch: bool,
    /// Pass `--force` to `git worktree remove` (dirty checkout).
    pub force: bool,
}

/// Run the removal on a background thread, waking the UI when it finishes.
/// The heavy `rm -rf` of the checkout must never run on the paint thread.
pub fn spawn_remove(req: RemoveRequest, ctx: egui::Context) -> Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = if req.prunable {
            prune_worktree(
                &req.project_root,
                &req.worktree_name,
                req.branch.as_deref(),
                req.delete_branch,
            )
        } else {
            delete_worktree(
                &req.project_root,
                &req.worktree_path,
                req.branch.as_deref(),
                req.force,
            )
        };
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree spawn_remove`
Expected: `test result: ok. 2 passed`.

Also run the whole crate to catch regressions: `cargo test -p alacritree`
Expected: all pass (emoji tests may skip; that is pre-existing).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/worktree.rs
git commit -m "feat(worktree): add spawn_remove background removal helper"
```

---

### Task 2: App wiring — dispatch, poll, re-entry guards

**Files:**
- Modify: `alacritree/src/app.rs`
  - struct field block (`pending_project_refresh` field, ~line 169)
  - constructor init block (`pending_project_refresh: HashMap::new()`, ~line 360)
  - `run_pending_delete` (~line 2924)
  - next to `poll_project_refreshes` (~line 461) — new `poll_worktree_deletes`
  - `update`'s `self.poll_project_refreshes();` call (~line 3493)
  - `activate_worktree` (~line 582)
  - the `delete_request.take()` handler (~line 1589)

**Interfaces:**
- Consumes: `wt::RemoveRequest` and `wt::spawn_remove(req, ctx) -> Receiver<Result<(), String>>` from Task 1 (`worktree` is imported as `wt` in app.rs).
- Produces (Task 3 depends on this exact field): `self.pending_worktree_deletes: HashMap<PathBuf, Receiver<Result<(), String>>>` — a worktree path present in this map is "deleting".

No unit test — this is `AlacritreeApp` state plumbing with no test harness (egui app, live PTYs). The gate is `cargo check` plus Task 3's manual verification; the underlying removal logic is covered by Task 1's tests.

- [ ] **Step 1: Add the field**

After the `pending_project_refresh` field (~line 169):

```rust
    /// In-flight background worktree removals, keyed by worktree path. A
    /// worktree in this map renders faded/non-interactive; results are
    /// adopted in `poll_worktree_deletes`.
    pending_worktree_deletes: HashMap<PathBuf, Receiver<Result<(), String>>>,
```

And in the constructor's `Self { ... }` literal (after `pending_project_refresh: HashMap::new(),` ~line 360):

```rust
            pending_worktree_deletes: HashMap::new(),
```

- [ ] **Step 2: Rewrite `run_pending_delete` to dispatch instead of block**

Replace the body from `let force = req.dirty.is_dirty();` through `self.refresh_project(ctx, req.project_idx);` (~lines 2938–2953). The session-kill / workspace-reset prologue above it stays unchanged. New tail:

```rust
        let rx = wt::spawn_remove(
            wt::RemoveRequest {
                project_root,
                worktree_path: req.worktree_path.clone(),
                worktree_name: req.worktree_name,
                branch: req.branch,
                prunable: req.prunable,
                delete_branch: req.delete_branch,
                force: req.dirty.is_dirty(),
            },
            ctx.clone(),
        );
        self.pending_worktree_deletes.insert(req.worktree_path, rx);
```

Note the deliberate changes: no `last_error` here (failures surface in the poll), and no `refresh_project` at dispatch — the row must stay listed so it can render as deleting. `req.project_idx` is no longer read; the poll resolves the project **by path** because indices can go stale (IPC `remove_project` reorders `projects`) while the removal runs. If the compiler now flags `project_idx` as dead, remove the field from `DeleteRequest` and from its construction site (~line 1518).

- [ ] **Step 3: Add `poll_worktree_deletes`**

Directly after `poll_project_refreshes` (~line 474):

```rust
    /// Adopt finished background removals: clear the in-flight marker,
    /// surface failures, and re-discover the owning project so the row
    /// disappears (or un-fades after a failure). The project is found by
    /// worktree path, not a stored index — `projects` can reorder while a
    /// removal runs.
    fn poll_worktree_deletes(&mut self, ctx: &Context) {
        let mut finished: Vec<(PathBuf, Result<(), String>)> = Vec::new();
        self.pending_worktree_deletes.retain(|path, rx| match rx.try_recv() {
            Ok(result) => {
                finished.push((path.clone(), result));
                false
            },
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        for (path, result) in finished {
            if let Err(e) = result {
                self.last_error = Some(format!("worktree removal failed: {e}"));
            }
            if let Some(idx) =
                self.projects.iter().position(|p| p.worktrees.iter().any(|w| w.path == path))
            {
                self.refresh_project(ctx, idx);
            }
        }
    }
```

Call it from `update`, right after the existing `self.poll_project_refreshes();` (~line 3493):

```rust
        self.poll_worktree_deletes(ctx);
```

- [ ] **Step 4: Add the re-entry guards**

Top of `activate_worktree` (~line 582), before the `!path.is_dir()` check:

```rust
        // A removal in flight owns the row; activating would spawn a shell
        // in a directory that is being deleted underneath it.
        if self.pending_worktree_deletes.contains_key(path) {
            return;
        }
```

This single guard covers both mouse (row click → `activate_request` → here) and keyboard (sidebar Enter calls `activate_worktree` directly, ~line 879).

Then the delete handler (~line 1589) — ignore a second delete for an in-flight path:

```rust
        if let Some(req) = delete_request.take() {
            if !self.pending_worktree_deletes.contains_key(&req.worktree_path) {
                self.pending_delete = Some(req);
            }
        }
```

- [ ] **Step 5: Check, test, commit**

Run: `cargo check -p alacritree` — expected: clean (a dead-code warning on `project_idx` means finish Step 2's removal note).
Run: `cargo test -p alacritree` — expected: all pass.

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): run worktree removal on a background thread"
```

---

### Task 3: Sidebar UI — faded spinner row while deleting

**Files:**
- Modify: `alacritree/src/app.rs`
  - `worktree_row` (~line 2463)
  - its call site in the projects loop (~line 1498)
  - the snapshot block before the sidebar panel closure (next to the `delete_request` Cell, ~line 1180)

**Interfaces:**
- Consumes: `self.pending_worktree_deletes` from Task 2.
- Produces: `worktree_row(ui, wt, deleting: bool, is_active, is_cursor, scroll_into_view, attention, agent_glyph, theme)` — `deleting` is the new second parameter.

- [ ] **Step 1: Snapshot the deleting set before the panel closure**

The projects loop holds `self.projects.iter_mut()` (~line 1333), so `self.pending_worktree_deletes` cannot be read inside it. Next to the `delete_request` Cell declaration (~line 1180), add:

```rust
        // Snapshot: the projects loop below borrows `self.projects` mutably,
        // so the in-flight set can't be read from `self` inside it.
        let deleting_paths: HashSet<PathBuf> =
            self.pending_worktree_deletes.keys().cloned().collect();
```

- [ ] **Step 2: Thread `deleting` into `worktree_row`**

Call site (~line 1498) — pass it as the second argument:

```rust
                                let action = worktree_row(
                                    ui,
                                    wt,
                                    deleting_paths.contains(&wt.path),
                                    is_active,
                                    is_cursor,
                                    cursor_moved,
                                    wt_attention,
                                    wt_glyph,
                                    &theme,
                                );
```

Signature (~line 2463):

```rust
fn worktree_row(
    ui: &mut egui::Ui,
    wt: &Worktree,
    deleting: bool,
    is_active: bool,
    is_cursor: bool,
    scroll_into_view: bool,
    attention: bool,
    agent_glyph: Option<char>,
    theme: &Theme,
) -> WorktreeAction {
```

- [ ] **Step 3: Render the deleting state**

Inside `worktree_row`, four edits:

(a) Fade the name — extend the existing `name_color` chain (~line 2487):

```rust
            let name_color = if deleting || wt.prunable {
                theme.text_muted
            } else if is_active {
                theme.text
            } else {
                theme.text_dim
            };
```

(b) Spinner instead of status icon — in the leading closure, wrap the `paint_row_status_icon` call (~line 2497):

```rust
                    if deleting {
                        // Spinner repaints itself every frame, keeping the
                        // animation alive without PTY or input events.
                        ui.add(
                            egui::Spinner::new()
                                .size(10.0 * theme.ui_scale)
                                .color(theme.text_muted),
                        );
                        ui.add_space(4.0 * theme.ui_scale);
                    } else {
                        paint_row_status_icon(
                            ui,
                            theme,
                            attention,
                            agent_glyph,
                            default_icon,
                            is_active,
                        );
                    }
```

(The `add_space` matches whatever gap `paint_row_status_icon`'s label leaves before the name; if the label has no explicit trailing space in the current layout, drop the `add_space` line and match what the non-deleting row shows.)

(c) No buttons — wrap the whole trailing closure body (~lines 2511–2529):

```rust
                |ui| {
                    if deleting {
                        return;
                    }
                    // ... existing × and + button code, unchanged ...
                },
```

(d) No hover highlight / hover text / actions — after the existing `resp` post-processing, gate the prunable hover text (~line 2535):

```rust
    let resp = if wt.prunable && !deleting {
        resp.on_hover_text("worktree directory is missing — × prunes it")
    } else {
        resp
    };
```

gate the hover background (~line 2555):

```rust
    let bg = if is_active {
        theme.row_active_bg
    } else if resp.hovered() && !deleting {
        theme.row_hover_bg
    } else {
        Color32::TRANSPARENT
    };
```

and force the returned action to all-false (~line 2572):

```rust
    if deleting {
        return WorktreeAction { activate: false, delete: false, spawn: false };
    }
    WorktreeAction {
        activate: resp.clicked() && !delete_clicked && !spawn_clicked && !wt.prunable,
        delete: delete_clicked,
        spawn: spawn_clicked,
    }
```

(The early return sits after the background/cursor painting so the row still paints its cursor outline; `delete_rect`/`spawn_rect` are `None` when deleting, so the click-routing recovery block is inert — no further edits needed there.)

- [ ] **Step 4: Check and test**

Run: `cargo check -p alacritree` — expected: clean.
Run: `cargo test -p alacritree` — expected: all pass.

- [ ] **Step 5: Manual verification (spec's test matrix)**

Run: `cargo run -p alacritree`, then:

1. Create a throwaway worktree on a real project, drop a bulky directory into it so the delete takes a few seconds (e.g. copy `target/` in), then × → confirm. Expected: modal closes instantly, UI stays responsive, the row fades with a spinner, its ×/+ are gone, clicking and Enter on it do nothing, other worktrees work; the row disappears when the delete finishes.
2. Confirm two deletes back-to-back. Expected: two spinners run concurrently.
3. Force a failure: open a file inside a worktree in another program (Windows file lock) or make the worktree dirty in a way `--force` can't fix mid-flight — easiest is holding a shell/`cmd` cwd'd inside it from *outside* alacritree — then delete. Expected: the row un-fades after the failure and the error bar shows the git message.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): fade deleting worktrees behind a spinner"
```

---

## Self-review notes

- Spec coverage: D1 (unbounded parallel) = per-confirm `spawn_remove` thread, no cap; D2 (prune backgrounds too) = `spawn_remove` routes on `prunable`; dispatch/poll/guards = Task 2; fade/spinner/non-interactive = Task 3; failure UX = `poll_worktree_deletes` + un-fade via refresh; project-by-root resolution = poll looks up by worktree path.
- The only spec item with no automated test is the egui rendering + app plumbing, matching the spec's "testable surface is thin" call; Task 1 covers both removal routes with real repos.
