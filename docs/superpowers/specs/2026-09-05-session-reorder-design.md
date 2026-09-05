# Session reorder design

**Goal:** a session's position in the sidebar and the tab strip is something the user sets, by dragging it with the mouse or by moving it with a key, and how far it may travel is a config decision rather than a hardcoded one.

**Issue:** [#20](https://github.com/AbysmalBiscuit/alacritree/issues/20).

**Branch:** `feat/session-reorder`. Cut from the open PR carrying the highest `[n]` marker, which was PR 210 (`fix/wsl-helper-liveness`, marker `[8]`) when this was written. Read the tip fresh at setup time rather than trusting that number.

**Platform:** all. Nothing here touches WSL, conpty or any per-platform path.

**Config:** two new keys under a new `[ui.session_reorder]` table, both defaulting to today's behaviour. `drag` is off, so a user who changes nothing sees the sidebar exactly as it is now.

## Context

A session's position is not stored anywhere. It is the order of `AlacritreeApp::sessions`, a plain `Vec<Session>` that spawning pushes onto, and both places a session appears read that order through a filter:

- `sidebar_session_ids` (`app.rs:6108`) keeps the pairs whose workspace matches, then hides the row entirely below a threshold of two, which `[ui.session_display] sidebar_always` lowers to one.
- `current_session_indices` (`app.rs:1552`) feeds `show_tab_strip` (`app.rs:3115`), which hides itself under the same rule.

So one vector decides both, and reordering it moves the sidebar rows and the tab-strip segments together with no second structure to keep in sync. No order is persisted: sessions are PTYs and die with the process, so `state.toml` holds nothing about them.

Everything else that reads the vector recomputes from it per call rather than caching a position: `SelectTab` (`app.rs:3094`), `select_last_tab`, the session ring (`app.rs:1596`) and `close_landing` (`app.rs:6274`). One visible consequence is intended rather than incidental: `close_landing` under `[ui] sidebar_focus = "preserve"` hands a workspace its *first* session after a close, so reordering changes which session the workspace falls back to.

The project list already has drag-to-reorder and is the model to follow. `DraggedProject` (`app.rs:669`) carries the dragged project's root; `drag_handle` (`app.rs:5729`) is a `Sense::drag()` grip that only appears while `reorder_mode` (`app.rs:475`) is on; the drop site (`app.rs:3683`) reads the raw payload rather than wrapping the row in a `dnd_drop_zone`, so no extra hover-sensing rect steals the row buttons' highlight; and `move_target` (`app.rs:5716`) isolates the off-by-one that removing before inserting creates, tested without an app at `app.rs:9156`.

Sessions have none of that today. The only way to move one is `alacritree session move`, which reaches `move_session_to` (`app.rs:1452`) over IPC. That is a *workspace* change, not a position change, and it is what issue #28 is about.

### Why Windows Terminal is not an implementation reference

`TabRowControl.xaml:33` sets `CanReorderTabs="True"` on WinUI's `TabView` and stops there. The insertion indicator is drawn by the framework. Windows Terminal's own drag code (`TerminalPage::_onTabDragStarting`, `_onTabDroppedOutside`, `TerminalPage.cpp:5983` onward) exists to tear a tab out into another window, which is not in scope here. The behaviour to match is the visible insertion line; there is no code to mirror.

## 1. The ordering model

A pure function set in `sidebar_nav.rs`, beside `visible_rows` (`sidebar_nav.rs:42`), which already defines sidebar order as Home, then each project's worktrees in project order.

```rust
pub enum ReorderScope { Workspace, Project, Anywhere }

/// Workspaces a session living in `origin` may move through, in sidebar order.
///
/// `order` is the caller's live workspace list — `workspace_order()` with the
/// pending deletes removed.
pub fn move_range(
    projects: &[Project],
    order: &[WorkspaceKey],
    origin: &WorkspaceKey,
    scope: ReorderScope,
) -> Vec<WorkspaceKey>
```

`Workspace` returns just `origin`. `Project` returns the entries of `order` belonging to the project that owns `origin`; Home belongs to no project, so under this scope Home is a range of one. `Anywhere` returns `order` unchanged.

**The range is a filter over `workspace_order()` (`app.rs:1621`), never over `projects` directly.** That list is already the set of workspaces the app is willing to switch to: `worktree_is_switchable` (`app.rs:6841`) drops a worktree whose directory is gone unless sessions still live there, matching the refusals at `app.rs:1287` and `app.rs:1124`. A range built from raw worktrees would let a session land somewhere `spawn` refuses to spawn and `SelectNextWorkspace` cannot reach, leaving it addressable only over IPC.

**Workspaces with a delete in flight are removed on top of that.** `workspace_order` does not consult `pending_deletes` (`app.rs:529`), but the delete's completion `retain`s away every session keyed to that worktree (`app.rs:7597`), so a session landing on a spinner row is a session about to be killed. Same list, same reason, for the mouse drop targets in section 4.

**`origin` stays in the range even when those filters would drop it.** A session whose project was removed is `Parent::Detached` (`sidebar_focus.rs:24`) and appears in no `workspace_order`; a session in a worktree being deleted is on a filtered row. Neither may be barred from moving *within* its own workspace, and neither has a position in a list it is not in, so the rule is one sentence: a range that does not contain `origin` collapses to `[origin]`. A detached session therefore reorders inside its own workspace under every scope and crosses nothing, and so does a session in a worktree whose delete is already running.

**The range ignores expansion.** `visible_rows` hides a collapsed project's worktrees, and `move_range` must not, or the set of destinations would depend on which projects happen to be open. Section 3 says what a keyboard move does when it lands somewhere collapsed.

**The range is built from live session pairs, not `ListedSessions`.** A workspace holding exactly one session paints no session row, because of the threshold in `sidebar_session_ids`, but it still owns a session that can be moved and is still a place another session can land. Feeding the cursor model's `ListedSessions` in here would make both of those impossible.

**A path listed by two projects** is possible, as `worktree_gone` (`app.rs:1505`) notes. Under `Project` scope the first project in sidebar order whose worktree list contains `origin` owns it. Nothing better is available: the session records a directory, not a project.

### The step

For a session at position `i` of workspace `w`, where `w` sits at index `k` of `move_range`:

- **Up, `i > 0`:** swap with `i - 1`. Stays in `w`.
- **Up, `i == 0`:** land at the end of the workspace at `k - 1`. No `k - 1` means no-op.
- **Down, `i < len(w) - 1`:** swap with `i + 1`. Stays in `w`.
- **Down, `i == len(w) - 1`:** land at position 0 of the workspace at `k + 1`. No `k + 1` means no-op.

Clamped at both ends; nothing wraps. An empty workspace has `len == 0`, so both landing rules resolve to position 0 and it needs no case of its own. Under `Workspace` scope the range has one entry, so both boundary rules are no-ops and the whole thing degenerates to a swap.

## 2. Applying a move

Two primitives over `self.sessions`.

**Within a workspace** the move is a permutation confined to the absolute indices that workspace already occupies, which `workspace_session_indices` (`app.rs:1536`) returns. Walking the element to its new position swaps `indices[j]` with `indices[j + 1]`, not vector-adjacent elements, so every other workspace's sessions keep their own absolute indices. Swapping is what avoids a `Clone` bound on `Session`, which does not have one and should not grow one: it owns a PTY.

A same-workspace mouse drop computes its destination with `move_target` (`app.rs:5716`) rather than re-deriving the remove-then-insert off-by-one beside the function that already isolates it and is already tested.

**Across workspaces** the session's `working_directory` changes, which is what `move_session_to` (`app.rs:1452`) already does, including the active-session bookkeeping that `plan_move` (`app.rs:6359`) decides: repairing the source workspace's active entry, claiming the target's, and following with `current_workspace` when the moved session was the one on screen. That last one is what keeps the terminal showing a session the user just moved out from under it.

`move_session_to` takes a `PathBuf` and wraps it as `Some(path)`, so it cannot express the home workspace. It becomes `move_session_to_key(id, target: WorkspaceKey)`, with the `IpcRequest::MoveSession` arm (`app.rs:8269`) wrapping its path as before. Nothing else about it changes.

The two compose: change the workspace first, then run the within-workspace permutation against the target's absolute indices, which by then include the moved session.

## 3. Keyboard: `MoveSessionUp` / `MoveSessionDown`

**Which session moves,** resolved in this order:

1. Sidebar focused, cursor on a `SidebarRow::Session` — that session.
2. Sidebar focused, cursor on a `Home` or `Worktree` row — that workspace's `active_session`, if it has one.
3. Otherwise — the session on screen in the current workspace.

This is the shape `CloseSession` (`app.rs:2732`) already has, extended by case 2. `DeleteSelected` (`app.rs:2758`) is the other convention, reading `sidebar_cursor` whatever has focus, and it is the wrong one here: a reorder key pressed at the terminal should act on the terminal you are looking at, not on a cursor left somewhere else three actions ago.

**Case 2 is what makes a held key work across a boundary,** and without it the design is broken at its most common landing. `sidebar_session_ids` paints no session rows below a threshold of two, so a session moving into an empty workspace lands somewhere with no session row at all; the reconciler climbs the cursor to the parent `Worktree` row (`sidebar_focus.rs:235`), and on the next press case 1 no longer matches. Falling through to case 3 would then move an unrelated session in a different workspace. Case 2 lands back on the session that just arrived, because `plan_move` sets `claim_target` when the target has no active session (`app.rs:6370`), making the moved session that workspace's active one.

**The cursor follows the session** when the sidebar has focus, staying the same `SidebarRow::Session(id)` across a move within a workspace, so a held key walks the session down the list rather than the two parting company on the first press. A move made from the terminal leaves the sidebar cursor where it was: nothing over there was being pointed at.

**The action sets `sidebar_cursor_moved` itself.** Both places that normally set it compare rows and find them equal here: `set_sidebar_cursor` (`app.rs:2404`) and the reconciler (`app.rs:2266`) each check `cursor != previous`, and the cursor's *key* is unchanged even though its row moved. Without the explicit flag a held key walks the session out of the `ScrollArea`'s visible range and nothing scrolls after it.

**A move into a collapsed project expands it,** through `set_project_expanded` (`app.rs:2573`), on the same condition: the sidebar has focus and its cursor is what needs somewhere to land. The cursor follows the session, and a cursor with no painted row is exactly the unprojected state the focus reconciler treats as a row that has gone away. Expanding is also the honest answer to what just happened: the session is over there now. That setter writes the expansion flag to `state.toml`, which is the existing behaviour of every other expand and the reason to call it rather than flipping `p.expanded` in place: an in-memory flip would leave the persisted flag disagreeing with the screen until the next toggle. Session order is still persisted nowhere.

No default key binding for either. Nor for the toggle in section 5. Binding keys that currently reach the PTY would change behaviour for a user who asked for nothing.

## 4. Mouse: dragging a session row

**A `DraggedSession(SessionId)` payload** beside `DraggedProject`.

**The whole row drags,** with no grip. Projects use a grip because a project row's own controls are what a click is usually for, and because `reorder_mode` is a rare deliberate act; a session row is a tab, and tabs drag.

The row's sense becomes `click_and_drag`. egui postpones the click-versus-drag decision and still reports `clicked()` on a release that never moved, so activating the session on click needs no special handling; the one behaviour change is egui's `max_click_duration`, which stops calling a press a click once it is held past 0.8s. A press-and-hold that never moves therefore selects nothing, which is the same thing every draggable tab strip does. The separate hazard `truncating_label` (`app.rs:5330`) documents is about the row's *children*: a selectable label unions drag into its own sense and takes the click the row was waiting for, so the labels inside a draggable row stay non-selectable.

**Drop targets are painted rows only,** which is what confines the mouse to expanded projects while the keyboard is not:

| Row dropped on | Means |
| --- | --- |
| A session row, upper half | Insert before that session |
| A session row, lower half | Insert after that session |
| A worktree row | Position 0 of that worktree |
| The Home row | Position 0 of the home workspace |

The worktree and Home rows are how a session reaches a workspace that lists no session rows: an empty one, and equally a one-session one hidden by the display threshold. Without them those workspaces would be unreachable by mouse under any scope.

**A row outside the range is not a target,** which is one rule covering three cases: a workspace the scope forbids, a worktree the liveness filter dropped, and a worktree with a delete in flight. No indicator is drawn over any of them, so the refusal is visible while the button is still down rather than as nothing happening after release. The dragged session's workspace and its range are resolved into a snapshot before the panel closure, next to the other per-frame snapshots at `app.rs:3231`, since the render pass cannot borrow `self.sessions`.

**The indicator** is a horizontal line at the row edge nearest the pointer, in `theme.accent`, scaled by `theme.ui_scale`, at the 2px the projects code already draws inline (`app.rs:3699`). That inline draw becomes one shared helper called from both sites so the two cannot drift. The weight stays 2px: changing it would restyle the existing project drag for a user who asked for nothing.

**Drop sites live at the two call sites,** the home session loop (`app.rs:3510`) and the worktree session loop (`app.rs:3868`), not inside `session_row` (`app.rs:6850`). Both loops already know the workspace and the display index; pushing that knowledge into `session_row` would mean handing it the payload type and the drag state for nothing. `session_row` grows one `bool` for whether the row senses drag.

## 5. Config and the toggle action

```toml
[ui.session_reorder]
drag = false
scope = "workspace"
```

`drag` is a startup default that the app copies into runtime state, the way `[ui.session_display]` already describes itself (`config.rs:695`). `ToggleSessionDrag` flips the runtime copy, mirroring `ToggleSessionRows` and `ToggleSessionTabs` (`app.rs:2711`). Neither key is written back.

`scope` is `"workspace"` (default), `"project"` or `"anywhere"`. It follows the `SearchScope` shape (`config.rs:644`): a plain Rust enum, an `Option<String>` on the raw struct, and a `parse_*` function that warns and falls back on an unknown value rather than failing the launch. `ReorderScope` lives in `config.rs` beside the other config enums and `sidebar_nav.rs` imports it, the direction `SidebarFocus` (`config.rs:612`) already established.

Doc comments on `RawSessionReorder` are the JSON Schema's hover text, so `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`, which fails the build while the schema is stale.

`scope` governs the keyboard and the mouse identically. One rule, read from one place, is why section 1 puts the range in a function both paths call.

## 6. Actions

Three variants on `NamedAction` (`bindings.rs:53`): `ToggleSessionDrag`, `MoveSessionUp`, `MoveSessionDown`. Each needs a parse arm, a `description()` arm, an entry in `bindable_actions()` (`command_palette.rs:259`, whose array length goes from 63 to 66) and an arm in `section_of` (`command_palette.rs:75`), where all three belong to `PaletteSection::Sessions`.

That single enum is what every caller reads. Key bindings, the Ctrl+K palette, `alacritree action MoveSessionUp` over IPC, and the MCP `run_action` tool all go through it, so none of them needs code of its own. `docs/keyboard-shortcuts.md` is the exception: it lists actions by hand, grouped by area, and the three belong beside `ToggleSessionRows` / `ToggleSessionTabs` in its sidebar section.

## 7. What is refused, and how

Every refusal is a silent no-op. There is no dialog and no error toast, because none of these is a failure — each is a move that had nowhere to go.

- The first session moving up, or the last moving down, at the end of its range.
- Any boundary crossing under `scope = "workspace"`.
- A scratchpad at a boundary. `move_session_to` already refuses to move one out of its workspace (`app.rs:1458`), and that refusal stands; a scratchpad reorders freely *inside* its workspace, since that changes no ownership.
- A diff pane at a boundary, refused the same way and for the same reason. `open_diff` finds a workspace's pane by workspace plus `SessionKind::Diff` and `retain`s away whatever it finds when a different diff is requested (`app.rs:4416`), so a pane carried into another workspace becomes the one the next git-row click closes, while the workspace it left opens a second. Inside its own workspace it reorders like any other row.
- A drag onto a row outside the range, which draws no indicator and so never becomes a drop at all.

## 8. Testing

The parts worth testing are pure and already have somewhere to live.

- `move_range` and the step rule, in `sidebar_nav.rs`'s existing test module, over fabricated projects and `(workspace, id)` pairs. No egui, no PTY. This is where the empty-workspace landing, the `Workspace`-scope degenerate case and both clamped ends get pinned, along with the two range filters: a workspace absent from `order` is not a destination, and an `origin` absent from `order` yields a range of one under every scope.
- The permutation applied to a concrete list, in `app.rs` beside the `move_target` tests (`app.rs:9156`), following the helper at `app.rs:8750` that applies the same math to a plain `Vec` so the semantics are checked without an app.
- Config parsing, beside `session_display_defaults_to_hidden` (`config.rs:3092`): the default, both non-default scopes, an unknown scope falling back with a warning, and a partial table leaving the other key alone.
- The binding name round trip, which the existing tables in `new_action_names_parse` (`bindings.rs:1314`) and `select_session_actions_parse_from_config_names` (`bindings.rs:1483`) cover once the three names are in them.

Full suite is `cargo nextest run -p alacritree`.

## 9. Files

| File | What changes |
| --- | --- |
| `sidebar_nav.rs` | `ReorderScope`, `move_range`, the step rule, their tests |
| `app.rs` | `DraggedSession`, the runtime drag flag, the two move primitives, `move_session_to_key`, three action arms, drop sites in two render loops, the shared indicator helper, the pending-delete filter feeding `move_range` |
| `bindings.rs` | three `NamedAction` variants, parse and description arms, test tables |
| `command_palette.rs` | `bindable_actions` array, `section_of` arms |
| `config.rs` | `[ui.session_reorder]`, `SessionReorder`, `ReorderScope` parsing, tests |
| `schema/alacritree-config.json` | regenerated |
| `docs/keyboard-shortcuts.md` | the three action names, in the sidebar section |

## Out of scope

**Dragging a session onto a worktree to move it there as a gesture in its own right.** Crossing a workspace boundary happens here only as a consequence of ordering, under a scope the user opted into. Issue #28 is where moving a session between worktrees gets its own UX.

**Dragging the tab-strip segments.** The strip is a few pixels tall and exists to switch, not to hold. Reordering from the sidebar already moves it.

**Persisting order across restarts.** Sessions do not survive the process.

## What the steady-state assertion does with this

`steady_state.rs` asserts the unchanged sidebar frame allocates nothing, and it measures `ObservedInputs::matches` only (`steady_state.rs:125`). The drag snapshot lives in the render pass, which already allocates per frame (`app.rs:3296`), and `DragAndDrop::payload` returns `None` when nothing is being dragged, so an idle frame is unaffected. A completed reorder does invalidate the tree snapshot, correctly: `ObservedInputs.sessions` is order-sensitive (`sidebar_focus.rs:483`).

## Unresolved questions

None. The scope default, the drag default, the target of the keyboard actions and the absence of default key bindings were decided before this was written. The keyboard target's second case, the two range filters, the diff-pane refusal and the persisted expansion flag were decided during review, and are the four places to push back on if any of them reads wrong.
