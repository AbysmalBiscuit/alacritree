# Session reorder implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** let a user set a session's position in the sidebar and tab strip, by dragging the row with the mouse or by pressing a bound key, with a config option deciding how far a session may travel.

**Architecture:** a session's position *is* its index in `AlacritreeApp::sessions`, so reordering is a permutation of that one `Vec` and there is no second structure to keep in sync. The rules that decide where a session may land are pure functions in `sidebar_nav.rs` (`move_range`, `step_target`) called by both the keyboard path and the mouse path, so the two cannot disagree. `app.rs` holds only the application of a decided move: swap within a workspace, or change `working_directory` and then swap.

**Tech Stack:** Rust 2024 (MSRV 1.85), egui/eframe 0.31.1, `alacritty_terminal`. Tests are in-module `#[cfg(test)]`, run with `cargo nextest run -p alacritree`.

**Spec:** `docs/superpowers/specs/2026-09-05-session-reorder-design.md`

## Global constraints

- Only `alacritree/` is edited. `alacritty*` and `egui-winit/` are vendored and read-only.
- Default behaviour must not change. `[ui.session_reorder] drag` defaults to `false`, `scope` defaults to `"workspace"`, and none of the three new actions gets a default key binding.
- Config doc comments are the published JSON Schema's hover text. After touching `config.rs`, regenerate with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.
- Commits use Conventional Commits, imperative subject, no trailing period, lowercase after the colon, wrapped at 72. Every commit ends with the trailer `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`.
- Comments explain *why*, never restate the *what*. No task references, no PR narration.
- `Session` owns a PTY and has no `Clone`. It must not grow one: every reorder is `Vec::swap`.
- Work happens in a worktree cut from the newest open PR, not from `master`. See Task 0.

---

### Task 0: Worktree setup

**Files:**
- Create: `../alacritree-worktrees/feat/session-reorder/` (a git worktree, not a source file)

**Interfaces:**
- Consumes: nothing.
- Produces: the working directory every later task edits in. All paths below are relative to this worktree root.

- [ ] **Step 1: Read the current tip of the PR stack**

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
```

Take the entry whose title carries the highest `[n]` marker. Its `headRefName` is the base for this branch and `n + 1` is this branch's marker. At the time this plan was written the tip was PR 210, `fix/wsl-helper-liveness`, marker `[8]`, so this branch would be marker `[9]`. Do not trust that number: the stack grows.

- [ ] **Step 2: Create the worktree**

```sh
devkit issue setup 20 --slug feat/session-reorder
```

- [ ] **Step 3: Re-point it at the stack tip**

`devkit issue setup` always cuts from `origin/master`, and this branch must sit on the tip instead. Substitute the `headRefName` from Step 1:

```sh
git -C ../alacritree-worktrees/feat/session-reorder reset --hard origin/fix/wsl-helper-liveness
```

- [ ] **Step 4: Confirm the baseline is green**

```sh
cargo nextest run -p alacritree
```

Expected: PASS. A red baseline is a stack problem, not yours — stop and report it rather than implementing on top of it.

---

### Task 1: `ReorderScope` and `[ui.session_reorder]` config

**Files:**
- Modify: `alacritree/src/config.rs` (enum beside `SearchScope` at 644; `UiTheme` field beside `session_display` at 903; `UiTheme::default` at 973; `RawSessionDisplay` neighbour at 1847; `RawUi` field at 1973; `into_config` at 2212; tests beside `session_display_defaults_to_hidden` at 3092)
- Modify: `alacritree/schema/alacritree-config.json` (regenerated, never hand-edited)
- Test: `alacritree/src/config.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum ReorderScope { Workspace, Project, Anywhere }` (`Default` = `Workspace`, derives `Debug, Clone, Copy, PartialEq, Eq, Default`) and `pub struct SessionReorder { pub drag: bool, pub scope: ReorderScope }` (derives `Debug, Clone, Copy, PartialEq, Eq, Default`), both in `crate::config`, reachable as `config.ui.session_reorder`.

`ReorderScope` lives in `config.rs` rather than in `sidebar_nav.rs` because that is the direction `SidebarFocus` (`config.rs:612`) already established: config owns the enum, the pure modules import it.

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s test module, next to `session_display_defaults_to_hidden` (around line 3092). `ui_from_toml` is the existing helper at `config.rs:2510`.

```rust
    #[test]
    fn session_reorder_defaults_to_off_and_workspace_scope() {
        let ui = ui_from_toml("");
        assert!(!ui.session_reorder.drag);
        assert_eq!(ui.session_reorder.scope, ReorderScope::Workspace);
    }

    #[test]
    fn session_reorder_parses_every_scope() {
        for (raw, expected) in [
            ("workspace", ReorderScope::Workspace),
            ("project", ReorderScope::Project),
            ("anywhere", ReorderScope::Anywhere),
        ] {
            let ui = ui_from_toml(&format!("[ui.session_reorder]\nscope = \"{raw}\""));
            assert_eq!(ui.session_reorder.scope, expected, "value {raw:?}");
        }
    }

    #[test]
    fn session_reorder_invalid_scope_falls_back_to_workspace() {
        let ui = ui_from_toml("[ui.session_reorder]\nscope = \"everywhere\"");
        assert_eq!(ui.session_reorder.scope, ReorderScope::Workspace);
    }

    #[test]
    fn session_reorder_partial_table_leaves_the_other_key_alone() {
        let ui = ui_from_toml("[ui.session_reorder]\ndrag = true");
        assert!(ui.session_reorder.drag);
        assert_eq!(ui.session_reorder.scope, ReorderScope::Workspace);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree session_reorder
```

Expected: FAIL to compile, `no field session_reorder on type UiTheme` and `cannot find type ReorderScope`.

- [ ] **Step 3: Add the enum and its parser**

In `config.rs`, immediately after `parse_search_scope` (which ends around line 663):

```rust
/// `[ui.session_reorder] scope`: how far a session may travel when the user
/// reorders it.  Widening it makes a reorder step able to change which
/// workspace a session belongs to, which is why the default keeps a session
/// inside the one it was spawned in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReorderScope {
    /// Only among the sessions of its own workspace.
    #[default]
    Workspace,
    /// Across the worktrees of the project that owns its workspace.  Home
    /// belongs to no project, so a home session stays home.
    Project,
    /// Home and every project's worktrees, in sidebar order.
    Anywhere,
}

fn parse_reorder_scope(raw: Option<&str>) -> ReorderScope {
    match raw {
        None => ReorderScope::default(),
        Some("workspace") => ReorderScope::Workspace,
        Some("project") => ReorderScope::Project,
        Some("anywhere") => ReorderScope::Anywhere,
        Some(other) => {
            log::warn!("unknown ui.session_reorder.scope value {other:?}, using \"workspace\"");
            ReorderScope::default()
        },
    }
}

/// Whether session rows can be dragged, and how far a reorder may carry a
/// session.  `drag` is a startup default only: the app copies it into runtime
/// state that `ToggleSessionDrag` flips, and nothing is persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionReorder {
    pub drag: bool,
    pub scope: ReorderScope,
}
```

- [ ] **Step 4: Add the raw struct**

In `config.rs`, immediately after `RawSessionDisplay` (which ends around line 1853):

```rust
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(default)]
struct RawSessionReorder {
    /// Let a session row be dragged with the mouse to reorder it.
    drag: Option<bool>,
    /// How far a reorder may carry a session: "workspace" (default) |
    /// "project" | "anywhere".
    #[schemars(extend("enum" = ["workspace", "project", "anywhere"]))]
    scope: Option<String>,
}
```

- [ ] **Step 5: Wire it through `RawUi`, `UiTheme` and `into_config`**

In `RawUi`, directly after the `session_display: RawSessionDisplay,` field (around line 1973):

```rust
    /// Whether session rows can be dragged, and how far a reorder may carry
    /// a session.
    session_reorder: RawSessionReorder,
```

In `UiTheme`, directly after the `pub session_display: SessionDisplay,` field (around line 903):

```rust
    /// Mouse-drag gate and travel limit for reordering sessions
    /// ([`SessionReorder`]).
    pub session_reorder: SessionReorder,
```

In `UiTheme`'s `Default` impl, after `session_display: SessionDisplay::default(),` (around line 973):

```rust
            session_reorder: SessionReorder::default(),
```

In `into_config`, after the `session_display: SessionDisplay { .. },` block (around line 2216):

```rust
            session_reorder: SessionReorder {
                drag: self.ui.session_reorder.drag.unwrap_or(false),
                scope: parse_reorder_scope(self.ui.session_reorder.scope.as_deref()),
            },
```

- [ ] **Step 6: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree session_reorder
```

Expected: PASS, four tests.

- [ ] **Step 7: Regenerate the schema**

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

Then confirm it is no longer stale:

```sh
cargo test -p alacritree --test config_schema
```

Expected: PASS. Do not hand-edit `schema/alacritree-config.json`; if the diff looks wrong, the doc comments are wrong.

- [ ] **Step 8: Commit**

```sh
git add alacritree/src/config.rs alacritree/schema/alacritree-config.json
git commit -m "$(cat <<'EOF'
feat(config): add [ui.session_reorder] drag and scope

Both keys default to today's behaviour: no mouse drag, and a session
that cannot leave the workspace it was spawned in.  ReorderScope sits
beside the other config enums so the pure sidebar modules can import it
the way sidebar_focus already imports SidebarFocus.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `move_range` in `sidebar_nav.rs`

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs` (new function after `visible_rows`, which ends at line 55)
- Test: `alacritree/src/sidebar_nav.rs` `#[cfg(test)] pub(crate) mod tests` (starts line 186)

**Interfaces:**
- Consumes: `crate::config::ReorderScope` from Task 1.
- Produces:

```rust
pub fn move_range(
    projects: &[Project],
    order: &[WorkspaceKey],
    origin: &WorkspaceKey,
    scope: ReorderScope,
) -> Vec<WorkspaceKey>
```

`order` is the caller's live workspace list, in sidebar order, Home first. Task 5 builds it. The returned range is always non-empty and always contains `origin`.

- [ ] **Step 1: Write the failing tests**

Add to `sidebar_nav.rs`'s test module. `project(root, expanded, worktrees)` is the existing helper at line 195.

```rust
    fn ws(path: &str) -> WorkspaceKey {
        Some(PathBuf::from(path))
    }

    /// Home plus every worktree of both projects, the shape `workspace_order`
    /// hands `move_range` when nothing is missing or being deleted.
    fn full_order() -> Vec<WorkspaceKey> {
        vec![None, ws("/a/wt1"), ws("/a/wt2"), ws("/b/wt1")]
    }

    fn two_projects() -> Vec<Project> {
        vec![project("/a", true, &["/a/wt1", "/a/wt2"]), project("/b", true, &["/b/wt1"])]
    }

    #[test]
    fn move_range_workspace_scope_is_the_origin_alone() {
        let range = move_range(
            &two_projects(),
            &full_order(),
            &ws("/a/wt1"),
            ReorderScope::Workspace,
        );
        assert_eq!(range, vec![ws("/a/wt1")]);
    }

    #[test]
    fn move_range_project_scope_lists_the_owning_projects_worktrees() {
        let range =
            move_range(&two_projects(), &full_order(), &ws("/a/wt1"), ReorderScope::Project);
        assert_eq!(range, vec![ws("/a/wt1"), ws("/a/wt2")]);
    }

    #[test]
    fn move_range_project_scope_keeps_home_alone() {
        let range = move_range(&two_projects(), &full_order(), &None, ReorderScope::Project);
        assert_eq!(range, vec![None]);
    }

    #[test]
    fn move_range_anywhere_scope_is_the_order_verbatim() {
        let range = move_range(&two_projects(), &full_order(), &None, ReorderScope::Anywhere);
        assert_eq!(range, full_order());
    }

    #[test]
    fn move_range_ignores_project_expansion() {
        let collapsed =
            vec![project("/a", false, &["/a/wt1", "/a/wt2"]), project("/b", false, &["/b/wt1"])];
        let range =
            move_range(&collapsed, &full_order(), &ws("/a/wt1"), ReorderScope::Anywhere);
        assert_eq!(range, full_order());
    }

    #[test]
    fn move_range_omits_workspaces_absent_from_the_order() {
        // /a/wt2 is gone or has a delete in flight, so the caller left it out.
        let order = vec![None, ws("/a/wt1"), ws("/b/wt1")];
        let range = move_range(&two_projects(), &order, &ws("/a/wt1"), ReorderScope::Project);
        assert_eq!(range, vec![ws("/a/wt1")]);
    }

    #[test]
    fn move_range_collapses_to_the_origin_when_the_order_omits_it() {
        // A session whose project was removed keeps running and keeps its
        // directory; it may still reorder inside it, and cross nothing.
        let order = vec![None, ws("/a/wt1")];
        for scope in [ReorderScope::Workspace, ReorderScope::Project, ReorderScope::Anywhere] {
            let range = move_range(&two_projects(), &order, &ws("/gone"), scope);
            assert_eq!(range, vec![ws("/gone")], "scope {scope:?}");
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree move_range
```

Expected: FAIL to compile, `cannot find function move_range`.

- [ ] **Step 3: Write the implementation**

Add `use crate::config::ReorderScope;` to `sidebar_nav.rs`'s imports, then this after `visible_rows`:

```rust
/// The project whose worktree list contains `path`.  A path two projects both
/// list resolves to the first in sidebar order: a session records a directory,
/// not a project, so there is nothing better to go on.
fn owning_project<'a>(projects: &'a [Project], path: &Path) -> Option<&'a Project> {
    projects.iter().find(|p| p.worktrees.iter().any(|w| w.path == path))
}

/// The workspaces a session living in `origin` may move through, in sidebar
/// order.  `order` is the caller's live workspace list — the workspaces it is
/// willing to switch to, minus any whose delete is already running — so a
/// reorder can never land a session somewhere the rest of the app refuses to
/// go.
///
/// Expansion is deliberately not consulted: a collapsed project's worktrees
/// are destinations like any other, or the set of them would depend on which
/// projects happen to be open.
///
/// The result always contains `origin`.  When the scope's list does not, it
/// collapses to `origin` alone: a detached session, or one in a worktree being
/// deleted, has no position in a list it is not in, and must still be free to
/// move inside its own workspace.
pub fn move_range(
    projects: &[Project],
    order: &[WorkspaceKey],
    origin: &WorkspaceKey,
    scope: ReorderScope,
) -> Vec<WorkspaceKey> {
    let range: Vec<WorkspaceKey> = match scope {
        ReorderScope::Workspace => Vec::new(),
        ReorderScope::Project => match origin.as_deref().and_then(|p| owning_project(projects, p)) {
            Some(project) => order
                .iter()
                .filter(|ws| {
                    ws.as_deref().is_some_and(|p| project.worktrees.iter().any(|w| w.path == p))
                })
                .cloned()
                .collect(),
            None => Vec::new(),
        },
        ReorderScope::Anywhere => order.to_vec(),
    };
    if range.contains(origin) { range } else { vec![origin.clone()] }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree move_range
```

Expected: PASS, seven tests.

- [ ] **Step 5: Commit**

```sh
git add alacritree/src/sidebar_nav.rs
git commit -m "$(cat <<'EOF'
feat(sidebar): model the workspaces a session may reorder through

move_range filters the caller's live workspace list by scope, so the
keyboard and the mouse read one rule from one place.  A range that
would not contain the session's own workspace collapses to it, which is
what keeps a detached session able to reorder in place.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `step_target` in `sidebar_nav.rs`

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs` (after `move_range`)
- Test: `alacritree/src/sidebar_nav.rs` test module

**Interfaces:**
- Consumes: `move_range` from Task 2.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepTarget {
    pub workspace: WorkspaceKey,
    pub position: usize,
}

pub fn step_target(
    range: &[WorkspaceKey],
    lens: &[usize],
    origin: &WorkspaceKey,
    index: usize,
    delta: i32,
) -> Option<StepTarget>
```

`lens[i]` is the current session count of `range[i]`. `index` is the moving session's position among `origin`'s sessions. `delta` is `-1` for up, `1` for down. `None` means the step has nowhere to go, which every caller treats as a silent no-op.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn step_swaps_with_the_neighbour_inside_a_workspace() {
        let range = vec![ws("/a"), ws("/b")];
        let lens = vec![3, 1];
        assert_eq!(
            step_target(&range, &lens, &ws("/a"), 1, -1),
            Some(StepTarget { workspace: ws("/a"), position: 0 })
        );
        assert_eq!(
            step_target(&range, &lens, &ws("/a"), 1, 1),
            Some(StepTarget { workspace: ws("/a"), position: 2 })
        );
    }

    #[test]
    fn step_down_off_the_end_lands_at_the_front_of_the_next_workspace() {
        let range = vec![ws("/a"), ws("/b")];
        let lens = vec![2, 2];
        assert_eq!(
            step_target(&range, &lens, &ws("/a"), 1, 1),
            Some(StepTarget { workspace: ws("/b"), position: 0 })
        );
    }

    #[test]
    fn step_up_off_the_front_lands_at_the_end_of_the_previous_workspace() {
        let range = vec![ws("/a"), ws("/b")];
        let lens = vec![2, 2];
        assert_eq!(
            step_target(&range, &lens, &ws("/b"), 0, -1),
            Some(StepTarget { workspace: ws("/a"), position: 2 })
        );
    }

    #[test]
    fn step_into_an_empty_workspace_lands_at_position_zero() {
        let range = vec![ws("/a"), ws("/b")];
        let lens = vec![1, 0];
        assert_eq!(
            step_target(&range, &lens, &ws("/a"), 0, 1),
            Some(StepTarget { workspace: ws("/b"), position: 0 })
        );
        // And the same landing read from the other direction: appending to an
        // empty workspace is position 0 too.
        let lens = vec![0, 1];
        assert_eq!(
            step_target(&range, &lens, &ws("/b"), 0, -1),
            Some(StepTarget { workspace: ws("/a"), position: 0 })
        );
    }

    #[test]
    fn step_clamps_at_both_ends_of_the_range() {
        let range = vec![ws("/a"), ws("/b")];
        let lens = vec![2, 2];
        assert_eq!(step_target(&range, &lens, &ws("/a"), 0, -1), None);
        assert_eq!(step_target(&range, &lens, &ws("/b"), 1, 1), None);
    }

    #[test]
    fn step_in_a_single_workspace_range_never_crosses() {
        let range = vec![ws("/a")];
        let lens = vec![2];
        assert_eq!(step_target(&range, &lens, &ws("/a"), 1, 1), None);
        assert_eq!(step_target(&range, &lens, &ws("/a"), 0, -1), None);
        assert_eq!(
            step_target(&range, &lens, &ws("/a"), 0, 1),
            Some(StepTarget { workspace: ws("/a"), position: 1 })
        );
    }

    #[test]
    fn step_rejects_an_origin_the_range_does_not_list() {
        let range = vec![ws("/a")];
        let lens = vec![2];
        assert_eq!(step_target(&range, &lens, &ws("/b"), 0, 1), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree step_
```

Expected: FAIL to compile, `cannot find function step_target`.

- [ ] **Step 3: Write the implementation**

```rust
/// Where a session lands after one reorder step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepTarget {
    pub workspace: WorkspaceKey,
    /// Position among that workspace's sessions once the move is applied.
    pub position: usize,
}

/// One reorder step for the session sitting at `index` of `origin`.
///
/// `range` comes from [`move_range`] and `lens[i]` is the current session
/// count of `range[i]`.  `delta` is negative for up and positive for down.
///
/// A step off either end of a workspace continues into the neighbouring one:
/// up lands past the last session there, down lands before the first.  An
/// empty neighbour resolves to position 0 either way, so it needs no case of
/// its own.  `None` is every refusal — both ends of the range are clamped and
/// nothing wraps.
pub fn step_target(
    range: &[WorkspaceKey],
    lens: &[usize],
    origin: &WorkspaceKey,
    index: usize,
    delta: i32,
) -> Option<StepTarget> {
    debug_assert_eq!(range.len(), lens.len(), "one length per workspace in the range");
    let k = range.iter().position(|ws| ws == origin)?;
    if index >= *lens.get(k)? {
        return None;
    }
    if delta < 0 {
        if index > 0 {
            return Some(StepTarget { workspace: origin.clone(), position: index - 1 });
        }
        let prev = k.checked_sub(1)?;
        Some(StepTarget { workspace: range[prev].clone(), position: lens[prev] })
    } else if delta > 0 {
        if index + 1 < lens[k] {
            return Some(StepTarget { workspace: origin.clone(), position: index + 1 });
        }
        Some(StepTarget { workspace: range.get(k + 1)?.clone(), position: 0 })
    } else {
        None
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree step_
```

Expected: PASS, seven new tests (plus any existing test whose name contains `step_`).

- [ ] **Step 5: Commit**

```sh
git add alacritree/src/sidebar_nav.rs
git commit -m "$(cat <<'EOF'
feat(sidebar): resolve one reorder step to a workspace and position

A step off the end of a workspace continues into the next one in the
range rather than stopping, which is what makes a held key walk a
session across a boundary.  Both ends clamp; nothing wraps.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: The three actions

**Files:**
- Modify: `alacritree/src/bindings.rs` (`NamedAction` enum around 120; `parse_action` around 1017; `description` around 353; test table `new_action_names_parse` at 1314)
- Modify: `alacritree/src/command_palette.rs` (`section_of` at 86; `bindable_actions` at 259)
- Modify: `docs/keyboard-shortcuts.md` (the sidebar/session list around line 239)
- Test: `alacritree/src/bindings.rs`, `alacritree/src/command_palette.rs` test modules

**Interfaces:**
- Consumes: nothing.
- Produces: `NamedAction::ToggleSessionDrag`, `NamedAction::MoveSessionUp`, `NamedAction::MoveSessionDown`. Tasks 6 and 7 add the `dispatch_action` arms; this task only makes the names exist, parse, describe themselves and appear in the palette.

- [ ] **Step 1: Write the failing tests**

In `bindings.rs`'s test module, a new test beside the existing name tables:

```rust
    #[test]
    fn session_reorder_actions_parse_from_config_names() {
        for (name, expected) in [
            ("ToggleSessionDrag", NamedAction::ToggleSessionDrag),
            ("MoveSessionUp", NamedAction::MoveSessionUp),
            ("MoveSessionDown", NamedAction::MoveSessionDown),
        ] {
            assert!(
                matches!(parse_action(name), BindingAction::Named(a) if a == expected),
                "{name} does not parse"
            );
        }
    }
```

In `command_palette.rs`'s test module:

```rust
    #[test]
    fn session_reorder_actions_file_under_sessions() {
        for action in [
            NamedAction::ToggleSessionDrag,
            NamedAction::MoveSessionUp,
            NamedAction::MoveSessionDown,
        ] {
            assert_eq!(section_of(action), PaletteSection::Sessions, "{action:?}");
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree session_reorder_actions
```

Expected: FAIL to compile, `no variant named ToggleSessionDrag found for enum NamedAction`.

- [ ] **Step 3: Add the enum variants**

In `bindings.rs`, directly after `ToggleSessionTabs` in `NamedAction`:

```rust
    /// Flip whether session rows can be dragged with the mouse.
    ToggleSessionDrag,
    /// Move a session one position earlier in the sidebar and tab strip,
    /// continuing into the previous workspace when `[ui.session_reorder]
    /// scope` allows it.
    MoveSessionUp,
    /// Move a session one position later, continuing into the next workspace
    /// when the scope allows it.
    MoveSessionDown,
```

- [ ] **Step 4: Add the parse and description arms**

In `parse_action`, after the `"ToggleSessionTabs"` arm (line 1018):

```rust
        "ToggleSessionDrag" => BindingAction::Named(ToggleSessionDrag),
        "MoveSessionUp" => BindingAction::Named(MoveSessionUp),
        "MoveSessionDown" => BindingAction::Named(MoveSessionDown),
```

In `description`, after the `Self::ToggleSessionTabs` arm (line 353):

```rust
            Self::ToggleSessionDrag => "Toggle dragging session rows to reorder".into(),
            Self::MoveSessionUp => "Move the session one position up".into(),
            Self::MoveSessionDown => "Move the session one position down".into(),
```

Add the three names to the `new_action_names_parse` table at `bindings.rs:1314`, after `("ToggleSessionTabs", NamedAction::ToggleSessionTabs),`:

```rust
            ("ToggleSessionDrag", NamedAction::ToggleSessionDrag),
            ("MoveSessionUp", NamedAction::MoveSessionUp),
            ("MoveSessionDown", NamedAction::MoveSessionDown),
```

- [ ] **Step 5: Register them with the palette**

In `command_palette.rs`, extend the `section_of` arm at line 86:

```rust
        ToggleSessionRows | ToggleSessionTabs | ToggleSessionDrag => Sessions,
        MoveSessionUp | MoveSessionDown => Sessions,
```

Change `bindable_actions`'s return type from `[NamedAction; 63]` to `[NamedAction; 66]` and add the three after `ToggleSessionTabs,` (line 305):

```rust
        ToggleSessionDrag,
        MoveSessionUp,
        MoveSessionDown,
```

- [ ] **Step 6: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree
```

Expected: PASS. A non-exhaustive `match` on `NamedAction` anywhere will fail the build instead — add the missing arm there rather than a catch-all, unless the existing arm is already `_ =>`.

- [ ] **Step 7: Document the three names**

In `docs/keyboard-shortcuts.md`, after the `ToggleSessionRows` / `ToggleSessionTabs` bullet (around line 239). Soft-wrapped: one line per bullet, no hard column wrap.

```markdown
- `ToggleSessionDrag` — flip whether a session row can be dragged with the mouse to reorder it. Starts from `[ui.session_reorder] drag`. No default key.
- `MoveSessionUp` / `MoveSessionDown` — move a session one position in the sidebar and the tab strip. With the sidebar focused this moves the cursored session, or the cursored workspace's active session when the cursor is on a Home or worktree row; from the terminal it moves the session on screen. How far a session may travel is `[ui.session_reorder] scope`. No default keys.
```

- [ ] **Step 8: Commit**

```sh
git add alacritree/src/bindings.rs alacritree/src/command_palette.rs docs/keyboard-shortcuts.md
git commit -m "$(cat <<'EOF'
feat(bindings): add the session reorder actions

ToggleSessionDrag, MoveSessionUp and MoveSessionDown, none with a
default key: binding keys that currently reach the PTY would change
behaviour for a user who asked for nothing.  One enum serves key
bindings, the palette, the CLI and MCP alike.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: The move primitives in `app.rs`

**Files:**
- Modify: `alacritree/src/app.rs` (`move_session_to` at 1452; `workspace_session_indices` at 1536; new free function beside `move_target` at 5716; the `Req::MoveSession` arm at 8272)
- Test: `alacritree/src/app.rs` `#[cfg(test)] mod tests`, beside the `move_target` tests at 9156

**Interfaces:**
- Consumes: `StepTarget` from Task 3.
- Produces:
  - `fn walk_swaps(indices: &[usize], j: usize, position: usize) -> Vec<(usize, usize)>` — free function, pure.
  - `fn move_session_to_key(&mut self, id: SessionId, target: WorkspaceKey) -> Result<WorkspaceKey, String>` — replaces `move_session_to`.
  - `fn reorder_session_within_workspace(&mut self, id: SessionId, position: usize)`
  - `fn apply_session_move(&mut self, id: SessionId, target: StepTarget)`
  - `fn reorderable_workspaces(&self) -> Vec<WorkspaceKey>`

- [ ] **Step 1: Write the failing tests**

Add to `app.rs`'s test module beside `move_target_reorders_forward_and_back` (line 9155). `walk_swaps` is tested through a concrete list the way `moved` (line 8752) already tests `move_target`.

```rust
    /// Apply `walk_swaps` to a concrete list, with `indices` standing in for
    /// the absolute slots one workspace occupies inside the session vector.
    fn walked(items: &[&str], indices: &[usize], j: usize, position: usize) -> Vec<String> {
        let mut v: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        for (a, b) in walk_swaps(indices, j, position) {
            v.swap(a, b);
        }
        v
    }

    #[test]
    fn walk_swaps_moves_within_a_contiguous_workspace() {
        assert_eq!(walked(&["a", "b", "c"], &[0, 1, 2], 0, 2), vec!["b", "c", "a"]);
        assert_eq!(walked(&["a", "b", "c"], &[0, 1, 2], 2, 0), vec!["c", "a", "b"]);
    }

    #[test]
    fn walk_swaps_leaves_interleaved_workspaces_in_place() {
        // Slots 0 and 2 belong to one workspace, slot 1 to another; moving the
        // first workspace's second session to the front must not disturb it.
        assert_eq!(walked(&["a", "x", "b"], &[0, 2], 1, 0), vec!["b", "x", "a"]);
    }

    #[test]
    fn walk_swaps_is_empty_when_nothing_moves() {
        assert!(walk_swaps(&[0, 1, 2], 1, 1).is_empty());
        // A position past the end clamps to the last slot, which is a no-op
        // for the element already there.
        assert!(walk_swaps(&[0, 1, 2], 2, 9).is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree walk_swaps
```

Expected: FAIL to compile, `cannot find function walk_swaps`.

- [ ] **Step 3: Write `walk_swaps`**

In `app.rs`, directly after `move_target` (which ends at line 5725):

```rust
/// The neighbour swaps that walk the element at `indices[j]` to slot
/// `position` of `indices`.
///
/// `indices` are the absolute positions one workspace occupies inside the
/// session vector, which are not contiguous: swapping only across them keeps
/// every other workspace's sessions at the index they were at.  Swapping is
/// also what avoids a `Clone` bound on `Session`, which owns a PTY.
fn walk_swaps(indices: &[usize], j: usize, position: usize) -> Vec<(usize, usize)> {
    let mut swaps = Vec::new();
    if indices.is_empty() || j >= indices.len() {
        return swaps;
    }
    let position = position.min(indices.len() - 1);
    let mut j = j;
    while j > position {
        swaps.push((indices[j - 1], indices[j]));
        j -= 1;
    }
    while j < position {
        swaps.push((indices[j], indices[j + 1]));
        j += 1;
    }
    swaps
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree walk_swaps
```

Expected: PASS, three tests.

- [ ] **Step 5: Widen `move_session_to` to a `WorkspaceKey`**

Rename it and take the key directly, so the home workspace is expressible. At `app.rs:1452`, the signature and the two lines that wrapped the path become:

```rust
    /// Re-key `id` to `target`, repairing both workspaces' active-session
    /// entries and following the move with the view when the session was the
    /// one on screen.
    fn move_session_to_key(
        &mut self,
        id: SessionId,
        target: WorkspaceKey,
    ) -> Result<WorkspaceKey, String> {
        let idx = self
            .sessions
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| format!("no session with id {id} — see list_sessions"))?;
        if matches!(&self.sessions[idx].kind, SessionKind::Scratchpad { .. }) {
            return Err("scratchpads belong to their backing workspace and cannot be moved".into());
        }
        // A workspace's diff pane is found by workspace plus kind, so a pane
        // carried elsewhere becomes the one the next git click closes while
        // the workspace it left opens a second.
        if matches!(&self.sessions[idx].kind, SessionKind::Diff { .. }) {
            return Err("diff panes belong to the workspace they were opened from".into());
        }
        let source = self.sessions[idx].working_directory.clone();
```

Delete the `let target: WorkspaceKey = Some(target);` line that followed. The rest of the function is unchanged.

At the `Req::MoveSession` arm (`app.rs:8272`), wrap the path at the call site:

```rust
                let workspace = self.move_session_to_key(session_id, Some(target))?;
```

- [ ] **Step 6: Add the three application helpers**

In the same `impl AlacritreeApp` block, after `workspace_session_indices` (line 1543):

```rust
    /// The workspaces a reorder may use: those the app is willing to switch
    /// to, minus any whose delete is already running.  A session landing on a
    /// spinner row is a session that delete is about to reap.
    fn reorderable_workspaces(&self) -> Vec<WorkspaceKey> {
        self.workspace_order()
            .into_iter()
            .filter(|ws| match ws {
                None => true,
                Some(path) => !self.pending_deletes.iter().any(|t| t.worktree_path == *path),
            })
            .collect()
    }

    /// Walk `id` to `position` among its own workspace's sessions.
    fn reorder_session_within_workspace(&mut self, id: SessionId, position: usize) {
        let Some(abs) = self.sessions.iter().position(|s| s.id == id) else { return };
        let ws = self.sessions[abs].working_directory.clone();
        let indices = self.workspace_session_indices(&ws);
        let Some(j) = indices.iter().position(|i| *i == abs) else { return };
        for (a, b) in walk_swaps(&indices, j, position) {
            self.sessions.swap(a, b);
        }
    }

    /// Apply a decided move: change the workspace first when the target is a
    /// different one, then walk the session to its position there.  A refused
    /// workspace change leaves the session where it was.
    fn apply_session_move(&mut self, id: SessionId, target: StepTarget) {
        let Some(abs) = self.sessions.iter().position(|s| s.id == id) else { return };
        if self.sessions[abs].working_directory != target.workspace
            && self.move_session_to_key(id, target.workspace).is_err()
        {
            return;
        }
        self.reorder_session_within_workspace(id, target.position);
    }
```

Add `StepTarget` to `app.rs`'s `sidebar_nav` import list.

- [ ] **Step 7: Run the full suite**

```sh
cargo nextest run -p alacritree
```

Expected: PASS. The IPC `move_session` behaviour changes in one visible way: moving a diff pane between workspaces now returns an error instead of succeeding. That is the intended refusal.

- [ ] **Step 8: Commit**

```sh
git add alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(sessions): apply a reorder as a permutation of the session vector

A session's position is its index in the vector both the sidebar and
the tab strip filter, so a move is neighbour swaps across the absolute
slots one workspace occupies.  move_session_to now takes a WorkspaceKey
so the home workspace is a destination, and refuses to carry a diff
pane out of the workspace that opened it.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: The keyboard actions

**Files:**
- Modify: `alacritree/src/app.rs` (new methods near `reorderable_workspaces`; `dispatch_action` arms beside `ToggleSessionRows` at 2711)
- Test: `alacritree/src/app.rs` test module

**Interfaces:**
- Consumes: `move_range` and `step_target` (Tasks 2, 3), the action names (Task 4), the move primitives (Task 5).
- Produces: `fn reorder_subject(&self) -> Option<SessionId>` and `fn step_session(&mut self, delta: i32)`, plus working `MoveSessionUp` / `MoveSessionDown` bindings.

The three-case subject rule is the part worth testing without an app, so it is factored into a free function over the cursor and the two candidate ids.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn reorder_subject_prefers_the_cursored_session() {
        assert_eq!(
            reorder_subject(true, Some(&SidebarRow::Session(7)), || None, |_| None, || Some(3)),
            Some(7)
        );
    }

    #[test]
    fn reorder_subject_takes_a_workspace_rows_active_session() {
        // The landing after a cross-workspace step: the session paints no row
        // yet, so the cursor sits on the worktree it arrived in.
        let row = SidebarRow::Worktree(PathBuf::from("/b"));
        assert_eq!(
            reorder_subject(true, Some(&row), || None, |p| (p == Path::new("/b")).then_some(9), || Some(3)),
            Some(9)
        );
        assert_eq!(
            reorder_subject(true, Some(&SidebarRow::Home), || Some(4), |_| None, || Some(3)),
            Some(4)
        );
    }

    #[test]
    fn reorder_subject_falls_back_to_the_session_on_screen() {
        // Terminal focused: the cursor is ignored entirely.
        assert_eq!(
            reorder_subject(false, Some(&SidebarRow::Session(7)), || None, |_| None, || Some(3)),
            Some(3)
        );
        // Sidebar focused on a project header, which owns no session.
        let row = SidebarRow::Project(PathBuf::from("/a"));
        assert_eq!(reorder_subject(true, Some(&row), || None, |_| None, || Some(3)), Some(3));
        // And an empty workspace row falls through rather than refusing.
        let row = SidebarRow::Worktree(PathBuf::from("/b"));
        assert_eq!(reorder_subject(true, Some(&row), || None, |_| None, || Some(3)), Some(3));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree reorder_subject
```

Expected: FAIL to compile, `cannot find function reorder_subject`.

- [ ] **Step 3: Write the subject rule**

As a free function in `app.rs`, next to the other pure helpers around `move_target`:

```rust
/// The session a reorder key acts on.
///
/// A cursored session wins, then the workspace the cursor is resting on lends
/// its active session, and otherwise the session on screen moves.  The middle
/// case is what makes a held key work across a workspace boundary: a session
/// arriving alone in a workspace paints no row of its own, so the cursor
/// climbs to that workspace's row, and the next press must still find it.
///
/// `CloseSession` has the same first-and-last shape; `DeleteSelected` reads
/// the cursor whatever has focus, which is the wrong convention here — a key
/// pressed at the terminal should move the terminal you are looking at.
fn reorder_subject(
    sidebar_focused: bool,
    cursor: Option<&SidebarRow>,
    home_active: impl Fn() -> Option<SessionId>,
    worktree_active: impl Fn(&Path) -> Option<SessionId>,
    on_screen: impl Fn() -> Option<SessionId>,
) -> Option<SessionId> {
    if sidebar_focused {
        match cursor {
            Some(SidebarRow::Session(id)) => return Some(*id),
            Some(SidebarRow::Home) => {
                if let Some(id) = home_active() {
                    return Some(id);
                }
            },
            Some(SidebarRow::Worktree(path)) => {
                if let Some(id) = worktree_active(path) {
                    return Some(id);
                }
            },
            _ => {},
        }
    }
    on_screen()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree reorder_subject
```

Expected: PASS, three tests.

- [ ] **Step 5: Write the step method and the dispatch arms**

In `impl AlacritreeApp`, after `apply_session_move`:

```rust
    /// One `MoveSessionUp` / `MoveSessionDown` press.  Every refusal is a
    /// silent no-op: a clamped end, a boundary the scope forbids, a scratchpad
    /// asked to leave its workspace.  None of those is a failure — each is a
    /// move with nowhere to go.
    fn step_session(&mut self, delta: i32) {
        let sidebar_focused = self.focus == PaneFocus::ProjectsSidebar;
        let Some(id) = reorder_subject(
            sidebar_focused,
            self.sidebar_cursor.as_ref(),
            || self.active_session.get(&None).copied(),
            |path| self.active_session.get(&Some(path.to_path_buf())).copied(),
            || self.active_session_index().map(|idx| self.sessions[idx].id),
        ) else {
            return;
        };
        let Some(abs) = self.sessions.iter().position(|s| s.id == id) else { return };
        let origin = self.sessions[abs].working_directory.clone();
        let range = sidebar_nav::move_range(
            &self.projects,
            &self.reorderable_workspaces(),
            &origin,
            self.config.ui.session_reorder.scope,
        );
        let lens: Vec<usize> =
            range.iter().map(|ws| self.workspace_session_indices(ws).len()).collect();
        let indices = self.workspace_session_indices(&origin);
        let Some(index) = indices.iter().position(|i| *i == abs) else { return };
        let Some(target) = sidebar_nav::step_target(&range, &lens, &origin, index, delta) else {
            return;
        };
        let landed_in = target.workspace.clone();
        self.apply_session_move(id, target);
        if sidebar_focused {
            self.follow_moved_session(id, &landed_in);
        }
    }

    /// Keep the sidebar pointed at the session a key just moved.
    ///
    /// The cursor key is unchanged across a move inside one workspace, so
    /// neither `set_sidebar_cursor` nor the focus reconciler would notice the
    /// row moved and scroll after it — this sets the one-shot itself.  A
    /// landing inside a collapsed project expands it, because a cursor with no
    /// painted row is the state the reconciler treats as a row that went away.
    fn follow_moved_session(&mut self, id: SessionId, landed_in: &WorkspaceKey) {
        self.sidebar_cursor = Some(SidebarRow::Session(id));
        self.sidebar_cursor_moved = true;
        let Some(path) = landed_in.as_deref() else { return };
        let root = self
            .projects
            .iter()
            .find(|p| p.worktrees.iter().any(|w| w.path == path))
            .map(|p| p.root.clone());
        if let Some(root) = root {
            self.set_project_expanded(&root, true);
        }
    }
```

In `dispatch_action`, after the `ToggleSessionTabs` arm (line 2718):

```rust
            BindingAction::Named(NamedAction::MoveSessionUp) => self.step_session(-1),
            BindingAction::Named(NamedAction::MoveSessionDown) => self.step_session(1),
```

- [ ] **Step 6: Run the full suite**

```sh
cargo nextest run -p alacritree
```

Expected: PASS.

- [ ] **Step 7: Check it by hand**

```sh
cargo run -p alacritree
```

Bind a key temporarily in `alacritree.toml` (remove it afterwards — this feature ships with no default bindings):

```toml
[[keyboard.bindings]]
key = "Up"
mods = "Control|Shift"
action = "MoveSessionUp"

[[keyboard.bindings]]
key = "Down"
mods = "Control|Shift"
action = "MoveSessionDown"

[ui.session_reorder]
scope = "anywhere"
```

Confirm, with three or more sessions open across two workspaces: the sidebar row and the tab-strip segment move together; a held key walks a session from the end of one workspace into the next and keeps moving on the following press; the terminal keeps showing the session you moved when it was the one on screen; and with `scope` back at its `"workspace"` default the session stops at both ends of its own workspace.

- [ ] **Step 8: Commit**

```sh
git add alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(sessions): move the selected session with a key

The sidebar cursor picks the session when it has focus, falling back to
the cursored workspace's active session so a held key keeps hold of one
that just landed alone in an empty workspace and paints no row yet.
From the terminal the session on screen moves instead.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Draggable session rows

**Files:**
- Modify: `alacritree/src/app.rs` (`AlacritreeApp` field beside `session_tabs_always` at 469 and its initializer; `DraggedSession` beside `DraggedProject` at 669; `SessionRowAction` at 6845; `session_row` at 6850; the two call sites at 3511 and 3876; `dispatch_action` beside `ToggleSessionTabs` at 2718)

**Interfaces:**
- Consumes: `NamedAction::ToggleSessionDrag` (Task 4), `config.ui.session_reorder.drag` (Task 1).
- Produces: `struct DraggedSession(SessionId)`, the `session_drag: bool` runtime field, and `SessionRowAction { activate, close, rect }`. Task 8 consumes `rect` and the payload.

- [ ] **Step 1: Add the runtime flag**

In `AlacritreeApp`, after `session_tabs_always: bool,` (line 469):

```rust
    /// Runtime copy of `[ui.session_reorder] drag`.  Like the display toggles
    /// above, the config is only the startup default and nothing is persisted.
    session_drag: bool,
```

In the constructor, beside where `session_rows_always` and `session_tabs_always` are seeded from the config:

```rust
            session_drag: config.ui.session_reorder.drag,
```

In `dispatch_action`, after the two `MoveSession*` arms from Task 6:

```rust
            BindingAction::Named(NamedAction::ToggleSessionDrag) => {
                self.session_drag = !self.session_drag;
            },
```

- [ ] **Step 2: Add the payload type**

In `app.rs`, directly after `DraggedProject` (line 669):

```rust
/// Drag-and-drop payload for reordering sessions.  Carries the id rather than
/// a position so a spawn, close or reorder mid-drag can't retarget the drop.
#[derive(Clone)]
struct DraggedSession(SessionId);
```

- [ ] **Step 3: Make the row draggable**

`SessionRowAction` (line 6845) gains the row's rect, which Task 8 hit-tests against:

```rust
struct SessionRowAction {
    activate: bool,
    close: bool,
    /// Full-width row rect, for a drop target to test the pointer against.
    rect: egui::Rect,
}
```

`session_row` (line 6850) gains a `draggable` parameter after `scroll_into_view`:

```rust
fn session_row(
    ui: &mut egui::Ui,
    row: &SessionRowData,
    is_cursor: bool,
    scroll_into_view: bool,
    draggable: bool,
    icons: &Icons,
    theme: &Theme,
) -> SessionRowAction {
```

Its `.interact(egui::Sense::click())` (line 6912) becomes:

```rust
        .interact(if draggable {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        });
```

egui postpones the click-versus-drag decision and still reports `clicked()` on a release that never moved, so activating on click needs no extra handling. The one behaviour change is that a press held past egui's `max_click_duration` stops counting as a click.

At the end of the function, replace the `SessionRowAction { .. }` construction (line 6949):

```rust
    if draggable {
        resp.dnd_set_drag_payload(DraggedSession(row.id));
    }
    SessionRowAction {
        activate: resp.clicked() && !close_clicked,
        close: close_clicked,
        rect: full_rect,
    }
```

- [ ] **Step 4: Pass the flag at both call sites**

Before the panel closure, next to the other per-frame snapshots (around line 3231):

```rust
        let session_drag = self.session_drag;
```

In the home session loop (line 3517) and the worktree session loop (line 3877), pass `session_drag` as the new argument:

```rust
                            let act = session_row(ui, row, is_cursor, scroll, session_drag, &icons, &theme);
```

- [ ] **Step 5: Build and run the suite**

```sh
cargo nextest run -p alacritree && cargo fmt --check
```

Expected: PASS. `cargo fmt --check` failing means run `cargo fmt`.

- [ ] **Step 6: Check it by hand**

```sh
cargo run -p alacritree
```

With `[ui.session_reorder] drag = true`, confirm a session row still activates on a plain click and now shows a drag in progress when you press and move. Nothing lands yet — Task 8 adds the targets. With `drag` unset, confirm the row behaves exactly as it does today.

- [ ] **Step 7: Commit**

```sh
git add alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(sidebar): let session rows be dragged

The whole row drags rather than a grip: a project row's own controls
are what a click is usually for, but a session row is a tab, and tabs
drag.  Gated behind [ui.session_reorder] drag, with ToggleSessionDrag
flipping the runtime copy.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Drop targets and the shared indicator

**Files:**
- Modify: `alacritree/src/app.rs` (new `draw_drop_indicator` helper beside `move_target` at 5716; the project drop site at 3683; `HomeAction` at 6078 and `home_row` at 5982; `WorktreeAction` at 6080 and `worktree_row` at 6611; both session loops at 3511 and 3877; new request `Cell` beside the others at 3231; the drain after the closure)
- Test: `alacritree/src/app.rs` test module

**Interfaces:**
- Consumes: `DraggedSession` and `SessionRowAction::rect` (Task 7), `move_range` (Task 2), `move_target` and `apply_session_move` (Task 5).
- Produces: `fn apply_session_drop(&mut self, id: SessionId, workspace: WorkspaceKey, insert_before: usize)`.

- [ ] **Step 1: Write the failing test**

The drop's own arithmetic is `move_target`, already tested. What is new is that a same-workspace drop reuses it rather than re-deriving it, and that a cross-workspace drop does not. Test the branch:

```rust
    #[test]
    fn a_same_workspace_drop_uses_the_move_target_arithmetic() {
        // Dropping below your own row is a no-op, the same as for projects.
        assert_eq!(move_target(3, 1, 2), None);
        // Dropping onto the row below moves you past it.
        assert_eq!(moved(&["a", "b", "c"], 1, 3), vec!["a", "c", "b"]);
    }

    #[test]
    fn a_cross_workspace_drop_lands_at_the_stated_position() {
        // Arriving from another workspace, the display slot is the position:
        // nothing was removed from this list first, so there is no off-by-one.
        assert_eq!(walk_swaps(&[0, 1, 2], 2, 0), vec![(1, 2), (0, 1)]);
    }
```

- [ ] **Step 2: Run the tests to verify they compile and pass**

```sh
cargo nextest run -p alacritree drop
```

Expected: PASS. These two pin the intent before the wiring exists; they exercise functions Task 5 already delivered.

- [ ] **Step 3: Extract the shared indicator**

In `app.rs`, after `move_target` (line 5725):

```rust
/// Paint the line a reorder drop would land on, at the row edge nearest the
/// pointer, and report whether that edge is the top — which is what "insert
/// before this row" means for both the project and the session drag.
fn draw_drop_indicator(
    ui: &egui::Ui,
    row_rect: egui::Rect,
    pointer: egui::Pos2,
    theme: &Theme,
) -> bool {
    let before = pointer.y < row_rect.center().y;
    let y = if before { row_rect.top() } else { row_rect.bottom() };
    ui.painter().hline(row_rect.x_range(), y, Stroke::new(2.0 * theme.ui_scale, theme.accent));
    before
}
```

Rewrite the project drop site (lines 3694-3701) to call it, leaving the surrounding payload check as it is:

```rust
                                let before = draw_drop_indicator(ui, row_rect, pointer, &theme);
                                if ui.input(|i| i.pointer.any_released()) {
                                    let insert_before = if before { idx } else { idx + 1 };
                                    reorder_request.set(Some((dragged.0.clone(), insert_before)));
                                    egui::DragAndDrop::clear_payload(ui.ctx());
                                }
```

The 2px weight is unchanged: a heavier line would restyle the existing project drag for a user who asked for nothing.

- [ ] **Step 4: Give the workspace rows their rects**

`HomeAction` (line 6078) and `WorktreeAction` (line 6080) each gain the same field `SessionRowAction` got in Task 7:

```rust
    /// Full-width row rect, for a drop target to test the pointer against.
    rect: egui::Rect,
```

Both `home_row` (line 5982) and `worktree_row` (line 6611) already compute a `full_rect` for their cursor outline and scroll-into-view. Return it: `HomeAction { activate: .., spawn: .., rect: full_rect }` and the same field on `WorktreeAction`'s construction.

- [ ] **Step 5: Snapshot the drag's range before the closure**

Next to the other per-frame snapshots (around line 3231), after the `session_drag` line from Task 7:

```rust
        // The render pass cannot borrow `self.sessions`, so the dragged
        // session's own scope is resolved here: a row outside this range draws
        // no indicator and never becomes a drop.
        let drag_range: Option<(SessionId, Vec<WorkspaceKey>)> =
            egui::DragAndDrop::payload::<DraggedSession>(ctx).and_then(|dragged| {
                let idx = self.sessions.iter().position(|s| s.id == dragged.0)?;
                let origin = self.sessions[idx].working_directory.clone();
                Some((
                    dragged.0,
                    sidebar_nav::move_range(
                        &self.projects,
                        &self.reorderable_workspaces(),
                        &origin,
                        self.config.ui.session_reorder.scope,
                    ),
                ))
            });
        let session_drop_request: std::cell::Cell<Option<(SessionId, WorkspaceKey, usize)>> =
            std::cell::Cell::new(None);
```

- [ ] **Step 6: Add the drop sites**

A closure beside the loops keeps the four sites from drifting. Define it just inside the panel closure, before the scroll area:

```rust
        let session_drop = |ui: &egui::Ui, row_rect: egui::Rect, ws: &WorkspaceKey, slot: Option<usize>| {
            let Some((dragged, range)) = drag_range.as_ref() else { return };
            if !range.contains(ws) {
                return;
            }
            let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else { return };
            if !row_rect.contains(pointer) {
                return;
            }
            let position = match slot {
                // A session row: the half the pointer is in decides.
                Some(idx) => {
                    if draw_drop_indicator(ui, row_rect, pointer, &theme) { idx } else { idx + 1 }
                },
                // A workspace row: its sessions start under it, so a drop
                // here means the front of that workspace.  This is the only
                // way to reach a workspace listing no session rows — an empty
                // one, or a single-session one below the display threshold.
                None => {
                    ui.painter().hline(
                        row_rect.x_range(),
                        row_rect.bottom(),
                        Stroke::new(2.0 * theme.ui_scale, theme.accent),
                    );
                    0
                },
            };
            if ui.input(|i| i.pointer.any_released()) {
                session_drop_request.set(Some((*dragged, ws.clone(), position)));
                egui::DragAndDrop::clear_payload(ui.ctx());
            }
        };
```

Call it at four places:

- After the `home_row` call, with the home workspace: `session_drop(ui, home_action.rect, &None, None);`
- Inside the home session loop, after `session_row`: `session_drop(ui, act.rect, &None, Some(display_idx));` where `display_idx` is the loop's index — change the loop to `for (display_idx, row) in home_session_rows.iter().enumerate()`.
- After the `worktree_row` call: `session_drop(ui, action.rect, &Some(wt.path.clone()), None);`
- Inside the worktree session loop, after `session_row`: `session_drop(ui, act.rect, &Some(wt.path.clone()), Some(display_idx));` with the same `enumerate()` change.

A session dragged onto its own row is not excluded here: `move_target` already resolves both "drop on yourself" and "drop just below yourself" to `None`.

- [ ] **Step 7: Drain the request**

After the panel closure, beside where `reorder_request` is drained:

```rust
        if let Some((id, workspace, position)) = session_drop_request.take() {
            self.apply_session_drop(id, workspace, position);
        }
```

And the method itself, in `impl AlacritreeApp` after `apply_session_move`:

```rust
    /// Apply a mouse drop.  A drop inside the session's own workspace is the
    /// same remove-then-insert the project rows do, so it goes through
    /// `move_target`; a drop from elsewhere inserts into a list the session is
    /// not in yet, where the display slot is already the position.
    fn apply_session_drop(&mut self, id: SessionId, workspace: WorkspaceKey, insert_before: usize) {
        let Some(abs) = self.sessions.iter().position(|s| s.id == id) else { return };
        if self.sessions[abs].working_directory == workspace {
            let indices = self.workspace_session_indices(&workspace);
            let Some(from) = indices.iter().position(|i| *i == abs) else { return };
            let Some(to) = move_target(indices.len(), from, insert_before) else { return };
            self.reorder_session_within_workspace(id, to);
        } else {
            self.apply_session_move(id, StepTarget { workspace, position: insert_before });
        }
    }
```

- [ ] **Step 8: Run the suite and the formatter**

```sh
cargo nextest run -p alacritree && cargo fmt --check && cargo clippy -p alacritree
```

Expected: PASS, no warnings introduced.

- [ ] **Step 9: Check it by hand**

```sh
cargo run -p alacritree
```

With `[ui.session_reorder] drag = true`:

- Default `scope`: dragging a session over rows of its own workspace draws the line and reorders on release; dragging over another workspace's rows draws nothing and drops nowhere.
- `scope = "anywhere"`: dragging onto a worktree row moves the session into that worktree at the front, including a worktree with no sessions at all and one whose single session paints no row.
- Dragging onto the Home row moves a session back to the home workspace.
- A worktree row with a delete spinner draws no indicator.
- A plain click on a session row still activates it.

- [ ] **Step 10: Commit**

```sh
git add alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(sidebar): drop a dragged session onto a row to place it

A session row's near half decides before or after; a worktree or Home
row means the front of that workspace, which is the only way to reach
one that lists no session rows.  A row outside the drag's scope draws
no indicator, so a refusal is visible while the button is still down.
The insertion line is now one helper shared with the project drag.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Ship it

**Files:**
- No source changes. This task opens the PR.

**Interfaces:**
- Consumes: every earlier task.
- Produces: an open PR against `mathix420/alacritree` and an updated `all-features` branch.

- [ ] **Step 1: Confirm the base has not moved out from under the branch**

Substitute the base branch from Task 0:

```sh
git -C ../alacritree-worktrees/feat/session-reorder fetch origin
git -C ../alacritree-worktrees/feat/session-reorder merge-base --is-ancestor origin/fix/wsl-helper-liveness HEAD
```

If that fails, the base rebased under you. Replay onto it:

```sh
git -C ../alacritree-worktrees/feat/session-reorder rebase --onto origin/<base> <recorded base> feat/session-reorder
```

- [ ] **Step 2: Run the full suite one more time**

```sh
cargo nextest run -p alacritree && cargo fmt --check && cargo clippy -p alacritree
```

Expected: PASS.

- [ ] **Step 3: Write the two body halves**

Write the human TL;DR and the summary to files rather than passing them inline — either can run long. Both are soft-wrapped: one line per paragraph, no hard column wrap.

- [ ] **Step 4: Open the PR**

`--arg stacked_on` takes the PR number from Task 0 and `[n]` is that PR's marker plus one. Never pass `--to`.

```sh
devkit issue pr create --ready \
  --pr-title 'feat(sidebar): reorder sessions by drag or by key [9]' \
  --pr-body "$(cat summary.md)" \
  --arg tldr="$(cat tldr.md)" \
  --arg stacked_on=210
```

The first `Closes` line comes from the worktree's own issue (#20), so `--arg closes` is not needed.

- [ ] **Step 5: Merge into all-features and install**

```sh
python3 install.local.py
```

Requesting review is the user's to run, not yours. Stop here.

---

## Self-review

**Spec coverage.** Section 1 (the ordering model) is Task 2; the step rule is Task 3; section 2 (applying a move) is Task 5; section 3 (keyboard) is Task 6; section 4 (mouse) is Tasks 7 and 8; section 5 (config and the toggle) is Task 1 plus the `ToggleSessionDrag` arm in Task 7; section 6 (actions) is Task 4, including `docs/keyboard-shortcuts.md`; section 7 (refusals) is spread across the tasks that own each one — clamped ends in Task 3, the scratchpad and diff-pane refusals in Task 5, the out-of-range drop in Task 8; section 8 (testing) is each task's own test step; section 9's file table matches the files these tasks touch.

**Not covered by any task, deliberately.** The spec's "what the steady-state assertion does with this" section asserts nothing new is needed, so no task implements it. If `steady_state.rs` goes red during Task 7 or 8, that analysis was wrong — stop and report rather than relaxing the assertion.

**Type consistency.** `ReorderScope` is defined once, in `config.rs` (Task 1), and imported by `sidebar_nav.rs` (Task 2) and read through `config.ui.session_reorder.scope` (Tasks 6, 8). `StepTarget` is defined in `sidebar_nav.rs` (Task 3) and consumed by `apply_session_move` and `apply_session_drop` (Tasks 5, 8). `WorkspaceKey` is `Option<PathBuf>` throughout; `move_session_to_key` takes it directly (Task 5), which is what makes Home a destination in Task 8. The `rect` field added to `SessionRowAction` (Task 7) and to `HomeAction` / `WorktreeAction` (Task 8) carries the same meaning and the same name in all three.

## Unresolved questions

None blocking. Four decisions in the spec were made during its review rather than with the user, and this plan implements them as written: the keyboard target's workspace-row case, the two `move_range` filters, the diff-pane refusal, and expanding a project through the persisting setter. Any of them can be changed before Task 6 without disturbing Tasks 1 through 5.
