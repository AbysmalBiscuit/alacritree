# Rebindable App Shortcuts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold the nine hardcoded app shortcuts (Ctrl+B/G/Tab/T/Q, Ctrl+Shift+Tab/O, Alt+Left/Right) into the existing `[[keyboard.bindings]]` system so users can rebind them or free the keys for the PTY.

**Architecture:** Add five alacritree-only `NamedAction` variants, append the nine app defaults to `default_bindings()`, implement alacritty's same-trigger replacement (a user binding with the same key+mods replaces the default), then delete the `consume_exact` pass in `app.rs` so all shortcuts dispatch through the one binding path. Spec: `docs/superpowers/specs/2026-07-12-rebindable-app-shortcuts-design.md`.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, plain `#[cfg(test)]` unit tests.

## Global Constraints

- Only touch the `alacritree/` crate; `alacritty*/` crates are read-only vendored code.
- Work in the dedicated worktree at `../alacritree-worktrees/feat/rebindable-app-shortcuts` (branch `feat/rebindable-app-shortcuts` off master — the repo's established worktree convention). All commands below run from that worktree root.
- `cargo fmt` before every commit (rustfmt is enforced).
- Conventional Commits, imperative, ≤72-char subject; end commit messages with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Comments explain *why*, never narrate the change; no PR/task references.
- Default key combinations must not change — this PR is refactor + new capability only.
- Never commit `docs/specs/`, `docs/plans/`, `docs/superpowers/` (git-excluded local files).

---

### Task 1: Alacritree action vocabulary

**Files:**
- Modify: `alacritree/src/bindings.rs` (enum `NamedAction` ~line 22, `parse_action` ~line 386, new `#[cfg(test)]` module at end of file)

**Interfaces:**
- Consumes: existing `NamedAction`, `BindingAction`, `parse_bindings`, `all_matches` in `bindings.rs`.
- Produces: `NamedAction::{ToggleLeftSidebar, ToggleRightSidebar, SelectNextWorkspace, SelectPreviousWorkspace, AddProject}` (fieldless, `Copy`), parseable from the same-named strings in `parse_action`. Test helpers `raw_action(key, mods, action) -> RawBinding` and `named_matches(&[KeyBinding], Key, Modifiers) -> Vec<NamedAction>` reused by Tasks 2–3.

- [ ] **Step 1: Write the failing tests**

Append to the end of `alacritree/src/bindings.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

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

    /// The `NamedAction`s that fire for a key press, ignoring other kinds.
    fn named_matches(bindings: &[KeyBinding], key: Key, mods: Modifiers) -> Vec<NamedAction> {
        all_matches(bindings, key, mods)
            .into_iter()
            .filter_map(|a| match a {
                BindingAction::Named(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn new_action_names_parse() {
        for (name, expected) in [
            ("ToggleLeftSidebar", NamedAction::ToggleLeftSidebar),
            ("ToggleRightSidebar", NamedAction::ToggleRightSidebar),
            ("SelectNextWorkspace", NamedAction::SelectNextWorkspace),
            ("SelectPreviousWorkspace", NamedAction::SelectPreviousWorkspace),
            ("AddProject", NamedAction::AddProject),
        ] {
            let b = parse_bindings(vec![raw_action("F1", None, name)]);
            assert_eq!(named_matches(&b, Key::F1, Modifiers::NONE), vec![expected], "{name}");
        }
    }

    #[test]
    fn unknown_action_is_unsupported() {
        let b = parse_bindings(vec![raw_action("F1", None, "FlyToTheMoon")]);
        let m = all_matches(&b, Key::F1, Modifiers::NONE);
        assert!(matches!(m.as_slice(), [BindingAction::Unsupported(n)] if n == "FlyToTheMoon"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail for the right reason**

Run: `cargo test -p alacritree`
Expected: compile error `no variant named 'ToggleLeftSidebar' found for enum 'NamedAction'` (Rust's RED for a missing variant). `unknown_action_is_unsupported` is blocked by the same compile error — that's fine.

- [ ] **Step 3: Add the variants and parser arms**

In the `NamedAction` enum, after the `SelectLastTab` variant and before `Quit`:

```rust
    ToggleLeftSidebar,
    ToggleRightSidebar,
    SelectNextWorkspace,
    SelectPreviousWorkspace,
    AddProject,
```

In `parse_action`, after the `"SelectLastTab"` arm and before `"Quit"`:

```rust
        "ToggleLeftSidebar" => BindingAction::Named(ToggleLeftSidebar),
        "ToggleRightSidebar" => BindingAction::Named(ToggleRightSidebar),
        "SelectNextWorkspace" => BindingAction::Named(SelectNextWorkspace),
        "SelectPreviousWorkspace" => BindingAction::Named(SelectPreviousWorkspace),
        "AddProject" => BindingAction::Named(AddProject),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: `test tests::new_action_names_parse ... ok`, `test tests::unknown_action_is_unsupported ... ok` — 2 passed, no warnings (`parse_action` constructs the new variants, so no dead-code lint fires even though `app.rs` doesn't dispatch them until Task 4).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs
git commit -m "feat(bindings): add alacritree app-level binding actions"
```

---

### Task 2: App shortcuts as default bindings

**Files:**
- Modify: `alacritree/src/bindings.rs` (`default_bindings()` ~line 107, tests module)

**Interfaces:**
- Consumes: `NamedAction` variants and test helpers from Task 1.
- Produces: `default_bindings()` additionally returns the nine app-shortcut bindings listed in the test below; Task 3's replacement filter and Task 4's `app.rs` rewrite rely on them being present.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `alacritree/src/bindings.rs`:

```rust
    #[test]
    fn default_app_shortcuts_present_without_user_config() {
        use NamedAction::*;
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let alt = Modifiers::ALT;
        let b = parse_bindings(Vec::new());
        for (key, mods, expected) in [
            (Key::B, ctrl, ToggleLeftSidebar),
            (Key::G, ctrl, ToggleRightSidebar),
            (Key::Tab, ctrl, SelectNextTab),
            (Key::Tab, ctrl_shift, SelectPreviousTab),
            (Key::ArrowRight, alt, SelectNextWorkspace),
            (Key::ArrowLeft, alt, SelectPreviousWorkspace),
            (Key::O, ctrl_shift, AddProject),
            (Key::T, ctrl, SpawnNewInstance),
            (Key::Q, ctrl, Quit),
        ] {
            assert_eq!(named_matches(&b, key, mods), vec![expected], "{key:?}+{mods:?}");
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alacritree default_app_shortcuts_present_without_user_config`
Expected: FAIL — `assertion 'left == right' failed: B+...` with `left: []` (no Ctrl+B binding exists yet).

- [ ] **Step 3: Append the app defaults in `default_bindings()`**

In `default_bindings()`, add `let alt = Modifiers::ALT;` next to the existing `let alt_shift = ...;` line. Remove the `#[cfg_attr(not(target_os = "macos"), allow(unused_mut))]` attribute on `let mut b` (the unconditional `extend` below uses the `mut` on every platform). Then, after the initial `vec![...]` and before the `#[cfg(target_os = "macos")]` block, insert:

```rust
    // App-level (alacritree) shortcuts: sidebars, session/workspace cycling,
    // project management.  Each can be rebound, or freed for the PTY with a
    // user binding on the same key+mods (`ReceiveChar` forwards the key,
    // `None` swallows it).
    b.extend([
        KeyBinding { key: Key::B, mods: ctrl, action: BindingAction::Named(ToggleLeftSidebar) },
        KeyBinding { key: Key::G, mods: ctrl, action: BindingAction::Named(ToggleRightSidebar) },
        KeyBinding { key: Key::Tab, mods: ctrl, action: BindingAction::Named(SelectNextTab) },
        KeyBinding {
            key: Key::Tab,
            mods: ctrl_shift,
            action: BindingAction::Named(SelectPreviousTab),
        },
        KeyBinding {
            key: Key::ArrowRight,
            mods: alt,
            action: BindingAction::Named(SelectNextWorkspace),
        },
        KeyBinding {
            key: Key::ArrowLeft,
            mods: alt,
            action: BindingAction::Named(SelectPreviousWorkspace),
        },
        KeyBinding { key: Key::O, mods: ctrl_shift, action: BindingAction::Named(AddProject) },
        KeyBinding { key: Key::T, mods: ctrl, action: BindingAction::Named(SpawnNewInstance) },
        KeyBinding { key: Key::Q, mods: ctrl, action: BindingAction::Named(Quit) },
    ]);
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: 3 passed (the two Task-1 tests still pass — F1 has no default binding, so they're unaffected). The app still behaves normally at runtime because `handle_shortcuts` consumes these keys before the binding dispatch sees them; that pass is removed in Task 4.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs
git commit -m "feat(bindings): expose app shortcuts as default bindings"
```

---

### Task 3: Same-trigger replacement

**Files:**
- Modify: `alacritree/src/bindings.rs` (`parse_bindings` ~line 72 and its trailing comment ~line 97, tests module)

**Interfaces:**
- Consumes: `default_bindings()` from Task 2, test helpers from Task 1.
- Produces: `parse_bindings` drops any default whose `(key, mods)` equals a user binding's before appending — Task 4's "free Ctrl+B for tmux" behavior depends on this. Adds test helper `raw_chars(key, mods, chars) -> RawBinding`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    fn raw_chars(key: &str, mods: Option<&str>, chars: &str) -> RawBinding {
        RawBinding {
            key: key.into(),
            mods: mods.map(Into::into),
            mode: None,
            chars: Some(chars.into()),
            action: None,
            command: None,
        }
    }

    #[test]
    fn user_binding_replaces_same_trigger_default() {
        // `ReceiveChar` on Ctrl+B frees the tmux prefix: the default
        // ToggleLeftSidebar must be gone, not merely outvoted.
        let b = parse_bindings(vec![raw_action("B", Some("Control"), "ReceiveChar")]);
        assert_eq!(named_matches(&b, Key::B, Modifiers::CTRL), vec![NamedAction::ReceiveChar]);
    }

    #[test]
    fn replacement_requires_exact_mods() {
        let b = parse_bindings(vec![raw_action("Tab", Some("Control|Shift"), "SelectLastTab")]);
        assert_eq!(
            named_matches(&b, Key::Tab, Modifiers::CTRL),
            vec![NamedAction::SelectNextTab],
            "Ctrl+Tab default must survive a Ctrl+Shift+Tab user binding"
        );
        assert_eq!(
            named_matches(&b, Key::Tab, Modifiers::CTRL | Modifiers::SHIFT),
            vec![NamedAction::SelectLastTab]
        );
    }

    #[test]
    fn user_rebind_suppresses_default_action() {
        // Regression guard: a rebound Ctrl+Shift+V must not also run the
        // default Paste.
        let b = parse_bindings(vec![raw_chars("V", Some("Control|Shift"), "x")]);
        let m = all_matches(&b, Key::V, Modifiers::CTRL | Modifiers::SHIFT);
        assert!(
            matches!(m.as_slice(), [BindingAction::Chars(c)] if c == b"x"),
            "expected only the user Chars binding, got {m:?}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree`
Expected: the three new tests FAIL — each match list contains both the user binding *and* the same-trigger default (e.g. `[ReceiveChar, ToggleLeftSidebar]`). The three earlier tests still pass.

- [ ] **Step 3: Filter same-trigger defaults in `parse_bindings`**

Replace the end of `parse_bindings` — the current comment ("Append alacritty's hardcoded defaults ... `matches` returns the first hit", which is stale: `all_matches` runs every hit) and the `out.extend(default_bindings());` line — with:

```rust
    // Alacritty replaces a default binding when a user binding has the same
    // trigger — key + mods (`Binding::triggers_match` in
    // `alacritty/src/config/bindings.rs`; modes don't apply here because
    // mode-bindings are dropped above).  Without the filter, a rebound key
    // would run both the user action and the default one, and a key freed
    // via `ReceiveChar` would still trigger the default.
    let defaults = default_bindings()
        .into_iter()
        .filter(|d| !out.iter().any(|u| u.key == d.key && u.mods == d.mods));
    out.extend(defaults);
    out
```

Keep the existing doc comment on `default_bindings()` itself (it explains *why* defaults exist); only the append-site comment changes.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs
git commit -m "fix(bindings): let user bindings replace same-trigger defaults"
```

---

### Task 4: Route app shortcuts through the binding system

**Files:**
- Modify: `alacritree/src/app.rs` (`handle_shortcuts` ~line 429, `dispatch_user_bindings` ~line 496, `dispatch_action` ~line 525, `consume_exact` ~line 1205)
- Modify: `alacritree/src/config.rs` (module doc header, lines 1–7)

**Interfaces:**
- Consumes: `NamedAction` variants (Task 1), default bindings (Task 2), replacement semantics (Task 3). Existing `AlacritreeApp` methods: `persist()`, `cycle_workspaces(&mut self, ctx: &Context, delta: i32)`, `add_project_via_dialog(&mut self)`, fields `show_left_sidebar`/`show_right_sidebar`.
- Produces: `handle_shortcuts` is the single binding-dispatch entry point called from `update()` (call site unchanged, still gated on `!self.is_modal_open()`).

- [ ] **Step 1: Replace `handle_shortcuts` and delete `dispatch_user_bindings`**

There is no unit test for this step: it needs a live egui `Context`, and every new `dispatch_action` arm is a one-line call to an existing method. The bindings-layer tests from Tasks 1–3 pin the matching logic; `cargo check` + the existing test suite + the manual smoke check in Step 4 gate this task.

Delete the entire current `handle_shortcuts` body (the `consume_exact` block, the split-out `ctrl_t` handling, and the `sidebars_changed`/`cycle_*`/`quit_requested`/`add_project_requested` locals) and the separate `dispatch_user_bindings` function. Replace both with one function (the body is `dispatch_user_bindings`' body minus the `is_empty` early return — defaults make the binding list never empty):

```rust
    /// Match key events against the binding table (user bindings + defaults)
    /// before the terminal sees raw events, so a binding wins over plain
    /// text input.  Matched events are consumed unless every matched action
    /// is `ReceiveChar` (alacritty's pass-through marker).
    fn handle_shortcuts(&mut self, ctx: &Context) {
        let actions: Vec<BindingAction> = ctx.input_mut(|i| {
            let mut actions = Vec::new();
            i.events.retain(|ev| {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    let matched =
                        crate::bindings::all_matches(&self.config.bindings, *key, *modifiers);
                    if !matched.is_empty() {
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

- [ ] **Step 2: Add the new `dispatch_action` arms and delete `consume_exact`**

In `dispatch_action`, insert these arms **before** the `BindingAction::Named(other) => { self.dispatch_scroll_or_other(other); }` catch-all (otherwise the new variants silently no-op through the scroll handler):

```rust
            BindingAction::Named(NamedAction::ToggleLeftSidebar) => {
                self.show_left_sidebar = !self.show_left_sidebar;
                self.persist();
            },
            BindingAction::Named(NamedAction::ToggleRightSidebar) => {
                self.show_right_sidebar = !self.show_right_sidebar;
                self.persist();
            },
            BindingAction::Named(NamedAction::SelectNextWorkspace) => {
                self.cycle_workspaces(ctx, 1);
            },
            BindingAction::Named(NamedAction::SelectPreviousWorkspace) => {
                self.cycle_workspaces(ctx, -1);
            },
            BindingAction::Named(NamedAction::AddProject) => self.add_project_via_dialog(),
```

Delete the now-unused `consume_exact` function and its doc comment (~app.rs:1205-1221). Do not touch `consume_modal_keys` — it uses egui's own `consume_key` and still serves the modal dialogs.

- [ ] **Step 3: Document alacritree-only actions in `config.rs`**

Extend the module doc header (after the sentence about `[ui]`) with:

```rust
//! Binding actions that only exist in alacritree (`ToggleLeftSidebar`,
//! `SelectNextWorkspace`, `AddProject`, …) belong in `alacritree.toml` too:
//! real alacritty warns about unknown actions if it sees them in the shared
//! `alacritty.toml`, and the array-concatenating merge means bindings placed
//! in `alacritree.toml` still add to (never clobber) the shared ones.
```

- [ ] **Step 4: Verify — tests, check, manual smoke**

Run: `cargo test -p alacritree` — expected: 6 passed.
Run: `cargo check -p alacritree` — expected: clean, no unused warnings (`consume_exact` is gone; all five new variants are constructed).
Manual smoke (needs a human at the GUI — if executing as a subagent, report this step as deferred): `cargo run -p alacritree`, then confirm Ctrl+B / Ctrl+G toggle sidebars, Ctrl+Tab / Ctrl+Shift+Tab cycle sessions, Alt+Left/Right cycle workspaces, Ctrl+T spawns a session, Ctrl+Shift+O opens the project dialog, Ctrl+Q opens the quit dialog; then add to `alacritree.toml` `[[keyboard.bindings]] key = "B", mods = "Control", action = "ReceiveChar"`, restart, and confirm Ctrl+B reaches the shell (tmux prefix works, sidebar stays put).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs alacritree/src/config.rs
git commit -m "feat(app): route app shortcuts through the binding system"
```

Body for this commit (it's the behavior-bearing change):

```
App shortcuts (Ctrl+B/G/Tab/T/Q, Ctrl+Shift+Tab/O, Alt+Left/Right) were
hardcoded and consumed key events before user bindings and the terminal
saw them, so Ctrl+B could never reach tmux and none of the shortcuts
could be moved. They are now default entries in the keyboard.bindings
table: a user binding on the same key+mods replaces the default, and
ReceiveChar forwards the key to the PTY. Default keys are unchanged.
```

---

## Verification after all tasks

Run the `verify` skill against the branch (drives the real app, not just tests) before requesting review. Update `docs/specs/planned_features.md` status lines for feature 1 (local file, not committed).
