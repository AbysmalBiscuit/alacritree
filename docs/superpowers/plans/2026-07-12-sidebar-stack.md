# Sidebar-Stack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `integration/sidebar-stack` branch (input + nav + multi-shell) and teach sidebar keyboard navigation to treat per-workspace session rows as first-class cursor stops — step onto them, open (Enter/Right), jump to parent (Left), and close (Ctrl+Shift+W).

**Architecture:** Part A assembles a linear rebase stack by cherry-picking each layer's unique commits onto a fresh branch, leaving the original feature branches untouched. Part B extends the pure `sidebar_nav.rs` row model with a `Session` variant, wires activation/close into `app.rs`, and adds a rebindable `CloseSession` action.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, `alacritty_terminal`, `git2`. Cargo workspace; only the `alacritree` crate is edited.

## Global Constraints

- All edits live in `alacritree/` (crate `alacritree`). Vendored `alacritty*` crates are read-only.
- Mirror upstream alacritty behavior where it already solves a problem; justify divergence in a comment.
- `SessionId = u64`, `WorkspaceKey = Option<PathBuf>` (`app.rs:23`), `Session.working_directory: Option<PathBuf>`.
- Sessions outlive workspace switches — never drop a `Session` because it isn't visible.
- Session rows render only at **2+ sessions per workspace** (`sidebar_session_ids` threshold) and, for worktrees, only when the owning project is expanded. The cursor model must match exactly what is painted.
- Comments explain *why*, not *what*. Conventional Commits for messages. `cargo fmt` is enforced.
- Build/test from the worktree: `cargo build -p alacritree`, `cargo test -p alacritree`.
- Spec/plan stay uncommitted (`docs/superpowers/` is git-excluded). Nothing pushed unless the user asks.

**Commit SHAs (verified 2026-07-12, base master `e27e3a0d`):**
- `fix/input-encoding` tip `e838a804` (5 commits; shared input base tip `06642c6b`).
- `fix/ime-input` tip `9b5ca95f` (10 commits; 7 unique above `06642c6b`).
- `feat/focus-navigation` tip `6a8dc22a` (15 commits = rebindable 6 + nav 9).
- `feat/multi-shell-ui` tip (10 commits).

---

## Task 1: Assemble the input layer (worktree + input-encoding + ime-input)

Create the branch/worktree at `fix/input-encoding`, then layer `fix/ime-input`'s unique commits on top.

**Files:** none edited by hand except conflict resolution in `alacritree/src/input.rs` (and possibly `alacritree/src/app.rs`).

- [ ] **Step 1: Create the worktree at the input-encoding tip**

```bash
cd C:/Users/Lev/Git/github/alacritree
git worktree add ../alacritree-worktrees/integration/sidebar-stack -b integration/sidebar-stack fix/input-encoding
cd ../alacritree-worktrees/integration/sidebar-stack
```

- [ ] **Step 2: Cherry-pick ime-input's unique commits**

The range excludes the shared 3-commit input base (`06642c6b` and below), replaying only the 7 IME commits:

```bash
git cherry-pick 06642c6b..fix/ime-input
```

- [ ] **Step 3: Resolve conflicts (expected in `input.rs`)**

IME commits were authored before input-encoding's kitty/mouse work, so overlapping edits in `input.rs` may conflict. Resolve keeping **both** behaviors (kitty disambiguation *and* IME routing). Then:

```bash
git add -A && git cherry-pick --continue
```

Repeat until the range completes.

- [ ] **Step 4: Build and test**

```bash
cargo build -p alacritree
cargo test -p alacritree
```

Expected: clean build; all input + ime tests pass.

- [ ] **Step 5: No extra commit** — the cherry-picks are the commits. Proceed.

---

## Task 2: Layer focus-navigation (rebindable-app-shortcuts + nav)

**Files:** conflict resolution likely in `alacritree/src/bindings.rs` and `alacritree/src/app.rs`.

- [ ] **Step 1: Cherry-pick the whole focus-navigation range**

`feat/focus-navigation` forks from master and already contains the 6 rebindable-app-shortcuts commits, so the range replays all 15:

```bash
git cherry-pick master..feat/focus-navigation
```

- [ ] **Step 2: Resolve conflicts**

Expect overlaps in `bindings.rs` (rebindable default-binding list vs the input-branch edits) and `input.rs`/`app.rs`. Keep both sides' behavior. `git add -A && git cherry-pick --continue` until complete.

- [ ] **Step 3: Build and test**

```bash
cargo build -p alacritree
cargo test -p alacritree
```

Expected: clean build; bindings + sidebar_nav tests pass.

---

## Task 3: Layer multi-shell-ui (the heavy merge)

**Files:** conflict resolution concentrated in `alacritree/src/app.rs` (both branches rewrite `home_row`, `worktree_row`, and the sidebar render loop); minor in `config.rs`/`session.rs`.

- [ ] **Step 1: Cherry-pick the multi-shell range**

```bash
git cherry-pick master..feat/multi-shell-ui
```

- [ ] **Step 2: Resolve the `app.rs` conflicts — preserve BOTH renderers**

At each conflicting commit, the merged sidebar render loop must keep:
- **focus-navigation's** cursor machinery: `cursor_row`, `cursor_moved`, `paint_cursor_outline`, and the `is_cursor`/`scroll_into_view` params on `worktree_row`.
- **multi-shell-ui's** session-row rendering: `home_session_rows`, `worktree_session_rows`, the `session_row(...)` calls, and the `+`/`×` request cells (`spawn_shell_request`, `activate_session_request`, `close_session_request`).

Concretely, `home_row` and `worktree_row` from multi-shell (which return `HomeAction`/`WorktreeAction` with a `spawn` field) must also carry focus-nav's `is_cursor` + `scroll_into_view` params. Reconcile the two signatures into one. `git add -A && git cherry-pick --continue` per commit.

- [ ] **Step 3: Build and test**

```bash
cargo build -p alacritree
cargo test -p alacritree
```

Expected: clean build; config `confirm_session_close` tests, session `looks_busy` tests, bindings, and sidebar_nav tests all pass.

- [ ] **Step 4: Manual smoke check**

```bash
cargo run -p alacritree
```

Verify the sidebar shows worktrees with cursor navigation AND, in a workspace with 2+ shells, the session rows render with `+`/`×`. This is the merged foundation for Part B.

---

## Task 4: Add `SidebarRow::Session` and interleave session rows

Move the pure grouping helper into `sidebar_nav.rs` and teach `visible_rows` about sessions.

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs`
- Modify: `alacritree/src/app.rs` (make `WorkspaceKey` pub; repoint `workspace_session_rows` at the moved helper)
- Test: `alacritree/src/sidebar_nav.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `SidebarRow::Session(SessionId)`; `pub fn sidebar_session_ids(pairs: &[(WorkspaceKey, SessionId)], ws: &WorkspaceKey) -> Vec<SessionId>`; `pub fn visible_rows(projects: &[Project], sessions: &[(WorkspaceKey, SessionId)]) -> Vec<SidebarRow>`.
- Consumes: `crate::session::SessionId`, `crate::app::WorkspaceKey`.

- [ ] **Step 1: Write the failing tests** (add to `sidebar_nav.rs` tests)

```rust
#[test]
fn sidebar_session_ids_below_threshold_is_empty() {
    let pairs = vec![(None, 1u64)];
    assert!(sidebar_session_ids(&pairs, &None).is_empty());
}

#[test]
fn sidebar_session_ids_lists_in_order_at_threshold() {
    let ws = Some(PathBuf::from("/a/wt1"));
    let pairs = vec![(ws.clone(), 1u64), (None, 9u64), (ws.clone(), 2u64)];
    assert_eq!(sidebar_session_ids(&pairs, &ws), vec![1, 2]);
}

#[test]
fn visible_rows_interleaves_session_rows_after_their_workspace() {
    let projects = vec![project("/a", true, &["/a/wt1"])];
    let sessions = vec![
        (None, 10u64),
        (Some(PathBuf::from("/a/wt1")), 20u64),
        (Some(PathBuf::from("/a/wt1")), 21u64),
    ];
    assert_eq!(
        visible_rows(&projects, &sessions),
        vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/a")),
            SidebarRow::Worktree(PathBuf::from("/a/wt1")),
            SidebarRow::Session(20),
            SidebarRow::Session(21),
        ]
    );
}

#[test]
fn visible_rows_hides_single_session_workspaces() {
    let projects = vec![project("/a", true, &["/a/wt1"])];
    let sessions = vec![(Some(PathBuf::from("/a/wt1")), 20u64)];
    assert_eq!(
        visible_rows(&projects, &sessions),
        vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/a")),
            SidebarRow::Worktree(PathBuf::from("/a/wt1")),
        ]
    );
}
```

Also update the existing `visible_rows`/`step`/`left_target` tests that call `visible_rows(&projects)` to `visible_rows(&projects, &[])`.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree sidebar_nav
```

Expected: FAIL — `SidebarRow::Session` and `sidebar_session_ids` not defined; `visible_rows` arity mismatch.

- [ ] **Step 3: Implement** — in `sidebar_nav.rs`

Add imports and the variant:

```rust
use crate::session::SessionId;
use crate::app::WorkspaceKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarRow {
    Home,
    Project(PathBuf),
    Worktree(PathBuf),
    /// Session row, keyed by its stable session id.  Only present when its
    /// workspace lists sessions (two-or-more threshold).
    Session(SessionId),
}
```

Move the grouping helper here (delete it from `app.rs`):

```rust
/// Spawn-ordered ids of the sessions in `ws`, or empty below the two-session
/// list threshold — a single-session workspace keeps its compact row, mirroring
/// the tab strip.  Pure over (workspace, id) pairs so the rule is testable.
pub fn sidebar_session_ids(pairs: &[(WorkspaceKey, SessionId)], ws: &WorkspaceKey) -> Vec<SessionId> {
    let ids: Vec<SessionId> = pairs.iter().filter(|(w, _)| w == ws).map(|(_, id)| *id).collect();
    if ids.len() < 2 { Vec::new() } else { ids }
}
```

Rewrite `visible_rows`:

```rust
pub fn visible_rows(projects: &[Project], sessions: &[(WorkspaceKey, SessionId)]) -> Vec<SidebarRow> {
    let mut rows = vec![SidebarRow::Home];
    rows.extend(sidebar_session_ids(sessions, &None).into_iter().map(SidebarRow::Session));
    for p in projects {
        rows.push(SidebarRow::Project(p.root.clone()));
        if p.expanded {
            for wt in &p.worktrees {
                rows.push(SidebarRow::Worktree(wt.path.clone()));
                let ws = Some(wt.path.clone());
                rows.extend(sidebar_session_ids(sessions, &ws).into_iter().map(SidebarRow::Session));
            }
        }
    }
    rows
}
```

In `app.rs`: change `type WorkspaceKey` to `pub type WorkspaceKey`, delete the local `sidebar_session_ids`, and point `workspace_session_rows` at `sidebar_nav::sidebar_session_ids`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree sidebar_nav
cargo build -p alacritree
```

Expected: PASS. (Build will still fail at the `apply_sidebar_nav` call site — fixed in Task 6. If executing tasks strictly independently, temporarily update that one call to `visible_rows(&self.projects, &[])` and note it; Task 6 replaces it.)

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/sidebar_nav.rs alacritree/src/app.rs
git commit -m "feat(sidebar): interleave session rows in the nav row model"
```

---

## Task 5: `left_target` and `seed` for session rows

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs`
- Test: `alacritree/src/sidebar_nav.rs`

**Interfaces:**
- Produces: `pub fn left_target(rows, cursor) -> Option<SidebarRow>` (now handles `Session`); `pub fn seed(projects: &[Project], current_workspace: Option<&Path>, sessions: &[(WorkspaceKey, SessionId)], active: Option<SessionId>) -> SidebarRow`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn left_target_of_session_is_its_owning_workspace_row() {
    let projects = vec![project("/a", true, &["/a/wt1"])];
    let sessions =
        vec![(Some(PathBuf::from("/a/wt1")), 20u64), (Some(PathBuf::from("/a/wt1")), 21u64)];
    let rows = visible_rows(&projects, &sessions);
    assert_eq!(
        left_target(&rows, &SidebarRow::Session(21)),
        Some(SidebarRow::Worktree(PathBuf::from("/a/wt1")))
    );
    let home_sessions = vec![(None, 1u64), (None, 2u64)];
    let rows = visible_rows(&[], &home_sessions);
    assert_eq!(left_target(&rows, &SidebarRow::Session(2)), Some(SidebarRow::Home));
}

#[test]
fn seed_lands_on_active_session_when_list_shown() {
    let projects = vec![project("/a", true, &["/a/wt1"])];
    let ws = PathBuf::from("/a/wt1");
    let sessions = vec![(Some(ws.clone()), 20u64), (Some(ws.clone()), 21u64)];
    assert_eq!(seed(&projects, Some(&ws), &sessions, Some(21)), SidebarRow::Session(21));
    // Below threshold -> the worktree row.
    let one = vec![(Some(ws.clone()), 20u64)];
    assert_eq!(seed(&projects, Some(&ws), &one, Some(20)), SidebarRow::Worktree(ws.clone()));
}
```

Update the existing `seed_lands_on_the_current_workspace_row` test's calls from `seed(&projects, Some(path))` to `seed(&projects, Some(path), &[], None)` (and `seed(&projects, None)` to `seed(&projects, None, &[], None)`).

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree sidebar_nav
```

Expected: FAIL — `seed` arity mismatch; `left_target` returns `None` for `Session`.

- [ ] **Step 3: Implement**

Extend `left_target` (keep Worktree→Project unchanged; add Session→nearest Worktree/Home):

```rust
pub fn left_target(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow> {
    let pos = rows.iter().position(|r| r == cursor)?;
    match cursor {
        SidebarRow::Worktree(_) => {
            rows[..pos].iter().rev().find(|r| matches!(r, SidebarRow::Project(_))).cloned()
        },
        SidebarRow::Session(_) => rows[..pos]
            .iter()
            .rev()
            .find(|r| matches!(r, SidebarRow::Worktree(_) | SidebarRow::Home))
            .cloned(),
        _ => None,
    }
}
```

Extend `seed`:

```rust
pub fn seed(
    projects: &[Project],
    current_workspace: Option<&Path>,
    sessions: &[(WorkspaceKey, SessionId)],
    active: Option<SessionId>,
) -> SidebarRow {
    let ws: WorkspaceKey = current_workspace.map(Path::to_path_buf);
    if let Some(id) = active {
        // Session rows for a worktree only show when its project is expanded.
        let shown = match &ws {
            None => true,
            Some(path) => projects
                .iter()
                .any(|p| p.expanded && p.worktrees.iter().any(|wt| wt.path == *path)),
        };
        if shown && sidebar_session_ids(sessions, &ws).contains(&id) {
            return SidebarRow::Session(id);
        }
    }
    let Some(path) = current_workspace else {
        return SidebarRow::Home;
    };
    for p in projects {
        if p.worktrees.iter().any(|wt| wt.path == path) {
            return if p.expanded {
                SidebarRow::Worktree(path.to_path_buf())
            } else {
                SidebarRow::Project(p.root.clone())
            };
        }
    }
    SidebarRow::Home
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree sidebar_nav
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/sidebar_nav.rs
git commit -m "feat(sidebar): jump-to-parent and seed for session rows"
```

---

## Task 6: Wire keyboard activation into `apply_sidebar_nav`

**Files:**
- Modify: `alacritree/src/app.rs` (`apply_sidebar_nav`, `focus_sidebar`, the `visible_rows`/`seed` call sites)

**Interfaces:**
- Consumes: `sidebar_nav::visible_rows`, `sidebar_nav::seed`, `sidebar_nav::left_target`, `SidebarRow::Session`.
- Produces: activating a `Session` cursor sets `self.current_workspace` + `self.active_session` for that session's workspace.

- [ ] **Step 1: Build a session-pairs snapshot at both call sites**

In `apply_sidebar_nav`, replace `let rows = sidebar_nav::visible_rows(&self.projects);` with:

```rust
let sessions: Vec<(WorkspaceKey, SessionId)> =
    self.sessions.iter().map(|s| (s.working_directory.clone(), s.id)).collect();
let rows = sidebar_nav::visible_rows(&self.projects, &sessions);
```

In `focus_sidebar`, update the seed call:

```rust
let sessions: Vec<(WorkspaceKey, SessionId)> =
    self.sessions.iter().map(|s| (s.working_directory.clone(), s.id)).collect();
let active = self.current_workspace.as_ref().and_then(|_| None).or_else(|| {
    self.active_session.get(&self.current_workspace).copied()
});
self.sidebar_cursor = Some(sidebar_nav::seed(
    &self.projects,
    self.current_workspace.as_deref(),
    &sessions,
    self.active_session.get(&self.current_workspace).copied(),
));
```

(Simplify to the single `seed(...)` call with `self.active_session.get(&self.current_workspace).copied()` as `active`; drop the scratch `active` binding.)

- [ ] **Step 2: Add the `Session` arms to the nav match**

The `match &cursor` arms in `apply_sidebar_nav` become non-exhaustive once `Session` exists — the compiler forces these additions:

```rust
Key::ArrowRight => match &cursor {
    SidebarRow::Project(root) => {
        let root = root.clone();
        self.set_project_expanded(&root, true);
    },
    SidebarRow::Session(id) => {
        let id = *id;
        self.activate_session_by_id(id);
        self.focus_terminal();
    },
    _ => {},
},
Key::ArrowLeft => match &cursor {
    SidebarRow::Project(root) => {
        let root = root.clone();
        self.set_project_expanded(&root, false);
    },
    SidebarRow::Worktree(_) | SidebarRow::Session(_) => {
        if let Some(target) = sidebar_nav::left_target(&rows, &cursor) {
            self.set_sidebar_cursor(target);
        }
    },
    SidebarRow::Home => {},
},
Key::Enter => match &cursor {
    SidebarRow::Home => {
        self.activate_home(ctx);
        self.focus_terminal();
    },
    SidebarRow::Worktree(path) => {
        let path = path.clone();
        self.activate_worktree(ctx, &path);
        self.focus_terminal();
    },
    SidebarRow::Session(id) => {
        let id = *id;
        self.activate_session_by_id(id);
        self.focus_terminal();
    },
    SidebarRow::Project(root) => {
        let root = root.clone();
        let expanded =
            self.projects.iter().find(|p| p.root == root).is_some_and(|p| p.expanded);
        self.set_project_expanded(&root, !expanded);
    },
},
```

- [ ] **Step 3: Add `activate_session_by_id`** (mirrors the mouse `activate_session_request` handler)

```rust
/// Switch to the session's workspace and mark it active — the keyboard
/// equivalent of clicking its sidebar row.  A stale id self-heals next
/// frame via `ensure_active_session`.
fn activate_session_by_id(&mut self, id: SessionId) {
    let Some(ws) = self.sessions.iter().find(|s| s.id == id).map(|s| s.working_directory.clone())
    else {
        return;
    };
    self.current_workspace = ws.clone();
    self.active_session.insert(ws, id);
}
```

- [ ] **Step 4: Build**

```bash
cargo build -p alacritree
cargo test -p alacritree
```

Expected: clean build; existing tests pass.

- [ ] **Step 5: Manual verification**

```bash
cargo run -p alacritree
```

Focus the sidebar; open a workspace with 2+ shells; confirm Up/Down land on each session row, Enter/Right switches to that shell, Left jumps back to the worktree/Home row.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "feat(sidebar): activate sessions from the keyboard cursor"
```

---

## Task 7: Highlight the cursored session row

**Files:**
- Modify: `alacritree/src/app.rs` (`session_row` signature + the two session-row render loops)

**Interfaces:**
- Consumes: `paint_cursor_outline`, `cursor_row: Option<SidebarRow>`, `cursor_moved: bool` (already in scope in the render fn).

- [ ] **Step 1: Add cursor params to `session_row`**

Mirror `worktree_row`'s `is_cursor` + `scroll_into_view`:

```rust
fn session_row(
    ui: &mut egui::Ui,
    row: &SessionRowData,
    is_cursor: bool,
    scroll_into_view: bool,
    theme: &Theme,
) -> SessionRowAction {
```

At the end of the function, before returning, paint the outline and auto-scroll (matching `worktree_row`):

```rust
if is_cursor {
    let rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
    paint_cursor_outline(ui, rect, theme);
    if scroll_into_view {
        ui.scroll_to_rect(rect, None);
    }
}
```

- [ ] **Step 2: Pass cursor state at both call sites**

In the Home session loop and the worktree session loop, compute `is_cursor` from `cursor_row` and pass `cursor_moved`:

```rust
for row in &home_session_rows {
    let is_cursor = matches!(&cursor_row, Some(SidebarRow::Session(id)) if *id == row.id);
    let act = session_row(ui, row, is_cursor, cursor_moved, &theme);
    // ...existing activate/close handling unchanged...
}
```

Apply the same `is_cursor` computation in the worktree session loop.

- [ ] **Step 3: Build and manually verify**

```bash
cargo build -p alacritree
cargo run -p alacritree
```

Expected: the accent outline follows the cursor onto session rows and auto-scrolls into view.

- [ ] **Step 4: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "feat(sidebar): highlight the cursored session row"
```

---

## Task 8: Rebindable `CloseSession` (Ctrl+Shift+W)

**Files:**
- Modify: `alacritree/src/bindings.rs` (`NamedAction` enum, `default_bindings`, `parse_action`)
- Modify: `alacritree/src/app.rs` (`dispatch_action` match arm)
- Test: `alacritree/src/bindings.rs`
- Modify: `docs/keyboard-shortcuts.md`

**Interfaces:**
- Produces: `NamedAction::CloseSession`; default binding `W` + `ctrl_shift`; config name `"CloseSession"`.
- Consumes: `self.focus`, `self.sidebar_cursor`, `self.request_close_session`, `self.active_session_index`.

- [ ] **Step 1: Write the failing binding tests** (add to `bindings.rs` tests)

```rust
#[test]
fn close_session_is_a_default_ctrl_shift_w_binding() {
    let b = parse_bindings(vec![]);
    assert_eq!(
        named_matches(&b, Key::W, Modifiers::CTRL | Modifiers::SHIFT),
        vec![NamedAction::CloseSession]
    );
}

#[test]
fn close_session_parses_from_config_name() {
    assert!(matches!(parse_action("CloseSession"), BindingAction::Named(NamedAction::CloseSession)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree --lib bindings
```

Expected: FAIL — `CloseSession` not a variant.

- [ ] **Step 3: Implement in `bindings.rs`**

Add the variant to `pub enum NamedAction` (near the app-level actions like `ToggleSidebarFocus`):

```rust
    CloseSession,
```

Add the default binding inside `default_bindings()` (near the other `ctrl_shift` bindings):

```rust
        KeyBinding { key: Key::W, mods: ctrl_shift, action: BindingAction::Named(CloseSession) },
```

Add the parse arm inside `parse_action` (near `"ToggleSidebarFocus"`):

```rust
        "CloseSession" => BindingAction::Named(CloseSession),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree --lib bindings
```

Expected: PASS.

- [ ] **Step 5: Dispatch the action in `app.rs`**

Add to the `dispatch_action` match (near `ToggleSidebarFocus`). Context-sensitive per the Linux "close current tab" convention:

```rust
BindingAction::Named(NamedAction::CloseSession) => {
    let cursored = if self.focus == PaneFocus::ProjectsSidebar {
        match &self.sidebar_cursor {
            Some(SidebarRow::Session(id)) => Some(*id),
            _ => None,
        }
    } else {
        None
    };
    let target = cursored.or_else(|| {
        self.active_session_index().map(|idx| self.sessions[idx].id)
    });
    if let Some(id) = target {
        self.request_close_session(id);
    }
},
```

- [ ] **Step 6: Document the shortcut** — add a row to `docs/keyboard-shortcuts.md` next to the other sidebar actions:

```markdown
| `Ctrl+Shift+W` | Close the cursored session (sidebar) or the current shell |
```

- [ ] **Step 7: Build, test, manually verify**

```bash
cargo build -p alacritree
cargo test -p alacritree
cargo run -p alacritree
```

Verify: with the sidebar focused on a session row, Ctrl+Shift+W closes that session (honoring `confirm_session_close`); with the terminal focused, it closes the on-screen shell.

- [ ] **Step 8: Commit**

```bash
git add alacritree/src/bindings.rs alacritree/src/app.rs docs/keyboard-shortcuts.md
git commit -m "feat(bindings): add rebindable CloseSession (ctrl+shift+w)"
```

---

## Self-Review

**Spec coverage:**
- Part A linear rebase stack (input-encoding → ime-input → focus-nav → multi-shell) → Tasks 1–3. ✓
- `SidebarRow::Session` + `visible_rows` interleave + threshold → Task 4. ✓
- `left_target` + `seed` for sessions → Task 5. ✓
- Enter/Right activate, Left jump, cursor-invalidation fallback → Task 6 (fallback: `apply_sidebar_nav`'s existing "cursor not in rows → Home" guard already covers a vanished session; `left_target`/`seed` reseed to the workspace row). ✓
- Cursor-outline rendering on session rows → Task 7. ✓
- `CloseSession` Ctrl+Shift+W rebindable + context-sensitive + confirm policy → Task 8. ✓
- Tests for interleave/threshold/left_target/seed → Tasks 4–5; binding tests → Task 8. ✓
- Deferred keyboard spawn → not implemented, per spec. ✓

**Placeholder scan:** none — every code step carries real code.

**Type consistency:** `SessionId = u64`, `WorkspaceKey = Option<PathBuf>` used uniformly; `sidebar_session_ids`/`visible_rows`/`seed`/`left_target` signatures match across Tasks 4–6; `activate_session_by_id`, `request_close_session`, `active_session_index` names consistent with the branch code.

**Note on cross-task build:** Task 4 changes `visible_rows`'s arity but its `app.rs` caller isn't fixed until Task 6. If tasks are executed in strict isolation, apply the one-line stopgap noted in Task 4 Step 4; otherwise Tasks 4→6 land together and the branch builds green at Task 6.
