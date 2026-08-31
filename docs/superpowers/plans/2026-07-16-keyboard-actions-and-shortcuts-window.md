# Keyboard Actions & Shortcuts Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the rebindable `CloseSession` action, add four rebindable sidebar-navigation actions (Home/End/PgUp/PgDn), and add a searchable F1 shortcuts window — as two stacked branches off `feat/searchable-sidebars`.

**Architecture:** Branch 1 (`feat/keyboard-actions`, Tasks 1–3) cherry-picks the archive CloseSession commit and adds sidebar-scoped `NamedAction`s whose bindings pass through to the terminal when the sidebar is unfocused. Branch 2 (`feat/shortcuts-window`, Tasks 4–6) stacks on branch 1 and adds a new `shortcuts_window` module (pure row model + fuzzy matcher) plus an egui overlay window toggled by a new `ShowShortcuts` action.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe. Crate: `alacritree/` only.

**Spec:** `docs/superpowers/specs/2026-07-16-keyboard-actions-and-shortcuts-window-design.md` (untracked — never commit it).

## Global Constraints

- All work in the `alacritree/` crate; vendored `alacritty*` crates are read-only.
- Base branch for Task 1: `feat/searchable-sidebars` at 7d601695. Base for Task 4: `feat/keyboard-actions` (branch 1's tip).
- Worktrees live under `C:/Users/Lev/Git/github/alacritree-worktrees/<branch-name>/`. Never touch running `alacritree.exe` processes; never run the built GUI.
- Conventional Commits, imperative, ≤72-char subject, lowercase after the colon, no trailing period. Every commit ends with the footer line `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` (blank line before it).
- TDD: run the failing test and confirm it fails for the right reason before implementing. Compile-error RED (E0599/E0425/E0609/E0004) is acceptable for new-API tests.
- `cargo fmt` before every commit (the "unstable features only on nightly" warnings are pre-existing noise). `cargo test -p alacritree` must be green at every commit.
- Comments explain *why*, never *what*; timeless, no task/PR references. Do not commit spec/plan docs.
- Search with `rg`/`fd` only (never `grep`/`find`; never `rg -r`, which means `--replace`).
- Exact action config names: `CloseSession`, `SidebarTop`, `SidebarBottom`, `SidebarNextProject`, `SidebarPreviousProject`, `ShowShortcuts`.
- Default keys: Ctrl+Shift+W = CloseSession; unmodified Home/End/PageDown/PageUp = SidebarTop/SidebarBottom/SidebarNextProject/SidebarPreviousProject; unmodified F1 = ShowShortcuts.
- Sidebar-scoped actions (the four `Sidebar*` ones and nothing else) fire only while the projects sidebar owns focus and must NOT consume the key event otherwise.

---

### Task 1: Branch setup + restore CloseSession (cherry-pick 530b4c07)

**Files:**
- Modify: `alacritree/src/app.rs` (dispatch arm — via cherry-pick)
- Modify: `alacritree/src/bindings.rs` (enum variant, default binding, parse arm, tests — via cherry-pick)
- Modify: `docs/keyboard-shortcuts.md` (via cherry-pick)

**Interfaces:**
- Consumes: `SidebarRow::Session(SessionId)` (exists on the base branch), `self.request_close_session(id)`, `self.active_session_index()`.
- Produces: `NamedAction::CloseSession` (later tasks add variants adjacent to it and the shortcuts window describes it).

- [ ] **Step 1: Create the branch and worktree**

From the main repo root `C:/Users/Lev/Git/github/alacritree`:

```bash
git worktree add ../alacritree-worktrees/feat/keyboard-actions -b feat/keyboard-actions feat/searchable-sidebars
cd ../alacritree-worktrees/feat/keyboard-actions
```

All Task 1–3 commands run in this worktree.

- [ ] **Step 2: Cherry-pick the archive commit**

```bash
git cherry-pick 530b4c07
```

The commit is `feat(bindings): add rebindable CloseSession (ctrl+shift+w)`; it touches `app.rs`, `bindings.rs`, `docs/keyboard-shortcuts.md`. Expect small context conflicts because the base branch has drifted (e.g. `FocusGitSidebar` now sits between `ToggleSidebarFocus` and `FocusProjectsSidebar`). Resolve by **keeping both sides**: the archive's additions slot in alongside the branch's newer entries. The intended end state per file:

`bindings.rs` — enum gains `CloseSession` after `ToggleSidebarFocus`:

```rust
    AddProject,
    ToggleSidebarFocus,
    CloseSession,
    FocusProjectsSidebar,
    FocusGitSidebar,
    FocusTerminal,
```

`bindings.rs` — `default_bindings()` gains, right after the `ToggleSidebarFocus` entry (before the `FocusGitSidebar` line):

```rust
        KeyBinding { key: Key::W, mods: ctrl_shift, action: BindingAction::Named(CloseSession) },
```

`bindings.rs` — `parse_action` gains, after the `"ToggleSidebarFocus"` arm:

```rust
        "CloseSession" => BindingAction::Named(CloseSession),
```

`bindings.rs` — two tests appended to the tests module:

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
        assert!(matches!(
            parse_action("CloseSession"),
            BindingAction::Named(NamedAction::CloseSession)
        ));
    }
```

Note: `parse_action` is private; if the second test conflicts, keep it — the tests module is in the same file so it compiles.

`app.rs` — `dispatch_action` gains this arm, placed immediately before the `FocusProjectsSidebar` arm (currently line ~1497):

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
                let target = cursored
                    .or_else(|| self.active_session_index().map(|idx| self.sessions[idx].id));
                if let Some(id) = target {
                    self.request_close_session(id);
                }
            },
```

`docs/keyboard-shortcuts.md` — defaults table gains, after the `Ctrl+Shift+B` row:

```markdown
| `Ctrl+Shift+W`       | Close the cursored session (sidebar) or the current shell |
```

and the supported-actions list gains (next to the other app actions, e.g. after `SelectLastTab`):

```markdown
- `CloseSession` — close the session under the sidebar cursor when the sidebar
  is focused on one, otherwise the active session in the current workspace.
  Honors the `confirm_session_close` policy (may open a confirmation dialog).
```

If conflicts occurred: `git add -A && git cherry-pick --continue` (keep the original commit message; it already lacks a co-author footer — that is fine for a cherry-picked commit whose author is the user).

- [ ] **Step 3: Verify build and tests**

```bash
cargo fmt
cargo test -p alacritree
```

Expected: all tests pass, including `close_session_is_a_default_ctrl_shift_w_binding` and `close_session_parses_from_config_name`. If `cargo fmt` changed files, amend them into the cherry-pick: `git add -u && git commit --amend --no-edit`.

---

### Task 2: Project-jump helpers in the cursor model

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs`

**Interfaces:**
- Consumes: `SidebarRow` (Home/Project/Worktree/Session), existing test helper `project(root, expanded, worktrees)` and `no_sessions()` in the tests module.
- Produces: `pub fn next_project(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow>` and `pub fn previous_project(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow>` (Task 3 dispatches to these).

- [ ] **Step 1: Write the failing tests**

Append to the tests module in `alacritree/src/sidebar_nav.rs`:

```rust
    #[test]
    fn next_project_jumps_to_the_nearest_header_below() {
        let projects = vec![project("/a", true, &["/a/wt1"]), project("/b", true, &["/b/wt1"])];
        let rows = visible_rows(&projects, &no_sessions());
        assert_eq!(
            next_project(&rows, &SidebarRow::Home),
            Some(SidebarRow::Project(PathBuf::from("/a")))
        );
        assert_eq!(
            next_project(&rows, &SidebarRow::Worktree(PathBuf::from("/a/wt1"))),
            Some(SidebarRow::Project(PathBuf::from("/b")))
        );
        // No header below the last project's subtree: stay put (None).
        assert_eq!(next_project(&rows, &SidebarRow::Worktree(PathBuf::from("/b/wt1"))), None);
    }

    #[test]
    fn previous_project_jumps_to_the_nearest_header_above() {
        let projects = vec![project("/a", true, &["/a/wt1"]), project("/b", true, &["/b/wt1"])];
        let rows = visible_rows(&projects, &no_sessions());
        assert_eq!(
            previous_project(&rows, &SidebarRow::Worktree(PathBuf::from("/b/wt1"))),
            Some(SidebarRow::Project(PathBuf::from("/b")))
        );
        assert_eq!(
            previous_project(&rows, &SidebarRow::Project(PathBuf::from("/b"))),
            Some(SidebarRow::Project(PathBuf::from("/a")))
        );
        // Nothing above the first header or on Home: None.
        assert_eq!(previous_project(&rows, &SidebarRow::Project(PathBuf::from("/a"))), None);
        assert_eq!(previous_project(&rows, &SidebarRow::Home), None);
    }

    #[test]
    fn project_jumps_from_session_rows_and_vanished_cursors() {
        let projects = vec![project("/a", true, &["/a/wt1"]), project("/b", true, &["/b/wt1"])];
        let sessions = HashMap::from([(Some(PathBuf::from("/a/wt1")), vec![7])]);
        let rows = visible_rows(&projects, &sessions);
        assert_eq!(
            next_project(&rows, &SidebarRow::Session(7)),
            Some(SidebarRow::Project(PathBuf::from("/b")))
        );
        assert_eq!(
            previous_project(&rows, &SidebarRow::Session(7)),
            Some(SidebarRow::Project(PathBuf::from("/a")))
        );
        // A cursor no longer in the rows has no anchor: None, caller reseats.
        let gone = SidebarRow::Worktree(PathBuf::from("/gone"));
        assert_eq!(next_project(&rows, &gone), None);
        assert_eq!(previous_project(&rows, &gone), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree sidebar_nav
```

Expected: FAIL to compile with E0425 (`cannot find function next_project` / `previous_project`).

- [ ] **Step 3: Implement the helpers**

Add after `left_target` in `alacritree/src/sidebar_nav.rs`:

```rust
/// The nearest project header strictly after `cursor` — the PageDown-style
/// project jump.  `None` when no header follows or the cursor has vanished
/// from `rows` (the caller reseats it, as `step` callers do).
pub fn next_project(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow> {
    let pos = rows.iter().position(|r| r == cursor)?;
    rows[pos + 1..].iter().find(|r| matches!(r, SidebarRow::Project(_))).cloned()
}

/// The nearest project header strictly before `cursor`.
pub fn previous_project(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow> {
    let pos = rows.iter().position(|r| r == cursor)?;
    rows[..pos].iter().rev().find(|r| matches!(r, SidebarRow::Project(_))).cloned()
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree sidebar_nav
```

Expected: PASS (all sidebar_nav tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add alacritree/src/sidebar_nav.rs
git commit -m "feat(sidebar): add project-jump cursor helpers

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Rebindable sidebar-navigation actions

**Files:**
- Modify: `alacritree/src/bindings.rs`
- Modify: `alacritree/src/app.rs`
- Modify: `docs/keyboard-shortcuts.md`

**Interfaces:**
- Consumes: `sidebar_nav::next_project` / `sidebar_nav::previous_project` (Task 2), `self.current_project_rows()`, `self.set_sidebar_cursor(row)`, `PaneFocus::ProjectsSidebar`.
- Produces: `NamedAction::{SidebarTop, SidebarBottom, SidebarNextProject, SidebarPreviousProject}` and `pub fn is_sidebar_scoped(&self) -> bool` on `NamedAction` (Task 5's window lists them; nothing else consumes them).

- [ ] **Step 1: Write the failing bindings tests**

Append to the tests module in `alacritree/src/bindings.rs`:

```rust
    #[test]
    fn sidebar_nav_actions_have_unmodified_defaults_and_parse() {
        let b = parse_bindings(vec![]);
        for (key, expected, name) in [
            (Key::Home, NamedAction::SidebarTop, "SidebarTop"),
            (Key::End, NamedAction::SidebarBottom, "SidebarBottom"),
            (Key::PageDown, NamedAction::SidebarNextProject, "SidebarNextProject"),
            (Key::PageUp, NamedAction::SidebarPreviousProject, "SidebarPreviousProject"),
        ] {
            assert_eq!(named_matches(&b, key, Modifiers::NONE), vec![expected], "{name}");
            assert!(
                matches!(parse_action(name), BindingAction::Named(a) if a == expected),
                "{name} does not parse"
            );
        }
    }

    /// Only the four sidebar cursor actions are focus-scoped: everything
    /// else (CloseSession included) must keep firing from the terminal.
    #[test]
    fn only_sidebar_cursor_actions_are_sidebar_scoped() {
        use NamedAction::*;
        for a in [SidebarTop, SidebarBottom, SidebarNextProject, SidebarPreviousProject] {
            assert!(a.is_sidebar_scoped(), "{a:?}");
        }
        for a in [CloseSession, ScrollToTop, ScrollPageUp, ToggleSidebarFocus, Quit] {
            assert!(!a.is_sidebar_scoped(), "{a:?}");
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree bindings
```

Expected: FAIL to compile with E0599 (no variant `SidebarTop` …, no method `is_sidebar_scoped`).

- [ ] **Step 3: Implement in bindings.rs**

Enum — add after `CloseSession`:

```rust
    CloseSession,
    SidebarTop,
    SidebarBottom,
    SidebarNextProject,
    SidebarPreviousProject,
```

Add an impl right after the enum (before `RawBinding`):

```rust
impl NamedAction {
    /// Actions that drive the projects-sidebar cursor.  Their default keys
    /// (unmodified Home/End/PageUp/PageDown) are terminal input the rest of
    /// the time, so dispatch must not consume them unless the sidebar owns
    /// focus.
    pub fn is_sidebar_scoped(&self) -> bool {
        matches!(
            self,
            Self::SidebarTop
                | Self::SidebarBottom
                | Self::SidebarNextProject
                | Self::SidebarPreviousProject
        )
    }
}
```

`default_bindings()` — add after the `CloseSession` entry (uses the existing `ctrl_shift`… locals; these four need none):

```rust
        KeyBinding { key: Key::Home, mods: Modifiers::NONE, action: BindingAction::Named(SidebarTop) },
        KeyBinding { key: Key::End, mods: Modifiers::NONE, action: BindingAction::Named(SidebarBottom) },
        KeyBinding {
            key: Key::PageDown,
            mods: Modifiers::NONE,
            action: BindingAction::Named(SidebarNextProject),
        },
        KeyBinding {
            key: Key::PageUp,
            mods: Modifiers::NONE,
            action: BindingAction::Named(SidebarPreviousProject),
        },
```

`parse_action` — add after the `"CloseSession"` arm:

```rust
        "SidebarTop" => BindingAction::Named(SidebarTop),
        "SidebarBottom" => BindingAction::Named(SidebarBottom),
        "SidebarNextProject" => BindingAction::Named(SidebarNextProject),
        "SidebarPreviousProject" => BindingAction::Named(SidebarPreviousProject),
```

- [ ] **Step 4: Wire focus-scoped dispatch in app.rs**

In `handle_shortcuts` (app.rs ~line 947), scope sidebar actions. Replace the function body's opening with:

```rust
    fn handle_shortcuts(&mut self, ctx: &Context) {
        let sidebar_focused = self.focus == PaneFocus::ProjectsSidebar;
        let actions: Vec<BindingAction> = ctx.input_mut(|i| {
            let mut actions = Vec::new();
            i.events.retain(|ev| {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    let matched =
                        crate::bindings::all_matches(&self.config.bindings, *key, *modifiers);
                    if !matched.is_empty() {
                        // Sidebar-cursor actions only exist while the sidebar
                        // owns focus; anywhere else their keys (unmodified
                        // Home/End/PageUp/PageDown) are terminal input and
                        // must pass through untouched.
                        let sidebar_only = matched.iter().all(|a| {
                            matches!(a, BindingAction::Named(n) if n.is_sidebar_scoped())
                        });
                        if sidebar_only && !sidebar_focused {
                            return true;
                        }
                        let suppress_chars = matched
                            .iter()
                            .all(|a| !matches!(a, BindingAction::Named(NamedAction::ReceiveChar)));
                        for a in matched {
                            actions.push(a.clone());
                        }
                        return !suppress_chars;
                    }
                }
                true
            });
            actions
        });
        for action in actions {
            self.dispatch_action(ctx, action);
        }
    }
```

Add the four dispatch arms in `dispatch_action`, after the `CloseSession` arm:

```rust
            BindingAction::Named(NamedAction::SidebarTop) => self.sidebar_cursor_to_edge(true),
            BindingAction::Named(NamedAction::SidebarBottom) => self.sidebar_cursor_to_edge(false),
            BindingAction::Named(NamedAction::SidebarNextProject) => {
                self.sidebar_cursor_project_jump(1)
            },
            BindingAction::Named(NamedAction::SidebarPreviousProject) => {
                self.sidebar_cursor_project_jump(-1)
            },
```

Add the two helpers next to `move_sidebar_cursor` (app.rs ~line 1041):

```rust
    /// Home/End for the sidebar cursor: first or last of the rows the arrow
    /// keys step over (the filtered set while a filter is active).
    fn sidebar_cursor_to_edge(&mut self, top: bool) {
        let rows = self.current_project_rows();
        let target = if top { rows.first() } else { rows.last() };
        if let Some(row) = target.cloned() {
            self.set_sidebar_cursor(row);
        }
    }

    /// PageUp/PageDown for the sidebar cursor: the nearest project header
    /// above/below, clamped at the extremes.  A stale cursor reseats on the
    /// first row, same as `apply_sidebar_nav`.
    fn sidebar_cursor_project_jump(&mut self, delta: i32) {
        let rows = self.current_project_rows();
        let Some(cursor) = self.sidebar_cursor.clone().filter(|c| rows.contains(c)) else {
            if let Some(first) = rows.first() {
                self.set_sidebar_cursor(first.clone());
            }
            return;
        };
        let target = if delta > 0 {
            sidebar_nav::next_project(&rows, &cursor)
        } else {
            sidebar_nav::previous_project(&rows, &cursor)
        };
        if let Some(row) = target {
            self.set_sidebar_cursor(row);
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p alacritree
```

Expected: PASS, including `sidebar_nav_actions_have_unmodified_defaults_and_parse` and `only_sidebar_cursor_actions_are_sidebar_scoped`.

- [ ] **Step 6: Document**

`docs/keyboard-shortcuts.md`, defaults table ("Defaults on every platform"), after the `Ctrl+Shift+W` row:

```markdown
| `Home` / `End`       | Sidebar focused: cursor to the first / last row       |
| `PageUp` / `PageDown`| Sidebar focused: jump to the previous / next project  |
```

Supported-actions list, after the `CloseSession` bullet:

```markdown
- `SidebarTop` / `SidebarBottom` — move the sidebar cursor to the first / last
  visible row.
- `SidebarPreviousProject` / `SidebarNextProject` — jump the sidebar cursor to
  the nearest project header above / below.

All four sidebar actions act only while the projects sidebar has keyboard
focus; anywhere else their keys pass through to the terminal untouched, so
the unmodified defaults don't shadow Home/End/PageUp/PageDown in TUIs.
```

- [ ] **Step 7: Commit**

```bash
cargo fmt && git add alacritree/src/bindings.rs alacritree/src/app.rs docs/keyboard-shortcuts.md
git commit -m "feat(bindings): add rebindable sidebar navigation actions

Home/End move the sidebar cursor to the first/last row and
PageUp/PageDown jump between project headers.  The actions are
focus-scoped: with the terminal focused their unmodified keys pass
through as CSI input instead of being consumed by the binding table.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Shortcuts-window branch + pure row model and fuzzy matcher

**Files:**
- Create: `alacritree/src/shortcuts_window.rs`
- Modify: `alacritree/src/main.rs` (module declaration)
- Modify: `alacritree/src/bindings.rs` (`config_name`/`description` on `NamedAction`)

**Interfaces:**
- Consumes: `crate::bindings::{BindingAction, KeyBinding, NamedAction}`, `egui::{Key, Modifiers, KeyboardShortcut, ModifierNames}`.
- Produces (Task 5 consumes all of these):
  - `pub struct ShortcutRow { pub keys: String, pub name: String, pub description: String }`
  - `pub fn fuzzy_match(query: &str, haystack: &str) -> bool`
  - `pub fn row_matches(query: &str, row: &ShortcutRow) -> bool`
  - `pub fn named_rows(bindings: &[KeyBinding]) -> Vec<ShortcutRow>`
  - `pub fn sidebar_nav_rows() -> Vec<ShortcutRow>`
  - `NamedAction::config_name(&self) -> String`, `NamedAction::description(&self) -> String`

- [ ] **Step 1: Create the stacked branch and worktree**

From `C:/Users/Lev/Git/github/alacritree`:

```bash
git worktree add ../alacritree-worktrees/feat/shortcuts-window -b feat/shortcuts-window feat/keyboard-actions
cd ../alacritree-worktrees/feat/shortcuts-window
```

All Task 4–6 commands run in this worktree.

- [ ] **Step 2: Write the failing tests**

Create `alacritree/src/shortcuts_window.rs` with a module doc, the code stubs REPLACED by nothing yet — write ONLY the tests module first (the file must exist to hold them), plus add `mod shortcuts_window;` to the alphabetical module list in `alacritree/src/main.rs` (between `session` and `sidebar_nav`):

```rust
//! Data model for the searchable shortcuts window: the effective binding
//! rows, the static sidebar-navigation entries, and the fuzzy matcher the
//! search box filters them with.  Pure — painting lives in `app.rs`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{parse_bindings, NamedAction, RawBinding};
    use egui::{Key, Modifiers};

    fn raw_action(key: &str, mods: Option<&str>, action: &str) -> RawBinding {
        RawBinding {
            key: key.into(),
            mods: mods.map(Into::into),
            mode: None,
            chars: None,
            action: Some(action.into()),
            command: None,
        }
    }

    #[test]
    fn fuzzy_match_is_a_case_insensitive_subsequence() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("csw", "Ctrl+Shift+W CloseSession"));
        assert!(fuzzy_match("CLOSE", "close the cursored session"));
        // Subsequence, not substring: letters may be spread out…
        assert!(fuzzy_match("cse", "CloseSession"));
        // …but order matters and letters aren't reused.
        assert!(!fuzzy_match("wsc", "Ctrl+Shift+W"));
        assert!(!fuzzy_match("zz", "\u{2318}z"));
    }

    #[test]
    fn named_rows_lists_defaults_with_descriptions() {
        let rows = named_rows(&parse_bindings(vec![]));
        let close = rows
            .iter()
            .find(|r| r.name == "CloseSession")
            .expect("CloseSession missing from rows");
        assert_eq!(close.keys, "Ctrl+Shift+W");
        assert!(!close.description.is_empty());
        // Chars defaults (Shift+Tab -> CSI Z) are terminal plumbing, not
        // app shortcuts: no row.
        assert!(!rows.iter().any(|r| r.keys == "Shift+Tab"));
    }

    #[test]
    fn named_rows_honors_user_overrides_and_unbinds() {
        let rows = named_rows(&parse_bindings(vec![
            raw_action("W", Some("Control|Shift"), "Quit"),
            raw_action("B", Some("Control"), "ReceiveChar"),
        ]));
        // The rebound trigger shows the user's action only.
        let w: Vec<_> = rows.iter().filter(|r| r.keys == "Ctrl+Shift+W").collect();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].name, "Quit");
        // A key freed with ReceiveChar disappears entirely.
        assert!(!rows.iter().any(|r| r.keys == "Ctrl+B"));
    }

    #[test]
    fn every_named_action_row_has_a_nonempty_description() {
        for row in named_rows(&parse_bindings(vec![])) {
            assert!(!row.description.is_empty(), "{} has no description", row.name);
        }
        // Parametrized actions too, which no default binding covers.
        assert!(!NamedAction::SelectTab(3).description().is_empty());
        assert!(!NamedAction::SpawnProfile(2).description().is_empty());
        assert_eq!(NamedAction::SelectTab(3).config_name(), "SelectTab3");
    }

    #[test]
    fn sidebar_nav_rows_cover_the_hardcoded_keys() {
        let rows = sidebar_nav_rows();
        for key in ["Up / Down", "Enter", "Escape", "/"] {
            assert!(rows.iter().any(|r| r.keys == key), "{key} missing");
        }
        assert!(rows.iter().all(|r| !r.description.is_empty()));
    }

    #[test]
    fn row_matches_searches_keys_name_and_description() {
        let row = ShortcutRow {
            keys: "Ctrl+Shift+W".into(),
            name: "CloseSession".into(),
            description: "Close the cursored or active session".into(),
        };
        assert!(row_matches("ctrl+shift", &row));
        assert!(row_matches("closesess", &row));
        assert!(row_matches("cursored", &row));
        assert!(!row_matches("font", &row));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p alacritree shortcuts_window
```

Expected: FAIL to compile with E0425/E0599 (`fuzzy_match`, `named_rows`, `ShortcutRow`, `config_name`, `description` not found).

- [ ] **Step 4: Implement `NamedAction::config_name` / `description`**

In `alacritree/src/bindings.rs`, extend the `impl NamedAction` block from Task 3:

```rust
    /// The name `parse_action` accepts for this action — what a user writes
    /// in `[[keyboard.bindings]]`, and the label the shortcuts window shows.
    pub fn config_name(&self) -> String {
        match self {
            Self::SelectTab(n) => format!("SelectTab{n}"),
            Self::SpawnProfile(n) => format!("SpawnProfile{n}"),
            other => format!("{other:?}"),
        }
    }

    /// One-line human description for the shortcuts window.
    pub fn description(&self) -> String {
        match self {
            Self::Paste => "Paste from the clipboard".into(),
            Self::PasteSelection => "Paste from the primary (X11) selection".into(),
            Self::Copy => "Copy the selection to the clipboard".into(),
            Self::CopySelection => "Copy the selection to the primary selection".into(),
            Self::ScrollPageUp => "Scroll the scrollback one page up".into(),
            Self::ScrollPageDown => "Scroll the scrollback one page down".into(),
            Self::ScrollHalfPageUp => "Scroll the scrollback half a page up".into(),
            Self::ScrollHalfPageDown => "Scroll the scrollback half a page down".into(),
            Self::ScrollLineUp => "Scroll the scrollback one line up".into(),
            Self::ScrollLineDown => "Scroll the scrollback one line down".into(),
            Self::ScrollToTop => "Scroll to the top of the scrollback".into(),
            Self::ScrollToBottom => "Scroll to the bottom of the scrollback".into(),
            Self::ClearHistory => "Clear the scrollback buffer".into(),
            Self::SpawnNewInstance => "Open a new shell session in the current workspace".into(),
            Self::IncreaseFontSize => "Increase the font size".into(),
            Self::DecreaseFontSize => "Decrease the font size".into(),
            Self::ResetFontSize => "Reset the font size".into(),
            Self::ToggleFullscreen => "Toggle fullscreen".into(),
            Self::ToggleMaximized => "Toggle the maximized window state".into(),
            Self::Minimize => "Minimize the window".into(),
            Self::SelectNextTab => "Cycle to the next session in the workspace".into(),
            Self::SelectPreviousTab => "Cycle to the previous session in the workspace".into(),
            Self::SelectTab(n) => format!("Select session {n} in the current workspace"),
            Self::SelectLastTab => "Select the last session in the current workspace".into(),
            Self::ToggleLeftSidebar => "Toggle the projects sidebar".into(),
            Self::ToggleRightSidebar => "Toggle the git sidebar".into(),
            Self::SelectNextWorkspace => "Switch to the next workspace".into(),
            Self::SelectPreviousWorkspace => "Switch to the previous workspace".into(),
            Self::AddProject => "Add a project to the sidebar".into(),
            Self::ToggleSidebarFocus => {
                "Toggle keyboard focus between terminal and sidebar".into()
            },
            Self::CloseSession => "Close the cursored or active session".into(),
            Self::SidebarTop => "Move the sidebar cursor to the first row".into(),
            Self::SidebarBottom => "Move the sidebar cursor to the last row".into(),
            Self::SidebarNextProject => "Jump the sidebar cursor to the next project".into(),
            Self::SidebarPreviousProject => {
                "Jump the sidebar cursor to the previous project".into()
            },
            Self::FocusProjectsSidebar => "Focus the projects sidebar".into(),
            Self::FocusGitSidebar => "Focus the git sidebar".into(),
            Self::FocusTerminal => "Focus the terminal".into(),
            Self::SpawnProfile(n) => format!("Open a session with shell profile {n}"),
            Self::Quit => "Open the quit confirmation dialog".into(),
            Self::NoOp | Self::ReceiveChar => String::new(),
        }
    }
```

(Note: `NamedAction` needs no new derives — `Debug` already exists for `config_name`'s fallback. Task 5 adds the `ShowShortcuts` arm to this match; until then the match is exhaustive without it.)

- [ ] **Step 5: Implement the module**

Fill in `alacritree/src/shortcuts_window.rs` above the tests module:

```rust
use crate::bindings::{BindingAction, KeyBinding, NamedAction};

/// One line in the shortcuts window.
pub struct ShortcutRow {
    pub keys: String,
    pub name: String,
    pub description: String,
}

/// Case-insensitive subsequence match — `csw` finds `Ctrl+Shift+W`.  An
/// empty query matches everything, so the unfiltered window needs no
/// special case.
pub fn fuzzy_match(query: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars().flat_map(char::to_lowercase);
    query.chars().flat_map(char::to_lowercase).all(|q| hay.any(|h| h == q))
}

pub fn row_matches(query: &str, row: &ShortcutRow) -> bool {
    fuzzy_match(query, &format!("{} {} {}", row.keys, row.name, row.description))
}

/// The effective app-shortcut rows: `parse_bindings` already replaced
/// shadowed defaults with the user's same-trigger bindings, so every Named
/// entry here genuinely fires.  `NoOp`/`ReceiveChar` unbind rather than
/// bind and `Chars`/`Unsupported` aren't app shortcuts — no rows for them.
pub fn named_rows(bindings: &[KeyBinding]) -> Vec<ShortcutRow> {
    bindings
        .iter()
        .filter_map(|b| {
            let BindingAction::Named(action) = &b.action else {
                return None;
            };
            if matches!(action, NamedAction::NoOp | NamedAction::ReceiveChar) {
                return None;
            }
            Some(ShortcutRow {
                keys: format_shortcut(b.key, b.mods),
                name: action.config_name(),
                description: action.description(),
            })
        })
        .collect()
}

fn format_shortcut(key: egui::Key, mods: egui::Modifiers) -> String {
    egui::KeyboardShortcut::new(mods, key)
        .format(&egui::ModifierNames::NAMES, cfg!(target_os = "macos"))
}

fn nav_row(keys: &str, description: &str) -> ShortcutRow {
    ShortcutRow { keys: keys.into(), name: String::new(), description: description.into() }
}

/// The hardcoded sidebar keys (`handle_sidebar_nav` / `PanelFilter`), which
/// the binding table never sees.  Kept in sync by hand; they change rarely.
pub fn sidebar_nav_rows() -> Vec<ShortcutRow> {
    vec![
        nav_row("Up / Down", "Move the cursor"),
        nav_row("Right", "Expand a project, or open the cursored session"),
        nav_row("Left", "Collapse a project, or jump to the parent row"),
        nav_row("Enter", "Activate the cursored row"),
        nav_row("Escape", "Clear the filter, or return focus to the terminal"),
        nav_row("/", "Start fuzzy-filtering the panel's rows"),
        nav_row("s", "Projects panel: show only workspaces with open sessions"),
        nav_row("a", "Projects panel: show only workspaces needing attention"),
        nav_row("m / d / u", "Git panel: show only modified / deleted / untracked files"),
        nav_row("Backspace", "Delete the last filter character (while filtering)"),
    ]
}
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p alacritree
```

Expected: PASS. If `fuzzy_match("csw", "Ctrl+Shift+W CloseSession")` or the `close.keys == "Ctrl+Shift+W"` assertion fails because `egui::ModifierNames::NAMES` renders differently (e.g. different separator), fix `format_shortcut` to produce the `Ctrl+Shift+W` style the tests demand — the tests are the contract, egui's formatting is an implementation detail behind our function.

- [ ] **Step 7: Commit**

```bash
cargo fmt && git add alacritree/src/shortcuts_window.rs alacritree/src/main.rs alacritree/src/bindings.rs
git commit -m "feat(shortcuts): add shortcut rows and fuzzy matcher

Pure data model for the upcoming searchable shortcuts window: effective
Named-binding rows with per-action descriptions, static entries for the
hardcoded sidebar keys, and a case-insensitive subsequence matcher.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: ShowShortcuts action + window UI

**Files:**
- Modify: `alacritree/src/bindings.rs`
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `shortcuts_window::{named_rows, sidebar_nav_rows, row_matches}` (Task 4), existing `modal_frame(&Theme)` helper in app.rs, `self.config.bindings`.
- Produces: `NamedAction::ShowShortcuts`, fields `shortcuts_window_open: bool`, `shortcuts_query: String`, `shortcuts_focus_search: bool`, method `show_shortcuts_window(&mut self, ctx: &Context)`.

- [ ] **Step 1: Write the failing bindings test**

Append to the tests module in `alacritree/src/bindings.rs`:

```rust
    #[test]
    fn show_shortcuts_is_a_default_f1_binding_and_parses() {
        let b = parse_bindings(vec![]);
        assert_eq!(named_matches(&b, Key::F1, Modifiers::NONE), vec![NamedAction::ShowShortcuts]);
        assert!(matches!(
            parse_action("ShowShortcuts"),
            BindingAction::Named(NamedAction::ShowShortcuts)
        ));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p alacritree bindings
```

Expected: FAIL to compile with E0599 (no variant `ShowShortcuts`).

- [ ] **Step 3: Implement the action in bindings.rs**

Enum — add after `SidebarPreviousProject`:

```rust
    ShowShortcuts,
```

`default_bindings()` — after the `SidebarPreviousProject` entry:

```rust
        KeyBinding { key: Key::F1, mods: Modifiers::NONE, action: BindingAction::Named(ShowShortcuts) },
```

`parse_action` — after the `"SidebarPreviousProject"` arm:

```rust
        "ShowShortcuts" => BindingAction::Named(ShowShortcuts),
```

`description()` — add before the `NoOp | ReceiveChar` arm:

```rust
            Self::ShowShortcuts => "Show this shortcuts window".into(),
```

`is_sidebar_scoped` stays unchanged (`ShowShortcuts` is global — a default F1 knowingly shadows F1 for terminal TUIs; it's rebindable).

Run `cargo test -p alacritree bindings` — expected: PASS.

- [ ] **Step 4: Add window state and dispatch in app.rs**

Fields — in `AlacritreeApp`, after `git_sidebar_auto_shown`:

```rust
    /// The F1 shortcuts overlay.  Transient: never persisted.
    shortcuts_window_open: bool,
    shortcuts_query: String,
    /// One-shot: give the search box focus on the next window paint (set on
    /// open and by `/`), mirroring the `*_cursor_moved` one-shots.
    shortcuts_focus_search: bool,
```

Constructor — with the other field inits:

```rust
            shortcuts_window_open: false,
            shortcuts_query: String::new(),
            shortcuts_focus_search: false,
```

Dispatch arm — after the `SidebarPreviousProject` arm:

```rust
            BindingAction::Named(NamedAction::ShowShortcuts) => {
                self.shortcuts_window_open = !self.shortcuts_window_open;
                if self.shortcuts_window_open {
                    self.shortcuts_query.clear();
                    self.shortcuts_focus_search = true;
                }
            },
```

- [ ] **Step 5: Route input around the open window in `update`**

The window is an overlay, not a modal — bindings keep dispatching (that's how F1 toggles it closed and font-size still works). But typed text must reach only its search box, so while it is open the sidebar/git nav drains are skipped and the terminal view goes inactive. In `update` (app.rs ~line 4587), change:

```rust
        if !modal_open && self.ime.preedit().is_none() {
            match self.focus {
                PaneFocus::ProjectsSidebar => self.handle_sidebar_nav(ctx),
                PaneFocus::GitSidebar => self.handle_git_sidebar_nav(ctx),
                PaneFocus::Terminal => {},
            }
            self.handle_shortcuts(ctx);
        }
```

to:

```rust
        if !modal_open && self.ime.preedit().is_none() {
            // While the shortcuts overlay is open, typed text belongs to its
            // search box — the panel filters must not intercept it.
            if !self.shortcuts_window_open {
                match self.focus {
                    PaneFocus::ProjectsSidebar => self.handle_sidebar_nav(ctx),
                    PaneFocus::GitSidebar => self.handle_git_sidebar_nav(ctx),
                    PaneFocus::Terminal => {},
                }
            }
            self.handle_shortcuts(ctx);
        }
```

Terminal input gate — in the `terminal_view::show` call (~line 4654), change the active flag:

```rust
                    !modal_open && !self.shortcuts_window_open && self.focus == PaneFocus::Terminal,
```

Paint — after the `show_quit_dialog` block (~line 4689), add (modals keep key priority — `consume_modal_keys` runs in their paint — so the overlay hides under them):

```rust
        if self.shortcuts_window_open && !modal_open {
            self.show_shortcuts_window(ctx);
        }
```

- [ ] **Step 6: Implement the window paint method**

Add to the `impl AlacritreeApp` block near `show_error_dialog`:

```rust
    /// The F1 shortcuts overlay: every effective app binding plus the
    /// hardcoded sidebar keys, filtered live by the search box.  An
    /// informational overlay, not a modal — bindings keep dispatching, which
    /// is also how the ShowShortcuts key toggles it closed.
    fn show_shortcuts_window(&mut self, ctx: &Context) {
        let theme = self.theme;
        let s = theme.ui_scale;

        // Esc narrows before it closes: drain it ahead of the TextEdit,
        // which would otherwise only drop focus.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.shortcuts_query.is_empty() {
                self.shortcuts_window_open = false;
                return;
            }
            self.shortcuts_query.clear();
        }
        // `/` re-focuses the search box instead of typing into it.
        let slash = ctx.input_mut(|i| {
            let mut hit = false;
            i.events.retain(|ev| {
                let is_slash = matches!(ev, egui::Event::Text(t) if t == "/");
                hit |= is_slash;
                !is_slash
            });
            hit
        });
        if slash {
            self.shortcuts_focus_search = true;
        }

        egui::Window::new(RichText::new("Keyboard shortcuts").color(theme.text).strong())
            .id(egui::Id::new("alacritree_shortcuts_window"))
            .frame(modal_frame(&theme))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(480.0 * s);
                ui.spacing_mut().item_spacing.y = 4.0 * s;

                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.shortcuts_query)
                        .hint_text("type to search — / refocuses, Esc clears")
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut self.shortcuts_focus_search) {
                    search.request_focus();
                }

                let query = self.shortcuts_query.clone();
                let app_rows: Vec<_> = shortcuts_window::named_rows(&self.config.bindings)
                    .into_iter()
                    .filter(|r| shortcuts_window::row_matches(&query, r))
                    .collect();
                let nav_rows: Vec<_> = shortcuts_window::sidebar_nav_rows()
                    .into_iter()
                    .filter(|r| shortcuts_window::row_matches(&query, r))
                    .collect();

                egui::ScrollArea::vertical().max_height(420.0 * s).show(ui, |ui| {
                    if app_rows.is_empty() && nav_rows.is_empty() {
                        ui.label(RichText::new("no matches").color(theme.text_dim));
                        return;
                    }
                    if !app_rows.is_empty() {
                        ui.label(RichText::new("App shortcuts").color(theme.text_muted).small());
                        egui::Grid::new("shortcuts_app_grid").num_columns(2).striped(true).show(
                            ui,
                            |ui| {
                                for row in &app_rows {
                                    ui.label(
                                        RichText::new(&row.keys).color(theme.accent).monospace(),
                                    );
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(&row.description).color(theme.text),
                                        );
                                        ui.label(
                                            RichText::new(&row.name)
                                                .color(theme.text_dim)
                                                .small(),
                                        );
                                    });
                                    ui.end_row();
                                }
                            },
                        );
                    }
                    if !nav_rows.is_empty() {
                        ui.add_space(6.0 * s);
                        ui.label(
                            RichText::new("Sidebar navigation (while a panel has focus)")
                                .color(theme.text_muted)
                                .small(),
                        );
                        egui::Grid::new("shortcuts_nav_grid").num_columns(2).striped(true).show(
                            ui,
                            |ui| {
                                for row in &nav_rows {
                                    ui.label(
                                        RichText::new(&row.keys).color(theme.accent).monospace(),
                                    );
                                    ui.label(RichText::new(&row.description).color(theme.text));
                                    ui.end_row();
                                }
                            },
                        );
                    }
                });
            });
    }
```

Add `use crate::shortcuts_window;` to app.rs's imports (alphabetical, near `use crate::sidebar_nav::…`). The `Theme` struct is defined in app.rs itself (~line 41) and provides every field used above: `text`, `text_dim`, `text_muted`, `accent`, `ui_scale`.

- [ ] **Step 7: Verify build and full test suite**

```bash
cargo test -p alacritree
```

Expected: PASS (including `show_shortcuts_is_a_default_f1_binding_and_parses`). Also `cargo check -p alacritree` clean of warnings introduced by this change.

- [ ] **Step 8: Commit**

```bash
cargo fmt && git add alacritree/src/bindings.rs alacritree/src/app.rs
git commit -m "feat(shortcuts): show a searchable shortcuts window on F1

ShowShortcuts (rebindable, default F1) toggles a centered overlay
listing every effective app binding with a description plus the
hardcoded sidebar keys, fuzzy-filtered by a search box; / refocuses
the box and Esc clears then closes.  The overlay is not a modal —
bindings keep dispatching so the same key closes it — but while open
the panel filters and terminal input are muted so typed text reaches
only the search box.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Document ShowShortcuts

**Files:**
- Modify: `docs/keyboard-shortcuts.md`

**Interfaces:**
- Consumes: the defaults table and supported-actions list as left by Task 3.
- Produces: nothing downstream.

- [ ] **Step 1: Add the documentation**

Defaults table ("Defaults on every platform"), after the `PageUp` / `PageDown` row from Task 3:

```markdown
| `F1`                 | Toggle the searchable shortcuts window                |
```

Supported-actions list, after the sidebar-actions note from Task 3:

```markdown
- `ShowShortcuts` — toggle a searchable window listing every effective
  binding. Type to fuzzy-filter, `/` refocuses the search box, `Escape`
  clears the query and then closes. The default `F1` shadows `F1` for
  terminal apps; rebind or free it with `ReceiveChar` if you need it there.
```

- [ ] **Step 2: Commit**

```bash
git add docs/keyboard-shortcuts.md
git commit -m "docs(shortcuts): document the ShowShortcuts window

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Integrate into integration/all-features + verification builds

**Files:**
- Modify: `C:/Users/Lev/Git/github/alacritree-worktrees/integration/all-features` (merge commits)

**Interfaces:**
- Consumes: branches `feat/keyboard-actions` and `feat/shortcuts-window` (Tasks 1–6).
- Produces: updated `integration/all-features` + release binary.

- [ ] **Step 1: Push the feature branches**

```bash
git push origin feat/keyboard-actions feat/shortcuts-window
```

(Push only — do NOT open PRs; they stack behind PR #101 and the user asks for PRs separately.)

- [ ] **Step 2: Merge into integration/all-features**

```bash
cd C:/Users/Lev/Git/github/alacritree-worktrees/integration/all-features
git merge feat/shortcuts-window
```

Merging the tip brings both branches. Expected conflicts: none or small context overlaps in `bindings.rs`/`app.rs` with the integration branch's extra features (IPC named-action runner cd525d85 dispatches `NamedAction`s — after the merge, confirm its exhaustive handling still compiles; if it matches on action names, the six new actions are reachable through it, which is correct and needs no change). Resolve by keeping both sides.

- [ ] **Step 3: Test and push**

```bash
cargo test -p alacritree
git push origin integration/all-features
```

Expected: full suite green (was 324 tests before this work; now more).

- [ ] **Step 4: Rebuild the release binary**

```bash
cargo build -p alacritree --release
```

Run in the integration worktree, in the background (takes ~1 min). Do not kill or restart any running alacritree instance — the user restarts on their own schedule.

- [ ] **Step 5: Manual GUI verification checklist (report, don't skip)**

Using the isolated GUI lab (separate APPDATA + scratch repo under `target/`, per the existing lab setup — never the user's production instance):

1. F1 opens the window; F1 again closes; Esc with text clears, Esc empty closes.
2. Typing filters; `/` refocuses the box; `csw` finds Ctrl+Shift+W.
3. Sidebar focused: Home/End jump to first/last row; PgUp/PgDn jump project headers; with a `/` filter active the jumps respect the filtered rows.
4. Terminal focused: Home/End/PgUp/PgDn reach the shell (e.g. `less` responds), F1 does not.
5. Ctrl+Shift+W closes the cursored session from the sidebar and the active session from the terminal; `confirm_session_close` prompt appears when configured.

---

## Unresolved questions

None — design decisions were settled in the approved spec. PR creation (stacked behind #101) is deliberately out of scope until requested.
