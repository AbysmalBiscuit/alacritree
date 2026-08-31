# Sidebar Focus Preservation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the projects-sidebar cursor jumping to Home when its row is hidden by a filter or removed from the tree, and optionally let the terminal follow a delete-induced landing.

**Architecture:** One reconciler observes what changed between frames instead of every mutation site reporting what it did. A snapshot holds model membership (arena with parent links) plus the current projection; a pure `repair` compares the previous and current snapshots and returns the new cursor, anchor, and optional terminal follow in one value. Two paths act before any observer could see them and therefore defer to the reconciler.

**Tech Stack:** Rust 2024 (MSRV 1.85), egui/eframe, git2, nucleo-matcher. One new dev-dependency: `divan` (benchmarks only).

## Prerequisite: the WSL discovery fix

This plan assumes `Project::discover` reports whether its answer is authoritative. That fix is **a separate branch and a separate PR**, planned in `2026-07-23-wsl-discovery-authoritative.md`, and it is executed **first**.

The reconciler infers row removal from absence, so a transient `wsl.exe` failure that collapses a project's worktree list is indistinguishable from "every worktree was deleted" — a bogus slide, and under `"follow"` a bogus terminal navigation. Do not start Task 1 here until that branch is merged or at least present in this worktree.

## Global Constraints

- Only the `alacritree/` **crate** is edited for code. `alacritty*/` crates are vendored and read-only. Task 9 edits repo-root documentation, which the crate restriction does not cover.
- Branch from `feat/sidebar-search-actions`, not `master` — Task 6 depends on `sidebar_search_confirm`/`sidebar_search_cancel`, which exist only there (commit bca658e7).
- **`ui.sidebar_focus` has two values, `"preserve"` (default) and `"follow"`.** There is no mode that restores the pre-change cursor behavior, and no mode that disables the reconciler. This is a deliberate, recorded departure from the repo's "gate new UX behind a config option" rule — see the spec's *One config key, and the parity exception it takes*. Every consequence below follows from it.
- **The reconciler now runs for every user on every frame, with no off-switch.** That makes the steady-state cost an invariant, not a target: every frame where nothing changed is one linear `ObservedInputs` compare with zero heap allocation, and nothing else. On a rebuild frame the whole reconciler is `O(projects + worktrees + sessions)`. No `Vec::contains` as set membership inside a per-node loop, no `active_toggles()`, no per-frame `String`/`Vec` construction. If a step here seems to want a quadratic scan, it is the wrong step. Task 8 makes this a CI gate rather than a promise.
- Conventional Commits, imperative mood, subject ≤50 chars including the type prefix (72 is the hard limit).
- Comments explain *why*, never restate *what*. No PR/task references, no change-relative phrasing ("now we", "previously").
- `cargo fmt` is enforced. Run it before every commit.
- Config additions go under `[ui]` in `alacritree.toml` (alacritree-only), documented on the `RawUi` field.
- Do not commit anything under `docs/superpowers/`.

## File Structure

| File | Responsibility |
| --- | --- |
| `alacritree/src/config.rs` | Add `SidebarFocus` enum, `parse_sidebar_focus`, `ui.sidebar_focus`. |
| `alacritree/src/panel_filter.rs` | Add `toggle_bits` — an allocation-free view of the toggle set. |
| `alacritree/src/sidebar_focus.rs` | **New.** Snapshot arena, `ObservedInputs`, `repair` and its private helpers. All pure. |
| `alacritree/src/sidebar_nav.rs` | Unchanged behavior. Test helper widened for the parent-rule equivalence test. |
| `alacritree/src/app.rs` | Snapshot construction from live state, two reconciler call sites, two navigation deferrals, three marked writes, the projection cache paint reads. |
| `alacritree/src/main.rs` | Register the new module. |
| `alacritree/src/steady_state.rs` | **New, `#[cfg(test)]` only.** Counting allocator with a thread-local gate, plus the cost assertions and the on-demand timing harness. |

**`alacritree` is a binary-only crate — there is no `src/lib.rs`.** So `tests/` and `benches/` targets are not available: neither can `use alacritree::…` because there is nothing to link against. Giving the crate a library target would mean moving every `mod` declaration out of `main.rs`, which would conflict with each of the ~15 in-flight branches in this repo. Task 8 therefore puts the cost gate in a `#[cfg(test)]` module inside the crate, and takes the isolation problem head-on with a thread-local gate rather than by wishing for a separate test binary.

## Two type decisions that ripple through Tasks 2–6

Read these before starting Task 2; several later tasks only make sense with them.

**1. `Parent` is a three-way enum, not `Option<NodeId>`.** The arena must hold live sessions whose workspace has no sidebar row — `remove_project` (`app.rs:1221`) deliberately keeps such sessions alive. They exist only so the reconciler does not read them as deleted. With `Option<NodeId>` they would carry `None`, which is also how Home and project headers say "top level", making an orphan session a *sibling of Home* and a legal landing for a slide. The enum says the three things separately:

```rust
pub enum Parent { Root, Node(NodeId), Detached }
```

**2. Toggles are compared as a bitmask.** `PanelFilter::active_toggles` (`panel_filter.rs:70`) collects a `Vec<char>`, and the per-frame comparison must not allocate. `toggle_bits` returns the same information as a `u32` over `allowed_toggles` order.

---


### Task 1: Add the `ui.sidebar_focus` config key

**Files:**
- Modify: `alacritree/src/config.rs:276-297` (enum + parser, beside `LastSessionClose`), `:391-394` (`UiTheme` field), `:429-432` (default), `:1073-1076` (`RawUi` field), `:1215-1218` (resolution)
- Test: `alacritree/src/config.rs` (existing `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `config::SidebarFocus { Preserve, Follow }` with `Preserve` as `Default` and one method `follows() -> bool`; field `config.ui.sidebar_focus`.

The resolved struct is `UiTheme` (`config.rs:380`), not `Ui`; the field on it is reached as `config.ui.sidebar_focus`.

There is no `preserves()`. With two variants it would be `true` for both — a predicate that never discriminates, read at call sites as if it did. Cursor preservation is unconditional; `follows()` is the only real gate, and it appears exactly twice (the reconciler's follow dispatch in Task 6, `defers_close_navigation` in Task 7).

- [x] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/config.rs`, mirroring the `last_session_close` tests:

```rust
#[test]
fn sidebar_focus_defaults_to_preserve() {
    let ui = ui_from_toml("");
    assert_eq!(ui.sidebar_focus, SidebarFocus::Preserve);
}

#[test]
fn sidebar_focus_parses_all_values() {
    for (raw, expected) in
        [("preserve", SidebarFocus::Preserve), ("follow", SidebarFocus::Follow)]
    {
        let ui = ui_from_toml(&format!("[ui]\nsidebar_focus = \"{raw}\""));
        assert_eq!(ui.sidebar_focus, expected, "value {raw:?}");
    }
}

#[test]
fn sidebar_focus_invalid_falls_back_to_preserve() {
    let ui = ui_from_toml("[ui]\nsidebar_focus = \"sideways\"");
    assert_eq!(ui.sidebar_focus, SidebarFocus::Preserve);
}

#[test]
fn a_retired_sidebar_focus_value_still_parses_to_the_default() {
    // "reset" named the pre-reconciler behavior and was removed rather than
    // kept as a mode.  A config file carrying it must start, not refuse.
    let ui = ui_from_toml("[ui]\nsidebar_focus = \"reset\"");
    assert_eq!(ui.sidebar_focus, SidebarFocus::Preserve);
}

#[test]
fn only_follow_moves_the_terminal() {
    assert!(!SidebarFocus::Preserve.follows());
    assert!(SidebarFocus::Follow.follows());
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p alacritree config::tests::sidebar_focus -- --nocapture`
Expected: FAIL to compile — `cannot find type 'SidebarFocus'`.

- [x] **Step 3: Add the enum and parser**

In `alacritree/src/config.rs`, directly after `parse_last_session_close`:

```rust
/// How far the projects sidebar goes when the cursor's row stops being
/// rendered.  Both values keep the cursor; they differ only in whether the
/// terminal comes along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarFocus {
    /// A filtered-out cursor climbs to its nearest visible ancestor and is
    /// restored when the filter widens; a removed cursor slides to a sibling
    /// bounded by its parent.  The terminal stays where it is.
    #[default]
    Preserve,
    /// `Preserve`, and a removal landing that has a live session also moves
    /// the terminal to it.
    Follow,
}

impl SidebarFocus {
    pub fn follows(self) -> bool {
        matches!(self, Self::Follow)
    }
}

fn parse_sidebar_focus(raw: Option<&str>) -> SidebarFocus {
    match raw {
        None => SidebarFocus::default(),
        Some("preserve") => SidebarFocus::Preserve,
        Some("follow") => SidebarFocus::Follow,
        Some(other) => {
            log::warn!("unknown ui.sidebar_focus value {other:?}, using \"preserve\"");
            SidebarFocus::default()
        },
    }
}
```

- [x] **Step 4: Wire it through the three structs**

`UiTheme` field, beside `last_session_close` (around line 393):

```rust
    /// How the projects sidebar repairs a cursor whose row stopped rendering.
    pub sidebar_focus: SidebarFocus,
```

`UiTheme` default, beside `last_session_close: LastSessionClose::Respawn` (around line 431):

```rust
            sidebar_focus: SidebarFocus::default(),
```

`RawUi` field, beside `last_session_close` (around line 1075):

```rust
    /// How far the projects sidebar goes when the cursor's row stops being
    /// rendered: "preserve" (default) | "follow".
    sidebar_focus: Option<String>,
```

Resolution, beside the `last_session_close` line (around line 1217):

```rust
            sidebar_focus: parse_sidebar_focus(self.ui.sidebar_focus.as_deref()),
```

- [x] **Step 5: Run to verify they pass**

Run: `cargo fmt && cargo test -p alacritree config::tests -- sidebar_focus only_follow a_retired`
Expected: PASS, 5 tests.

- [x] **Step 6: Commit**

```bash
git add alacritree/src/config.rs
git commit -m "feat(config): add ui.sidebar_focus"
```

---

### Task 2: Snapshot types and change detection

**Files:**
- Create: `alacritree/src/sidebar_focus.rs`
- Modify: `alacritree/src/panel_filter.rs` (add `toggle_bits`)
- Modify: `alacritree/src/main.rs` (add `mod sidebar_focus;`)
- Test: inline `#[cfg(test)] mod tests` in both files

**Interfaces:**
- Consumes: `sidebar_nav::SidebarRow`, `session::SessionId`, `app::WorkspaceKey`, `projects::Project`.
- Produces: `NodeId`, `Parent { Root, Node(NodeId), Detached }`, `Node { row, parent }`, `TreeSnapshot { nodes, projected, inputs }` with `find`, `is_projected`, `row`, `children`, `is_descendant`; `SnapshotBuilder` with `push`/`finish`; `ObservedInputs` with `capture`, `matches`, `is_filtering`; `SessionInput`, `UiInputs`; `PanelFilter::toggle_bits`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/panel_filter.rs`:

```rust
#[test]
fn toggle_bits_report_the_set_without_allocating() {
    let mut f = PanelFilter::new(TOGGLES);
    assert_eq!(f.toggle_bits(), 0);

    f.on_text("s");
    assert_eq!(f.toggle_bits(), 0b01, "'s' is index 0 in TOGGLES");

    f.on_text("a");
    assert_eq!(f.toggle_bits(), 0b11);

    f.on_text("s");
    assert_eq!(f.toggle_bits(), 0b10, "'a' alone is index 1");
}
```

Create `alacritree/src/sidebar_focus.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn row_project(p: &str) -> SidebarRow {
        SidebarRow::Project(PathBuf::from(p))
    }

    fn row_worktree(p: &str) -> SidebarRow {
        SidebarRow::Worktree(PathBuf::from(p))
    }

    fn ui(query: &str, toggles: u32) -> UiInputs<'_> {
        UiInputs { session_rows_always: false, query, toggles }
    }

    /// home, project /a expanded with worktree /a/wt1 holding sessions 1 and 2.
    fn snapshot() -> TreeSnapshot {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        let a = b.push(row_project("/a"), Parent::Root, true);
        let wt1 = b.push(row_worktree("/a/wt1"), Parent::Node(a), true);
        b.push(SidebarRow::Session(1), Parent::Node(wt1), true);
        b.push(SidebarRow::Session(2), Parent::Node(wt1), true);
        b.finish(ObservedInputs::default())
    }

    #[test]
    fn find_matches_across_snapshots_by_stable_key() {
        let s = snapshot();
        let id = s.find(&row_worktree("/a/wt1")).expect("worktree is in the model");
        assert_eq!(*s.row(id), row_worktree("/a/wt1"));
        assert_eq!(s.find(&row_worktree("/a/gone")), None);
    }

    #[test]
    fn unprojected_nodes_stay_in_the_model() {
        let mut b = SnapshotBuilder::default();
        let a = b.push(row_project("/a"), Parent::Root, true);
        b.push(row_worktree("/a/wt1"), Parent::Node(a), false);
        let s = b.finish(ObservedInputs::default());

        let wt = s.find(&row_worktree("/a/wt1")).expect("collapsed worktrees stay in the model");
        assert!(!s.is_projected(wt), "a collapsed worktree is not navigable");
    }

    #[test]
    fn a_detached_node_is_in_the_model_but_is_nobodys_sibling() {
        let mut b = SnapshotBuilder::default();
        let home = b.push(SidebarRow::Home, Parent::Root, true);
        b.push(SidebarRow::Session(9), Parent::Detached, false);
        let s = b.finish(ObservedInputs::default());

        assert!(s.find(&SidebarRow::Session(9)).is_some(), "an orphan session is not deleted");
        assert!(
            !s.children(Parent::Root).contains(&s.find(&SidebarRow::Session(9)).unwrap()),
            "an orphan must never be a root sibling — it would be a legal slide landing"
        );
        assert_eq!(s.children(Parent::Root), vec![home]);
    }

    #[test]
    fn children_are_projected_only_and_in_render_order() {
        let s = snapshot();
        let wt1 = s.find(&row_worktree("/a/wt1")).unwrap();
        let kids = s.children(Parent::Node(wt1));
        assert_eq!(
            kids.iter().map(|&id| s.row(id).clone()).collect::<Vec<_>>(),
            vec![SidebarRow::Session(1), SidebarRow::Session(2)]
        );
    }

    #[test]
    fn is_filtering_tracks_the_query_and_the_toggle_bits() {
        assert!(!ObservedInputs::capture(&[], std::iter::empty(), ui("", 0)).is_filtering());
        assert!(ObservedInputs::capture(&[], std::iter::empty(), ui("x", 0)).is_filtering());
        assert!(ObservedInputs::capture(&[], std::iter::empty(), ui("", 0b10)).is_filtering());
    }

    #[test]
    fn every_observed_input_in_isolation_triggers_a_rebuild() {
        let session = |ws: &'static WorkspaceKey, id, attention| SessionInput {
            workspace: ws,
            id,
            attention,
        };
        static HOME: WorkspaceKey = None;

        let base = ObservedInputs::capture(&[], [session(&HOME, 1, false)].into_iter(), ui("", 0));

        assert!(base.matches(&[], [session(&HOME, 1, false)].into_iter(), ui("", 0)));

        // Each UI input on its own.
        assert!(!base.matches(&[], [session(&HOME, 1, false)].into_iter(), ui("x", 0)));
        assert!(!base.matches(&[], [session(&HOME, 1, false)].into_iter(), ui("", 0b01)));
        assert!(!base.matches(
            &[],
            [session(&HOME, 1, false)].into_iter(),
            UiInputs { session_rows_always: true, query: "", toggles: 0 },
        ));

        // Each session input on its own: attention, id, count.
        assert!(!base.matches(&[], [session(&HOME, 1, true)].into_iter(), ui("", 0)));
        assert!(!base.matches(&[], [session(&HOME, 2, false)].into_iter(), ui("", 0)));
        assert!(!base.matches(&[], std::iter::empty(), ui("", 0)));
        assert!(!base.matches(
            &[],
            [session(&HOME, 1, false), session(&HOME, 2, false)].into_iter(),
            ui("", 0),
        ));
    }

    #[test]
    fn project_shape_changes_trigger_a_rebuild() {
        use crate::sidebar_nav::tests::project;

        let a = vec![project("/a", true, &["/a/wt1", "/a/wt2"])];
        let base = ObservedInputs::capture(&a, std::iter::empty(), ui("", 0));
        assert!(base.matches(&a, std::iter::empty(), ui("", 0)));

        // Expansion, worktree set, worktree order, root, and count each count.
        assert!(!base.matches(
            &[project("/a", false, &["/a/wt1", "/a/wt2"])],
            std::iter::empty(),
            ui("", 0)
        ));
        assert!(!base.matches(
            &[project("/a", true, &["/a/wt1"])],
            std::iter::empty(),
            ui("", 0)
        ));
        assert!(!base.matches(
            &[project("/a", true, &["/a/wt2", "/a/wt1"])],
            std::iter::empty(),
            ui("", 0)
        ));
        assert!(!base.matches(
            &[project("/b", true, &["/a/wt1", "/a/wt2"])],
            std::iter::empty(),
            ui("", 0)
        ));
        assert!(!base.matches(&[], std::iter::empty(), ui("", 0)));
    }
}
```

The `project` helper comes from `sidebar_nav`'s test module; Task 5 widens its visibility, so do that part now: change `mod tests` on `sidebar_nav.rs:187` to `pub(crate) mod tests` and `pub(super) fn project` on `sidebar_nav.rs:195` to `pub(crate) fn project`.

- [ ] **Step 2: Run to verify it fails**

Add `mod sidebar_focus;` to `alacritree/src/main.rs` beside `mod sidebar_nav;`.

Run: `cargo test -p alacritree sidebar_focus`
Expected: FAIL to compile — `cannot find type 'TreeSnapshot'`.

Run: `cargo test -p alacritree panel_filter::tests::toggle_bits`
Expected: FAIL to compile — `no method named 'toggle_bits'`.

- [ ] **Step 3: Add `toggle_bits`**

In `alacritree/src/panel_filter.rs`, beside `active_toggles`:

```rust
    /// The active toggles as a bitmask over `allowed_toggles` order.  The
    /// focus reconciler compares this on every frame, where `active_toggles`'s
    /// `Vec` would put an allocation in the steady-state path.
    pub fn toggle_bits(&self) -> u32 {
        self.allowed_toggles
            .iter()
            .enumerate()
            .filter(|(_, key)| self.toggles.contains(key))
            .fold(0, |bits, (i, _)| bits | (1 << i))
    }
```

- [ ] **Step 4: Write the snapshot implementation**

Prepend to `alacritree/src/sidebar_focus.rs`:

```rust
//! Cursor repair for the projects sidebar, derived by observation.
//!
//! Every mutation site reporting what it removed proved unbounded — sessions
//! leave through four paths and worktrees through a background refresh — so
//! this module infers the repair from what changed instead.  The distinction
//! that matters is model versus projection: a row hidden by a filter or a
//! collapsed project still exists and the cursor climbs to an ancestor it can
//! return from, while a row gone from the model was deleted and the cursor
//! slides to a sibling.

use std::path::PathBuf;

use crate::app::WorkspaceKey;
use crate::projects::Project;
use crate::session::SessionId;
use crate::sidebar_nav::SidebarRow;

/// Index into a single snapshot's `nodes`.  Deliberately not stable across
/// snapshots: cross-snapshot matching goes through the row's own path/session
/// key, because the project list mutates under the cursor and an index would
/// silently retarget.
pub type NodeId = usize;

/// A node's place in the tree.  `Detached` exists because a live session whose
/// project was dropped keeps running: it must be in the model so it never
/// reads as deleted, while never being a sibling of anything and so never a
/// landing the cursor could slide onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    Root,
    Node(NodeId),
    Detached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub row: SidebarRow,
    pub parent: Parent,
}

/// Model membership plus the current projection.  `nodes` holds every
/// project, worktree, and live session regardless of expansion, listing
/// threshold, or filter; `projected` holds exactly the navigable rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeSnapshot {
    pub nodes: Vec<Node>,
    pub projected: Vec<NodeId>,
    pub inputs: ObservedInputs,
}

impl TreeSnapshot {
    pub fn find(&self, row: &SidebarRow) -> Option<NodeId> {
        self.nodes.iter().position(|n| n.row == *row)
    }

    pub fn is_projected(&self, id: NodeId) -> bool {
        self.projected.contains(&id)
    }

    pub fn row(&self, id: NodeId) -> &SidebarRow {
        &self.nodes[id].row
    }

    pub fn parent(&self, id: NodeId) -> Parent {
        self.nodes[id].parent
    }

    /// `parent`'s projected children in render order — the sibling group a
    /// slide chooses from.
    pub fn children(&self, parent: Parent) -> Vec<NodeId> {
        if parent == Parent::Detached {
            return Vec::new();
        }
        self.projected.iter().copied().filter(|&id| self.nodes[id].parent == parent).collect()
    }

    pub fn is_descendant(&self, id: NodeId, ancestor: NodeId) -> bool {
        let mut cur = self.nodes[id].parent;
        while let Parent::Node(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.nodes[p].parent;
        }
        false
    }
}

#[derive(Default)]
pub struct SnapshotBuilder {
    nodes: Vec<Node>,
    projected: Vec<NodeId>,
}

impl SnapshotBuilder {
    pub fn push(&mut self, row: SidebarRow, parent: Parent, projected: bool) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node { row, parent });
        if projected {
            self.projected.push(id);
        }
        id
    }

    pub fn finish(self, inputs: ObservedInputs) -> TreeSnapshot {
        TreeSnapshot { nodes: self.nodes, projected: self.projected, inputs }
    }
}

/// One live session, borrowed for the per-frame comparison.
#[derive(Debug, Clone, Copy)]
pub struct SessionInput<'a> {
    pub workspace: &'a WorkspaceKey,
    pub id: SessionId,
    pub attention: bool,
}

/// Sidebar UI inputs that change the projection without changing the model.
/// `toggles` is a bitmask rather than a slice so the comparison never
/// allocates.
#[derive(Debug, Clone, Copy)]
pub struct UiInputs<'a> {
    pub session_rows_always: bool,
    pub query: &'a str,
    pub toggles: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProjectInput {
    root: PathBuf,
    name: String,
    expanded: bool,
    worktrees: Vec<(PathBuf, String, bool)>,
}

/// Everything the snapshot is a function of.  Captured on rebuild, compared
/// borrowed on every other frame so the steady state allocates nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedInputs {
    projects: Vec<ProjectInput>,
    sessions: Vec<(WorkspaceKey, SessionId, bool)>,
    session_rows_always: bool,
    query: String,
    toggles: u32,
}

impl ObservedInputs {
    pub fn capture<'a>(
        projects: &[Project],
        sessions: impl Iterator<Item = SessionInput<'a>>,
        ui: UiInputs<'_>,
    ) -> Self {
        Self {
            projects: projects
                .iter()
                .map(|p| ProjectInput {
                    root: p.root.clone(),
                    name: p.display_name().to_string(),
                    expanded: p.expanded,
                    worktrees: p
                        .worktrees
                        .iter()
                        .map(|wt| (wt.path.clone(), wt.name.clone(), wt.prunable))
                        .collect(),
                })
                .collect(),
            sessions: sessions.map(|s| (s.workspace.clone(), s.id, s.attention)).collect(),
            session_rows_always: ui.session_rows_always,
            query: ui.query.to_string(),
            toggles: ui.toggles,
        }
    }

    /// Whether a filter is narrowing the tree.  The anchor exists only for the
    /// duration of one filter episode, so this is what ends it.
    pub fn is_filtering(&self) -> bool {
        !self.query.is_empty() || self.toggles != 0
    }

    /// Whether every observed input still holds.  Allocation-free: this runs
    /// on every frame the sidebar is live.
    pub fn matches<'a>(
        &self,
        projects: &[Project],
        sessions: impl Iterator<Item = SessionInput<'a>>,
        ui: UiInputs<'_>,
    ) -> bool {
        if self.session_rows_always != ui.session_rows_always
            || self.query != ui.query
            || self.toggles != ui.toggles
        {
            return false;
        }
        if self.projects.len() != projects.len() {
            return false;
        }
        for (was, now) in self.projects.iter().zip(projects) {
            if was.root != now.root
                || was.name != now.display_name()
                || was.expanded != now.expanded
                || was.worktrees.len() != now.worktrees.len()
            {
                return false;
            }
            for (wt_was, wt_now) in was.worktrees.iter().zip(&now.worktrees) {
                if wt_was.0 != wt_now.path || wt_was.1 != wt_now.name || wt_was.2 != wt_now.prunable
                {
                    return false;
                }
            }
        }
        let mut seen = 0usize;
        for s in sessions {
            match self.sessions.get(seen) {
                Some((ws, id, attention))
                    if ws == s.workspace && *id == s.id && *attention == s.attention =>
                {
                    seen += 1;
                },
                _ => return false,
            }
        }
        seen == self.sessions.len()
    }
}
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo fmt && cargo test -p alacritree sidebar_focus panel_filter`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/sidebar_focus.rs alacritree/src/panel_filter.rs alacritree/src/sidebar_nav.rs alacritree/src/main.rs
git commit -m "feat(sidebar): add a tree snapshot for focus"
```

---

### Task 3: The sibling slide

The slide resolves against the **next** tree, by parent key and child ordinal. Choosing one candidate from the previous projection and climbing when it fails is wrong whenever more than one row disappears at once — which is routine, since removing a worktree takes all its sessions with it — and it also picks the wrong row when a background refresh reorders worktrees.

**Files:**
- Modify: `alacritree/src/sidebar_focus.rs`
- Test: same file

**Interfaces:**
- Consumes: Task 2's `TreeSnapshot`, `SnapshotBuilder`, `Parent`.
- Produces: private `fn slide(prev: &TreeSnapshot, next: &TreeSnapshot, removed: NodeId) -> Option<SidebarRow>`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/sidebar_focus.rs`:

```rust
/// The reference tree from the design spec.
fn reference_tree() -> TreeSnapshot {
    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);

    let p1 = b.push(row_project("/p1"), Parent::Root, true);
    let p1wt1 = b.push(row_worktree("/p1/wt1"), Parent::Node(p1), true);
    b.push(SidebarRow::Session(11), Parent::Node(p1wt1), true);
    b.push(SidebarRow::Session(12), Parent::Node(p1wt1), true);
    let p1wt2 = b.push(row_worktree("/p1/wt2"), Parent::Node(p1), true);
    b.push(SidebarRow::Session(21), Parent::Node(p1wt2), true);
    b.push(SidebarRow::Session(22), Parent::Node(p1wt2), true);
    b.push(SidebarRow::Session(23), Parent::Node(p1wt2), true);

    let p2 = b.push(row_project("/p2"), Parent::Root, true);
    let p2wt1 = b.push(row_worktree("/p2/wt1"), Parent::Node(p2), true);
    b.push(SidebarRow::Session(31), Parent::Node(p2wt1), true);
    b.push(SidebarRow::Session(32), Parent::Node(p2wt1), true);
    b.push(row_worktree("/p2/wt2"), Parent::Node(p2), true);
    b.push(row_worktree("/p2/wt3"), Parent::Node(p2), true);

    b.finish(ObservedInputs::default())
}

/// The reference tree with every row in `drop` and their descendants absent
/// from the model — what a deletion leaves behind.
fn reference_tree_without(drop: &[SidebarRow]) -> TreeSnapshot {
    let full = reference_tree();
    let dropped: Vec<NodeId> =
        drop.iter().map(|r| full.find(r).expect("row is in the reference tree")).collect();
    let gone = |id: NodeId| {
        dropped.iter().any(|&d| d == id || full.is_descendant(id, d))
    };

    let mut b = SnapshotBuilder::default();
    let mut remap = std::collections::HashMap::new();
    for (old, node) in full.nodes.iter().enumerate() {
        if gone(old) {
            continue;
        }
        let parent = match node.parent {
            Parent::Node(p) => Parent::Node(remap[&p]),
            other => other,
        };
        let new = b.push(node.row.clone(), parent, full.is_projected(old));
        remap.insert(old, new);
    }
    b.finish(ObservedInputs::default())
}

fn slide_from(prev: &TreeSnapshot, next: &TreeSnapshot, row: &SidebarRow) -> Option<SidebarRow> {
    slide(prev, next, prev.find(row).expect("row is in the previous model"))
}

#[test]
fn a_last_child_slides_back_to_its_previous_sibling() {
    let prev = reference_tree();
    let next = reference_tree_without(&[SidebarRow::Session(12)]);
    assert_eq!(
        slide_from(&prev, &next, &SidebarRow::Session(12)),
        Some(SidebarRow::Session(11))
    );
}

#[test]
fn a_middle_child_slides_forward_into_the_vacated_slot() {
    let prev = reference_tree();
    let next = reference_tree_without(&[SidebarRow::Session(22)]);
    assert_eq!(
        slide_from(&prev, &next, &SidebarRow::Session(22)),
        Some(SidebarRow::Session(23))
    );
}

#[test]
fn a_middle_worktree_slides_to_the_next_worktree() {
    let prev = reference_tree();
    let next = reference_tree_without(&[row_worktree("/p2/wt2")]);
    assert_eq!(
        slide_from(&prev, &next, &row_worktree("/p2/wt2")),
        Some(row_worktree("/p2/wt3"))
    );
}

#[test]
fn a_removed_worktree_carries_its_sessions_out_of_the_slot() {
    let prev = reference_tree();
    let next = reference_tree_without(&[row_worktree("/p1/wt1")]);
    // /p1/wt1 owns sessions 11 and 12; the slot must hold /p1/wt2, never a
    // session orphaned by the removal.
    assert_eq!(
        slide_from(&prev, &next, &row_worktree("/p1/wt1")),
        Some(row_worktree("/p1/wt2"))
    );
}

#[test]
fn two_siblings_removed_at_once_still_land_on_a_survivor() {
    let prev = reference_tree();
    // Sessions 22 and 23 both go; 21 survives and must catch the cursor
    // instead of the parent worktree.
    let next = reference_tree_without(&[SidebarRow::Session(22), SidebarRow::Session(23)]);
    assert_eq!(
        slide_from(&prev, &next, &SidebarRow::Session(22)),
        Some(SidebarRow::Session(21))
    );
}

#[test]
fn a_reorder_alongside_a_removal_takes_the_row_now_in_the_slot() {
    let prev = reference_tree();
    // A background refresh reinstalls /p2's worktrees in a different order
    // while wt2 disappears: the row now occupying wt2's ordinal is wt1.
    let next = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        let p2 = b.push(row_project("/p2"), Parent::Root, true);
        b.push(row_worktree("/p2/wt3"), Parent::Node(p2), true);
        b.push(row_worktree("/p2/wt1"), Parent::Node(p2), true);
        b.finish(ObservedInputs::default())
    };

    assert_eq!(
        slide_from(&prev, &next, &row_worktree("/p2/wt2")),
        Some(row_worktree("/p2/wt1")),
        "the vacated ordinal wins, not whichever row used to follow"
    );
}

#[test]
fn an_only_child_falls_back_to_its_parent() {
    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    let p = b.push(row_project("/a"), Parent::Root, true);
    let wt = b.push(row_worktree("/a/wt1"), Parent::Node(p), true);
    b.push(SidebarRow::Session(7), Parent::Node(wt), true);
    let prev = b.finish(ObservedInputs::default());

    let without_session = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        let p = b.push(row_project("/a"), Parent::Root, true);
        b.push(row_worktree("/a/wt1"), Parent::Node(p), true);
        b.finish(ObservedInputs::default())
    };
    assert_eq!(
        slide_from(&prev, &without_session, &SidebarRow::Session(7)),
        Some(row_worktree("/a/wt1"))
    );

    let without_worktree = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        b.push(row_project("/a"), Parent::Root, true);
        b.finish(ObservedInputs::default())
    };
    assert_eq!(
        slide_from(&prev, &without_worktree, &row_worktree("/a/wt1")),
        Some(row_project("/a"))
    );
}

#[test]
fn top_level_rows_are_siblings_of_home() {
    let prev = reference_tree();

    // A middle project takes the next project.
    let next = reference_tree_without(&[row_project("/p1")]);
    assert_eq!(slide_from(&prev, &next, &row_project("/p1")), Some(row_project("/p2")));

    // The last project falls back to the previous one.
    let next = reference_tree_without(&[row_project("/p2")]);
    assert_eq!(slide_from(&prev, &next, &row_project("/p2")), Some(row_project("/p1")));

    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    b.push(row_project("/only"), Parent::Root, true);
    let single = b.finish(ObservedInputs::default());
    let bare = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        b.finish(ObservedInputs::default())
    };
    // The only project has no project sibling, so Home stands in.
    assert_eq!(slide_from(&single, &bare, &row_project("/only")), Some(SidebarRow::Home));
}

#[test]
fn a_detached_row_has_no_slide() {
    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    b.push(SidebarRow::Session(9), Parent::Detached, false);
    let prev = b.finish(ObservedInputs::default());

    let next = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        b.finish(ObservedInputs::default())
    };

    assert_eq!(slide_from(&prev, &next, &SidebarRow::Session(9)), None);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p alacritree sidebar_focus`
Expected: FAIL to compile — `cannot find function 'slide'`.

- [ ] **Step 3: Write the implementation**

Add to `alacritree/src/sidebar_focus.rs`, after the `TreeSnapshot` impl:

```rust
/// Where the cursor lands when `removed` leaves the model.
///
/// The removed row's parent and its ordinal among that parent's children come
/// from `prev`; the landing is chosen from the *surviving* children in `next`.
/// Resolving forward is what makes a simultaneous removal land on a survivor
/// rather than escaping to the parent, and what makes a reordered refresh land
/// on the row that actually occupies the vacated slot.
///
/// `Home` and project headers share `Parent::Root`, which makes them siblings
/// and lets the only project fall back to Home with no special case.
fn slide(prev: &TreeSnapshot, next: &TreeSnapshot, removed: NodeId) -> Option<SidebarRow> {
    let parent = prev.parent(removed);
    if parent == Parent::Detached {
        return None;
    }

    let was = prev.children(parent);
    let ordinal = was.iter().position(|&id| id == removed)?;

    let parent_in_next = match parent {
        Parent::Root => Parent::Root,
        Parent::Node(p) => Parent::Node(next.find(prev.row(p))?),
        Parent::Detached => return None,
    };
    let survivors = next.children(parent_in_next);

    if let Some(&landed) = survivors.get(ordinal) {
        return Some(next.row(landed).clone());
    }
    // The removed row was last, so the nearest preceding survivor is the new
    // last child.
    if let Some(&last) = survivors.last() {
        return Some(next.row(last).clone());
    }
    match parent {
        Parent::Node(p) => Some(prev.row(p).clone()),
        _ => None,
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo fmt && cargo test -p alacritree sidebar_focus`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/sidebar_focus.rs
git commit -m "feat(sidebar): slide a removed cursor to a sibling"
```

---

### Task 4: The climb, the anchor, and `repair`

The row being repaired is the **logical cursor**: the anchor when one is set, otherwise the visible cursor. A climb parks the visible cursor on an ancestor while the user's real position lives in the anchor, so removal has to be judged against the anchor or a hidden row's deletion goes unnoticed forever.

**Files:**
- Modify: `alacritree/src/sidebar_focus.rs`
- Test: same file

**Interfaces:**
- Consumes: Task 3's `slide`.
- Produces: `FollowTarget { Session(SessionId), Workspace(WorkspaceKey) }`; `Repair { cursor, anchor, follow }`; `pub fn repair(prev: &TreeSnapshot, next: &TreeSnapshot, cursor: Option<&SidebarRow>, anchor: Option<&SidebarRow>) -> Repair`.

`repair` always computes `follow`; the caller ignores it unless `ui.sidebar_focus.follows()`. Keeping the config out of the pure function halves the test matrix and leaves the gate in exactly one place (Task 6). Whether a filter is active is read from `next.inputs`, so the signature stays four arguments.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
/// Inputs standing for "a query is narrowing the tree", so the anchor's
/// filter episode is open.  `ObservedInputs::default()` is *not* filtering,
/// which would retire the anchor on sight.
fn filtering() -> ObservedInputs {
    ObservedInputs::capture(&[], std::iter::empty(), ui("wt", 0))
}

/// The reference tree with /p1/wt2 and its sessions hidden by a filter.
fn reference_tree_filtered() -> TreeSnapshot {
    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    let p1 = b.push(row_project("/p1"), Parent::Root, true);
    let p1wt1 = b.push(row_worktree("/p1/wt1"), Parent::Node(p1), true);
    b.push(SidebarRow::Session(11), Parent::Node(p1wt1), true);
    b.push(SidebarRow::Session(12), Parent::Node(p1wt1), true);
    let p1wt2 = b.push(row_worktree("/p1/wt2"), Parent::Node(p1), false);
    b.push(SidebarRow::Session(21), Parent::Node(p1wt2), false);
    b.push(SidebarRow::Session(22), Parent::Node(p1wt2), false);
    b.push(SidebarRow::Session(23), Parent::Node(p1wt2), false);
    b.finish(filtering())
}

#[test]
fn a_visible_cursor_is_left_alone() {
    let t = reference_tree();
    let r = repair(&t, &t, Some(&SidebarRow::Session(11)), None);
    assert_eq!(r.cursor, Some(SidebarRow::Session(11)));
    assert_eq!(r.anchor, None);
    assert_eq!(r.follow, None);
}

#[test]
fn an_unrelated_removal_does_not_move_the_cursor() {
    let prev = reference_tree();
    let next = reference_tree_without(&[SidebarRow::Session(31)]);

    let r = repair(&prev, &next, Some(&SidebarRow::Session(11)), None);
    assert_eq!(r.cursor, Some(SidebarRow::Session(11)));
    assert_eq!(r.follow, None);
}

#[test]
fn collapsing_a_project_climbs_rather_than_slides() {
    let prev = reference_tree();
    // /p1 collapses: its worktrees and their sessions leave the projection
    // but stay in the model.
    let next = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        let p1 = b.push(row_project("/p1"), Parent::Root, true);
        let wt1 = b.push(row_worktree("/p1/wt1"), Parent::Node(p1), false);
        b.push(SidebarRow::Session(11), Parent::Node(wt1), false);
        b.push(SidebarRow::Session(12), Parent::Node(wt1), false);
        b.finish(filtering())
    };

    let r = repair(&prev, &next, Some(&SidebarRow::Session(12)), None);
    assert_eq!(r.cursor, Some(row_project("/p1")), "the collapsed header is the nearest visible ancestor");
    assert_eq!(r.anchor, Some(SidebarRow::Session(12)), "expanding again must restore the row");
    assert_eq!(r.follow, None, "collapsing is a projection change, so nothing follows");
}

#[test]
fn dropping_below_the_listing_threshold_climbs_rather_than_slides() {
    // Two sessions under /a/wt1 are listed; one is closed, so the survivor
    // falls below the threshold and stops being a row while staying live.
    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    let p = b.push(row_project("/a"), Parent::Root, true);
    let wt = b.push(row_worktree("/a/wt1"), Parent::Node(p), true);
    b.push(SidebarRow::Session(1), Parent::Node(wt), true);
    b.push(SidebarRow::Session(2), Parent::Node(wt), true);
    let prev = b.finish(filtering());

    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    let p = b.push(row_project("/a"), Parent::Root, true);
    let wt = b.push(row_worktree("/a/wt1"), Parent::Node(p), true);
    b.push(SidebarRow::Session(1), Parent::Node(wt), false);
    let next = b.finish(filtering());

    let r = repair(&prev, &next, Some(&SidebarRow::Session(1)), None);
    assert_eq!(r.cursor, Some(row_worktree("/a/wt1")));
    assert_eq!(r.anchor, Some(SidebarRow::Session(1)), "the session is live, so it can come back");
    assert_eq!(r.follow, None);
}

#[test]
fn a_filtered_out_cursor_climbs_and_anchors() {
    let prev = reference_tree();
    let next = reference_tree_filtered();

    let r = repair(&prev, &next, Some(&SidebarRow::Session(22)), None);
    assert_eq!(r.cursor, Some(row_project("/p1")), "wt2 is hidden too, so the climb continues");
    assert_eq!(r.anchor, Some(SidebarRow::Session(22)));
    assert_eq!(r.follow, None, "a filter never moves the terminal");
}

#[test]
fn successive_narrowing_keeps_the_deepest_anchor() {
    let prev = reference_tree();
    let next = reference_tree_filtered();

    let r = repair(&prev, &next, Some(&row_worktree("/p1/wt2")), Some(&SidebarRow::Session(22)));
    assert_eq!(r.anchor, Some(SidebarRow::Session(22)), "the intermediate ancestor must not win");
}

#[test]
fn a_visible_anchor_is_restored_and_retired() {
    let prev = reference_tree_filtered();
    let mut next = reference_tree();
    next.inputs = filtering();

    let r = repair(&prev, &next, Some(&row_project("/p1")), Some(&SidebarRow::Session(22)));
    assert_eq!(r.cursor, Some(SidebarRow::Session(22)));
    assert_eq!(r.anchor, None);
    assert_eq!(r.follow, None, "restoring an anchor is a filter event, not a removal");
}

#[test]
fn ending_the_filter_episode_retires_the_anchor() {
    // Confirm/cancel/Shift+Esc all clear the query.  The confirmed row here is
    // the same one the climb already chose, so nothing observable changed —
    // only the episode ending can retire the anchor.
    let prev = reference_tree_filtered();
    let next = reference_tree();
    assert!(!next.inputs.is_filtering(), "the reference tree is unfiltered");

    let r = repair(&prev, &next, Some(&row_project("/p1")), Some(&SidebarRow::Session(22)));
    assert_eq!(r.cursor, Some(row_project("/p1")), "the confirmed row stands");
    assert_eq!(r.anchor, None, "a stale anchor must not yank the cursor away later");
}

#[test]
fn an_anchored_row_deleted_while_hidden_drops_the_anchor() {
    let prev = reference_tree_filtered();
    // Session 22 was hidden and anchored; it exits while out of sight.
    let mut next = reference_tree_without(&[SidebarRow::Session(22)]);
    next.inputs = filtering();

    let r = repair(&prev, &next, Some(&row_project("/p1")), Some(&SidebarRow::Session(22)));
    assert_eq!(r.anchor, None, "an anchor that left the model can never be restored");
    assert_eq!(r.cursor, Some(row_project("/p1")), "the visible cursor is still fine");
    assert_eq!(r.follow, None, "the row was not on screen, so the terminal does not chase it");
}

#[test]
fn a_removed_cursor_slides_and_follows() {
    let prev = reference_tree();
    let next = reference_tree_without(&[SidebarRow::Session(22)]);

    let r = repair(&prev, &next, Some(&SidebarRow::Session(22)), None);
    assert_eq!(r.cursor, Some(SidebarRow::Session(23)));
    assert_eq!(r.follow, Some(FollowTarget::Session(23)));
}

#[test]
fn a_landing_on_a_workspace_follows_its_live_session() {
    let prev = reference_tree();
    // /p2/wt2 has no sessions, so landing there offers nothing to follow.
    let next = reference_tree_without(&[row_worktree("/p2/wt3")]);
    let r = repair(&prev, &next, Some(&row_worktree("/p2/wt3")), None);
    assert_eq!(r.cursor, Some(row_worktree("/p2/wt2")));
    assert_eq!(r.follow, None);

    // Landing on a worktree that does have sessions follows the workspace.
    let next = reference_tree_without(&[row_worktree("/p1/wt2")]);
    let r = repair(&prev, &next, Some(&row_worktree("/p1/wt2")), None);
    assert_eq!(r.cursor, Some(row_worktree("/p1/wt1")));
    assert_eq!(
        r.follow,
        Some(FollowTarget::Workspace(Some(PathBuf::from("/p1/wt1"))))
    );
}

#[test]
fn a_project_landing_never_follows() {
    let mut b = SnapshotBuilder::default();
    b.push(SidebarRow::Home, Parent::Root, true);
    let p = b.push(row_project("/a"), Parent::Root, true);
    b.push(row_worktree("/a/wt1"), Parent::Node(p), true);
    let prev = b.finish(ObservedInputs::default());

    let next = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        b.push(row_project("/a"), Parent::Root, true);
        b.finish(ObservedInputs::default())
    };

    let r = repair(&prev, &next, Some(&row_worktree("/a/wt1")), None);
    assert_eq!(r.cursor, Some(row_project("/a")));
    assert_eq!(r.follow, None, "a project header is not a workspace");
}

#[test]
fn a_landing_hidden_by_the_filter_falls_through_to_the_climb() {
    let prev = reference_tree();
    // Session 22 is deleted, and in the same pass the filter hides everything
    // under /p1/wt2 that could have caught the slide.
    let next = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        let p1 = b.push(row_project("/p1"), Parent::Root, true);
        let wt2 = b.push(row_worktree("/p1/wt2"), Parent::Node(p1), false);
        b.push(SidebarRow::Session(21), Parent::Node(wt2), false);
        b.push(SidebarRow::Session(23), Parent::Node(wt2), false);
        b.finish(filtering())
    };

    let r = repair(&prev, &next, Some(&SidebarRow::Session(22)), None);
    assert_eq!(r.cursor, Some(row_project("/p1")));
    assert_eq!(r.follow, None, "the climb is a filter outcome, so nothing follows");
}

#[test]
fn a_cursor_gone_with_nothing_to_land_on_takes_the_first_row() {
    let prev = reference_tree();
    let next = {
        let mut b = SnapshotBuilder::default();
        b.push(SidebarRow::Home, Parent::Root, true);
        b.finish(ObservedInputs::default())
    };

    let r = repair(&prev, &next, Some(&SidebarRow::Session(22)), None);
    assert_eq!(r.cursor, Some(SidebarRow::Home));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p alacritree sidebar_focus`
Expected: FAIL to compile — `cannot find function 'repair'`.

- [ ] **Step 3: Write the implementation**

Add to `alacritree/src/sidebar_focus.rs`:

```rust
/// What the terminal switches to when a removal landing has something live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowTarget {
    Session(SessionId),
    /// The caller activates this workspace's active session, or its first
    /// live one when the active entry is stale.
    Workspace(WorkspaceKey),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub cursor: Option<SidebarRow>,
    pub anchor: Option<SidebarRow>,
    /// Only ever `Some` for a removal landing.  The caller drops it unless
    /// `ui.sidebar_focus` is `"follow"`.
    pub follow: Option<FollowTarget>,
}

/// The nearest ancestor of `from` that `next` still projects, walking `from`'s
/// own snapshot so a removed row's chain is still readable.  Root rows have no
/// ancestor, so the first projected row — Home whenever it is visible — is the
/// last resort.
fn climb(from_tree: &TreeSnapshot, next: &TreeSnapshot, from: NodeId) -> Option<SidebarRow> {
    let mut cur = from_tree.parent(from);
    while let Parent::Node(id) = cur {
        let row = from_tree.row(id);
        if next.find(row).is_some_and(|n| next.is_projected(n)) {
            return Some(row.clone());
        }
        cur = from_tree.parent(id);
    }
    next.projected.first().map(|&id| next.row(id).clone())
}

/// What a removal landing offers the terminal.  A workspace row with no live
/// session yields `None`: spawning a shell the user did not ask for is not
/// this module's job.
fn follow_target(next: &TreeSnapshot, landing: &SidebarRow) -> Option<FollowTarget> {
    match landing {
        SidebarRow::Session(id) => Some(FollowTarget::Session(*id)),
        SidebarRow::Project(_) => None,
        SidebarRow::Home | SidebarRow::Worktree(_) => {
            let id = next.find(landing)?;
            let has_session = next.nodes.iter().any(|node| {
                matches!(node.row, SidebarRow::Session(_)) && node.parent == Parent::Node(id)
            });
            if !has_session {
                return None;
            }
            let ws = match landing {
                SidebarRow::Worktree(path) => Some(path.clone()),
                _ => None,
            };
            Some(FollowTarget::Workspace(ws))
        },
    }
}

/// Repair the cursor against what changed between two snapshots.
///
/// The row under repair is the anchor when one is set — a climb parks the
/// visible cursor on an ancestor while the user's real position waits in the
/// anchor, so judging removal by the visible cursor would never notice a
/// hidden row being deleted.  Cursor, anchor, and terminal resolve together so
/// the caller cannot apply them out of order.
pub fn repair(
    prev: &TreeSnapshot,
    next: &TreeSnapshot,
    cursor: Option<&SidebarRow>,
    anchor: Option<&SidebarRow>,
) -> Repair {
    // The anchor belongs to one filter episode.  Nothing is filtering, so the
    // episode is over however it ended — confirmed, cancelled, or widened.
    let anchor = anchor.filter(|_| next.inputs.is_filtering());

    if let Some(a) = anchor {
        match next.find(a) {
            // Visible again: the user gets their row back.
            Some(id) if next.is_projected(id) => {
                return Repair { cursor: Some(a.clone()), anchor: None, follow: None };
            },
            // Still hidden: leave it parked and repair the visible cursor.
            Some(_) => {},
            // Deleted while out of sight; there is nothing left to restore.
            None => {
                return repair_visible(prev, next, cursor, None);
            },
        }
    }

    repair_visible(prev, next, cursor, anchor)
}

fn repair_visible(
    prev: &TreeSnapshot,
    next: &TreeSnapshot,
    cursor: Option<&SidebarRow>,
    anchor: Option<&SidebarRow>,
) -> Repair {
    let unchanged = Repair { cursor: cursor.cloned(), anchor: anchor.cloned(), follow: None };

    let Some(c) = cursor else {
        return unchanged;
    };

    match next.find(c) {
        Some(id) if next.is_projected(id) => unchanged,
        // Still in the model, so a filter or a collapse hid it: climb, and
        // remember the deepest row the user actually chose.
        Some(id) => Repair {
            cursor: climb(next, next, id),
            anchor: Some(anchor.cloned().unwrap_or_else(|| c.clone())),
            follow: None,
        },
        None => {
            let Some(removed) = prev.find(c) else {
                return Repair {
                    cursor: next.projected.first().map(|&id| next.row(id).clone()),
                    anchor: None,
                    follow: None,
                };
            };
            let landing = slide(prev, next, removed)
                .filter(|row| next.find(row).is_some_and(|id| next.is_projected(id)));
            match landing {
                Some(row) => {
                    let follow = follow_target(next, &row);
                    Repair { cursor: Some(row), anchor: None, follow }
                },
                // The slide target is itself hidden — a removal that also
                // changed what the filter keeps.  Fall through to the climb.
                None => Repair { cursor: climb(prev, next, removed), anchor: None, follow: None },
            }
        },
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo fmt && cargo test -p alacritree sidebar_focus`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/sidebar_focus.rs
git commit -m "feat(sidebar): repair the cursor from a diff"
```

---

### Task 5: Build the snapshot from live app state

The projection must be exactly `current_project_rows()` so the cursor model cannot drift from the paint pass. **Model membership must not come from `listed_session_ids`.** That map is the projection: `sidebar_session_ids` (`app.rs:4488`) drops a workspace's sessions entirely below its threshold (`if always { 1 } else { 2 }`), so a worktree going from two sessions to one would report the *surviving* session as removed. It also omits sessions whose project `remove_project` dropped, which keeps them running.

So the arena is built from the live `(workspace, id)` pairs, and `listed` only decides the `projected` flag.

**Files:**
- Modify: `alacritree/src/app.rs` (free function near `sidebar_session_ids` at `app.rs:4488`, methods beside `current_project_rows` at `app.rs:1490`)
- Test: `alacritree/src/app.rs` (existing `mod tests`)

**Interfaces:**
- Consumes: Task 2's `SnapshotBuilder`, `Parent`, `ObservedInputs`, `SessionInput`, `UiInputs`; existing `listed_session_ids` (`app.rs:5089`), `current_project_rows` (`app.rs:1490`), `sidebar_session_ids` (`app.rs:4488`).
- Produces: `fn build_sidebar_snapshot(projects: &[Project], live: &[(WorkspaceKey, SessionId)], rows: &[SidebarRow], skip_worktree: Option<&Path>, inputs: ObservedInputs) -> TreeSnapshot`; `fn session_pairs(&self) -> Vec<(WorkspaceKey, SessionId)>`; `fn session_inputs(&self) -> impl Iterator<Item = SessionInput<'_>>`; `fn sidebar_snapshot(&mut self, skip_worktree: Option<&Path>) -> TreeSnapshot`.

`skip_worktree` is used by Task 7 — a worktree whose deletion is in flight must read as gone from the model immediately, even though `projects` still lists it until the async git operation finishes. Pass `None` until then.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `alacritree/src/app.rs`:

```rust
#[test]
fn snapshot_parents_agree_with_the_row_model() {
    use crate::sidebar_focus::Parent;
    use crate::sidebar_nav::{self, SidebarRow};

    // Two projects, one collapsed, with sessions under the expanded one.
    let projects = vec![
        sidebar_nav::tests::project("/a", true, &["/a/wt1", "/a/wt2"]),
        sidebar_nav::tests::project("/b", false, &["/b/wt1"]),
    ];
    let live = vec![
        (None, 1),
        (Some(PathBuf::from("/a/wt1")), 2),
        (Some(PathBuf::from("/a/wt1")), 3),
    ];
    let listed = sidebar_nav::ListedSessions::from([
        (None, vec![1]),
        (Some(PathBuf::from("/a/wt1")), vec![2, 3]),
    ]);
    let rows = sidebar_nav::visible_rows(&projects, &listed);
    let snapshot = build_sidebar_snapshot(&projects, &live, &rows, None, Default::default());

    for row in &rows {
        let id = snapshot.find(row).expect("every projected row is in the model");
        let arena_parent = match snapshot.parent(id) {
            Parent::Root => None,
            Parent::Node(p) => Some(snapshot.row(p).clone()),
            Parent::Detached => panic!("a projected row is never detached: {row:?}"),
        };
        assert_eq!(
            arena_parent,
            sidebar_nav::left_target(&rows, row),
            "arena parent must agree with the row model for {row:?}"
        );
    }

    // The collapsed project's worktree is in the model but not projected.
    let hidden = snapshot
        .find(&SidebarRow::Worktree(PathBuf::from("/b/wt1")))
        .expect("collapsed worktrees stay in the model");
    assert!(!snapshot.is_projected(hidden));
}

#[test]
fn a_session_below_the_listing_threshold_is_still_in_the_model() {
    use crate::sidebar_nav::{self, SidebarRow};

    let projects = vec![sidebar_nav::tests::project("/a", true, &["/a/wt1"])];
    // One live session in the worktree.  The real rule needs two before it
    // lists any, so this one is live but unprojected.
    let live = vec![(Some(PathBuf::from("/a/wt1")), 7)];
    let listed = {
        let mut l = sidebar_nav::ListedSessions::new();
        let ids = sidebar_session_ids(&live, &Some(PathBuf::from("/a/wt1")), false);
        assert!(ids.is_empty(), "the threshold rule must actually drop this session");
        if !ids.is_empty() {
            l.insert(Some(PathBuf::from("/a/wt1")), ids);
        }
        l
    };
    let rows = sidebar_nav::visible_rows(&projects, &listed);
    let snapshot = build_sidebar_snapshot(&projects, &live, &rows, None, Default::default());

    let id = snapshot
        .find(&SidebarRow::Session(7))
        .expect("a live session is in the model whatever the listing threshold says");
    assert!(!snapshot.is_projected(id), "but it is not a navigable row");
}

#[test]
fn a_session_whose_project_is_gone_is_detached_not_deleted() {
    use crate::sidebar_focus::Parent;
    use crate::sidebar_nav::{self, SidebarRow};

    // `remove_project` drops the project but keeps its sessions running.
    let projects: Vec<crate::projects::Project> = vec![];
    let live = vec![(Some(PathBuf::from("/orphan/wt1")), 5)];
    let listed = sidebar_nav::ListedSessions::new();
    let rows = sidebar_nav::visible_rows(&projects, &listed);
    let snapshot = build_sidebar_snapshot(&projects, &live, &rows, None, Default::default());

    let id = snapshot.find(&SidebarRow::Session(5)).expect("the session is still running");
    assert_eq!(
        snapshot.parent(id),
        Parent::Detached,
        "an orphan must not become a sibling of Home"
    );
}

#[test]
fn a_worktree_being_deleted_reads_as_gone_immediately() {
    use crate::sidebar_nav::{self, SidebarRow};

    let projects = vec![sidebar_nav::tests::project("/a", true, &["/a/wt1", "/a/wt2"])];
    let listed = sidebar_nav::ListedSessions::new();
    let rows = sidebar_nav::visible_rows(&projects, &listed);
    let doomed = PathBuf::from("/a/wt2");
    let snapshot =
        build_sidebar_snapshot(&projects, &[], &rows, Some(doomed.as_path()), Default::default());

    assert_eq!(
        snapshot.find(&SidebarRow::Worktree(doomed)),
        None,
        "the async git delete has not finished, but the row must not read as present"
    );
    assert!(snapshot.find(&SidebarRow::Worktree(PathBuf::from("/a/wt1"))).is_some());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p alacritree snapshot_parents_agree`
Expected: FAIL to compile — `cannot find function 'build_sidebar_snapshot'`.

- [ ] **Step 3: Write the implementation**

Add a free function near `sidebar_session_ids` (`app.rs:4488`), pure over the same kind of snapshot the other helpers take:

```rust
/// Assemble the model arena and the projection.  `rows` is the projection —
/// exactly what the cursor steps over — and `live` is the model: every running
/// session, whatever the listing threshold or the filter says.  Building
/// membership from `listed` instead would make the last session in a workspace
/// read as deleted the moment its sibling closed.
///
/// `skip_worktree` drops a worktree whose deletion is already committed but
/// whose git operation has not finished, so nothing lands the cursor — or a
/// new shell — inside a directory on its way out.
///
/// Nodes are pushed in exactly the order `sidebar_nav::visible_rows` emits,
/// with unprojected nodes interleaved, so one forward index into `rows`
/// classifies every node.  Asking `rows.contains` per node instead would be
/// quadratic in path comparisons on a path that runs whenever the user types.
fn build_sidebar_snapshot(
    projects: &[Project],
    live: &[(WorkspaceKey, SessionId)],
    rows: &[SidebarRow],
    skip_worktree: Option<&Path>,
    inputs: sidebar_focus::ObservedInputs,
) -> sidebar_focus::TreeSnapshot {
    use sidebar_focus::Parent;

    let mut b = sidebar_focus::SnapshotBuilder::default();
    let mut next_row = 0usize;
    let mut placed = vec![false; live.len()];

    // Consume `rows` in lockstep: a node is projected exactly when it is the
    // row the projection expects next.
    let mut push = |b: &mut sidebar_focus::SnapshotBuilder,
                    next_row: &mut usize,
                    row: SidebarRow,
                    parent: Parent| {
        let projected = rows.get(*next_row) == Some(&row);
        if projected {
            *next_row += 1;
        }
        b.push(row, parent, projected)
    };

    let home_id = push(&mut b, &mut next_row, SidebarRow::Home, Parent::Root);
    for (i, (ws, id)) in live.iter().enumerate() {
        if ws.is_none() {
            push(&mut b, &mut next_row, SidebarRow::Session(*id), Parent::Node(home_id));
            placed[i] = true;
        }
    }

    for p in projects {
        let project_id =
            push(&mut b, &mut next_row, SidebarRow::Project(p.root.clone()), Parent::Root);
        for wt in &p.worktrees {
            if skip_worktree == Some(wt.path.as_path()) {
                continue;
            }
            let wt_id = push(
                &mut b,
                &mut next_row,
                SidebarRow::Worktree(wt.path.clone()),
                Parent::Node(project_id),
            );
            for (i, (ws, id)) in live.iter().enumerate() {
                if ws.as_deref() == Some(wt.path.as_path()) {
                    push(&mut b, &mut next_row, SidebarRow::Session(*id), Parent::Node(wt_id));
                    placed[i] = true;
                }
            }
        }
    }

    // Sessions whose workspace has no row left — a removed project, or a
    // worktree already treated as gone.  They are running, so they belong in
    // the model; they have no place in the tree, so they are nobody's sibling.
    for (i, (_, id)) in live.iter().enumerate() {
        if !placed[i] {
            b.push(SidebarRow::Session(*id), Parent::Detached, false);
        }
    }

    debug_assert_eq!(next_row, rows.len(), "every projected row must be in the arena");
    b.finish(inputs)
}
```

The lockstep only holds while the arena visits rows in the same order `visible_rows` emits them, so the `debug_assert` is load-bearing: if a future change reorders either side, the projection silently goes wrong in release and this fires in debug and in every test.

`listed` is not a parameter — the builder never consults it. It reaches the snapshot through `rows`, which `current_project_rows` already built from it.

Add the three `AlacritreeApp` methods beside `current_project_rows`:

```rust
    /// Every live session as a `(workspace, id)` pair — the same shape
    /// `close_fallback` and `sidebar_session_ids` take, and the model the
    /// focus reconciler observes.
    fn session_pairs(&self) -> Vec<(WorkspaceKey, SessionId)> {
        self.sessions.iter().map(|s| (s.working_directory.clone(), s.id)).collect()
    }

    /// Live sessions borrowed for the unchanged-inputs check, which runs on
    /// every frame and must not allocate.
    fn session_inputs(&self) -> impl Iterator<Item = sidebar_focus::SessionInput<'_>> {
        self.sessions.iter().map(|s| sidebar_focus::SessionInput {
            workspace: &s.working_directory,
            id: s.id,
            attention: s.needs_attention,
        })
    }

    fn sidebar_snapshot(&mut self, skip_worktree: Option<&Path>) -> sidebar_focus::TreeSnapshot {
        let inputs = sidebar_focus::ObservedInputs::capture(
            &self.projects,
            self.session_inputs(),
            sidebar_focus::UiInputs {
                session_rows_always: self.session_rows_always,
                query: self.project_filter.query(),
                toggles: self.project_filter.toggle_bits(),
            },
        );
        let rows = self.current_project_rows();
        let live = self.session_pairs();
        let snapshot =
            build_sidebar_snapshot(&self.projects, &live, &rows, skip_worktree, inputs);
        // Paint reuses these until the next rebuild, so an unchanged filtering
        // frame runs no fuzzy matching at all.
        self.sidebar_rows_cache = Some(rows);
        snapshot
    }
```

`sidebar_rows_cache` is declared in Task 6; add it there before running this task's tests, or declare it now and leave it unread.

Add `use crate::sidebar_focus;` and `use std::path::Path;` to the imports at the top of `app.rs` if not already present.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS, including the four new tests.

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "feat(sidebar): build the focus snapshot"
```

---

### Task 6: Wire the reconciler

**Files:**
- Modify: `alacritree/src/app.rs:267-290` (fields), `:600-615` (construction), `:919` (`ensure_active_session`), `:936` (`adopt_active_session`), `:1260-1279` (`focus_sidebar`), `:2375` (sidebar paint), `:6341-6366` (`update`)
- Test: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: Task 4's `repair`, Task 5's `sidebar_snapshot`, Task 1's `SidebarFocus`.
- Produces: `struct SidebarFocusWrite { cursor: Option<SidebarRow>, workspace: WorkspaceKey, active: Option<SessionId> }`; `fn sidebar_focus_overtaken(written: &Option<SidebarFocusWrite>, cursor: Option<&SidebarRow>, workspace: &WorkspaceKey, active: Option<SessionId>) -> bool`; `fn reconcile_sidebar_focus(&mut self, ctx: &Context)`; `fn mark_sidebar_focus_write(&mut self)`; `fn apply_follow_target(&mut self, ctx: &Context, target: FollowTarget)`; fields `sidebar_focus_prev`, `sidebar_anchor`, `sidebar_focus_written`, `sidebar_rows_this_frame`.

The sentinel tracks a **triple**, not a pair. Eight actions change which session is on screen — `SelectNextTab`/`SelectPreviousTab` (`cycle_tabs`, `app.rs:1113`), `SelectNextSession`/`SelectPreviousSession` (`cycle_sessions`, `app.rs:1143`), `SelectNextWorkspace`/`SelectPreviousWorkspace` (`app.rs:1914`), and `SelectTab(n)`/`SelectLastTab` (`select_tab`, `app.rs:2215`) — and half of them stay inside the current workspace, writing `active_session` while leaving both the cursor and `current_workspace` untouched. A pair would miss every one of those.

Do not match on action names. The triple is what makes the list above a description rather than a table to maintain: "changes which session is on screen" *is* "changes `active_session` or `current_workspace`", so every action, every rebinding, the command palette, and `run_action` over MCP are all covered without the reconciler naming any of them. The cost is that the two self-healing writers must mark themselves.

- [ ] **Step 1: Add the state**

In the `AlacritreeApp` struct, beside `sidebar_cursor` (`app.rs:275`):

```rust
    /// Last reconciled snapshot, the baseline for the next cursor repair.
    sidebar_focus_prev: Option<sidebar_focus::TreeSnapshot>,
    /// The deepest row a filter hid, restored when it becomes visible again.
    sidebar_anchor: Option<SidebarRow>,
    /// What the reconciler itself last wrote.  Different values on the next
    /// pass mean the user navigated — a click, session cycling, the palette, a
    /// notification, IPC — and the anchor has been overtaken.
    sidebar_focus_written: Option<SidebarFocusWrite>,
    /// The projection from the last rebuild, valid until the next one.  Paint
    /// reads it instead of re-running the fuzzy matcher, which it otherwise
    /// does on every frame a filter is active.
    sidebar_rows_cache: Option<Vec<SidebarRow>>,
```

In the constructor, beside `sidebar_cursor: None` (`app.rs:609`):

```rust
            sidebar_focus_prev: None,
            sidebar_anchor: None,
            sidebar_focus_written: None,
            sidebar_rows_cache: None,
```

- [ ] **Step 2: Write the failing test**

Add to `mod tests` in `alacritree/src/app.rs`:

```rust
#[test]
fn the_sentinel_sees_a_same_workspace_session_switch() {
    let written = SidebarFocusWrite {
        cursor: Some(SidebarRow::Home),
        workspace: None,
        active: Some(1),
    };
    let written = Some(written);

    // The reconciler's own values still stand.
    assert!(!sidebar_focus_overtaken(&written, Some(&SidebarRow::Home), &None, Some(1)));

    // Any action that switches sessions without leaving the workspace —
    // SelectNextTab, SelectNextSession, SelectTab(n) — changes neither the
    // cursor nor the workspace, only the active session.
    assert!(sidebar_focus_overtaken(&written, Some(&SidebarRow::Home), &None, Some(2)));

    // A different workspace, and a different cursor, each count too.
    assert!(sidebar_focus_overtaken(
        &written,
        Some(&SidebarRow::Home),
        &Some(PathBuf::from("/a/wt1")),
        Some(1),
    ));
    assert!(sidebar_focus_overtaken(
        &written,
        Some(&SidebarRow::Project(PathBuf::from("/a"))),
        &None,
        Some(1),
    ));

    // Nothing written yet cannot have been overtaken.
    assert!(!sidebar_focus_overtaken(&None, Some(&SidebarRow::Home), &None, Some(1)));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p alacritree the_sentinel_sees_a_same_workspace`
Expected: FAIL to compile — `cannot find function 'sidebar_focus_overtaken'`.

- [ ] **Step 4: Write the reconciler**

Add near `build_sidebar_snapshot`:

```rust
/// The cursor, workspace, and active session the reconciler last wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SidebarFocusWrite {
    cursor: Option<SidebarRow>,
    workspace: WorkspaceKey,
    active: Option<SessionId>,
}

/// Whether focus moved behind the reconciler's back.  The active session is
/// part of the comparison because the tab and session cycling actions can
/// switch sessions without leaving the workspace, changing nothing else.
/// Comparing the resulting state rather than matching on action names covers
/// every route to them — rebound keys, the command palette, MCP — at the price
/// of `ensure_active_session` and `adopt_active_session` marking their own
/// writes so their self-healing does not read as navigation.
fn sidebar_focus_overtaken(
    written: &Option<SidebarFocusWrite>,
    cursor: Option<&SidebarRow>,
    workspace: &WorkspaceKey,
    active: Option<SessionId>,
) -> bool {
    match written {
        None => false,
        Some(w) => {
            w.cursor.as_ref() != cursor || w.workspace != *workspace || w.active != active
        },
    }
}
```

Add the reconciler methods beside `sidebar_snapshot`:

```rust
    /// Repair the sidebar cursor against what changed since the last pass.
    /// Called twice per `update` — before paint for everything the input and
    /// background drains produced, and again at the end for what only
    /// `reap_exited_sessions` and paint-time clicks can produce.  A pass with
    /// nothing to do costs one `ObservedInputs` compare, which is the whole
    /// steady-state budget: there is no setting that skips this.
    fn reconcile_sidebar_focus(&mut self, ctx: &Context) {
        if sidebar_focus_overtaken(
            &self.sidebar_focus_written,
            self.sidebar_cursor.as_ref(),
            &self.current_workspace,
            self.active_session.get(&self.current_workspace).copied(),
        ) {
            self.sidebar_anchor = None;
        }

        let deferred = self.sidebar_deferred_close.take();
        let skip = deferred.as_ref().and_then(|d| d.removed_worktree.clone());

        if deferred.is_none() {
            if let Some(prev) = &self.sidebar_focus_prev {
                let unchanged = prev.inputs.matches(
                    &self.projects,
                    self.session_inputs(),
                    sidebar_focus::UiInputs {
                        session_rows_always: self.session_rows_always,
                        query: self.project_filter.query(),
                        toggles: self.project_filter.toggle_bits(),
                    },
                );
                if unchanged {
                    return;
                }
            }
        }

        let next = self.sidebar_snapshot(skip.as_deref());
        let prev = self.sidebar_focus_prev.take().unwrap_or_else(|| next.clone());
        let outcome = sidebar_focus::repair(
            &prev,
            &next,
            self.sidebar_cursor.as_ref(),
            self.sidebar_anchor.as_ref(),
        );

        if outcome.cursor != self.sidebar_cursor {
            self.sidebar_cursor = outcome.cursor;
            self.sidebar_cursor_moved = true;
        }
        self.sidebar_anchor = outcome.anchor;
        self.sidebar_focus_prev = Some(next);

        if self.config.ui.sidebar_focus.follows() {
            match (outcome.follow, deferred) {
                (Some(target), _) => self.apply_follow_target(ctx, target),
                // Nothing live to land on, so the verdict this pass took over
                // from still decides where the terminal goes.
                (None, Some(deferred)) => self.apply_close_fallback(ctx, deferred.verdict),
                (None, None) => {},
            }
        }

        self.mark_sidebar_focus_write();
    }

    /// Record the current focus triple as the reconciler's own, so the next
    /// pass does not mistake it for the user navigating.
    fn mark_sidebar_focus_write(&mut self) {
        self.sidebar_focus_written = Some(SidebarFocusWrite {
            cursor: self.sidebar_cursor.clone(),
            workspace: self.current_workspace.clone(),
            active: self.active_session.get(&self.current_workspace).copied(),
        });
    }

    /// Move the terminal to a removal landing.  A workspace target adopts its
    /// active session, or its first live one when that entry went stale.
    fn apply_follow_target(&mut self, ctx: &Context, target: sidebar_focus::FollowTarget) {
        match target {
            sidebar_focus::FollowTarget::Session(id) => self.activate_session_by_id(id),
            sidebar_focus::FollowTarget::Workspace(ws) => {
                let id = self
                    .active_session
                    .get(&ws)
                    .copied()
                    .filter(|id| self.sessions.iter().any(|s| s.id == *id))
                    .or_else(|| {
                        self.sessions.iter().find(|s| s.working_directory == ws).map(|s| s.id)
                    });
                if let Some(id) = id {
                    self.activate_session_by_id(id);
                }
            },
        }
        ctx.request_repaint();
    }
```

`sidebar_deferred_close` and `apply_close_fallback` are introduced in Task 7. Until then, stub the two lines that use them by treating `deferred` as always `None`:

```rust
        let deferred: Option<()> = None;
        let skip: Option<PathBuf> = None;
```

and drop the `(None, Some(deferred))` arm. Task 7 replaces both.

- [ ] **Step 5: Call it at both points, and mark the three self-writes**

In `update` (`app.rs:6365`), immediately after `self.process_session_events(ctx);`:

```rust
        self.reconcile_sidebar_focus(ctx);
```

And at the very end of `update`, immediately after `self.reap_exited_sessions(ctx);` (`app.rs:6477`):

```rust
        // A shell that exited on its own is only removed here, after paint.
        // Without this pass its deferred verdict would wait for unrelated
        // input; with it, the repair is queued for the frame the repaint
        // request has already scheduled.
        self.reconcile_sidebar_focus(ctx);
```

The second call is the same method. When nothing changed since the first, it returns after one `ObservedInputs` compare — a linear scan of contiguous memory, no allocation.

At the end of `focus_sidebar` (`app.rs:1278`), after `self.sidebar_cursor_moved = true;`:

```rust
        // Seeding rewrites the cursor from terminal state, which the overtaken
        // check would otherwise read as the user navigating.  The anchor
        // outlives a trip through the terminal by design.
        self.mark_sidebar_focus_write();
```

At the end of `ensure_active_session` (`app.rs:919`) and `adopt_active_session` (`app.rs:936`), after each has settled `active_session`:

```rust
        // Filling in a missing active entry is self-healing, not navigation.
        self.mark_sidebar_focus_write();
```

- [ ] **Step 6: Let paint reuse the projection**

At `app.rs:2375`, inside the `if filtering` branch, read the cache instead of recomputing:

```rust
        let rows = match &self.sidebar_rows_cache {
            Some(rows) => rows.clone(),
            None => self.current_project_rows(),
        };
```

This is the one place the feature makes the app *faster*. `current_project_rows` re-runs the nucleo matcher over every row on every frame a filter is active; the cache is rebuilt only when an observed input changes, so held-still filtering frames now do no matching at all.

The cache cannot go stale: the reconciler runs before paint on every frame and rebuilds whenever any observed input differs, so whatever sits here already matches the current inputs.

This is what pays for the unconditional reconciler. `app.rs:2375` currently rebuilds the filtered rows and re-runs the nucleo matcher *every frame* a filter is open; reading them from here instead makes an unchanged filtering frame strictly cheaper than it is today. The steady-state compare is a new cost on unfiltered frames and a net saving on filtered ones.

Take the `clone` for now rather than restructuring the borrow — the sidebar paint mutates `self` while iterating rows. It is one `Vec<SidebarRow>` per filtering frame against the fuzzy match it replaces.

- [ ] **Step 7: Run the suite**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS, including `the_sentinel_sees_a_same_workspace_session_switch`.

- [ ] **Step 8: Verify the default is untouched**

Run: `cargo run -p alacritree`
With no `sidebar_focus` in `alacritree.toml`: focus the sidebar, delete a session with the Delete action, confirm the cursor still falls to Home exactly as before. Then set `sidebar_focus = "preserve"` and confirm it lands on the sibling.

- [ ] **Step 9: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "feat(sidebar): reconcile cursor focus per frame"
```

---

### Task 7: Hand the three eager paths to the reconciler

Observation cannot un-spawn a PTY, un-navigate, or un-reset a cursor. Three paths act before the reconciler could see the state that motivated them.

`after_filter_changed` is the one that decides whether the feature works at all: it calls `ensure_cursor` the instant a filter changes, so by the time the reconciler looks, the cursor is already on row 0 and the row that was hidden is unknowable.

**Its deferral is unconditional, and that is a behavior change for every user.** With no `"reset"` mode there is no configuration under which `after_filter_changed` still repairs eagerly, so Step 4 deletes it rather than guarding it. The consequence is worth stating plainly: nothing catches a filtered-out cursor any more except the reconciler, so if the reconciler ever fails to run, the cursor is left wherever the filter put it. That is the trade the two-value enum buys, and Step 8's GUI pass is the only thing that exercises the removed path end to end.

Only `close_session` and `run_pending_delete` stay mode-gated, on `follows()`.

**Files:**
- Modify: `alacritree/src/app.rs:944-996` (`close_session`), `:1434-1441` (`after_filter_changed`), `:5571-5586` (`run_pending_delete`)
- Test: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: Task 1's `SidebarFocus`, Task 6's reconciler.
- Produces: `struct DeferredClose { verdict: CloseFallback, removed_worktree: Option<PathBuf> }`; field `sidebar_deferred_close: Option<DeferredClose>`; `fn apply_close_fallback(&mut self, ctx: &Context, verdict: CloseFallback)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_deferred_verdict_survives_instead_of_being_re_derived() {
    // `close_fallback` is the only thing that knows to hop to the project's
    // main checkout; a generic "spawn something" fallback would strand
    // last_session_close = "navigate" in the workspace that just emptied.
    let main = PathBuf::from("/p/main");
    let removed = Some(PathBuf::from("/p/feature"));
    let remaining = vec![(Some(main.clone()), 1)];

    let verdict = close_fallback(&removed, &removed, &remaining, Some(main.clone()));
    assert_eq!(verdict, CloseFallback::Activate(main.clone()));

    let deferred = DeferredClose { verdict, removed_worktree: None };
    assert_eq!(
        deferred.verdict,
        CloseFallback::Activate(main),
        "the verdict is carried, not recomputed from whatever state remains"
    );
}

#[test]
fn only_follow_defers_close_navigation() {
    use crate::config::SidebarFocus;

    assert!(defers_close_navigation(SidebarFocus::Follow));
    assert!(!defers_close_navigation(SidebarFocus::Preserve));
}
```

There is deliberately no test for the filter deferral here. It is unconditional, so the only thing a predicate test could assert is that a constant is constant — the kind of assertion that restates its implementation and covers nothing. Its real coverage is Task 4's `repair` transitions (a hidden cursor climbs and anchors) plus Step 8's GUI pass.

`app.rs` imports config items individually; add `SidebarFocus` to the existing `use crate::config::{...}` list so the non-test code can name it too.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p alacritree deferred_verdict only_follow_defers`
Expected: FAIL to compile — `cannot find struct 'DeferredClose'`, `cannot find function 'defers_close_navigation'`.

- [ ] **Step 3: Add the state and the predicates**

Beside `close_fallback` (`app.rs:4514`):

```rust
/// A close-fallback verdict the reconciler owes the terminal, and the worktree
/// whose rows must already read as gone.  The verdict is carried rather than
/// recomputed because only `close_fallback` knows the difference between
/// staying put, hopping to the project's main checkout, and going home.
#[derive(Debug, Clone)]
struct DeferredClose {
    verdict: CloseFallback,
    /// Set when an asynchronous worktree deletion is in flight: `projects`
    /// still lists it, so without this the reconciler would see an intact row
    /// and could spawn a shell inside the directory being removed.
    removed_worktree: Option<PathBuf>,
}

/// Whether the reconciler owns post-removal navigation.  Under `"follow"` the
/// landing row decides where the terminal goes, so acting here first would
/// show one workspace for a frame and another the next.
fn defers_close_navigation(mode: SidebarFocus) -> bool {
    mode.follows()
}
```

Add the field beside the Task 6 fields:

```rust
    /// A close verdict the reconciler still owes the terminal.
    sidebar_deferred_close: Option<DeferredClose>,
```

and `sidebar_deferred_close: None,` in the constructor. Replace the Task 6 Step 4 stub lines with the real `take()`, and restore the `(None, Some(deferred))` arm.

- [ ] **Step 4: Hand the filter cursor to the reconciler**

`after_filter_changed` (`app.rs:1432-1441`) exists only to run that `ensure_cursor` repair, and it has exactly one caller — the `Outcome::FilterChanged` arm of `apply_filter_outcome` (`app.rs:1425`). Delete the method, doc comment included, and make the arm say who owns the repair now:

```rust
            // The reconciler repairs the cursor later in this same update, from
            // a snapshot that still knows which row the filter hid.  Repairing
            // here would reset it before anything could observe that.
            Outcome::FilterChanged => {},
```

`sidebar_nav::ensure_cursor` keeps its other callers (`app.rs:1277` in `focus_sidebar`, `app.rs:2131` in `finish_project_search_at`), so it stays. Confirm both facts rather than assuming them:

```bash
rg -n "after_filter_changed|sidebar_nav::ensure_cursor" alacritree/src/
```

Expected: no `after_filter_changed`, and `sidebar_nav::ensure_cursor` still called from at least two places. If it ends up with none, say so — a `sidebar_nav` function with no callers should go along with its tests, not sit as dead code.

- [ ] **Step 5: Defer both close paths**

Extract the existing verdict dispatch in `close_session` (`app.rs:972-995`) into a method so the reconciler can run the same code later:

```rust
    /// Act on a close verdict: stay put, move to the project's main checkout,
    /// or go home.
    fn apply_close_fallback(&mut self, ctx: &Context, verdict: CloseFallback) {
        match verdict {
            CloseFallback::Stay => {},
            CloseFallback::Activate(main) => self.activate_worktree(ctx, &main),
            CloseFallback::Home => self.activate_home(ctx),
        }
    }
```

Then in `close_session`, replace the inline dispatch with:

```rust
        let verdict = close_fallback(&workspace, &self.current_workspace, &remaining, main);
        if defers_close_navigation(self.config.ui.sidebar_focus) && verdict != CloseFallback::Stay {
            self.sidebar_deferred_close = Some(DeferredClose { verdict, removed_worktree: None });
            // `reap_exited_sessions` runs after paint, so a shell that exited
            // on its own has no reconciler pass left this frame; without this
            // the deferral would wait for unrelated input.
            ctx.request_repaint();
            return;
        }
        self.apply_close_fallback(ctx, verdict);
```

In `run_pending_delete`, guard the eager home hop (`app.rs:5578-5585`):

```rust
        self.sessions.retain(|s| s.working_directory.as_deref() != Some(&req.worktree_path));
        self.active_session.remove(&Some(req.worktree_path.clone()));
        if self.current_workspace.as_deref() == Some(&req.worktree_path) {
            if defers_close_navigation(self.config.ui.sidebar_focus) {
                self.sidebar_deferred_close = Some(DeferredClose {
                    verdict: CloseFallback::Home,
                    removed_worktree: Some(req.worktree_path.clone()),
                });
                ctx.request_repaint();
            } else {
                // Deleting the on-screen worktree is an explicit user action,
                // so home should greet with a live shell rather than the "no
                // session" placeholder.
                self.activate_home(ctx);
            }
        }
```

- [ ] **Step 6: Run the suite**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 7: Prove the filter deferral was the load-bearing one**

Temporarily restore the deleted repair inside the `Outcome::FilterChanged` arm:

```rust
            Outcome::FilterChanged => {
                let rows = self.current_project_rows();
                let next = sidebar_nav::ensure_cursor(&rows, self.sidebar_cursor.as_ref());
                if next != self.sidebar_cursor {
                    self.sidebar_cursor = next;
                    self.sidebar_cursor_moved = true;
                }
            },
```

Run: `cargo run -p alacritree`. Put the cursor on a session, type `/` and a character that excludes it.
Expected: the cursor drops to the first row and widening the query does not bring it back — the pre-change behavior, reproduced with the whole reconciler still in place. That is the point: it proves the deferral, not the reconciler, is what makes filtering work.

Restore the empty arm, repeat: the cursor climbs to the ancestor and returns when the query widens. Record both outcomes.

- [ ] **Step 8: Verify in the GUI**

Run: `cargo run -p alacritree` with `sidebar_focus = "follow"`.
- Delete a middle session in a multi-session worktree: the cursor lands on the next session and the terminal shows it.
- Delete the last session in a feature worktree whose project main has a live session, with `last_session_close = "navigate"`: the terminal lands on the main checkout, not on a new shell in the emptied worktree.
- Delete the on-screen worktree: no home flash, and no shell spawned inside the directory being deleted.
- Let a shell exit on its own with `exit`: the terminal settles without needing a keypress.
- Set `sidebar_focus = "preserve"` (the default), repeat all four: the cursor moves exactly as under `"follow"`, and the terminal stays put in every case.

This pass is load-bearing beyond the usual smoke test. With `after_filter_changed` deleted rather than guarded, and the close paths deferred for every `"follow"` user, there is no configuration left that exercises the old immediate paths — so nothing else in the suite can tell you that deferral preserved their outcomes. Specifically confirm there is no frame showing the "no session" placeholder, and no repaint that needs a keypress to arrive.

- [ ] **Step 9: Commit**

```bash
git add alacritree/src/app.rs
git commit -m "feat(sidebar): defer navigation to the reconciler"
```

---

### Task 8: Gate the steady-state cost

The reconciler has no off-switch, so "one linear compare, zero allocation" stopped being a performance goal and became an invariant every user depends on. An argument in a design document does not survive the next refactor; a failing test does.

Two things are worth enforcing, and only one of them can be enforced deterministically:

- **Zero heap allocation on an unchanged frame.** Deterministic, and it catches the exact regression that is easy to reintroduce — an `active_toggles()`, a `to_string()`, a temporary `Vec` — because the counter goes from 0 to non-zero regardless of machine or load.
- **Linear, not quadratic, work.** Wall-clock thresholds on a shared CI runner are either flaky or so loose they detect nothing, so this is asserted with a counter rather than a timer: `matches` records how many records it examined, and the test asserts that examining a 5× larger tree does under 10× the work.

Timing gets a harness, but not a pass/fail gate — see Step 5.

**Files:**
- Create: `alacritree/src/steady_state.rs`
- Modify: `alacritree/src/main.rs` (register the module under `#[cfg(test)]`)
- Modify: `alacritree/src/sidebar_focus.rs` (the visit counter)

**Interfaces:**
- Consumes: Task 2's `ObservedInputs`, `SessionInput`, `UiInputs`; Task 1's config key is not involved — the invariant holds for both modes.
- Produces: `steady_state::CountingAllocator` (test-only global allocator), `steady_state::measure(f) -> Counts { allocs, bytes }`; `sidebar_focus::visits()` and `sidebar_focus::reset_visits()`, both `#[cfg(test)]`.

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/steady_state.rs`:

```rust
//! Cost gate for the sidebar reconciler's per-frame path.
//!
//! The reconciler runs on every frame with no setting that disables it, so
//! "an unchanged frame allocates nothing" is a property the app depends on
//! rather than a target to aim at.  A counting allocator is the only way to
//! observe it: a timing threshold on a shared runner is either flaky or too
//! loose to detect anything.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    /// Counting is per-thread because a `#[global_allocator]` is process-wide
    /// and this crate has no library target to put an isolated test binary
    /// against: the test harness itself allocates, `cargo test` runs tests
    /// concurrently, and the app's own threads allocate whenever they like.
    /// Gating on the measuring thread is what makes the count attributable.
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if MEASURING.try_with(|m| m.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if MEASURING.try_with(|m| m.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub allocs: usize,
    pub bytes: usize,
}

/// Run `f` with allocation counting on for this thread only.  Everything the
/// assertion needs — formatting, panicking, `Vec` growth in the caller — must
/// happen outside the closure or it counts itself.
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Counts) {
    // Touch the TLS slot first: its own lazy initialisation allocates on some
    // platforms, and that allocation is not the one under test.
    MEASURING.with(|m| m.set(false));
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);

    MEASURING.with(|m| m.set(true));
    let out = f();
    MEASURING.with(|m| m.set(false));

    (out, Counts { allocs: ALLOCS.load(Ordering::Relaxed), bytes: BYTES.load(Ordering::Relaxed) })
}
```

Then the tests, in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_focus::{self, ObservedInputs, SessionInput, UiInputs};
    use crate::sidebar_nav::tests::project;

    /// `projects` × `worktrees` each, with `sessions` sessions per worktree.
    fn tree(projects: usize, worktrees: usize) -> Vec<crate::projects::Project> {
        (0..projects)
            .map(|p| {
                let wts: Vec<String> =
                    (0..worktrees).map(|w| format!("/home/user/code/p{p}/worktree-{w}")).collect();
                let refs: Vec<&str> = wts.iter().map(String::as_str).collect();
                project(&format!("/home/user/code/p{p}"), true, &refs)
            })
            .collect()
    }

    fn sessions(count: usize) -> Vec<(Option<std::path::PathBuf>, u64)> {
        (0..count)
            .map(|i| (Some(std::path::PathBuf::from(format!("/home/user/code/p0/worktree-{i}"))), i as u64))
            .collect()
    }

    fn inputs<'a>(
        s: &'a [(Option<std::path::PathBuf>, u64)],
    ) -> impl Iterator<Item = SessionInput<'a>> {
        s.iter().map(|(ws, id)| SessionInput { workspace: ws, id: *id, attention: false })
    }

    #[test]
    fn an_unchanged_frame_allocates_nothing() {
        let projects = tree(10, 5);
        let live = sessions(150);
        let ui = UiInputs { session_rows_always: false, query: "", toggles: 0 };
        let base = ObservedInputs::capture(&projects, inputs(&live), ui);

        let (same, counts) = measure(|| base.matches(&projects, inputs(&live), ui));

        assert!(same, "the fixture must actually be unchanged, or this measures the wrong path");
        assert_eq!(
            counts.allocs, 0,
            "an unchanged frame allocated {} times ({} bytes) — the steady-state path has no \
             off-switch, so this is a per-frame tax on every user",
            counts.allocs, counts.bytes
        );
    }

    #[test]
    fn an_unchanged_filtering_frame_allocates_nothing() {
        let projects = tree(10, 5);
        let live = sessions(150);
        let ui = UiInputs { session_rows_always: false, query: "worktree-3", toggles: 0b11 };
        let base = ObservedInputs::capture(&projects, inputs(&live), ui);

        let (same, counts) = measure(|| base.matches(&projects, inputs(&live), ui));

        assert!(same);
        assert_eq!(counts.allocs, 0, "a filter must not put an allocation back in the frame path");
    }

    #[test]
    fn the_compare_is_linear_in_the_tree_size() {
        let small = tree(10, 5);
        let big = tree(50, 10);
        let ui = UiInputs { session_rows_always: false, query: "", toggles: 0 };

        let base_small = ObservedInputs::capture(&small, std::iter::empty(), ui);
        sidebar_focus::reset_visits();
        assert!(base_small.matches(&small, std::iter::empty(), ui));
        let small_visits = sidebar_focus::visits();

        let base_big = ObservedInputs::capture(&big, std::iter::empty(), ui);
        sidebar_focus::reset_visits();
        assert!(base_big.matches(&big, std::iter::empty(), ui));
        let big_visits = sidebar_focus::visits();

        // 50×10 is 10× the records of 10×5.  Linear work lands near 10×;
        // anything quadratic lands near 100× and trips this well before a
        // timing threshold would notice.
        assert!(
            big_visits < small_visits * 20,
            "comparing a 10× larger tree examined {big_visits} records against {small_visits} \
             — that is superlinear, so something is scanning inside a per-node loop"
        );
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Add `#[cfg(test)] mod steady_state;` to `alacritree/src/main.rs`.

Run: `cargo test -p alacritree steady_state`
Expected: FAIL to compile — `cannot find function 'visits'` / `cannot find function 'reset_visits'` in `sidebar_focus`.

The allocation tests may compile and pass immediately. That is fine and expected: they assert an invariant the Task 2 implementation already satisfies, so their job is to fail *later*, when someone reintroduces an allocation. Step 4 proves they can.

- [ ] **Step 3: Add the visit counter**

In `alacritree/src/sidebar_focus.rs`, beside `ObservedInputs`:

```rust
#[cfg(test)]
thread_local! {
    static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records one examined record.  Compiled out of release builds entirely —
/// the counter exists so the linearity of `matches` can be asserted without a
/// wall-clock threshold, which on a shared runner is either flaky or blind.
#[inline(always)]
fn visit() {
    #[cfg(test)]
    VISITS.with(|v| v.set(v.get() + 1));
}

#[cfg(test)]
pub fn visits() -> usize {
    VISITS.with(|v| v.get())
}

#[cfg(test)]
pub fn reset_visits() {
    VISITS.with(|v| v.set(0));
}
```

Call `visit()` once at the top of the project loop body, once at the top of the nested worktree loop body, and once per session in the session loop of `ObservedInputs::matches`. Three call sites, nothing else.

- [ ] **Step 4: Prove the allocation gate can fail (RED for the right reason)**

An assertion that has never failed is not a gate. Temporarily reintroduce the exact regression it exists to catch — swap the `toggles` comparison in `matches` for the allocating form:

```rust
        if self.toggle_chars != ui.filter.active_toggles() {
            return false;
        }
```

or, if that is awkward to wire up, simply add `let _ = ui.query.to_string();` as the first line of `matches`.

Run: `cargo test -p alacritree steady_state::tests::an_unchanged_frame_allocates_nothing -- --exact`
Expected: FAIL — `an unchanged frame allocated 1 times (N bytes)`.

The full path matters: `--exact` against a prefix matches nothing and reports success having run zero tests.

Revert, re-run, confirm PASS. Record both outcomes.

- [ ] **Step 5: Add the on-demand timing harness**

Absolute numbers are still worth having — for the PR description, and as a baseline to compare against later. They are **not** a pass/fail gate: a single-digit-microsecond measurement on a shared runner moves with CPU frequency and load, so asserting on it produces either flake or noise.

`criterion` and `divan` both need a `benches/` target, which needs a library target this crate does not have. So the harness is an ignored test:

```rust
    /// Not a gate — run it by hand when changing the frame path:
    /// `cargo test -p alacritree --release -- --ignored --nocapture steady_state`
    #[test]
    #[ignore = "timing harness, not an assertion"]
    fn report_steady_state_cost() {
        for (p, w, s) in [(10, 5, 150), (50, 10, 500)] {
            let projects = tree(p, w);
            let live = sessions(s);
            let ui = UiInputs { session_rows_always: false, query: "", toggles: 0 };
            let base = ObservedInputs::capture(&projects, inputs(&live), ui);

            // Warm the caches so the first iteration is not the whole sample.
            for _ in 0..1_000 {
                std::hint::black_box(base.matches(&projects, inputs(&live), ui));
            }

            let iterations = 100_000;
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(base.matches(&projects, inputs(&live), ui));
            }
            let each = start.elapsed() / iterations;

            println!("{p} projects x {w} worktrees, {s} sessions: {each:?} per unchanged frame");
        }
    }
}
```

Run it and record both numbers. At 120 fps a frame budget is 8.3 ms; anything in single-digit microseconds is noise against painting and PTY reads, and anything above ~100 µs means the estimate was wrong and the design needs revisiting before the PR — say so rather than shipping it quietly.

- [ ] **Step 6: Run the suite**

Run: `cargo fmt && cargo test -p alacritree`
Expected: PASS. The timing harness is skipped (`1 ignored`).

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/steady_state.rs alacritree/src/sidebar_focus.rs alacritree/src/main.rs
git commit -m "test(sidebar): gate the steady-state frame cost"
```

---

### Task 9: Document the option

**Files:**
- Modify: `docs/alacritree.md` (the `[ui]` block in `## Configuration`, around line 229)
- Modify: `CLAUDE.md` (the `app.rs` bullet, to name the reconciler)

**Interfaces:**
- Consumes: Task 1's config key.
- Produces: no code.

The repo ships no sample `alacritree.toml`; `docs/alacritree.md` § Configuration is the only place `[ui]` options are documented.

- [ ] **Step 1: Add the option to the `[ui]` block**

Match the surrounding aligned-comment style (`sidebar_click_focus` is the nearest neighbour):

```toml
sidebar_focus    = "preserve"   # how far the projects sidebar goes when the
                                # cursor's row stops being rendered.
                                # "preserve" (default): a filtered-out cursor
                                # climbs to its nearest visible ancestor and
                                # returns when the filter widens; a deleted row
                                # slides to a sibling bounded by its parent.
                                # "follow": also moves the terminal to a delete
                                # landing that has a live session
```

- [ ] **Step 2: Note the changed default**

`"preserve"` is the default, so upgrading changes where the cursor goes without anyone editing a config file, and there is no value that restores the old behavior. Say that where a user upgrading will see it — in `docs/alacritree.md`, immediately under the `[ui]` block:

```markdown
The sidebar cursor used to drop to the first row whenever its own row stopped
being rendered — by a filter, or by deleting a session or worktree. It now
climbs or slides instead, under `sidebar_focus = "preserve"`. There is no
setting that restores the old drop-to-first-row behavior.
```

Keep it to the fact and the missing escape hatch. This is documentation, not a changelog entry: no PR link, no "as of this version".

- [ ] **Step 3: Update the architecture note**

In `CLAUDE.md`, extend the `app.rs` bullet:

```
Cursor repair for the left sidebar is reconciled once per frame in
`sidebar_focus.rs` by diffing a snapshot of the tree, rather than by each
mutation site reporting what it removed. The reconcile runs unconditionally, so
its unchanged-frame path must stay allocation-free — `steady_state.rs` asserts
that.
```

- [ ] **Step 4: Commit**

```bash
git add docs/alacritree.md CLAUDE.md
git commit -m "docs: describe ui.sidebar_focus"
```

---

## Verification before opening the PR

- [ ] The prerequisite WSL branch is merged, or its commit is present in this worktree. The reconciler is unsafe without it
- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy -p alacritree --all-targets` no new warnings
- [ ] `cargo test -p alacritree` green
- [ ] Manual GUI pass under **both** `sidebar_focus` values: delete flows, search narrow/widen, search confirm landing on the climbed row, Esc and Shift+Esc mid-search, tab out and back mid-search
- [ ] **Deferral parity, since nothing else covers it.** With `after_filter_changed` deleted and the close paths deferred, no configuration exercises the old immediate paths any more. Confirm by hand that deferring changed only *when* they happen: no frame showing the "no session" placeholder, no repaint waiting on a keypress, `last_session_close = "navigate"` still landing on the project's main checkout
- [ ] Anchor retirement by action, not by chord: with an anchor set, confirm each of `SelectNextTab`, `SelectNextSession`, `SelectNextWorkspace`, and `SelectTab(2)` discards it — including the three that stay inside the current workspace. Repeat one of them from the command palette to confirm the route does not matter
- [ ] `cargo test -p alacritree --release -- --ignored --nocapture steady_state` run, and both numbers recorded in the PR description. If either is above ~100 µs, stop and say so — the cost argument was wrong and the design needs revisiting, not shipping
- [ ] Re-read `build_sidebar_snapshot`, `ObservedInputs::matches`, and the paint path for any `contains` in a per-node loop, any `active_toggles()`, and any per-frame allocation. The allocation gate covers `matches`; it does not cover `build_sidebar_snapshot` or paint
- [ ] No perceptible delay under `"follow"`: a keyboard delete settles the terminal in the same frame; `exit` in the last shell settles without a keypress
- [ ] The PR description states, in its own words, that this changes default behavior with no opt-out. A reviewer must not have to infer that from the diff
- [ ] `git log --oneline feat/sidebar-search-actions..HEAD` shows one commit per task, none touching `docs/superpowers/`

## Unresolved questions

1. **Task 5's `push` closure captures `rows` while `b` is passed in.** Written that way to keep the borrow checker happy without threading a struct through. If it fights, make it a free function taking `(&mut SnapshotBuilder, &[SidebarRow], &mut usize, SidebarRow, Parent)` — same cost, more noise at the call sites.
2. **The `a` toggle still has no special handling.** A background session flipping attention changes the projection with no user input, firing a climb attributable to no action. The anchor makes it recoverable and it was deferred once already; revisit if it bites in daily use.
3. **The allocation gate covers `matches` and nothing else.** `build_sidebar_snapshot` and the paint path are in `app.rs`, which cannot be exercised without an `eframe::CreationContext`. Those stay covered by review and by the Task 5 design (monotonic pointer, no set membership in a per-node loop) rather than by a test. Closing that gap means either an app fixture or a library target, both larger than this feature.
4. **A real benchmark harness needs a library target.** `divan` and `criterion` both require `benches/`, which requires `lib.rs`, which this crate does not have — and adding one would conflict with every in-flight branch. Task 8 ships an ignored timing test instead: honest numbers, no statistical machinery. If per-frame cost ever becomes a live question, the sequence is `lib.rs` first, then `divan`, as its own change.
5. **PGO is not what this harness is for.** An instrumented run of the Task 8 timing test would tell LLVM the reconciler is ~100% of the program, skewing inlining and layout for a binary that actually spends its time in egui painting and PTY reads. If alacritree ever gets PGO, the training workload is a scripted end-to-end session — startup, sustained terminal output, typing, scrolling, resizing, tab and workspace switching, sidebar filtering and deletion, shutdown — under `cargo-pgo run`, where the reconciler earns its true weight. Recorded here so nobody wires the two together later on the strength of both being called "benchmarks".
