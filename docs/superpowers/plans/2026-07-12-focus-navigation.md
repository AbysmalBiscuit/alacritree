# Keyboard Focus Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Configurable shortcuts move keyboard focus between the projects sidebar and the terminal; while the sidebar is focused, Up/Down/Left/Right/Enter/Escape navigate rows, expand/collapse projects, and activate worktrees — no mouse required.

**Architecture:** An app-owned `PaneFocus` enum gates the terminal's per-frame egui focus grab; a dedicated event-interception pass (same `ctx.input_mut` retain pattern as `handle_shortcuts`) consumes six unmodified nav keys while the sidebar owns focus. Cursor logic is pure functions in a new `sidebar_nav.rs` module keyed by stable row identities (paths, not indices). Three new `NamedAction`s ride the existing rebindable-bindings system.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe 0.31, existing alacritree crate only.

Spec: `docs/superpowers/specs/2026-07-12-focus-navigation-design.md` (in the **main checkout** at `C:/Users/Lev/Git/github/alacritree` — spec/plan files are git-excluded and exist only there, not in worktrees).

## Global Constraints

- Branch `feat/focus-navigation`, worktree `C:/Users/Lev/Git/github/alacritree-worktrees/feat/focus-navigation`, based on `feat/rebindable-app-shortcuts` (NOT master — the binding plumbing only exists there).
- All edits inside `alacritree/`; vendored crates (`alacritty*`) are read-only.
- No new dependencies.
- `cargo fmt` before every commit (rustfmt is enforced).
- Conventional Commits, imperative subject ≤50 chars, lowercase after the colon.
- Comments explain *why*, never restate the code; no task/PR references in comments.
- Never `git add` anything under `docs/specs/` or `docs/superpowers/` — those are local-only (git-excluded in the main checkout; do not recreate them in the worktree).
- All `cargo` commands run from the worktree root `C:/Users/Lev/Git/github/alacritree-worktrees/feat/focus-navigation`.

---

### Task 1: Worktree setup + sidebar row flattening

**Files:**
- Create: `alacritree/src/sidebar_nav.rs`
- Modify: `alacritree/src/main.rs` (module declaration list)

**Interfaces:**
- Consumes: `crate::projects::{Project, Worktree}` (public fields: `Project { root: PathBuf, name: String, default_branch: Option<String>, worktrees: Vec<Worktree>, expanded: bool }`, `Worktree { name: String, path: PathBuf, branch: Option<String>, is_main: bool }`).
- Produces: `pub enum SidebarRow { Home, Project(PathBuf), Worktree(PathBuf) }` (derives `Debug, Clone, PartialEq, Eq`) and `pub fn visible_rows(projects: &[Project]) -> Vec<SidebarRow>`. Tasks 2, 5, 6 rely on these exact names.

- [ ] **Step 1: Create the worktree and verify a green baseline**

```bash
cd C:/Users/Lev/Git/github/alacritree
git worktree add ../alacritree-worktrees/feat/focus-navigation -b feat/focus-navigation feat/rebindable-app-shortcuts
cd ../alacritree-worktrees/feat/focus-navigation
cargo test -p alacritree
```

Expected: worktree created; all existing tests PASS (the rebindable branch has 11+ passing tests). If the baseline fails, STOP and report — do not build on a red base.

- [ ] **Step 2: Write the failing tests**

Create `alacritree/src/sidebar_nav.rs`:

```rust
//! Pure cursor model for keyboard navigation of the projects sidebar.
//!
//! Rows are identified by stable keys (project root / worktree path), not
//! indices: the project list mutates underneath the cursor (git-status
//! refresh, worktree add/remove), and an index would silently retarget.

use std::path::PathBuf;

use crate::projects::Project;

/// A row the sidebar cursor can rest on, in render order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarRow {
    Home,
    /// Project header, keyed by the project root.
    Project(PathBuf),
    /// Worktree row, keyed by the worktree path.
    Worktree(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::Worktree;

    pub(super) fn project(root: &str, expanded: bool, worktrees: &[&str]) -> Project {
        Project {
            root: PathBuf::from(root),
            name: root.to_string(),
            default_branch: None,
            worktrees: worktrees
                .iter()
                .map(|p| Worktree {
                    name: p.to_string(),
                    path: PathBuf::from(p),
                    branch: None,
                    is_main: false,
                })
                .collect(),
            expanded,
        }
    }

    #[test]
    fn visible_rows_lists_home_then_projects_in_render_order() {
        let projects =
            vec![project("/a", true, &["/a/wt1", "/a/wt2"]), project("/b", true, &["/b/wt1"])];
        assert_eq!(visible_rows(&projects), vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/a")),
            SidebarRow::Worktree(PathBuf::from("/a/wt1")),
            SidebarRow::Worktree(PathBuf::from("/a/wt2")),
            SidebarRow::Project(PathBuf::from("/b")),
            SidebarRow::Worktree(PathBuf::from("/b/wt1")),
        ]);
    }

    #[test]
    fn visible_rows_hides_worktrees_of_collapsed_projects() {
        let projects = vec![project("/a", false, &["/a/wt1"])];
        assert_eq!(visible_rows(&projects), vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/a")),
        ]);
    }

    #[test]
    fn visible_rows_with_no_projects_is_just_home() {
        assert_eq!(visible_rows(&[]), vec![SidebarRow::Home]);
    }
}
```

Register the module in `alacritree/src/main.rs` — the declaration list is alphabetical; insert between `mod session;` and `mod state;`:

```rust
mod sidebar_nav;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alacritree sidebar_nav`
Expected: COMPILE ERROR — `cannot find function visible_rows`. That is the correct RED (the enum exists, the function doesn't).

- [ ] **Step 4: Write the minimal implementation**

Add to `sidebar_nav.rs` above the `tests` module:

```rust
/// Every row the sidebar currently renders, in render order: Home first,
/// then each project's header followed by its worktrees when expanded.
pub fn visible_rows(projects: &[Project]) -> Vec<SidebarRow> {
    let mut rows = vec![SidebarRow::Home];
    for p in projects {
        rows.push(SidebarRow::Project(p.root.clone()));
        if p.expanded {
            rows.extend(p.worktrees.iter().map(|wt| SidebarRow::Worktree(wt.path.clone())));
        }
    }
    rows
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alacritree sidebar_nav`
Expected: 3 passed. (`cargo check -p alacritree` may warn about unused `SidebarRow`/`visible_rows` outside tests — fine until Task 4 wires them in; do not add `#[allow(dead_code)]`.)

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/sidebar_nav.rs alacritree/src/main.rs
git commit -m "feat(sidebar): add row model for keyboard nav"
```

---

### Task 2: Cursor stepping, left-jump, and seeding

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs`

**Interfaces:**
- Consumes: `SidebarRow`, `visible_rows` from Task 1.
- Produces (Task 5 calls these exact signatures):
  - `pub fn step(rows: &[SidebarRow], cursor: &SidebarRow, delta: i32) -> SidebarRow`
  - `pub fn left_target(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow>`
  - `pub fn seed(projects: &[Project], current_workspace: Option<&Path>) -> SidebarRow`

- [ ] **Step 1: Write the failing tests**

Append inside the existing `tests` module in `sidebar_nav.rs`:

```rust
    #[test]
    fn step_moves_and_clamps_at_both_ends() {
        let rows = visible_rows(&[project("/a", true, &["/a/wt1"])]);
        // Home -> Project -> Worktree
        assert_eq!(step(&rows, &SidebarRow::Home, 1), SidebarRow::Project(PathBuf::from("/a")));
        assert_eq!(
            step(&rows, &SidebarRow::Project(PathBuf::from("/a")), 1),
            SidebarRow::Worktree(PathBuf::from("/a/wt1"))
        );
        // Clamp: no wrap in either direction.
        assert_eq!(step(&rows, &SidebarRow::Home, -1), SidebarRow::Home);
        assert_eq!(
            step(&rows, &SidebarRow::Worktree(PathBuf::from("/a/wt1")), 1),
            SidebarRow::Worktree(PathBuf::from("/a/wt1"))
        );
    }

    #[test]
    fn step_from_vanished_cursor_falls_back_to_home() {
        let rows = visible_rows(&[project("/a", true, &["/a/wt1"])]);
        let gone = SidebarRow::Worktree(PathBuf::from("/a/removed"));
        assert_eq!(step(&rows, &gone, 1), SidebarRow::Home);
    }

    #[test]
    fn left_target_is_the_owning_project_header() {
        let rows =
            vec![project("/a", true, &["/a/wt1"]), project("/b", true, &["/b/wt1", "/b/wt2"])];
        let rows = visible_rows(&rows);
        assert_eq!(
            left_target(&rows, &SidebarRow::Worktree(PathBuf::from("/b/wt2"))),
            Some(SidebarRow::Project(PathBuf::from("/b")))
        );
        // Only worktree rows have a left-jump target.
        assert_eq!(left_target(&rows, &SidebarRow::Home), None);
        assert_eq!(left_target(&rows, &SidebarRow::Project(PathBuf::from("/a"))), None);
    }

    #[test]
    fn seed_lands_on_the_current_workspace_row() {
        use std::path::Path;
        let projects = vec![project("/a", true, &["/a/wt1"]), project("/b", false, &["/b/wt1"])];
        // Expanded project: the worktree row itself.
        assert_eq!(
            seed(&projects, Some(Path::new("/a/wt1"))),
            SidebarRow::Worktree(PathBuf::from("/a/wt1"))
        );
        // Collapsed project: its header stands in for the hidden row.
        assert_eq!(
            seed(&projects, Some(Path::new("/b/wt1"))),
            SidebarRow::Project(PathBuf::from("/b"))
        );
        // Home workspace and unknown paths both land on Home.
        assert_eq!(seed(&projects, None), SidebarRow::Home);
        assert_eq!(seed(&projects, Some(Path::new("/nowhere"))), SidebarRow::Home);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree sidebar_nav`
Expected: COMPILE ERROR — `cannot find function step` (and `left_target`, `seed`).

- [ ] **Step 3: Write the implementation**

Add above the `tests` module (note the `Path` import joins the existing `use std::path::PathBuf;` line):

```rust
use std::path::{Path, PathBuf};
```

```rust
/// The row `delta` steps away from `cursor`, clamped to the list ends.
/// A cursor no longer in `rows` (worktree removed, project collapsed) falls
/// back to Home rather than guessing a neighbor.
pub fn step(rows: &[SidebarRow], cursor: &SidebarRow, delta: i32) -> SidebarRow {
    let Some(pos) = rows.iter().position(|r| r == cursor) else {
        return SidebarRow::Home;
    };
    let last = rows.len() as i32 - 1;
    let new = (pos as i32 + delta).clamp(0, last) as usize;
    rows[new].clone()
}

/// The project header owning a worktree row — the standard tree-view
/// "Left jumps to parent" idiom.  `None` for Home and project cursors.
pub fn left_target(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow> {
    if !matches!(cursor, SidebarRow::Worktree(_)) {
        return None;
    }
    let pos = rows.iter().position(|r| r == cursor)?;
    rows[..pos].iter().rev().find(|r| matches!(r, SidebarRow::Project(_))).cloned()
}

/// Where the cursor lands when the sidebar gains focus: the current
/// workspace's row, its project header when that project is collapsed,
/// Home otherwise.
pub fn seed(projects: &[Project], current_workspace: Option<&Path>) -> SidebarRow {
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

Run: `cargo test -p alacritree sidebar_nav`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/sidebar_nav.rs
git commit -m "feat(sidebar): add cursor stepping and seeding"
```

---

### Task 3: Focus-navigation binding actions

**Files:**
- Modify: `alacritree/src/bindings.rs`

**Interfaces:**
- Consumes: existing `NamedAction`, `parse_action`, `default_bindings`, `parse_bindings` (all in `bindings.rs`), test helpers `raw_action` / `named_matches`.
- Produces: `NamedAction::{ToggleSidebarFocus, FocusProjectsSidebar, FocusTerminal}` and a `Ctrl+Shift+B → ToggleSidebarFocus` default. Task 4's `dispatch_action` arms match on these exact variant names.

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `bindings.rs`, extend the array in `new_action_names_parse` with three entries:

```rust
            ("ToggleSidebarFocus", NamedAction::ToggleSidebarFocus),
            ("FocusProjectsSidebar", NamedAction::FocusProjectsSidebar),
            ("FocusTerminal", NamedAction::FocusTerminal),
```

Extend the array in `default_app_shortcuts_present_without_user_config` with one entry:

```rust
            (Key::B, ctrl_shift, ToggleSidebarFocus),
```

Add one new test (the "free Ctrl+Shift+B" path, mirroring `user_binding_replaces_same_trigger_default`):

```rust
    #[test]
    fn user_binding_replaces_sidebar_focus_default() {
        let b = parse_bindings(vec![raw_action("B", Some("Control|Shift"), "ReceiveChar")]);
        assert_eq!(
            named_matches(&b, Key::B, Modifiers::CTRL | Modifiers::SHIFT),
            vec![NamedAction::ReceiveChar]
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree bindings`
Expected: COMPILE ERROR — `no variant named ToggleSidebarFocus`. Correct RED.

- [ ] **Step 3: Implement**

In the `NamedAction` enum, after the `AddProject` variant:

```rust
    ToggleSidebarFocus,
    FocusProjectsSidebar,
    FocusTerminal,
```

In `parse_action`, after the `"AddProject"` arm:

```rust
        "ToggleSidebarFocus" => BindingAction::Named(ToggleSidebarFocus),
        "FocusProjectsSidebar" => BindingAction::Named(FocusProjectsSidebar),
        "FocusTerminal" => BindingAction::Named(FocusTerminal),
```

In `default_bindings`, inside the app-level `b.extend([...])` block, after the `AddProject` entry:

```rust
        KeyBinding {
            key: Key::B,
            mods: ctrl_shift,
            action: BindingAction::Named(ToggleSidebarFocus),
        },
```

`FocusProjectsSidebar` and `FocusTerminal` get **no** default keys — they exist for users who want explicit directional bindings.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree bindings`
Expected: all bindings tests pass (previous 9 + 1 new; two extended). Then `cargo check -p alacritree` — expect a non-exhaustive-match ERROR is **not** possible (`dispatch_action` has a `BindingAction::Named(other)` catch-all that routes unknown named actions to `dispatch_scroll_or_other`, a no-op for these), so the crate compiles. The new actions are dead until Task 4.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs
git commit -m "feat(bindings): add focus navigation actions"
```

---

### Task 4: Pane focus state, dispatch arms, terminal gating

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `crate::sidebar_nav::{self, SidebarRow}` (Tasks 1–2), `NamedAction::{ToggleSidebarFocus, FocusProjectsSidebar, FocusTerminal}` (Task 3).
- Produces (Task 5 and 6 rely on these): fields `self.focus: PaneFocus`, `self.sidebar_cursor: Option<SidebarRow>`, `self.sidebar_auto_shown: bool`, `self.sidebar_cursor_moved: bool`; methods `fn focus_sidebar(&mut self)`, `fn focus_terminal(&mut self)`.

No unit tests — everything here needs an `egui::Context`; verification is `cargo check` plus the Task 7 manual checklist. Keep the diff mechanical.

- [ ] **Step 1: Add the import, enum, and fields**

Near the top of `app.rs`, add to the crate imports:

```rust
use crate::sidebar_nav::{self, SidebarRow};
```

Below the `Theme` impl (before `pub struct AlacritreeApp`), add:

```rust
/// Which pane owns keyboard input.  The terminal re-requests egui focus
/// every frame while it owns this; anything else holding focus (modals
/// aside) must win here first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneFocus {
    Terminal,
    ProjectsSidebar,
}
```

In `struct AlacritreeApp`, after the `show_right_sidebar: bool` field:

```rust
    focus: PaneFocus,
    sidebar_cursor: Option<SidebarRow>,
    /// The focus toggle opened a hidden sidebar; returning focus closes it
    /// again so a keyboard round trip leaves the layout untouched.
    sidebar_auto_shown: bool,
    /// One-shot: scroll the cursor row into view on the next sidebar paint.
    sidebar_cursor_moved: bool,
```

In `AlacritreeApp::new`, in the `Self { ... }` literal after `show_right_sidebar: persisted.show_right_sidebar,`:

```rust
            focus: PaneFocus::Terminal,
            sidebar_cursor: None,
            sidebar_auto_shown: false,
            sidebar_cursor_moved: false,
```

- [ ] **Step 2: Add the focus-transition helpers**

In the first `impl AlacritreeApp` block, after `fn is_modal_open`:

```rust
    fn focus_sidebar(&mut self) {
        if !self.show_left_sidebar {
            self.show_left_sidebar = true;
            self.sidebar_auto_shown = true;
            self.persist();
        }
        self.focus = PaneFocus::ProjectsSidebar;
        self.sidebar_cursor =
            Some(sidebar_nav::seed(&self.projects, self.current_workspace.as_deref()));
        self.sidebar_cursor_moved = true;
    }

    fn focus_terminal(&mut self) {
        self.focus = PaneFocus::Terminal;
        if self.sidebar_auto_shown {
            self.show_left_sidebar = false;
            self.sidebar_auto_shown = false;
            self.persist();
        }
    }
```

- [ ] **Step 3: Dispatch arms and the visibility-toggle interaction**

In `dispatch_action`, after the `AddProject` arm:

```rust
            BindingAction::Named(NamedAction::ToggleSidebarFocus) => match self.focus {
                PaneFocus::Terminal => self.focus_sidebar(),
                PaneFocus::ProjectsSidebar => self.focus_terminal(),
            },
            BindingAction::Named(NamedAction::FocusProjectsSidebar) => {
                if self.focus != PaneFocus::ProjectsSidebar {
                    self.focus_sidebar();
                }
            },
            BindingAction::Named(NamedAction::FocusTerminal) => self.focus_terminal(),
```

Replace the existing `ToggleLeftSidebar` arm body with:

```rust
            BindingAction::Named(NamedAction::ToggleLeftSidebar) => {
                self.show_left_sidebar = !self.show_left_sidebar;
                // A deliberate visibility change opts out of the auto-shown
                // round trip, and a hidden sidebar cannot keep keyboard focus.
                self.sidebar_auto_shown = false;
                if !self.show_left_sidebar && self.focus == PaneFocus::ProjectsSidebar {
                    self.focus = PaneFocus::Terminal;
                }
                self.persist();
            },
```

- [ ] **Step 4: Gate the terminal's focus grab and reclaim on click**

In `update()`, the terminal call currently reads:

```rust
                let session = &mut self.sessions[idx];
                let _ = terminal_view::show(
                    ui,
                    session,
                    &self.config,
                    !modal_open,
                    &mut self.builtin_glyphs,
                );
```

Replace with:

```rust
                let session = &mut self.sessions[idx];
                let response = terminal_view::show(
                    ui,
                    session,
                    &self.config,
                    !modal_open && self.focus == PaneFocus::Terminal,
                    &mut self.builtin_glyphs,
                );
                if response.clicked() && self.focus != PaneFocus::Terminal {
                    self.focus_terminal();
                }
```

- [ ] **Step 5: Verify it compiles and existing tests still pass**

Run: `cargo check -p alacritree && cargo test -p alacritree`
Expected: clean check (the cursor/moved fields are written but not yet read by rendering — no dead-code warnings since they're struct fields), all tests pass. `Ctrl+Shift+B` now flips focus, but nothing visible changes until Tasks 5–6.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): own pane focus, gate terminal grab"
```

---

### Task 5: Sidebar navigation key pass

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `sidebar_nav::{visible_rows, step, left_target}` (Tasks 1–2), `focus_terminal`, `sidebar_cursor`, `sidebar_cursor_moved` (Task 4), existing `activate_home` / `activate_worktree` / `persist`.
- Produces: `fn handle_sidebar_nav(&mut self, ctx: &Context)` wired into `update()`; helpers `set_sidebar_cursor`, `set_project_expanded` (Task 6's rendering does not call these, but Enter/expand behavior is what the manual checklist exercises).

- [ ] **Step 1: Implement the interception pass**

In the first `impl AlacritreeApp` block, after `handle_shortcuts`:

```rust
    /// Arrow/Enter/Escape navigation while the projects sidebar owns
    /// keyboard focus.  Consumes only unmodified keys, so modifier-bound
    /// app shortcuts still match in `handle_shortcuts` afterwards.
    fn handle_sidebar_nav(&mut self, ctx: &Context) {
        use egui::Key;
        let keys: Vec<Key> = ctx.input_mut(|i| {
            let mut pressed = Vec::new();
            i.events.retain(|ev| {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    if modifiers.is_none() && is_sidebar_nav_key(*key) {
                        pressed.push(*key);
                        return false;
                    }
                }
                true
            });
            pressed
        });
        for key in keys {
            self.apply_sidebar_nav(ctx, key);
        }
    }

    fn apply_sidebar_nav(&mut self, ctx: &Context, key: egui::Key) {
        use egui::Key;
        let rows = sidebar_nav::visible_rows(&self.projects);
        let cursor = match self.sidebar_cursor.clone() {
            Some(c) if rows.contains(&c) => c,
            // Stale or unseeded cursor (worktree removed, project collapsed
            // by mouse): land on Home and let the next press act from there.
            _ => {
                self.set_sidebar_cursor(SidebarRow::Home);
                return;
            },
        };
        match key {
            Key::ArrowUp => self.set_sidebar_cursor(sidebar_nav::step(&rows, &cursor, -1)),
            Key::ArrowDown => self.set_sidebar_cursor(sidebar_nav::step(&rows, &cursor, 1)),
            Key::ArrowRight => {
                if let SidebarRow::Project(root) = &cursor {
                    self.set_project_expanded(root, true);
                }
            },
            Key::ArrowLeft => match &cursor {
                SidebarRow::Project(root) => self.set_project_expanded(root, false),
                SidebarRow::Worktree(_) => {
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
                SidebarRow::Project(root) => {
                    let root = root.clone();
                    let expanded = self
                        .projects
                        .iter()
                        .find(|p| p.root == root)
                        .is_some_and(|p| p.expanded);
                    self.set_project_expanded(&root, !expanded);
                },
            },
            Key::Escape => self.focus_terminal(),
            _ => {},
        }
    }

    fn set_sidebar_cursor(&mut self, row: SidebarRow) {
        if self.sidebar_cursor.as_ref() != Some(&row) {
            self.sidebar_cursor = Some(row);
            self.sidebar_cursor_moved = true;
        }
    }

    fn set_project_expanded(&mut self, root: &Path, expanded: bool) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.root == *root) {
            if p.expanded != expanded {
                p.expanded = expanded;
                self.persist();
            }
        }
    }
```

Add the free function next to the other file-level helpers (e.g. after `row_with_trailing`):

```rust
fn is_sidebar_nav_key(key: egui::Key) -> bool {
    use egui::Key;
    matches!(
        key,
        Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Enter | Key::Escape
    )
}
```

- [ ] **Step 2: Wire it into `update()`**

Replace:

```rust
        let modal_open = self.is_modal_open();
        if !modal_open {
            self.handle_shortcuts(ctx);
        }
```

with (nav runs first so plain arrows are consumed before any binding could see them; shortcuts still run so Ctrl+B / Ctrl+Shift+B / Ctrl+Q work from the sidebar):

```rust
        let modal_open = self.is_modal_open();
        if !modal_open {
            if self.focus == PaneFocus::ProjectsSidebar {
                self.handle_sidebar_nav(ctx);
            }
            self.handle_shortcuts(ctx);
        }
```

- [ ] **Step 3: Verify compile + tests**

Run: `cargo check -p alacritree && cargo test -p alacritree`
Expected: clean; all tests pass. (`Path` is already imported in `app.rs` via `std::path::{Path, PathBuf}` — if not, extend the existing `use std::path` line.)

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): sidebar keyboard navigation pass"
```

---

### Task 6: Cursor highlight and scroll-into-view

**Files:**
- Modify: `alacritree/src/app.rs` (`show_project_sidebar`, `home_row`, `worktree_row`, `row_with_trailing`)

**Interfaces:**
- Consumes: `self.sidebar_cursor`, `self.sidebar_cursor_moved` (Task 4), `theme.accent`.
- Produces: visible 1 px accent outline on the cursor row; cursor row scrolled into view on movement. Signature changes: `home_row` and `worktree_row` gain `is_cursor: bool, scroll_into_view: bool` parameters; `row_with_trailing` returns `egui::Rect`.

- [ ] **Step 1: Make `row_with_trailing` return the row rect**

Change its signature and tail (the body is otherwise unchanged — `allocate_ui_with_layout` already produces the response):

```rust
fn row_with_trailing<L, T>(ui: &mut egui::Ui, leading: L, trailing: T) -> egui::Rect
where
    L: FnOnce(&mut egui::Ui),
    T: FnOnce(&mut egui::Ui),
{
    let row_size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(row_size, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        trailing(ui);
        let remaining = ui.available_width();
        if remaining <= 0.0 {
            return;
        }
        let row_h = ui.available_height();
        ui.allocate_ui_with_layout(
            egui::vec2(remaining, row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            leading,
        );
    })
    .response
    .rect
}
```

(The call inside `worktree_row` already ends with a semicolon, so the returned `Rect` is simply discarded there — no change needed at that call site.)

- [ ] **Step 2: Add a shared cursor-outline helper**

Next to `home_row` at file level:

```rust
/// Keyboard-cursor indicator: an outline rather than a fill so it stays
/// legible on top of the active row's lightened background.
fn paint_cursor_outline(ui: &egui::Ui, rect: egui::Rect, theme: &Theme) {
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, theme.accent),
        egui::StrokeKind::Inside,
    );
}
```

- [ ] **Step 3: Thread cursor state into `home_row` and `worktree_row`**

`home_row` signature becomes:

```rust
fn home_row(
    ui: &mut egui::Ui,
    is_active: bool,
    is_cursor: bool,
    scroll_into_view: bool,
    attention: bool,
    agent_glyph: Option<char>,
    theme: &Theme,
) -> egui::Response {
```

After the existing background `if bg != Color32::TRANSPARENT { ... }` block, before `resp`:

```rust
    if is_cursor {
        paint_cursor_outline(ui, hit_rect, theme);
        if scroll_into_view {
            ui.scroll_to_rect(hit_rect, None);
        }
    }
```

`worktree_row` signature becomes:

```rust
fn worktree_row(
    ui: &mut egui::Ui,
    wt: &Worktree,
    is_active: bool,
    is_cursor: bool,
    scroll_into_view: bool,
    attention: bool,
    agent_glyph: Option<char>,
    theme: &Theme,
) -> WorktreeAction {
```

Its background block currently computes the full-width rect only inside the `if`; hoist it so the outline can reuse it:

```rust
    let full_rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
    if bg != Color32::TRANSPARENT {
        ui.painter().set(bg_idx, egui::Shape::rect_filled(full_rect, 0.0, bg));
    }
    if is_cursor {
        paint_cursor_outline(ui, full_rect, theme);
        if scroll_into_view {
            ui.scroll_to_rect(full_rect, None);
        }
    }
```

- [ ] **Step 4: Drive it from `show_project_sidebar`**

At the top of `show_project_sidebar`, after `let theme = self.theme;`:

```rust
        let cursor_row =
            if self.focus == PaneFocus::ProjectsSidebar { self.sidebar_cursor.clone() } else { None };
        let cursor_moved = std::mem::take(&mut self.sidebar_cursor_moved);
```

(`take` clears the one-shot flag whether or not the row renders this frame; the outline itself is hidden whenever the terminal owns focus.)

Home call site becomes:

```rust
                    if home_row(
                        ui,
                        self.current_workspace.is_none(),
                        matches!(&cursor_row, Some(SidebarRow::Home)),
                        cursor_moved,
                        home_attention,
                        home_agent_glyph,
                        &theme,
                    )
                    .clicked()
```

Project header: capture the rect and outline it when it is the cursor. The `row_with_trailing(ui, |ui| {...}, |ui| {...});` call for the project row becomes:

```rust
                        let row_rect = row_with_trailing(
                            ui,
                            /* leading closure unchanged */,
                            /* trailing closure unchanged */,
                        );
                        if matches!(&cursor_row, Some(SidebarRow::Project(r)) if *r == project.root)
                        {
                            let rect = egui::Rect::from_x_y_ranges(
                                ui.max_rect().x_range(),
                                row_rect.y_range(),
                            );
                            paint_cursor_outline(ui, rect, &theme);
                            if cursor_moved {
                                ui.scroll_to_rect(rect, None);
                            }
                        }
```

(Copy the two existing closures verbatim — only the surrounding `let row_rect = ...` binding and the new `if` block after it change.)

Worktree call site becomes:

```rust
                                let is_cursor = matches!(
                                    &cursor_row,
                                    Some(SidebarRow::Worktree(p)) if *p == wt.path
                                );
                                let action = worktree_row(
                                    ui,
                                    wt,
                                    is_active,
                                    is_cursor,
                                    cursor_moved,
                                    wt_attention,
                                    wt_glyph,
                                    &theme,
                                );
```

- [ ] **Step 5: Verify compile + tests, quick visual smoke**

Run: `cargo check -p alacritree && cargo test -p alacritree`
Expected: clean, all pass.

Run: `cargo run -p alacritree`, press Ctrl+Shift+B — an accent outline appears on the current workspace's row; arrows move it; Enter activates and returns; Escape returns. (Full checklist is Task 7.)

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): cursor highlight and auto-scroll"
```

---

### Task 7: Docs, release verification, manual checklist, status update

**Files:**
- Modify: `docs/keyboard-shortcuts.md` (in the worktree — this file IS committed)
- Modify: `docs/specs/planned_features.md` (in the MAIN checkout `C:/Users/Lev/Git/github/alacritree` — local-only, NEVER committed)

**Interfaces:**
- Consumes: everything shipped in Tasks 1–6.
- Produces: user-facing docs; the appended status paragraph other sessions read.

- [ ] **Step 1: Update `docs/keyboard-shortcuts.md`**

Read the file first — the rebindable branch restructured it, so integrate rather than assume. Make exactly these additions:

1. In the app-shortcut defaults table, after the Ctrl+B row:

```markdown
| `Ctrl+Shift+B`       | Move keyboard focus between the terminal and the projects sidebar |
```

2. In the supported-actions section, alongside the other alacritree-only actions:

```markdown
### Focus navigation

- `ToggleSidebarFocus` — flip keyboard focus between the terminal and the
  projects sidebar. Focusing a hidden sidebar shows it; returning focus
  hides it again unless you toggled it open yourself.
- `FocusProjectsSidebar` / `FocusTerminal` — the same moves as explicit
  directional actions (no default keys) for users who prefer distinct
  bindings.

While the sidebar has focus: `Up`/`Down` move between rows, `Right`/`Left`
expand/collapse a project (`Left` on a worktree jumps to its project),
`Enter` activates the selected workspace and returns to the terminal,
`Escape` returns without switching. All other keys behave as usual — the
shell receives nothing while the sidebar is focused.
```

- [ ] **Step 2: Full verification**

```bash
cargo fmt
cargo test -p alacritree
cargo build -p alacritree --release
```

Expected: fmt makes no changes (or re-stage), all tests pass (baseline + 7 sidebar_nav + 1 new bindings + 2 extended), release build clean with no warnings from `alacritree`.

- [ ] **Step 3: Commit the docs**

```bash
git add docs/keyboard-shortcuts.md
git commit -m "docs: document focus navigation shortcuts"
```

- [ ] **Step 4: Manual GUI acceptance checklist (user runs the release build)**

Present this list to the user; do not check items yourself:

1. Ctrl+Shift+B from the terminal focuses the sidebar; outline lands on the current workspace's row.
2. Up/Down move the outline and clamp at Home / last row; a long project list scrolls the outlined row into view.
3. Right/Left expand/collapse a project header; Left on a worktree jumps to its header; expansion state survives restart.
4. Enter on a worktree switches to it and puts typing back in the terminal immediately; Enter on Home does the same for the home workspace.
5. Escape returns focus without switching workspaces.
6. With the sidebar hidden: Ctrl+Shift+B shows+focuses it; Enter/Escape hides it again (round trip); after opening it manually with Ctrl+B instead, Enter/Escape leaves it open.
7. Ctrl+B while the sidebar is focused hides it and typing goes to the terminal.
8. While the sidebar is focused, typing letters does nothing (nothing appears in the shell afterwards); Ctrl+Q still opens the quit dialog; a modal's Enter/Escape work and the sidebar cursor is still there after cancel.
9. Clicking the terminal while the sidebar is focused returns focus (and hides an auto-shown sidebar).
10. Rebind test in `alacritree.toml`: `[[keyboard.bindings]] key = "B" mods = "Control|Shift" action = "ReceiveChar"` disables the toggle; a `key = "F6" action = "ToggleSidebarFocus"` binding drives it from F6.

- [ ] **Step 5: Append the status paragraph**

In the MAIN checkout, re-read `C:/Users/Lev/Git/github/alacritree/docs/specs/planned_features.md` (concurrent sessions edit it — never rewrite others' entries), then append under feature 7:

```markdown
Status <today's date>: feat/focus-navigation implemented (stacked on
feat/rebindable-app-shortcuts, worktree at
../alacritree-worktrees/feat/focus-navigation, 7 commits, 10 new tests).
PaneFocus enum gates the terminal's per-frame focus grab; Ctrl+Shift+B
toggles focus (FocusProjectsSidebar/FocusTerminal directional aliases, no
defaults); Up/Down/Left/Right/Enter/Escape drive the sidebar; auto-shown
sidebar hides on return. Pending: user GUI acceptance check (manual
checklist in plan Task 7 Step 4), review, push/PR decision (merges after
feat/rebindable-app-shortcuts). Spec/plan:
docs/superpowers/{specs,plans}/2026-07-12-focus-navigation*.
```

Adjust commit/test counts to reality before writing. Do NOT commit this file.

---

## Self-review notes

- Spec coverage: focus model (Task 4), six-key nav pass (Task 5), stable-key cursor + pure functions (Tasks 1–2), three actions + default + replacement (Task 3), highlight + scroll (Task 6), docs + manual checklist + prunable-worktrees integration note carried by the spec (Task 7).
- The spec's "click on the terminal returns focus" is Task 4 Step 4; "Ctrl+B clears auto-shown" is Task 4 Step 3.
- Type consistency: `SidebarRow`, `visible_rows`, `step`, `left_target`, `seed`, `focus_sidebar`, `focus_terminal`, `set_sidebar_cursor`, `set_project_expanded`, `is_sidebar_nav_key`, `paint_cursor_outline` are used with identical signatures across tasks.
