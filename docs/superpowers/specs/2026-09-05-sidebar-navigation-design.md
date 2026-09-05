# Sidebar navigation design

**Goal:** closing a workspace's last session lands on the session next to it rather than dropping to home, the sidebar scrolls to the session you navigate to, and the row it scrolls to can be parked in the middle of the panel instead of against an edge. All three are off by default.

**Issues:** [#63](https://github.com/AbysmalBiscuit/alacritree/issues/63), [#64](https://github.com/AbysmalBiscuit/alacritree/issues/64), [#65](https://github.com/AbysmalBiscuit/alacritree/issues/65), all sub-issues of [#43](https://github.com/AbysmalBiscuit/alacritree/issues/43).

**Branch:** `feat/sidebar-navigation`. Cut from the open PR carrying the highest `[n]` marker, which was PR 210 (`fix/wsl-helper-liveness`, marker `[8]`) when this was written. The unimplemented session-reorder spec claims the same base; read the tip fresh at setup time rather than trusting that number, and see the open decision at the end.

**Platform:** all. Nothing here touches WSL, conpty or any per-platform path.

**Config:** two new `[ui]` keys and two new values on an existing one, all defaulting to today's behaviour. A user who changes nothing sees no difference.

## Context

Three unrelated pieces of the sidebar, one theme: the panel knows where you are and does not act on it.

### Where a close lands

`close_session` (`app.rs:1337`) resolves two separate questions. `close_landing` (`app.rs:6274`) picks the workspace's next active session from its own siblings, and under `[ui] sidebar_focus = "follow"` that is the closed session's successor, or its predecessor when the last one went. Then `close_fallback` (`app.rs:6303`) decides whether the view moves, and it knows exactly two destinations: the project's main checkout when it still has a live session, otherwise home.

So a close that empties the on-screen workspace always leaves the neighbourhood, however many sessions are running one row away. Given

```
- h
- p1
  - w1 -> s1
  - w2 -> s2
- p2
  - w3 -> s3
```

closing `s1` lands on `h`, and so does closing `s3`.

The ordering that would answer this already exists. `session_ring_target` (`app.rs:6456`) is the flat ring over every open session, workspaces in sidebar order and each workspace's sessions in spawn order, built in `cycle_sessions` (`app.rs:1595`) and cycled by `SelectNextSession` / `SelectPreviousSession`. A close never consults it.

`[ui] last_session_close` (`config.rs:586`) is the key that owns this decision, worded as "what happens when the on-screen workspace's last session closes", with `respawn` recycling a shell in place and `navigate` doing the main-then-home hop. `respawn` is the default and returns before `close_fallback`'s verdict is applied, which is why the behaviour above is only reachable under `navigate`.

`sidebar_focus` (`config.rs:612`) has no say here. It governs the sidebar cursor and, through `defers_close_navigation` (`app.rs:6336`), which frame the navigation happens on. Its doc comment is explicit that both of its values keep the cursor and differ only in whether the terminal comes along.

### What scrolls, and when

The projects panel gates every scroll on one predicate, `let scrolls = |is_cursor: bool| is_cursor && cursor_moved;` (`app.rs:3258`), where `cursor_moved` is the taken value of `sidebar_cursor_moved` (`app.rs:480`). That flag is raised by sidebar navigation, by leaving a search, and by focus seeding. The git panel has the same shape with `git_cursor_moved` (`app.rs:494`).

Nothing else raises it. `cycle_tabs` (`app.rs:1565`), `cycle_sessions` and `cycle_workspaces` (`app.rs:1578`) write `active_session` and `current_workspace` and stop there, so every session-switching binding changes which row is highlighted without bringing it into view. A click, the command palette and an MCP `select_workspace` behave the same way. With enough worktrees open the panel keeps showing somewhere you no longer are.

The row painters already take the decision as a parameter rather than making it: `home_row` (`app.rs:5982`), `worktree_row` (`app.rs:6611`) and `session_row` (`app.rs:6850`) each accept a `scroll_into_view: bool` the panel closure computes. Adding a second reason to scroll therefore touches the closure and not the painters.

### How far it scrolls

All five sidebar scrolls pass `None` as the alignment: the project header (`app.rs:3679`), the git row (`app.rs:5820`), and the home, worktree and session rows (`app.rs:6075`, `app.rs:6820`, `app.rs:6947`). `None` is egui's minimal scroll, which moves the row just far enough to be visible, so walking down the tree pins the cursor to the bottom edge and walking up pins it to the top. There is no way to ask for the row to be parked in the middle.

## 1. Where a close lands

`LastSessionClose` grows two values, so the enum reads:

```toml
[ui]
last_session_close = "respawn"       # recycle a shell in place (default)
                     "navigate"      # the project's main checkout, else home
                     "ring_global"   # the nearest session in the flat ring, else home
                     "ring_project"  # the nearest in the same project, then the ring, else home
```

Putting these on the existing key rather than on a new one keeps the four policies mutually exclusive. A separate key would sit beside `last_session_close` and fire on the same event, and every combination of the two would need a defined meaning.

### The landing rule

```rust
/// The session an emptied workspace lands on under the `ring_*` close policies.
/// `ring` is the flat session ring taken before the close and `alive` is what
/// survived it.  Successor first, predecessor when the closed session was the
/// ring's last live entry: the ordinal rule `close_landing` lands by, so a
/// close that moves the cursor and the terminal cannot point them at different
/// sessions.  `prefer_project` runs that search over the closed session's own
/// project before running it over the whole ring.
fn ring_landing(
    ring: &[RingEntry],
    closed: SessionId,
    alive: &HashSet<SessionId>,
    prefer_project: bool,
) -> Option<(WorkspaceKey, SessionId)>
```

`ring_project` is a strict refinement of `ring_global`: it runs the same search over a subset first and then over the whole ring. The two can only differ while the closed session's project still holds another live session, which is a property the tests assert directly rather than a claim in prose.

**Nothing wraps.** Closing the ring's last live session lands on its predecessor rather than on the first entry. `session_ring_target` wraps because cycling is a loop with no ends; a close is not, and `close_landing` already resolves its own boundary by stepping backwards.

**The ring is captured before the removal.** Ring order is sidebar order, while `close_session`'s existing `removed_idx` indexes `self.sessions` in spawn order, so the two disagree as soon as a project sits above home's sessions in the tree. `close_session` takes the ring first, removes the session, then passes the survivors as `alive`.

**`ring_landing` never competes with `close_landing`.** A surviving sibling in the same worktree means the workspace did not empty, `close_fallback` returns `Stay`, and no ring policy is consulted. The two functions answer disjoint questions on the same event.

### Carrying the verdict

`CloseFallback` (`app.rs:6255`) gains a fourth variant:

```rust
/// A session in another workspace, chosen by `ring_landing`.  Carries the
/// workspace because activating it must not re-adopt that workspace's
/// previously active session.
ActivateSession(WorkspaceKey, SessionId),
```

`apply_close_fallback` (`app.rs:1385`) handles it by writing `active_session` before switching, for the reason `cycle_sessions` already documents at `app.rs:1612`: `ensure_active_session` would otherwise re-adopt the target workspace's old pick. It then routes through `activate_home` or `activate_worktree` exactly as the existing variants do.

The variant rides the existing `DeferredClose` (`app.rs:6324`) path unchanged, so `sidebar_focus = "follow"` keeps owning which frame the navigation lands on and the cursor and the terminal still cannot disagree for a frame.

### The project grouping

`ring_project` needs one fact the ring does not carry: which project owns a workspace. That is `project_of`, a pure function over `&[Project]` living in `sidebar_nav.rs` beside the other egui-free sidebar models:

```rust
/// The project whose worktree list contains `ws`, or None for home and for a
/// workspace no listed project owns.  A path listed by two projects belongs to
/// the first in sidebar order; the session records a directory, not a project,
/// so nothing better is available.
fn project_of(projects: &[Project], ws: &WorkspaceKey) -> Option<&Path>
```

This is an extraction, not an invention: `project_main_for` (`app.rs:6376`) opens with exactly this lookup and then asks a further question of the answer, so it becomes a caller.

Home belongs to no project, so `project_of` returns `None` for it and home's group is home itself. Closing one of several home sessions under `ring_project` therefore prefers the other home sessions, and falls through to the global ring when home has none left.

`RingEntry` carries the project root rather than recomputing the lookup per candidate:

```rust
struct RingEntry {
    /// The owning project's root, None for home.
    project: Option<PathBuf>,
    workspace: WorkspaceKey,
    id: SessionId,
}
```

`workspace_order` (`app.rs:1621`) already walks `self.projects` with `project.root` in hand, so the tag costs nothing at build time. It becomes a thin mapping over a new `workspace_order_with_projects`, rather than the switchability filter being written twice.

### What this makes unreachable

`navigate`'s main-checkout hop cannot fire from either ring policy. `close_fallback` only takes that branch when the main checkout has a live session, and a ring search that found nothing means the project holds no session at all, main included. Both ring policies therefore end at home. The four values stay genuinely distinct, but this belongs in the docs so the fallback chain is not read as three stops when it is two.

## 2. Following the active session

```toml
[ui]
sidebar_follow_active = false  # default
```

When on, the projects panel scrolls the session you are looking at into view whenever it changes, for any reason: a cycling binding, a click, the command palette, an IPC or MCP request.

**Detection is a comparison, not a flag.** `AlacritreeApp` holds `last_followed: (WorkspaceKey, Option<SessionId>)` and the panel compares it against `(current_workspace, active_session)` each frame, cloning only when they differ. A flag set by each mutation site would be the `sidebar_cursor_moved` pattern, and it would have to be added to every present and future writer of those two fields; the comparison cannot be forgotten. `steady_state.rs` asserts that an unchanged frame allocates nothing, and an unchanged frame here compares two `Option<PathBuf>`s and allocates nothing.

**The scroll target is the row with `is_displayed` set**, which `SessionRowData` (`app.rs:6100`) already computes as "active *and* the workspace is current", precisely the session on screen.

**It falls back to the workspace row when no session row is painted.** `sidebar_session_ids` (`app.rs:6108`) returns nothing below a threshold of two, so a worktree running a single shell has no session row to scroll to, and neither does home. The panel resolves which of the two rows to aim at before the paint loop, so exactly one `scroll_to_rect` fires per frame and the row painters keep the signatures they have.

**The sidebar cursor is never written.** `sidebar_focus`'s whole premise is that the cursor is user state that survives a trip through the terminal, and dragging it along would make `preserve` mean nothing. Pressing the sidebar-focus binding still lands you where you left off, on a row the panel may have scrolled past.

## 3. Scroll alignment

```toml
[ui]
sidebar_scroll_align = "minimal"  # default, egui's scroll-just-far-enough
                       "center"   # park the row in the middle of the panel
```

`Theme` (`app.rs:73`) gains `scroll_align: Option<egui::Align>`, following `icon_tooltips` (`app.rs:115`) as the precedent for a config-derived field that is not a colour. All five `scroll_to_rect(rect, None)` calls become `scroll_to_rect(rect, theme.scroll_align)`. `Theme` is already threaded to every site that scrolls, including `paint_git_row_cursor` (`app.rs:5805`), so nothing new is passed down.

One key governs both panels and both reasons to scroll, because it describes where a row is parked rather than why it was chosen.

Hard centring rather than a `scrolloff` row margin. egui clamps a centred target to the scroll range, so a short tree and the top of a long one degrade to today's behaviour instead of overscrolling, which is the case a margin would otherwise be needed for. A margin also needs row height and manual offset arithmetic where alignment is a parameter egui already takes.

## 4. Config surface

| Key | Values | Default | Owns |
| --- | --- | --- | --- |
| `ui.last_session_close` | `respawn`, `navigate`, `ring_global`, `ring_project` | `respawn` | where the view goes when the on-screen workspace empties |
| `ui.sidebar_follow_active` | bool | `false` | whether the panel scrolls to the session on screen |
| `ui.sidebar_scroll_align` | `minimal`, `center` | `minimal` | where a scrolled-to row is parked |

Each gets a doc comment on its `RawUi` field, since those are the hover text the published schema carries. `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`. `docs/alacritree.md` carries the annotated `[ui]` block and the sidebar-focus prose, so all three keys are documented there; `docs/keyboard-shortcuts.md` describes what `last_session_close` does to a close and needs the two new values.

## Testing

`cargo nextest run -p alacritree`.

`ring_landing` and `project_of` are pure over `(project, workspace, id)` snapshots, the way `close_landing` and `close_fallback` are, so they test without spawning a PTY: successor, predecessor at the tail, an empty ring, a closed session absent from the ring, home's group, and a worktree listed by two projects. Both worked examples from the context section go in verbatim. The refinement between the two ring policies gets its own case: with no other session in the closed session's project, `ring_project` and `ring_global` must return the same landing.

The follow-scroll's target selection comes out as a pure function over the rendered rows, so the collapsed-worktree and single-session fallbacks are testable; the scrolling itself is egui and stays untested.

Config parsing gets the defaults / all-values / invalid-falls-back trio the other `[ui]` keys have (`config.rs:2741` onward is the pattern). The schema test fails the build while the checked-in schema is stale, so it needs no case of its own.

`steady_state.rs` already covers the allocation-free unchanged frame and must keep passing with `sidebar_follow_active` on.

## Commits

Three, one per issue, in this order. Each stands alone and each is independently revertable.

1. `feat(sidebar): land a close on the neighbouring session` (#63)
2. `feat(sidebar): scroll to the session navigation lands on` (#64)
3. `feat(sidebar): option to centre the scrolled-to row` (#65)

## Open decision

The unimplemented session-reorder spec (`2026-09-05-session-reorder-design.md`, issue #20) needs the same project grouping this design needs, and specifies it as `ReorderScope::Project` inside `move_range`, a function returning the workspaces a session may move through. Under that scope its range is exactly `order.filter(|w| project_of(w) == project_of(origin))`, and it resolves home and the two-project case the same way section 1 does.

So `project_of` is the shared primitive and `move_range` is a caller, which means whichever branch lands first adds `project_of` to `sidebar_nav.rs` and the other consumes it. Both specs currently claim the same base, PR 210. Deciding the order before either is set up costs nothing; discovering it at the second rebase costs a merge.
