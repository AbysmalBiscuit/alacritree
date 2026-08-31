# Sidebar Filter Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the sidebars' toggle filters into bindable, palette-listed, MCP-callable actions; add PR-state filters to the project panel; and add a configurable search scope that lets a query reach rows the toggles hide.

**Architecture:** Four layers land in dependency order. First the input layer learns which key events produced query text, so a search-box keystroke can never also run a binding — that removes four duplicated per-action guards and is what makes the new actions safe. Then `PanelFilter` stops owning key handling and exposes a toggle API the action dispatcher drives. Then the reconciler's observed inputs grow the fields that keep filtered rows from going stale. Finally the PR filters land on top of all three.

**Tech Stack:** Rust 2024 (MSRV 1.85), egui 0.31 with a vendored patched `egui-winit`, `nucleo-matcher` for fuzzy matching, `gh` shelled out for PR lookups.

**Source spec:** `docs/superpowers/specs/2026-07-29-sidebar-filter-actions-design.md`

## Global Constraints

- Only `alacritree/` and `docs/` may be edited. `alacritty/`, `alacritty_terminal/`, `alacritty_config/`, `alacritty_config_derive/` and `egui-winit/` are vendored and read-only — read them freely, never modify them.
- Every user-visible change is opt-in or reproduces today's behavior. There are exactly two deliberate exceptions, both spelled out in their tasks (Task 2 and Task 5). Nothing else may change behavior.
- No new bare-letter default keybinding may exist for a filter that does not exist today. `s`/`a`/`m`/`d`/`u` keep their keys; the four PR filters ship with no default key.
- `[ui] search_scope` defaults to `"filtered"`. `[ui] pr_status_concurrency` defaults to `0` (unlimited). Both defaults reproduce master exactly.
- Comments explain *why*, never restate *what*. No comment may reference this plan, the spec, a PR, an issue, or a task number. No change-relative phrasing (`now we`, `used to`, `previously`, `this PR`).
- Conventional Commits. Subject line imperative, lowercase after the colon, no trailing period, ≤50 chars including the `type(scope):` prefix.
- Run `cargo fmt` before every commit. If it reformats a file you did not touch, revert that file with `git checkout --` rather than sweeping it into the commit.
- Stage selectively — never `git add -A` or `git add .`. Never commit `AGENTS.md`, `PLAN-REVIEW-LOG.md`, `docs/bugs_to_fix.md`, `docs/wsl-console-crlf-fix.md`, or anything under `docs/superpowers/` or `.superpowers/`.
- The full suite is `cargo test -p alacritree`. It has a known flake on the first run after a full recompile (conpty load order); if exactly one test fails on a cold run, re-run before investigating, and report the flake if it recurs.
- `cargo test -- <name> --exact` needs the full module path (`app::tests::<name>`). Substring filtering without `--exact` is what works: `cargo test -p alacritree <substring>`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `alacritree/src/app.rs` | App state, input routing, row projection, painting | Modified throughout |
| `alacritree/src/panel_filter.rs` | Panel mode, query, toggle set | Toggle API replaces key handling |
| `alacritree/src/bindings.rs` | Action vocabulary, parsing, defaults, focus scopes | 13 new actions, 3 new predicates |
| `alacritree/src/command_palette.rs` | Palette rows and sections | New `Filters` section, array grows to 63 |
| `alacritree/src/pr_status.rs` | `gh` lookups, PR cache | Non-polling read, cap, drain, generation |
| `alacritree/src/sidebar_focus.rs` | Cursor reconciliation, observed inputs | 4 new observed fields |
| `alacritree/src/steady_state.rs` | Allocation-free steady-state assertions | Literals updated |
| `alacritree/src/config.rs` | TOML parsing | 2 new `[ui]` keys |
| `docs/alacritree.md` | User-facing reference | New keys, actions, collision rule |

---

## Task 1: Text-key detection in the input layer

**Files:**
- Modify: `alacritree/src/app.rs:1628-1657` (`handle_sidebar_nav`), `alacritree/src/app.rs:2005-2034` (`handle_git_sidebar_nav`), `alacritree/src/app.rs:4971-5011` (`drain_search_or_nav`)
- Test: `alacritree/src/app.rs` `mod tests` (same file, existing module)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `fn keys_paired_with_text(events: &[egui::Event]) -> Vec<bool>` and a sixth parameter `produced_text: bool` on `fn drain_search_or_nav(steps: &mut Vec<SidebarNavStep>, filter: &mut PanelFilter, bindings: &[crate::bindings::KeyBinding], key: egui::Key, modifiers: egui::Modifiers, produced_text: bool) -> bool`.

**Why a positional `Vec<bool>` and not a `(Key, Modifiers)` set:** two presses in one frame can share a tuple — key conversion falls back to `logical_key.or(physical_key)` (`egui-winit/src/lib.rs:764`) so distinct physical keys resolve to one `egui::Key`, and key repeat reuses the tuple outright. If one press carries adjacent text and another does not, a value-keyed set consumes both. `Vec::retain` visits every element exactly once in order, so an index counter in the closure reads the parallel vector positionally.

- [ ] **Step 1: Write the failing test for the pre-pass**

Add to `mod tests` in `alacritree/src/app.rs`:

```rust
fn key_ev(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn text_pairing_marks_only_keys_followed_by_text() {
    let events = vec![
        key_ev(egui::Key::A, true),
        egui::Event::Text("a".into()),
        key_ev(egui::Key::Enter, true),
        key_ev(egui::Key::B, true),
        egui::Event::Text("b".into()),
    ];
    assert_eq!(keys_paired_with_text(&events), vec![true, false, false, true, false]);
}

#[test]
fn text_pairing_ignores_released_keys_and_orphan_text() {
    let events = vec![
        key_ev(egui::Key::A, false),
        egui::Event::Text("a".into()),
        egui::Event::Text("pasted".into()),
    ];
    assert_eq!(keys_paired_with_text(&events), vec![false, false, false]);
}

/// Two presses sharing one `(key, modifiers)` in a frame: only the occurrence
/// actually followed by text is marked. A set keyed by value would mark both.
#[test]
fn text_pairing_is_per_occurrence_not_per_trigger() {
    let events = vec![
        key_ev(egui::Key::A, true),
        egui::Event::Text("a".into()),
        key_ev(egui::Key::A, true),
    ];
    assert_eq!(keys_paired_with_text(&events), vec![true, false, false]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree text_pairing`
Expected: FAIL — `cannot find function 'keys_paired_with_text' in this scope`.

- [ ] **Step 3: Implement the pre-pass**

Add as a free function in `alacritree/src/app.rs`, directly above `drain_search_or_nav`:

```rust
/// Which events are key presses whose text the search box will swallow.
///
/// egui-winit pushes `Event::Key` and then `Event::Text` adjacently for one
/// printable press, so adjacency identifies the pair.  The result is positional
/// rather than a set of triggers: key repeat and the `logical_key.or(physical_key)`
/// fallback both let two presses in one frame share a `(key, modifiers)`, and
/// only the occurrence carrying text may be treated as query input.
fn keys_paired_with_text(events: &[egui::Event]) -> Vec<bool> {
    events
        .iter()
        .enumerate()
        .map(|(n, ev)| {
            matches!(ev, egui::Event::Key { pressed: true, .. })
                && matches!(events.get(n + 1), Some(egui::Event::Text(_)))
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree text_pairing`
Expected: PASS, 3 tests.

- [ ] **Step 5: Write the failing tests for the consume rule**

The existing direct tests of `drain_search_or_nav` start at `alacritree/src/app.rs:7588`. Find the helper they use to build a searching filter (`searching_filter()`) and add alongside them:

```rust
/// A text-producing key in search mode is query input and must not also run a
/// binding — including one bound to a search action, since text input is
/// unconditional.
#[test]
fn a_text_key_in_search_is_consumed_before_any_binding() {
    let binds = crate::bindings::parse_bindings(vec![crate::bindings::RawBinding {
        key: "G".into(),
        mods: None,
        mode: None,
        chars: None,
        action: Some("SidebarSearchConfirm".into()),
        command: None,
    }]);
    let mut f = searching_filter();

    let mut steps = Vec::new();
    let retain = drain_search_or_nav(
        &mut steps,
        &mut f,
        &binds,
        egui::Key::G,
        egui::Modifiers::NONE,
        true,
    );

    assert!(!retain, "a key carrying query text is consumed");
    assert!(steps.is_empty(), "and dispatches nothing, not even a search action");
}

/// Shift+letter still produces text, so it must be consumed too. The modifier
/// early-return would otherwise let the built-in Shift+R reach RenameSelected.
#[test]
fn shift_letter_in_search_is_consumed() {
    let binds = crate::bindings::parse_bindings(vec![]);
    let mut f = searching_filter();

    let mut steps = Vec::new();
    let retain = drain_search_or_nav(
        &mut steps,
        &mut f,
        &binds,
        egui::Key::R,
        egui::Modifiers::SHIFT,
        true,
    );

    assert!(!retain);
    assert!(steps.is_empty());
}

/// Bare Delete carries no text, so the pairing rule cannot claim it. It is a
/// search-box editing key, so it is consumed as a no-op instead of reaching the
/// cursored row.
#[test]
fn bare_delete_in_search_is_consumed_as_a_no_op() {
    let binds = crate::bindings::parse_bindings(vec![]);
    let mut f = searching_filter();

    let mut steps = Vec::new();
    let retain = drain_search_or_nav(
        &mut steps,
        &mut f,
        &binds,
        egui::Key::Delete,
        egui::Modifiers::NONE,
        false,
    );

    assert!(!retain);
}

/// Keys that produce no text keep falling through to the binding table, which
/// is what lets Home/End/PageUp/PageDown navigate filtered results.
#[test]
fn non_text_keys_in_search_still_fall_through() {
    let binds = crate::bindings::parse_bindings(vec![]);
    for key in [egui::Key::ArrowLeft, egui::Key::ArrowRight, egui::Key::Tab, egui::Key::Home] {
        let mut f = searching_filter();
        let mut steps = Vec::new();
        let retain = drain_search_or_nav(
            &mut steps,
            &mut f,
            &binds,
            key,
            egui::Modifiers::NONE,
            false,
        );
        assert!(retain, "{key:?} produces no query text and must reach the binding table");
    }
}

/// Browsing mode is untouched: a letter must reach the binding table, which is
/// how the filter toggle actions fire.
#[test]
fn a_text_key_in_browsing_is_not_consumed_by_the_pairing_rule() {
    let binds = crate::bindings::parse_bindings(vec![]);
    let mut f = PanelFilter::new(&['s', 'a']);

    let mut steps = Vec::new();
    let retain =
        drain_search_or_nav(&mut steps, &mut f, &binds, egui::Key::S, egui::Modifiers::NONE, true);

    assert!(retain);
}
```

- [ ] **Step 6: Run the tests to verify they fail for the right reason**

Run: `cargo test -p alacritree in_search`
Expected: FAIL to compile — `this function takes 5 arguments but 6 arguments were supplied`. That is the correct RED: the parameter does not exist yet.

- [ ] **Step 7: Add the parameter and the two consume points**

In `alacritree/src/app.rs`, change the signature and doc comment of `drain_search_or_nav`, and insert the two new rules. The full function afterwards:

```rust
/// Decide one key event for a focused sidebar panel and record its step.
///
/// In search mode a key whose text the query already swallowed is consumed
/// outright — text input is unconditional, so it outranks even a search-scoped
/// binding on that letter.  Otherwise a search-scoped binding match (any
/// modifiers, so `Shift+Esc` counts) is dispatched through the binding table,
/// keeping `Enter`/`Esc` rebindable; an unmodified key drives the filter or
/// browsing nav; and a modified non-search key is retained for
/// `handle_shortcuts`.  Returns whether the event stays in the queue (`true`)
/// or is consumed here (`false`).
fn drain_search_or_nav(
    steps: &mut Vec<SidebarNavStep>,
    filter: &mut PanelFilter,
    bindings: &[crate::bindings::KeyBinding],
    key: egui::Key,
    modifiers: egui::Modifiers,
    produced_text: bool,
) -> bool {
    let searching = filter.mode() == panel_filter::Mode::Search;
    if searching && produced_text {
        return false;
    }
    if searching {
        let mut matched = false;
        for a in crate::bindings::all_matches(bindings, key, modifiers) {
            if let BindingAction::Named(n) = a {
                if n.is_search_scoped() {
                    steps.push(SidebarNavStep::SearchAction(*n));
                    matched = true;
                }
            }
        }
        if matched {
            return false;
        }
    }
    if !modifiers.is_none() {
        return true;
    }
    if let Some(outcome) = filter.on_key(key) {
        steps.push(SidebarNavStep::Filter(outcome));
        return false;
    }
    // Browsing consumes the whole nav-key set.  In search only Space and Delete
    // stay consumed as no-ops: Space preserves the fake-click guard on the
    // terminal view, and Delete is a text-editing key the append-only query has
    // nothing to do with, so it must not fall through to the cursored row.
    let consume = if filter.mode() == panel_filter::Mode::Browsing {
        is_sidebar_nav_key(key)
    } else {
        key == egui::Key::Space || key == egui::Key::Delete
    };
    if consume {
        steps.push(SidebarNavStep::Nav(key));
        return false;
    }
    true
}
```

Note the `Delete` arm pushes `SidebarNavStep::Nav(egui::Key::Delete)`. `apply_sidebar_nav` and `apply_git_sidebar_nav` both `match` on the key and have a `_ => {}` arm, so an unhandled `Delete` is already a no-op there — verify this before moving on by reading `apply_sidebar_nav`'s match arms.

- [ ] **Step 8: Update both call sites to feed the pre-pass**

In `alacritree/src/app.rs`, `handle_sidebar_nav`:

```rust
    fn handle_sidebar_nav(&mut self, ctx: &Context) {
        let filter = &mut self.project_filter;
        let bindings = &self.config.bindings;
        let steps: Vec<SidebarNavStep> = ctx.input_mut(|i| {
            let mut steps = Vec::new();
            let text_keys = keys_paired_with_text(&i.events);
            let mut idx = 0;
            i.events.retain(|ev| {
                let produced_text = text_keys[idx];
                idx += 1;
                match ev {
                    egui::Event::Text(text) => match filter.on_text(text) {
                        Some(outcome) => {
                            steps.push(SidebarNavStep::Filter(outcome));
                            false
                        },
                        None => true,
                    },
                    egui::Event::Key { key, pressed: true, modifiers, .. } => drain_search_or_nav(
                        &mut steps,
                        filter,
                        bindings,
                        *key,
                        *modifiers,
                        produced_text,
                    ),
                    _ => true,
                }
            });
            steps
        });
        for step in steps {
            match step {
                SidebarNavStep::Filter(outcome) => self.apply_filter_outcome(outcome),
                SidebarNavStep::Nav(key) => self.apply_sidebar_nav(ctx, key),
                SidebarNavStep::SearchAction(action) => {
                    self.dispatch_action(ctx, BindingAction::Named(action), ActionOrigin::Keyboard);
                },
            }
        }
    }
```

Apply the identical `text_keys` / `idx` change to `handle_git_sidebar_nav` (`alacritree/src/app.rs:2005`), keeping its own `SidebarNavStep` arms as they are.

`handle_shortcuts` runs a second `retain` over the same queue afterwards and does **not** need the pre-pass: by then the sidebar pass has already removed every key it claimed.

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p alacritree in_search`
Expected: PASS.

Run: `cargo test -p alacritree drain_search`
Expected: PASS — the pre-existing direct tests, updated for the new argument.

Run: `cargo test -p alacritree search_enter_with_no_binding`
Expected: PASS unchanged. `Enter` emits `\r`, which `is_printable_char` rejects (`egui-winit/src/lib.rs:1043`), so no `Event::Text` is ever produced for it and `produced_text` is `false`.

- [ ] **Step 10: Run the full suite**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "fix(sidebar): keep search text out of the binding table"
```

---

## Task 2: Retire the four per-action browsing guards

**Files:**
- Modify: `alacritree/src/bindings.rs:154-165` (add a predicate beside `is_sidebar_scoped`), `alacritree/src/app.rs:232-235` (`ActionOrigin`), `alacritree/src/app.rs:2165` (`dispatch_action` head), `alacritree/src/app.rs:2346-2404` (the four guards), `alacritree/src/app.rs:6381-6385` (`run_palette_action`)
- Test: `alacritree/src/bindings.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1's consume rule, which is what makes the keyboard half of these guards redundant.
- Produces: `NamedAction::requires_project_browsing(&self) -> bool` and `ActionOrigin::Palette`.

**Deliberate behavior change (exception 1 of 2).** A text-producing key bound to an action with *no* guard today stops firing mid-query — a letter bound to `SidebarTop`/`SidebarBottom`/`SidebarNextProject`/`SidebarPreviousProject`, or to any unscoped action such as `Quit`, fires during search on master and now types into the query instead. That is the defect the four guards were patching one action at a time.

- [ ] **Step 1: Write the failing test for the predicate**

Add to `mod tests` in `alacritree/src/bindings.rs`:

```rust
/// Exactly the four actions that carry a browsing-mode guard at dispatch. It
/// must not be widened to `is_sidebar_scoped`, whose four extra actions run
/// from the palette during search today.
#[test]
fn requires_project_browsing_is_exactly_the_four_guarded_actions() {
    use NamedAction::*;
    for a in [RefreshProjects, DeleteSelected, RenameSelected, ToggleProjectExpanded] {
        assert!(a.requires_project_browsing(), "{a:?}");
    }
    for a in [
        SidebarTop,
        SidebarBottom,
        SidebarNextProject,
        SidebarPreviousProject,
        CloseSession,
        Quit,
    ] {
        assert!(!a.requires_project_browsing(), "{a:?}");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p alacritree requires_project_browsing`
Expected: FAIL — `no method named 'requires_project_browsing'`.

- [ ] **Step 3: Add the predicate**

In `alacritree/src/bindings.rs`, directly after `is_sidebar_scoped`:

```rust
    /// Sidebar-cursor actions whose meaning depends on the project panel
    /// browsing rather than searching.  Narrower than `is_sidebar_scoped`: the
    /// four cursor *moves* stay valid mid-query, because navigating filtered
    /// results is the point of filtering.
    pub fn requires_project_browsing(&self) -> bool {
        matches!(
            self,
            Self::RefreshProjects
                | Self::DeleteSelected
                | Self::RenameSelected
                | Self::ToggleProjectExpanded
        )
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p alacritree requires_project_browsing`
Expected: PASS.

- [ ] **Step 5: Add the palette origin**

In `alacritree/src/app.rs:232`:

```rust
enum ActionOrigin {
    Keyboard,
    Palette,
    Ipc,
}
```

In `run_palette_action` (`alacritree/src/app.rs:6383`):

```rust
            PaletteAction::Run(a) => {
                self.dispatch_action(ctx, BindingAction::Named(a), ActionOrigin::Palette);
            },
```

- [ ] **Step 6: Add the single guard and delete the four**

At the top of `dispatch_action` in `alacritree/src/app.rs`, before the `match action`:

```rust
        // A palette row is dispatched with the panel still searching, and the
        // cursor operations below act on a row the query may have hidden.  The
        // keyboard path cannot reach here mid-query at all: a letter's text is
        // swallowed by the query before the binding table sees the key.
        if origin == ActionOrigin::Palette
            && matches!(&action, BindingAction::Named(n) if n.requires_project_browsing())
            && self.project_filter.mode() != panel_filter::Mode::Browsing
        {
            return;
        }
```

Then delete the guard from each of the four arms, leaving their bodies intact:

- `RefreshProjects` (`alacritree/src/app.rs:2346-2355`) becomes `self.refresh_all_projects(ctx)` unconditionally.
- `DeleteSelected` (`:2356-2364`) loses its `if origin != ActionOrigin::Ipc && ... { return; }`.
- `RenameSelected` (`:2379-2386`) loses the same block.
- `ToggleProjectExpanded` (`:2397-2404`) loses the same block.

Delete their explanatory comments with them — they describe a mechanism that no longer lives there.

- [ ] **Step 7: Verify the whole suite**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS. If a test asserts one of the four actions is inert in search mode via the *keyboard* path, it should now be expressed through `drain_search_or_nav` instead; update it rather than reinstating a guard.

- [ ] **Step 8: Commit**

```bash
git add alacritree/src/app.rs alacritree/src/bindings.rs
git commit -m "refactor(sidebar): hoist the search-mode action guard"
```

---

## Task 3: PanelFilter toggle API

**Files:**
- Modify: `alacritree/src/panel_filter.rs:117-142` (`on_text`), and add methods to `impl PanelFilter`
- Test: `alacritree/src/panel_filter.rs` `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `PanelFilter::toggle(&mut self, key: char)` and `PanelFilter::clear_toggles(&mut self)`. `allowed_toggles` stops being a key list and becomes the ordered identity list `toggle_bits` indexes and `active_toggles` renders.

- [ ] **Step 1: Update the existing tests and add the new ones**

In `alacritree/src/panel_filter.rs` `mod tests`, replace `toggle_keys_flip_in_browsing_and_are_inert_in_search` with:

```rust
    #[test]
    fn toggle_flips_an_allowed_identity_and_ignores_an_unknown_one() {
        let mut f = PanelFilter::new(TOGGLES);
        f.toggle('s');
        assert!(f.is_toggled('s'));
        assert_eq!(f.active_toggles(), vec!['s']);

        f.toggle('s');
        assert!(!f.is_toggled('s'));

        f.toggle('z');
        assert!(!f.is_toggled('z'), "a char outside allowed_toggles is not a filter");
        assert_eq!(f.toggle_bits(), 0);
    }

    #[test]
    fn clear_toggles_empties_the_set_and_leaves_the_query() {
        let mut f = PanelFilter::new(TOGGLES);
        f.toggle('s');
        f.toggle('a');
        f.on_text("/");
        f.on_text("foo");

        f.clear_toggles();
        assert_eq!(f.toggle_bits(), 0);
        assert_eq!(f.query(), "foo", "the query is a separate dimension");
    }

    /// Browsing recognizes only `/`; every other char falls through so the
    /// binding table can act on the paired key event.
    #[test]
    fn browsing_text_other_than_slash_is_not_consumed() {
        let mut f = PanelFilter::new(TOGGLES);
        assert_eq!(f.on_text("s"), None);
        assert_eq!(f.on_text("x"), None);
        assert_eq!(f.on_text("/"), Some(Outcome::Consumed));
        assert_eq!(f.on_text("s"), Some(Outcome::FilterChanged), "in search it is query input");
        assert_eq!(f.query(), "s");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree --lib panel_filter`
Expected: FAIL — `no method named 'toggle'`, plus `browsing_text_other_than_slash_is_not_consumed` failing on `Some(Outcome::FilterChanged)` where `None` is expected.

- [ ] **Step 3: Replace the toggle branch and add the API**

In `alacritree/src/panel_filter.rs`, `on_text`'s `Browsing` arm becomes:

```rust
            Mode::Browsing => {
                if text == "/" {
                    self.mode = Mode::Search;
                    return Some(Outcome::Consumed);
                }
                None
            },
```

Add to `impl PanelFilter`, beside `is_toggled`:

```rust
    /// Flip one toggle by its identity char.  A char outside `allowed_toggles`
    /// names no filter on this panel and is ignored.
    pub fn toggle(&mut self, key: char) {
        if !self.allowed_toggles.contains(&key) {
            return;
        }
        if !self.toggles.remove(&key) {
            self.toggles.insert(key);
        }
    }

    pub fn clear_toggles(&mut self) {
        self.toggles.clear();
    }
```

Update the doc comment on the `allowed_toggles` field to say it is the render/bit order of the panel's filters, not a set of keys.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alacritree --lib panel_filter`
Expected: PASS.

- [ ] **Step 5: Fix the fallout in `app.rs`**

`cargo check -p alacritree` will now fail where `esc_in_browsing_clears_toggles_before_leaving_the_panel` or `app.rs`'s own filter tests drive toggles through `on_text`. Rewrite each to call `toggle` directly. Toggling via text is no longer how the feature works.

Run: `cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/panel_filter.rs alacritree/src/app.rs
git commit -m "refactor(sidebar): expose panel toggles as an API"
```

---

## Task 4: The thirteen actions in the binding layer

**Files:**
- Modify: `alacritree/src/bindings.rs` — `enum NamedAction`, `is_projects_filter_scoped`/`is_git_filter_scoped` (new), `parse_action` (`:797`), `description` (`:221`), `default_bindings` (`:368`)
- Test: `alacritree/src/bindings.rs` `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: variants `ToggleSessionsFilter`, `ToggleAttentionFilter`, `TogglePrOpenFilter`, `TogglePrDraftFilter`, `TogglePrMergedFilter`, `TogglePrClosedFilter`, `ClearProjectFilters`, `ToggleModifiedFilter`, `ToggleDeletedFilter`, `ToggleUntrackedFilter`, `ClearGitFilters`, `ToggleSearchScope`, `RefreshPrStatus`; predicates `is_projects_filter_scoped(&self) -> bool` and `is_git_filter_scoped(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/bindings.rs`:

```rust
    const PROJECT_FILTER_ACTIONS: [NamedAction; 7] = [
        NamedAction::ToggleSessionsFilter,
        NamedAction::ToggleAttentionFilter,
        NamedAction::TogglePrOpenFilter,
        NamedAction::TogglePrDraftFilter,
        NamedAction::TogglePrMergedFilter,
        NamedAction::TogglePrClosedFilter,
        NamedAction::ClearProjectFilters,
    ];

    const GIT_FILTER_ACTIONS: [NamedAction; 4] = [
        NamedAction::ToggleModifiedFilter,
        NamedAction::ToggleDeletedFilter,
        NamedAction::ToggleUntrackedFilter,
        NamedAction::ClearGitFilters,
    ];

    #[test]
    fn every_new_action_round_trips_and_is_described() {
        let mut all = PROJECT_FILTER_ACTIONS.to_vec();
        all.extend(GIT_FILTER_ACTIONS);
        all.push(NamedAction::ToggleSearchScope);
        all.push(NamedAction::RefreshPrStatus);
        for a in all {
            let name = a.config_name();
            assert!(
                matches!(parse_action(&name), BindingAction::Named(p) if p == a),
                "{name} does not parse back"
            );
            assert!(!a.description().is_empty(), "{name} has no description");
        }
    }

    #[test]
    fn filter_actions_are_scoped_to_their_own_panel() {
        for a in PROJECT_FILTER_ACTIONS {
            assert!(a.is_projects_filter_scoped(), "{a:?}");
            assert!(!a.is_git_filter_scoped(), "{a:?}");
        }
        for a in GIT_FILTER_ACTIONS {
            assert!(a.is_git_filter_scoped(), "{a:?}");
            assert!(!a.is_projects_filter_scoped(), "{a:?}");
        }
    }

    /// The filter actions own their own focus predicates and must not leak into
    /// the ones that already gate other dispatch paths.
    #[test]
    fn filter_actions_carry_no_other_scope() {
        let mut all = PROJECT_FILTER_ACTIONS.to_vec();
        all.extend(GIT_FILTER_ACTIONS);
        for a in all {
            assert!(!a.is_sidebar_scoped(), "{a:?}");
            assert!(!a.is_search_scoped(), "{a:?}");
            assert!(!a.is_palette_scoped(), "{a:?}");
            assert!(!a.requires_project_browsing(), "{a:?}");
        }
    }

    #[test]
    fn refresh_pr_status_is_sidebar_scoped_and_toggle_search_scope_is_unscoped() {
        assert!(NamedAction::RefreshPrStatus.is_sidebar_scoped());
        assert!(!NamedAction::RefreshPrStatus.requires_project_browsing());
        assert!(!NamedAction::ToggleSearchScope.is_sidebar_scoped());
        assert!(!NamedAction::ToggleSearchScope.is_projects_filter_scoped());
        assert!(!NamedAction::ToggleSearchScope.is_git_filter_scoped());
    }

    /// The five filters that exist today keep their keys; the four PR filters
    /// introduce no new bare-letter default for anyone.
    #[test]
    fn default_bindings_cover_the_existing_filters_and_no_pr_filter() {
        let binds = parse_bindings(vec![]);
        let bound = |a: NamedAction| {
            binds.iter().find(|b| matches!(&b.action, BindingAction::Named(n) if *n == a))
        };
        for (key, action) in [
            (Key::S, NamedAction::ToggleSessionsFilter),
            (Key::A, NamedAction::ToggleAttentionFilter),
            (Key::M, NamedAction::ToggleModifiedFilter),
            (Key::D, NamedAction::ToggleDeletedFilter),
            (Key::U, NamedAction::ToggleUntrackedFilter),
        ] {
            let b = bound(action).unwrap_or_else(|| panic!("{action:?} has no default"));
            assert_eq!(b.key, key);
            assert_eq!(b.mods, Modifiers::NONE);
        }
        for a in [
            NamedAction::TogglePrOpenFilter,
            NamedAction::TogglePrDraftFilter,
            NamedAction::TogglePrMergedFilter,
            NamedAction::TogglePrClosedFilter,
            NamedAction::RefreshPrStatus,
            NamedAction::ToggleSearchScope,
        ] {
            assert!(bound(a).is_none(), "{a:?} must ship without a default key");
        }
    }

    /// Two user bindings on one trigger both survive and both come back from
    /// `all_matches`, which is how a user who claimed a letter recovers the
    /// default it displaced. Only the *default* is dropped.
    #[test]
    fn two_user_bindings_on_one_trigger_both_survive() {
        let raw = |action: &str| RawBinding {
            key: "D".into(),
            mods: None,
            mode: None,
            chars: None,
            action: Some(action.into()),
            command: None,
        };
        let binds = parse_bindings(vec![raw("DeleteSelected"), raw("ToggleDeletedFilter")]);

        let matched: Vec<_> = all_matches(&binds, Key::D, Modifiers::NONE).collect();
        assert!(
            matched.iter().any(|a| matches!(a, BindingAction::Named(NamedAction::DeleteSelected)))
        );
        assert!(matched.iter().any(
            |a| matches!(a, BindingAction::Named(NamedAction::ToggleDeletedFilter))
        ));
    }
```

If `all_matches` returns something other than an iterator of `&BindingAction`, adjust the last test's `.collect()` to match its real signature — read `bindings.rs:582` first.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree --lib bindings`
Expected: FAIL — the variants do not exist.

- [ ] **Step 3: Add the variants**

In `enum NamedAction` in `alacritree/src/bindings.rs`, after the existing sidebar actions:

```rust
    /// Narrow the projects sidebar to workspaces with a live session.
    ToggleSessionsFilter,
    /// Narrow the projects sidebar to workspaces whose session wants attention.
    ToggleAttentionFilter,
    /// PR-state filters.  One dimension: the active states union, and the
    /// result ANDs with the session and attention filters.
    TogglePrOpenFilter,
    TogglePrDraftFilter,
    TogglePrMergedFilter,
    TogglePrClosedFilter,
    /// Drop every projects-sidebar toggle.  Reachable without knowing which are
    /// set, which `Esc` cannot offer a caller that has no view of the state.
    ClearProjectFilters,
    /// Git sidebar change-kind filters.  The active kinds union.
    ToggleModifiedFilter,
    ToggleDeletedFilter,
    ToggleUntrackedFilter,
    ClearGitFilters,
    /// Switch between a query confined by the active toggles and one evaluated
    /// against every row.  Session-only; restarting returns to `[ui] search_scope`.
    ToggleSearchScope,
    /// Re-query `gh` for every cached worktree.
    RefreshPrStatus,
```

- [ ] **Step 4: Add the scope predicates**

Beside `requires_project_browsing` in `alacritree/src/bindings.rs`:

```rust
    /// Valid only while the projects sidebar owns focus: the default triggers
    /// are bare letters that belong to the PTY anywhere else.  No mode
    /// component is needed — a letter typed into the search box never reaches
    /// the binding table, because its text is swallowed first.
    pub fn is_projects_filter_scoped(&self) -> bool {
        matches!(
            self,
            Self::ToggleSessionsFilter
                | Self::ToggleAttentionFilter
                | Self::TogglePrOpenFilter
                | Self::TogglePrDraftFilter
                | Self::TogglePrMergedFilter
                | Self::TogglePrClosedFilter
                | Self::ClearProjectFilters
        )
    }

    /// The git sidebar's equivalent.
    pub fn is_git_filter_scoped(&self) -> bool {
        matches!(
            self,
            Self::ToggleModifiedFilter
                | Self::ToggleDeletedFilter
                | Self::ToggleUntrackedFilter
                | Self::ClearGitFilters
        )
    }
```

- [ ] **Step 5: Add parse arms, descriptions, and defaults**

In `parse_action` (`alacritree/src/bindings.rs:799`), one arm per variant, e.g. `"ToggleSessionsFilter" => BindingAction::Named(ToggleSessionsFilter),` — thirteen in total, each name matching `config_name()`'s `Debug` output exactly.

In `description` (`:221`):

```rust
            Self::ToggleSessionsFilter => "Filter the sidebar to workspaces with a session".into(),
            Self::ToggleAttentionFilter => "Filter the sidebar to workspaces wanting attention".into(),
            Self::TogglePrOpenFilter => "Filter the sidebar to worktrees with an open PR".into(),
            Self::TogglePrDraftFilter => "Filter the sidebar to worktrees with a draft PR".into(),
            Self::TogglePrMergedFilter => "Filter the sidebar to worktrees with a merged PR".into(),
            Self::TogglePrClosedFilter => "Filter the sidebar to worktrees with a closed PR".into(),
            Self::ClearProjectFilters => "Clear every projects-sidebar filter".into(),
            Self::ToggleModifiedFilter => "Filter git changes to modified and renamed files".into(),
            Self::ToggleDeletedFilter => "Filter git changes to deleted files".into(),
            Self::ToggleUntrackedFilter => "Filter git changes to untracked and added files".into(),
            Self::ClearGitFilters => "Clear every git-sidebar filter".into(),
            Self::ToggleSearchScope => "Search inside the active filters or across every row".into(),
            Self::RefreshPrStatus => "Re-query GitHub for every worktree's PR".into(),
```

In `default_bindings` (`:368`), beside the existing sidebar defaults:

```rust
        KeyBinding {
            key: Key::S,
            mods: Modifiers::NONE,
            action: BindingAction::Named(ToggleSessionsFilter),
        },
        KeyBinding {
            key: Key::A,
            mods: Modifiers::NONE,
            action: BindingAction::Named(ToggleAttentionFilter),
        },
        KeyBinding {
            key: Key::M,
            mods: Modifiers::NONE,
            action: BindingAction::Named(ToggleModifiedFilter),
        },
        KeyBinding {
            key: Key::D,
            mods: Modifiers::NONE,
            action: BindingAction::Named(ToggleDeletedFilter),
        },
        KeyBinding {
            key: Key::U,
            mods: Modifiers::NONE,
            action: BindingAction::Named(ToggleUntrackedFilter),
        },
```

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p alacritree --lib bindings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/bindings.rs
git commit -m "feat(sidebar): add filter toggle actions"
```

---

## Task 5: Dispatch, focus routing, and palette rows

**Files:**
- Modify: `alacritree/src/app.rs:1560-1602` (`handle_shortcuts`), `alacritree/src/app.rs:2165+` (`dispatch_action`), `alacritree/src/command_palette.rs:40-91` (sections), `alacritree/src/command_palette.rs:218` (`bindable_actions`)
- Test: `alacritree/src/command_palette.rs` `mod tests`

**Interfaces:**
- Consumes: Task 3's `toggle`/`clear_toggles`; Task 4's thirteen variants and two scope predicates.
- Produces: `PaletteSection::Filters`; `bindable_actions() -> [NamedAction; 63]`.

**Deliberate behavior change (exception 2 of 2).** Toggling moves from text-driven to key-driven. `egui-winit` resolves keys as `logical_key.or(physical_key)` (`egui-winit/src/lib.rs:764`), so on a non-Latin layout the physical `S` position toggles the sessions filter even though its text is not `"s"`; and with Caps Lock on, the text is `"S"` but the key event is `Key::S` with no Shift, so the binding matches where lowercase-only text matching did not. Ordinary lowercase Latin input is unaffected, and Shift+letter stays inert because bindings match modifiers exactly.

- [ ] **Step 1: Write the failing palette test**

Add to `mod tests` in `alacritree/src/command_palette.rs`:

```rust
    #[test]
    fn filter_actions_are_listed_under_their_own_section() {
        let items = action_items(&parse_bindings(vec![]));
        for name in [
            "ToggleSessionsFilter",
            "ToggleAttentionFilter",
            "TogglePrOpenFilter",
            "TogglePrDraftFilter",
            "TogglePrMergedFilter",
            "TogglePrClosedFilter",
            "ClearProjectFilters",
            "ToggleModifiedFilter",
            "ToggleDeletedFilter",
            "ToggleUntrackedFilter",
            "ClearGitFilters",
            "ToggleSearchScope",
        ] {
            let item = find(&items, name).unwrap_or_else(|| panic!("{name} is not in the palette"));
            assert_eq!(item.section, PaletteSection::Filters, "{name}");
        }
        let refresh = find(&items, "RefreshPrStatus").expect("RefreshPrStatus missing");
        assert_eq!(refresh.section, PaletteSection::Sidebar);
    }

    /// The PR filters ship keyless, so the palette is the only place they are
    /// discoverable until a user binds them.
    #[test]
    fn pr_filter_actions_are_listed_without_keys() {
        let items = action_items(&parse_bindings(vec![]));
        assert_eq!(find(&items, "TogglePrOpenFilter").unwrap().keys, "");
        assert_eq!(find(&items, "ToggleSessionsFilter").unwrap().keys, "S");
    }
```

Read the existing `find` helper before writing these — it matches on `secondary` (the `config_name`). If `format_shortcut(Key::S, Modifiers::NONE)` renders something other than `"S"`, use whatever it actually produces.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree --lib command_palette`
Expected: FAIL — no `PaletteSection::Filters`.

- [ ] **Step 3: Add the section and route the actions**

In `alacritree/src/command_palette.rs`, add `Filters` to `enum PaletteSection` (after `Sidebar`), give it `Self::Filters => "Filters"` in `title`, and add to `section_of`:

```rust
        ToggleSessionsFilter | ToggleAttentionFilter | ClearProjectFilters => Filters,
        TogglePrOpenFilter | TogglePrDraftFilter => Filters,
        TogglePrMergedFilter | TogglePrClosedFilter => Filters,
        ToggleModifiedFilter | ToggleDeletedFilter => Filters,
        ToggleUntrackedFilter | ClearGitFilters | ToggleSearchScope => Filters,
        RefreshPrStatus => Sidebar,
```

Place these arms **above** the `_ => Window` fallback.

Extend `bindable_actions()` to `[NamedAction; 63]` and add the thirteen names to its array.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alacritree --lib command_palette`
Expected: PASS.

- [ ] **Step 5: Route focus in `handle_shortcuts`**

In `alacritree/src/app.rs:1561`, beside `sidebar_focused`:

```rust
        let git_focused = self.focus == PaneFocus::GitSidebar && !self.palette.is_open();
```

In the `valid_for_focus` closure (`:1580`), replace the single expression with:

```rust
                            let valid_for_focus = match a {
                                BindingAction::Named(n) if n.is_projects_filter_scoped() => {
                                    sidebar_focused
                                },
                                BindingAction::Named(n) if n.is_git_filter_scoped() => git_focused,
                                BindingAction::Named(n) if n.is_sidebar_scoped() => sidebar_focused,
                                _ => true,
                            };
```

- [ ] **Step 6: Add the dispatch arms**

In `dispatch_action` in `alacritree/src/app.rs`:

```rust
            BindingAction::Named(NamedAction::ToggleSessionsFilter) => {
                self.project_filter.toggle('s');
            },
            BindingAction::Named(NamedAction::ToggleAttentionFilter) => {
                self.project_filter.toggle('a');
            },
            BindingAction::Named(NamedAction::TogglePrOpenFilter) => {
                self.project_filter.toggle('o');
            },
            BindingAction::Named(NamedAction::TogglePrDraftFilter) => {
                self.project_filter.toggle('d');
            },
            BindingAction::Named(NamedAction::TogglePrMergedFilter) => {
                self.project_filter.toggle('m');
            },
            BindingAction::Named(NamedAction::TogglePrClosedFilter) => {
                self.project_filter.toggle('c');
            },
            BindingAction::Named(NamedAction::ClearProjectFilters) => {
                self.project_filter.clear_toggles();
            },
            BindingAction::Named(NamedAction::ToggleModifiedFilter) => {
                self.git_filter.toggle('m');
                self.after_git_filter_changed();
            },
            BindingAction::Named(NamedAction::ToggleDeletedFilter) => {
                self.git_filter.toggle('d');
                self.after_git_filter_changed();
            },
            BindingAction::Named(NamedAction::ToggleUntrackedFilter) => {
                self.git_filter.toggle('u');
                self.after_git_filter_changed();
            },
            BindingAction::Named(NamedAction::ClearGitFilters) => {
                self.git_filter.clear_toggles();
                self.after_git_filter_changed();
            },
```

The project arms deliberately do nothing further: the focus reconciler repairs the cursor later in the same `update` from a snapshot that still knows which row the filter hid, which is why `Outcome::FilterChanged` is also a no-op at `alacritree/src/app.rs:1665`.

`ToggleSearchScope` and `RefreshPrStatus` get their arms in Tasks 7 and 10. Until then give each a `{}` body so the match compiles — and remove those placeholders in the task that fills them, do not leave one behind.

- [ ] **Step 7: Verify**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 8: Manual smoke check**

Run: `cargo run -p alacritree`

Focus the projects sidebar, press `s` — the `[s]` chip appears and rows narrow to workspaces with sessions. Press `s` again to clear. Focus the git sidebar, press `d` — it filters to deleted files and does *not* touch the project panel. Press `/`, type `sad`, and confirm the letters land in the query without toggling anything. In the terminal, type `sad` and confirm all three characters reach the shell.

- [ ] **Step 9: Commit**

```bash
git add alacritree/src/app.rs alacritree/src/command_palette.rs
git commit -m "feat(sidebar): dispatch filter toggles as actions"
```

---

## Task 6: Widen the reconciler's observed inputs

**Files:**
- Modify: `alacritree/src/sidebar_focus.rs:296-304` (`UiInputs`), `:330-347` (`ProjectInput`, `ObservedInputs`), `:349-374` (`capture`), `:384-429` (`matches`), `:444` (test helper); `alacritree/src/app.rs:1792-1809`, `:1830-1845`; `alacritree/src/steady_state.rs:114`, `:132`
- Test: `alacritree/src/sidebar_focus.rs` `mod tests`, `alacritree/src/steady_state.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `UiInputs { session_rows_always: bool, query: &'a str, toggles: u32, toggles_apply: bool, pr_generation: u64, active_workspace: Option<&'a Path>, active_branch: Option<&'a str> }`, and `ProjectInput`'s worktree tuple widened to `(PathBuf, String, bool, Option<String>)` carrying `wt.branch`.

**Why all four at once:** every one of them is a way the projected row set can change while the current `matches` reports "unchanged", which makes the reconciler skip its rebuild and painting reuse `sidebar_rows_cache` (`alacritree/src/app.rs:2778-2781`). They also touch the same four literal sites, so splitting them means editing those sites twice.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/sidebar_focus.rs`:

```rust
    fn ui_full<'a>(
        query: &'a str,
        toggles: u32,
        toggles_apply: bool,
        pr_generation: u64,
        active_workspace: Option<&'a Path>,
        active_branch: Option<&'a str>,
    ) -> UiInputs<'a> {
        UiInputs {
            session_rows_always: false,
            query,
            toggles,
            toggles_apply,
            pr_generation,
            active_workspace,
            active_branch,
        }
    }

    /// Each new field is a way the row set moves without any older field
    /// moving. Missing one leaves the sidebar showing a stale projection.
    #[test]
    fn each_new_ui_input_invalidates_the_snapshot() {
        let projects: Vec<Project> = Vec::new();
        let none: [SessionInput<'_>; 0] = [];
        let wt = PathBuf::from("/repo/wt");

        let base = ui_full("q", 0b01, true, 7, Some(&wt), Some("main"));
        let captured = ObservedInputs::capture(&projects, none.iter().copied(), base);

        assert!(captured.matches(&projects, none.iter().copied(), base), "control");

        for changed in [
            ui_full("q", 0b01, false, 7, Some(&wt), Some("main")),
            ui_full("q", 0b01, true, 8, Some(&wt), Some("main")),
            ui_full("q", 0b01, true, 7, None, Some("main")),
            ui_full("q", 0b01, true, 7, Some(&wt), Some("feature")),
        ] {
            assert!(
                !captured.matches(&projects, none.iter().copied(), changed),
                "a changed input reported unchanged: {changed:?}"
            );
        }
    }
```

Add a worktree-branch case to the same module, building two `Project` values that differ only in `worktrees[0].branch`, and assert `matches` returns `false`. Follow whatever `Project`/`Worktree` construction the existing tests in this module already use.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree --lib sidebar_focus`
Expected: FAIL — `UiInputs` has no field `toggles_apply`.

- [ ] **Step 3: Widen the structs**

In `alacritree/src/sidebar_focus.rs`:

```rust
/// Sidebar UI inputs that change the projection without changing the model.
/// `toggles` is a bitmask rather than a slice so the comparison never
/// allocates.
#[derive(Debug, Clone, Copy)]
pub struct UiInputs<'a> {
    pub session_rows_always: bool,
    pub query: &'a str,
    pub toggles: u32,
    /// Whether the toggles narrow rows this frame.  A search scope that stands
    /// them down changes the projection while `toggles` itself holds still.
    pub toggles_apply: bool,
    /// Advances when a PR lookup is banked or invalidated.  Fed as `0` unless a
    /// PR filter is active, so a completion cannot invalidate a projection it
    /// could not have changed.
    pub pr_generation: u64,
    /// The workspace whose live branch `active_branch` describes.  Without it a
    /// switch between two worktrees whose caches hold the same branch string
    /// moves every PR lookup key while nothing observed changes.
    pub active_workspace: Option<&'a Path>,
    pub active_branch: Option<&'a str>,
}
```

Mirror the five new fields on `ObservedInputs` (owning `Option<PathBuf>` and `Option<String>` for the last two), widen `ProjectInput`'s worktree tuple to `(PathBuf, String, bool, Option<String>)`, and update `capture` and `matches`:

```rust
                    worktrees: p
                        .worktrees
                        .iter()
                        .map(|wt| {
                            (wt.path.clone(), wt.name.clone(), wt.prunable, wt.branch.clone())
                        })
                        .collect(),
```

```rust
        if self.session_rows_always != ui.session_rows_always
            || self.query != ui.query
            || self.toggles != ui.toggles
            || self.toggles_apply != ui.toggles_apply
            || self.pr_generation != ui.pr_generation
            || self.active_workspace.as_deref() != ui.active_workspace
            || self.active_branch.as_deref() != ui.active_branch
        {
            return false;
        }
```

and in the worktree loop:

```rust
                if wt_was.0 != wt_now.path
                    || wt_was.1 != wt_now.name
                    || wt_was.2 != wt_now.prunable
                    || wt_was.3 != wt_now.branch
                {
                    return false;
                }
```

Add `use std::path::Path;` if the module does not already import it.

- [ ] **Step 4: Feed the new fields at both app call sites**

`sidebar_snapshot` (`alacritree/src/app.rs:1792`) and `reconcile_sidebar_focus` (`:1834`) build the same literal. Both get the same five values. For now, pass constants that reproduce today's behavior — later tasks replace them:

```rust
            sidebar_focus::UiInputs {
                session_rows_always: self.session_rows_always,
                query: self.project_filter.query(),
                toggles: self.project_filter.toggle_bits(),
                toggles_apply: true,
                pr_generation: 0,
                active_workspace: self.current_workspace.as_deref(),
                active_branch: self
                    .current_workspace
                    .as_deref()
                    .and_then(|p| self.git_status.get(p))
                    .and_then(|c| c.current_branch()),
            }
```

`toggles_apply` and `pr_generation` become real in Tasks 7 and 10. `active_workspace`/`active_branch` are correct as written and need no revisit.

If the borrow checker rejects reading `self.git_status` alongside the existing `self.project_filter.query()` borrow, hoist both into locals before the literal — they are all immutable reads.

- [ ] **Step 5: Update the steady-state literals**

`alacritree/src/steady_state.rs` builds `UiInputs` at `:114` and `:132` (and possibly elsewhere — `rg -n 'UiInputs' alacritree/src/steady_state.rs`). Add the five fields to each, using `toggles_apply: true, pr_generation: 0, active_workspace: None, active_branch: None`.

- [ ] **Step 6: Run to verify everything passes**

Run: `cargo test -p alacritree --lib sidebar_focus`
Expected: PASS.

Run: `cargo test -p alacritree allocates_nothing`
Expected: PASS — the added scalars and the `Option<String>` per worktree cost allocations only on capture, never on the compare path.

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/sidebar_focus.rs alacritree/src/app.rs alacritree/src/steady_state.rs
git commit -m "refactor(sidebar): observe branch and scope in the reconciler"
```

---

## Task 7: Search scope

**Files:**
- Modify: `alacritree/src/config.rs` (`SearchScope`, `RawUi`, `UiTheme`), `alacritree/src/panel_filter.rs` (`toggles_apply`), `alacritree/src/app.rs:1719-1769` (`current_project_rows`), `:2094-2113` (`filtered_git_rows`), `:4934-4944` (`panel_header_filter_ui`), `dispatch_action`, both `UiInputs` sites
- Test: `alacritree/src/config.rs`, `alacritree/src/panel_filter.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: Task 6's `UiInputs.toggles_apply`.
- Produces: `config::SearchScope { Filtered, All }`; `PanelFilter::toggles_apply(&self, scope: SearchScope) -> bool`; `AlacritreeApp.search_scope: SearchScope`.

- [ ] **Step 1: Write the failing config and helper tests**

In `alacritree/src/config.rs` `mod tests`, following `sidebar_focus_defaults_to_preserve`:

```rust
    #[test]
    fn search_scope_defaults_to_filtered() {
        let ui = ui_from_toml("");
        assert_eq!(ui.search_scope, SearchScope::Filtered);
    }

    #[test]
    fn search_scope_parses_both_values() {
        for (raw, expected) in [("filtered", SearchScope::Filtered), ("all", SearchScope::All)] {
            let ui = ui_from_toml(&format!("[ui]\nsearch_scope = \"{raw}\""));
            assert_eq!(ui.search_scope, expected, "value {raw:?}");
        }
    }

    #[test]
    fn search_scope_invalid_falls_back_to_filtered() {
        let ui = ui_from_toml("[ui]\nsearch_scope = \"everywhere\"");
        assert_eq!(ui.search_scope, SearchScope::Filtered);
    }
```

In `alacritree/src/panel_filter.rs` `mod tests`:

```rust
    #[test]
    fn toggles_apply_only_stands_down_for_a_live_query_under_all() {
        use crate::config::SearchScope;
        let mut f = PanelFilter::new(TOGGLES);

        assert!(f.toggles_apply(SearchScope::Filtered));
        assert!(f.toggles_apply(SearchScope::All), "an empty query narrows nothing");

        f.on_text("/");
        f.on_text("foo");
        assert!(f.toggles_apply(SearchScope::Filtered));
        assert!(!f.toggles_apply(SearchScope::All));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree search_scope toggles_apply`
Expected: FAIL — `SearchScope` does not exist.

- [ ] **Step 3: Add the config type**

In `alacritree/src/config.rs`, beside `SidebarFocus`:

```rust
/// `[ui] search_scope`: whether a fuzzy query is confined by the panel's active
/// toggle filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    /// A query narrows the rows the toggles already allow.
    #[default]
    Filtered,
    /// A query is evaluated against every row; the toggles stand aside until it
    /// empties.
    All,
}

fn parse_search_scope(raw: Option<&str>) -> SearchScope {
    match raw {
        None => SearchScope::default(),
        Some("filtered") => SearchScope::Filtered,
        Some("all") => SearchScope::All,
        Some(other) => {
            log::warn!("unknown ui.search_scope value {other:?}, using \"filtered\"");
            SearchScope::default()
        },
    }
}
```

Add `pub search_scope: SearchScope` to `UiTheme` (beside `sidebar_focus` at `:586`) with a doc comment, `SearchScope::default()` in the `Default` impl (`:635`), `search_scope: Option<String>` to `RawUi` (`:1297`), and `search_scope: parse_search_scope(self.ui.search_scope.as_deref()),` in the builder (`:1465`).

- [ ] **Step 4: Add the helper and run**

In `alacritree/src/panel_filter.rs`:

```rust
    /// Whether the toggle filters apply this frame.  Under `All` a live query
    /// stands them down, so a search reaches rows the toggles hide.
    pub fn toggles_apply(&self, scope: crate::config::SearchScope) -> bool {
        scope == crate::config::SearchScope::Filtered || self.query.is_empty()
    }
```

Run: `cargo test -p alacritree search_scope toggles_apply`
Expected: PASS.

- [ ] **Step 5: Write the failing row-projection tests**

The helper being right does not prove either panel consults it. In `alacritree/src/app.rs` `mod tests`, add tests that drive `current_project_rows` and `filtered_git_rows` — or, if constructing an `AlacritreeApp` is not possible (it is not; `Session` owns a real PTY), extract the two predicates into free functions first and test those:

```rust
/// Whether a workspace survives the projects panel's toggle dimension.
fn project_toggles_pass(
    apply: bool,
    toggle_sessions: bool,
    has_sessions: bool,
    toggle_attention: bool,
    needs_attention: bool,
) -> bool {
    if !apply {
        return true;
    }
    (!toggle_sessions || has_sessions) && (!toggle_attention || needs_attention)
}
```

```rust
    #[test]
    fn a_wide_search_stands_down_the_project_toggles() {
        // Toggled on, workspace fails both: excluded while the toggles apply,
        // included once a wide search stands them down.
        assert!(!project_toggles_pass(true, true, false, true, false));
        assert!(project_toggles_pass(false, true, false, true, false));
    }
```

- [ ] **Step 6: Wire the scope into both projections**

In `alacritree/src/app.rs`, add `search_scope: SearchScope` to `AlacritreeApp`, initialized from `config.ui.search_scope` beside the other config-seeded fields at `:649`.

In `current_project_rows` (`:1725`):

```rust
        let apply = self.project_filter.toggles_apply(self.search_scope);
        let toggle_sessions = apply && self.project_filter.is_toggled('s');
        let toggle_attention = apply && self.project_filter.is_toggled('a');
        let any_toggle = toggle_sessions || toggle_attention;
```

Forcing the two booleans to `false` rather than only short-circuiting `toggles_pass` is required: `project_self` is `!any_toggle && project_matches...` (`:1755`), so a still-set `any_toggle` would stop project headers from matching their own name during a wide search.

In `filtered_git_rows` (`:2095`):

```rust
        let apply = self.git_filter.toggles_apply(self.search_scope);
        let m = apply && self.git_filter.is_toggled('m');
        let d = apply && self.git_filter.is_toggled('d');
        let u = apply && self.git_filter.is_toggled('u');
```

- [ ] **Step 7: Feed `toggles_apply` to the reconciler and add the action**

At both `UiInputs` sites in `alacritree/src/app.rs`, replace the `toggles_apply: true` placeholder from Task 6 with:

```rust
                toggles_apply: self.project_filter.toggles_apply(self.search_scope),
```

Replace the `ToggleSearchScope` placeholder arm in `dispatch_action`:

```rust
            BindingAction::Named(NamedAction::ToggleSearchScope) => {
                self.search_scope = match self.search_scope {
                    SearchScope::Filtered => SearchScope::All,
                    SearchScope::All => SearchScope::Filtered,
                };
            },
```

- [ ] **Step 8: Dim the chips while the toggles are stood down**

In `panel_header_filter_ui` (`alacritree/src/app.rs:4934`), add a `toggles_apply: bool` parameter and use it for the chip color, updating both call sites:

```rust
    let chip = if toggles_apply { theme.accent } else { theme.text_muted };
    for key in filter.active_toggles() {
        ui.label(RichText::new(format!("[{key}]")).color(chip).monospace().small());
    }
```

- [ ] **Step 9: Verify**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 10: Manual smoke check**

Add `search_scope = "all"` under `[ui]` in a scratch config, run the app, toggle `s` in the projects sidebar so rows disappear, then `/` and type part of a hidden workspace's name. It appears, and its `[s]` chip is dimmed. Clear the query and it vanishes again. Repeat with `"filtered"` and confirm the hidden row stays hidden.

- [ ] **Step 11: Commit**

```bash
git add alacritree/src/config.rs alacritree/src/panel_filter.rs alacritree/src/app.rs
git commit -m "feat(sidebar): add a configurable search scope"
```

---

## Task 8: PR filter predicates

**Files:**
- Modify: `alacritree/src/pr_status.rs`
- Test: `alacritree/src/pr_status.rs` `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `pr_status::pr_pass(state: Option<PrState>, open: bool, draft: bool, merged: bool, closed: bool) -> bool`; `pr_status::effective_branch<'a>(wt: &'a Worktree, current_workspace: Option<&Path>, live_branch: Option<&'a str>) -> Option<&'a str>`; `PrCache::state(&self, path: &Path, branch: Option<&str>) -> Option<PrState>`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn no_active_pr_toggle_passes_every_state() {
        for state in
            [None, Some(PrState::Open), Some(PrState::Draft), Some(PrState::Merged), Some(PrState::Closed)]
        {
            assert!(pr_pass(state, false, false, false, false), "{state:?}");
        }
    }

    #[test]
    fn an_active_pr_toggle_admits_only_its_own_state() {
        assert!(pr_pass(Some(PrState::Open), true, false, false, false));
        assert!(!pr_pass(Some(PrState::Draft), true, false, false, false));
        assert!(!pr_pass(Some(PrState::Merged), true, false, false, false));
    }

    #[test]
    fn pr_toggles_union_within_the_dimension() {
        for state in [PrState::Open, PrState::Draft] {
            assert!(pr_pass(Some(state), true, true, false, false), "{state:?}");
        }
        assert!(!pr_pass(Some(PrState::Closed), true, true, false, false));
    }

    /// No lookup yet, no PR, or no `gh` are indistinguishable here, and none of
    /// them is evidence a worktree belongs in a PR-filtered list.
    #[test]
    fn an_unknown_state_never_satisfies_an_active_toggle() {
        assert!(!pr_pass(None, true, false, false, false));
        assert!(!pr_pass(None, true, true, true, true));
    }
```

For `effective_branch`, build a `Worktree` with `path: "/repo/wt"` and `branch: Some("stored")`:

```rust
    #[test]
    fn effective_branch_prefers_the_live_branch_for_the_active_worktree() {
        let wt = worktree("/repo/wt", Some("stored"));
        let active = Some(Path::new("/repo/wt"));
        assert_eq!(effective_branch(&wt, active, Some("live")), Some("live"));
    }

    /// A workspace that just became active has a fresh `StatusCache` with no
    /// branch yet; falling back to the stored one is what stops a valid cached
    /// lookup from reading as unknown for a frame.
    #[test]
    fn effective_branch_falls_back_to_the_stored_branch() {
        let wt = worktree("/repo/wt", Some("stored"));
        let active = Some(Path::new("/repo/wt"));
        assert_eq!(effective_branch(&wt, active, None), Some("stored"));
    }

    #[test]
    fn effective_branch_ignores_a_live_branch_from_another_workspace() {
        let wt = worktree("/repo/wt", Some("stored"));
        let active = Some(Path::new("/repo/other"));
        assert_eq!(effective_branch(&wt, active, Some("live")), Some("stored"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree --lib pr_status`
Expected: FAIL — the functions do not exist.

- [ ] **Step 3: Implement**

In `alacritree/src/pr_status.rs`:

```rust
/// Whether a worktree in `state` survives the projects panel's PR dimension.
/// The active states union; with none active every worktree passes.  An unknown
/// state — no lookup yet, no PR, or no `gh` — satisfies no active toggle.
pub fn pr_pass(
    state: Option<PrState>,
    open: bool,
    draft: bool,
    merged: bool,
    closed: bool,
) -> bool {
    if !(open || draft || merged || closed) {
        return true;
    }
    match state {
        None => false,
        Some(PrState::Open) => open,
        Some(PrState::Draft) => draft,
        Some(PrState::Merged) => merged,
        Some(PrState::Closed) => closed,
    }
}

/// The branch a worktree's PR lookup is keyed to.  The active worktree prefers
/// its live status branch; every other worktree, and an active one whose
/// `StatusCache` has not produced a branch yet, uses the stored snapshot.
pub fn effective_branch<'a>(
    wt: &'a Worktree,
    current_workspace: Option<&Path>,
    live_branch: Option<&'a str>,
) -> Option<&'a str> {
    if current_workspace == Some(wt.path.as_path()) {
        live_branch.or(wt.branch.as_deref())
    } else {
        wt.branch.as_deref()
    }
}
```

Add `use crate::projects::Worktree;`.

And on `impl PrCache`:

```rust
    /// The state of a cached lookup, without starting or refreshing one.
    /// `None` unless the entry was queried for `branch`: an entry is keyed by
    /// path but only ever valid for one branch, so a caller reading it under a
    /// different branch would be reading the previous branch's PR.
    pub fn state(&self, path: &Path, branch: Option<&str>) -> Option<PrState> {
        let entry = self.entries.get(path)?;
        if entry.branch.as_deref() != branch {
            return None;
        }
        entry.info.as_ref().map(|i| i.state)
    }
```

- [ ] **Step 4: Add the `state` branch test and run**

```rust
    #[test]
    fn state_is_none_for_a_branch_the_entry_was_not_queried_for() {
        let mut cache = PrCache::new();
        cache.entries.insert(
            PathBuf::from("/repo/wt"),
            Entry {
                branch: Some("main".into()),
                info: Some(PrInfo {
                    number: 1,
                    base_branch: "master".into(),
                    url: String::new(),
                    state: PrState::Open,
                }),
                queried_at: None,
                pending: None,
            },
        );

        let p = Path::new("/repo/wt");
        assert_eq!(cache.state(p, Some("main")), Some(PrState::Open));
        assert_eq!(cache.state(p, Some("feature")), None);
        assert_eq!(cache.state(p, None), None);
    }
```

Run: `cargo test -p alacritree --lib pr_status`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/pr_status.rs
git commit -m "feat(pr): add PR filter and branch predicates"
```

---

## Task 9: PR cache mechanics

**Files:**
- Modify: `alacritree/src/pr_status.rs` (`Entry`, `PrCache`, `poll`, `spawn_lookup`), `alacritree/src/config.rs` (`pr_status_concurrency`)
- Test: `alacritree/src/pr_status.rs` `mod tests`, `alacritree/src/config.rs` `mod tests`

**Interfaces:**
- Consumes: Task 8's `PrCache::state`.
- Produces: `PrCache::drain_completed(&mut self)`, `PrCache::generation(&self) -> u64`, `PrCache::invalidate_all(&mut self)`, `PrCache::set_concurrency(&mut self, cap: usize)`; `Entry.refresh_requested: bool`; `[ui] pr_status_concurrency`.

- [ ] **Step 1: Add the config key with tests**

Mirroring Task 7 step 3: `pr_status_concurrency: Option<usize>` on `RawUi`, `pub pr_status_concurrency: usize` on `UiTheme`, `self.ui.pr_status_concurrency.unwrap_or(0)` in the builder, `0` in `Default`. Test:

```rust
    #[test]
    fn pr_status_concurrency_defaults_to_unlimited() {
        assert_eq!(ui_from_toml("").pr_status_concurrency, 0);
        assert_eq!(ui_from_toml("[ui]\npr_status_concurrency = 4").pr_status_concurrency, 4);
    }
```

`0` is required: `poll` already spawns one thread per path and the paint loop polls every eligible worktree in one frame, so any positive default would change how fast badges populate on an existing install.

- [ ] **Step 2: Write the failing cache tests**

```rust
    /// A collapsed project stops polling its entry, so a decrement that lived
    /// in `poll` would strand the slot forever.
    #[test]
    fn drain_completed_frees_a_slot_for_an_entry_nobody_polls() {
        let mut cache = PrCache::new();
        cache.set_concurrency(1);
        let (tx, rx) = mpsc::channel();
        cache.insert_pending(PathBuf::from("/repo/wt"), rx);
        assert_eq!(cache.in_flight(), 1);

        tx.send(LookupResult { branch: "main".into(), info: None }).unwrap();
        cache.drain_completed();

        assert_eq!(cache.in_flight(), 0);
    }

    /// A worker that panics never sends. Without a decrement here a capped
    /// cache stops polling permanently.
    #[test]
    fn drain_completed_frees_a_slot_for_a_disconnected_worker() {
        let mut cache = PrCache::new();
        cache.set_concurrency(1);
        let (tx, rx) = mpsc::channel::<LookupResult>();
        cache.insert_pending(PathBuf::from("/repo/wt"), rx);
        drop(tx);

        cache.drain_completed();

        assert_eq!(cache.in_flight(), 0);
    }

    #[test]
    fn generation_advances_on_a_banked_result_and_holds_still_otherwise() {
        let mut cache = PrCache::new();
        let before = cache.generation();
        cache.drain_completed();
        assert_eq!(cache.generation(), before, "a frame that banks nothing must not invalidate");

        let (tx, rx) = mpsc::channel();
        cache.insert_pending(PathBuf::from("/repo/wt"), rx);
        tx.send(LookupResult { branch: "main".into(), info: None }).unwrap();
        cache.drain_completed();
        assert!(cache.generation() > before);
    }

    /// A refresh that lands while a lookup is in flight must survive it: `poll`
    /// only spawns when `pending` is empty, and the drain would otherwise stamp
    /// a fresh `queried_at` and swallow the request.
    #[test]
    fn a_refresh_during_a_lookup_survives_the_drain() {
        let mut cache = PrCache::new();
        let (tx, rx) = mpsc::channel();
        cache.insert_pending(PathBuf::from("/repo/wt"), rx);

        cache.invalidate_all();

        tx.send(LookupResult { branch: "main".into(), info: None }).unwrap();
        cache.drain_completed();

        let entry = cache.entries.get(Path::new("/repo/wt")).unwrap();
        assert!(entry.queried_at.is_none(), "the next poll must re-query");
        assert!(!entry.refresh_requested, "and the request is spent, not sticky");
    }

    /// Setting the flag on idle entries too would double-poll every one of
    /// them: the drain banks nothing, `poll` starts the lookup, and the still-set
    /// flag then refuses to stamp `queried_at`, so a second lookup starts.
    #[test]
    fn a_refresh_on_an_idle_entry_does_not_set_the_flag() {
        let mut cache = PrCache::new();
        cache.entries.insert(
            PathBuf::from("/repo/wt"),
            Entry {
                branch: Some("main".into()),
                info: None,
                queried_at: Some(Instant::now()),
                pending: None,
                refresh_requested: false,
            },
        );

        cache.invalidate_all();

        let entry = cache.entries.get(Path::new("/repo/wt")).unwrap();
        assert!(entry.queried_at.is_none());
        assert!(!entry.refresh_requested);
    }
```

`insert_pending` and `in_flight` are `#[cfg(test)]` helpers on `PrCache` — add them alongside the implementation.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p alacritree --lib pr_status`
Expected: FAIL — `drain_completed` does not exist.

- [ ] **Step 4: Implement**

Add `refresh_requested: bool` to `Entry` (and to every construction site, including `poll`'s `or_insert_with`). Add to `PrCache`:

```rust
#[derive(Default)]
pub struct PrCache {
    entries: HashMap<PathBuf, Entry>,
    in_flight: usize,
    concurrency: usize,
    generation: u64,
}
```

```rust
    /// Advances whenever what `state` would answer may have moved.  The sidebar
    /// reconciler compares it to know a filtered row set needs rebuilding; a
    /// banked result that happens to match the previous one costs one extra
    /// rebuild, which is cheaper than diffing states to avoid it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// `0` means unlimited, which is how many `gh` processes a cold cache
    /// already forks: one per eligible worktree, all in one frame.
    pub fn set_concurrency(&mut self, cap: usize) {
        self.concurrency = cap;
    }

    /// Bank every finished lookup and free its slot.  Runs once a frame ahead
    /// of every poll site rather than inside `poll`: an entry whose project
    /// collapsed mid-lookup is never polled again, and a slot it still held
    /// would never come back.
    pub fn drain_completed(&mut self) {
        for entry in self.entries.values_mut() {
            let Some(rx) = entry.pending.as_ref() else {
                continue;
            };
            match rx.try_recv() {
                Ok(result) => {
                    entry.branch = Some(result.branch);
                    entry.info = result.info;
                    // A refresh that arrived mid-lookup wants the *next* answer,
                    // so leave the entry stale and let the next poll re-query.
                    entry.queried_at =
                        if entry.refresh_requested { None } else { Some(Instant::now()) };
                    entry.refresh_requested = false;
                    entry.pending = None;
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.generation = self.generation.wrapping_add(1);
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    entry.pending = None;
                    entry.refresh_requested = false;
                    self.in_flight = self.in_flight.saturating_sub(1);
                },
                Err(mpsc::TryRecvError::Empty) => {},
            }
        }
    }

    /// Mark every entry stale.  Entries with a lookup already running also get
    /// `refresh_requested`, because clearing `queried_at` alone cannot reach
    /// them: `poll` will not spawn while `pending` is occupied, and the drain
    /// would stamp a fresh timestamp over the request.
    pub fn invalidate_all(&mut self) {
        for entry in self.entries.values_mut() {
            entry.queried_at = None;
            if entry.pending.is_some() {
                entry.refresh_requested = true;
            }
        }
        self.generation = self.generation.wrapping_add(1);
    }
```

In `poll`, delete the inline drain (`pr_status.rs:88-96`) and gate the spawn:

```rust
        if (invalidate || !fresh)
            && entry.pending.is_none()
            && (self.concurrency == 0 || self.in_flight < self.concurrency)
        {
            if invalidate {
                entry.info = None;
            }
            entry.pending = Some(spawn_lookup(path.to_path_buf(), branch.to_string(), ctx.clone()));
            self.in_flight += 1;
        }
```

The borrow of `entry` must end before `self.in_flight` is read; restructure with an early scope or compute the cap check before taking the entry.

In `spawn_lookup`, move the repaint into a guard so a panicking worker still wakes the app — without it, a full cap on an idle app never runs another frame and never observes the disconnect:

```rust
fn spawn_lookup(path: PathBuf, branch: String, ctx: egui::Context) -> Receiver<LookupResult> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // Fires on a panicking unwind too: the drain that frees this lookup's
        // concurrency slot only runs on a frame, so an exit without a repaint
        // can stall polling for good.
        let _wake = RepaintOnDrop(ctx);
        let info = query_gh(&path, &branch);
        let _ = tx.send(LookupResult { branch, info });
    });
    rx
}

struct RepaintOnDrop(egui::Context);

impl Drop for RepaintOnDrop {
    fn drop(&mut self) {
        self.0.request_repaint();
    }
}
```

- [ ] **Step 5: Run**

Run: `cargo test -p alacritree --lib pr_status && cargo test -p alacritree pr_status_concurrency`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/pr_status.rs alacritree/src/config.rs
git commit -m "feat(pr): cap and drain PR lookups centrally"
```

---

## Task 10: Wire the PR filters into the sidebar

**Files:**
- Modify: `alacritree/src/app.rs:649` (filter construction), `:1719-1769` (`current_project_rows`), `:2887-2925` (the poll loop), `dispatch_action`, both `UiInputs` sites, `update` (drain site)
- Test: `alacritree/src/app.rs` `mod tests`

**Interfaces:**
- Consumes: Tasks 8 and 9 in full; Task 6's `pr_generation`.
- Produces: the finished feature.

- [ ] **Step 1: Select the toggle identities by config**

In `alacritree/src/app.rs:649`:

```rust
            project_filter: PanelFilter::new(if config.ui.pr_status {
                &['s', 'a', 'o', 'd', 'm', 'c']
            } else {
                // Without polling every PR state reads as unknown, so offering
                // the filters would only ever empty the panel.
                &['s', 'a']
            }),
```

Config is read once at startup and has no reload path, so the choice never needs revisiting.

Call `self.pr_cache.set_concurrency(config.ui.pr_status_concurrency)` where the cache is built (`:667`).

- [ ] **Step 2: Drain once per frame**

In `update` in `alacritree/src/app.rs`, before the sidebars are painted and before the first `reconcile_sidebar_focus`, add:

```rust
        self.pr_cache.drain_completed();
```

It must be unconditional. There are two poll sites — the projects sidebar (`:2895`) and the git sidebar (`:3530`) — and either sidebar can be hidden, so hanging the drain off a panel would strand entries whenever that panel is not drawn.

- [ ] **Step 3: Apply the PR dimension to the rows**

In `current_project_rows`, after the existing toggle reads:

```rust
        let pr_open = apply && self.project_filter.is_toggled('o');
        let pr_draft = apply && self.project_filter.is_toggled('d');
        let pr_merged = apply && self.project_filter.is_toggled('m');
        let pr_closed = apply && self.project_filter.is_toggled('c');
        let any_pr = pr_open || pr_draft || pr_merged || pr_closed;
```

Fold `any_pr` into `any_toggle` so `project_self` keeps behaving as it does with any toggle set. Precompute the per-worktree verdicts before building the closures, the same way `worktree_matches` already releases its borrow:

```rust
        let live_branch = self
            .current_workspace
            .as_deref()
            .and_then(|p| self.git_status.get(p))
            .and_then(|c| c.current_branch());
        let current_workspace = self.current_workspace.as_deref();
        let pr_matches: HashMap<PathBuf, bool> = self
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .map(|wt| {
                let branch = pr_status::effective_branch(wt, current_workspace, live_branch);
                let state = self.pr_cache.state(&wt.path, branch);
                (wt.path.clone(), pr_status::pr_pass(state, pr_open, pr_draft, pr_merged, pr_closed))
            })
            .collect();
```

and extend the `worktree` predicate with `&& pr_matches.get(&wt.path).copied().unwrap_or(false)`.

- [ ] **Step 4: Widen and deduplicate the poll**

In the loop at `alacritree/src/app.rs:2892`, poll a worktree when `pr_enabled && (project.expanded || any_pr_toggle_active)`, and skip a path already polled this frame:

```rust
        let mut polled: HashSet<&Path> = HashSet::new();
```

with the poll guarded by `polled.insert(wt.path.as_path())`. Reuse the first occurrence's `Option<PrInfo>` for any later row on the same path.

`PrCache` is keyed by path alone and clears `info` on a branch mismatch (`pr_status.rs:106`), so it cannot hold two branches for one path. The same path can appear as a worktree of two projects — add a repo and one of its own worktrees as separate sidebar projects and it does — and two callers alternately invalidating each other would burn a `gh` process per frame.

Use `pr_status::effective_branch` here too, replacing the inline `is_active`/`branch` computation at `:2909-2917`, so the poller and the reader cannot drift.

- [ ] **Step 5: Feed `pr_generation`**

At both `UiInputs` sites, replace the `pr_generation: 0` placeholder:

```rust
                pr_generation: if any_pr_toggle_active { self.pr_cache.generation() } else { 0 },
```

Compute `any_pr_toggle_active` from the four identities. Feeding it unconditionally would invalidate the reconciler on every banked result for every user — roughly thirty full row rebuilds on a cold cache with `pr_status = true` and a cap of `1`, for someone who never touches a PR filter. Flipping a PR toggle changes `toggles`, which forces the first rebuild on its own.

- [ ] **Step 6: Add the refresh action**

Replace the `RefreshPrStatus` placeholder arm in `dispatch_action`:

```rust
            BindingAction::Named(NamedAction::RefreshPrStatus) => {
                self.pr_cache.invalidate_all();
                // The poll sites run while the sidebars paint, and the palette
                // dispatches after both have; without a wake the re-query would
                // wait for whatever repaint happened to come next.
                ctx.request_repaint();
            },
```

- [ ] **Step 7: Verify**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 8: Manual smoke check**

This needs a repo with worktrees that have real PRs and a working `gh auth status`. Use the isolated `APPDATA` lab rather than your live config.

Set `pr_status = true` and `pr_status_concurrency = 2` under `[ui]`, bind `TogglePrOpenFilter` to a key, and run. Press it: worktrees narrow to those with an open PR, rows appearing as lookups land. Collapse a project and confirm its worktrees still participate. Toggle off, then run `RefreshPrStatus` from the palette and confirm badges re-query without needing a keystroke.

- [ ] **Step 9: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "feat(sidebar): filter worktrees by PR state"
```

---

## Task 11: Documentation

**Files:**
- Modify: `docs/alacritree.md`

- [ ] **Step 1: Document the two config keys**

In the `[ui]` block, with the same comment style the neighbouring keys use:

```toml
# whether a sidebar search is confined by the active toggle filters
# "filtered" (default): a query narrows what the toggles already allow
# "all": a query reaches every row; the toggles resume when it empties
search_scope = "filtered"

# cap concurrent `gh` PR lookups; 0 (default) is unlimited
pr_status_concurrency = 0
```

- [ ] **Step 2: Document the thirteen actions**

Add a table listing each action, its panel, and its default key — `ToggleSessionsFilter` (`s`), `ToggleAttentionFilter` (`a`), the four PR filters (no default), `ClearProjectFilters` (no default), `ToggleModifiedFilter` (`m`), `ToggleDeletedFilter` (`d`), `ToggleUntrackedFilter` (`u`), `ClearGitFilters` (no default), `ToggleSearchScope` (no default), `RefreshPrStatus` (no default).

State that the PR filters exist only when `pr_status = true`, and that filtered rows fill in as `gh` answers arrive rather than appearing at once — and that a PR filter activated while the left sidebar is hidden starts no lookups until it is shown.

- [ ] **Step 3: Document the collision rule**

This is the one whose failure mode is silent, so it needs prose rather than a table row:

```markdown
A binding you write replaces the built-in binding on the same key. If you bind
a key that a filter toggle uses by default, that toggle loses its key — silently.
Two of your own bindings on one key both stay, and each runs only in the panel it
belongs to, so you can give a key back its old meaning alongside the new one:

    [[keyboard.bindings]]
    key = "D"
    action = "DeleteSelected"        # projects sidebar

    [[keyboard.bindings]]
    key = "D"
    action = "ToggleDeletedFilter"   # git sidebar
```

- [ ] **Step 4: Commit**

```bash
git add docs/alacritree.md
git commit -m "docs: document filter actions and search scope"
```

---

## Self-review notes

Checked against the spec section by section. Every spec requirement maps to a task: feature 0 → Tasks 1-2, feature 1 → Tasks 3-5, feature 2 → Task 7, feature 3 → Tasks 8-10, docs → Task 11, and the reconciler plumbing feature 2 and feature 3 share → Task 6.

Two places where the plan is deliberately less prescriptive than the rest, both because the exact shape depends on code the implementer must read first:

- **Task 5 step 5, the `valid_for_focus` rewrite.** The surrounding closure at `alacritree/src/app.rs:1579-1591` also computes `terminal_only` and combines it with `scratchpad_focused`. The replacement shown covers only the focus half; keep the `terminal_only` clause exactly as it is.
- **Task 7 step 5, the row-projection tests.** `current_project_rows` is a method on `AlacritreeApp`, which cannot be constructed in tests, so the predicate must be extracted to a free function to be testable at all. The extraction is the deliverable, not a workaround — if it turns out `project_toggles_pass` needs a different shape to fit the call site, change the shape and keep the test.

Type consistency verified across tasks: `toggles_apply` takes `config::SearchScope` everywhere; `effective_branch` returns `Option<&'a str>` and is consumed by `PrCache::state(&self, path: &Path, branch: Option<&str>)`; `pr_pass` takes `Option<PrState>` first and four bools in `open, draft, merged, closed` order at both its definition and its one call site.
