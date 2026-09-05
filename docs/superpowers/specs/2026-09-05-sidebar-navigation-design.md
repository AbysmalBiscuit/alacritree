# Sidebar navigation design

**Goal:** removing the workspace you are looking at lands on the session next to it rather than dropping to home, the sidebar scrolls to the session you navigate to, and the row it scrolls to can be parked in the middle of the panel instead of against an edge. All three are off by default.

**Issues:** [#63](https://github.com/AbysmalBiscuit/alacritree/issues/63), [#64](https://github.com/AbysmalBiscuit/alacritree/issues/64), [#65](https://github.com/AbysmalBiscuit/alacritree/issues/65), all sub-issues of [#43](https://github.com/AbysmalBiscuit/alacritree/issues/43).

**Branch:** `feat/sidebar-navigation`. Cut from the open PR carrying the highest `[n]` marker, which was PR 210 (`fix/wsl-helper-liveness`, marker `[8]`) when this was written. The unimplemented session-reorder spec claims the same base; read the tip fresh at setup time rather than trusting that number, and see the open decision at the end.

**Platform:** all. Nothing here touches WSL, conpty or any per-platform path.

**Config:** two new `[ui]` keys and two new values on an existing one, all defaulting to today's behaviour. A user who changes nothing sees no difference.

## Context

Three unrelated pieces of the sidebar, one theme: the panel knows where you are and does not act on it.

### Where a removal lands

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

Deleting a worktree is the other removal, and it answers the question separately. `run_pending_delete` (`app.rs:7590`) drops the worktree's sessions and then hard-codes the destination: `CloseFallback::Home` on the deferred path, `activate_home` otherwise. It never enters `close_session`, so nothing `last_session_close` says reaches it.

The ordering that would answer both already exists. `session_ring_target` (`app.rs:6456`) is the flat ring over every open session, workspaces in sidebar order and each workspace's sessions in spawn order, built in `cycle_sessions` (`app.rs:1595`) and cycled by `SelectNextSession` / `SelectPreviousSession`. No removal consults it.

`[ui] last_session_close` (`config.rs:586`) is the key that owns this decision, worded as "what happens when the on-screen workspace's last session closes", with `respawn` recycling a shell in place and `navigate` doing the main-then-home hop. `respawn` is the default and returns before `close_fallback`'s verdict is applied, which is why the behaviour above is only reachable under `navigate`.

`sidebar_focus` (`config.rs:612`) has no say in the destination. It governs the sidebar cursor and, through `defers_close_navigation` (`app.rs:6336`), which frame the navigation happens on and who decides it. Under `follow` the reconciler applies the cursor's own landing when that landing has a live session, and falls back to the carried verdict only when it does not (`app.rs:2274-2280`). That precedence is the existing contract and section 1 keeps it.

### What scrolls, and when

The projects panel gates every scroll on one predicate, `let scrolls = |is_cursor: bool| is_cursor && cursor_moved;` (`app.rs:3258`), where `cursor_moved` is the taken value of `sidebar_cursor_moved` (`app.rs:480`). Four places raise that flag: focus seeding (`app.rs:1838`), the reconciler's cursor repair (`app.rs:2268`), `set_sidebar_cursor` (`app.rs:2407`) and leaving a search (`app.rs:3013`). The git panel has the same shape with `git_cursor_moved` (`app.rs:494`).

Navigating between sessions raises nothing. `cycle_tabs` (`app.rs:1565`), `cycle_sessions` and `cycle_workspaces` (`app.rs:1578`) write `active_session` and `current_workspace` and stop there, so every session-switching binding changes which row is highlighted without bringing it into view. A click, the command palette and an MCP `select_workspace` behave the same way. With enough worktrees open the panel keeps showing somewhere you no longer are.

The row painters take the decision as a parameter rather than making it: `home_row` (`app.rs:5982`), `worktree_row` (`app.rs:6611`) and `session_row` (`app.rs:6850`) each accept a `scroll_into_view: bool`, and the panel closure computes it at five call sites (`app.rs:3498`, `3515`, `3678`, `3811`, `3874`). A second reason to scroll is a change to those five expressions and to no painter signature.

### How far it scrolls

All five sidebar scrolls pass `None` as the alignment: the project header (`app.rs:3679`), the git row (`app.rs:5820`), and the home, worktree and session rows (`app.rs:6075`, `app.rs:6820`, `app.rs:6947`). `None` is egui's minimal scroll, which moves the row just far enough to be visible, so walking down the tree pins the cursor to the bottom edge and walking up pins it to the top. There is no way to ask for the row to be parked in the middle.

## 1. Where a removal lands

`LastSessionClose` grows two values, so the enum reads:

```toml
[ui]
last_session_close = "respawn"       # recycle a shell in place (default)
                     "navigate"      # the project's main checkout, else home
                     "ring_global"   # the nearest session in the flat ring, else home
                     "ring_project"  # the nearest in the same project, then the ring, else home
```

Putting these on the existing key rather than on a new one keeps the four policies mutually exclusive. A separate key would sit beside `last_session_close` and fire on the same event, and every combination of the two would need a defined meaning.

The key's name says "close" and the two new values also govern worktree deletion. Its doc comment says so, and so does `docs/alacritree.md`: the policy is where the view goes when the workspace you are looking at stops having sessions, whichever removal took them.

### The landing rule

```rust
/// The session a removal lands on under the `ring_*` policies.  `ring` is the
/// flat session ring captured before the removal and `removed` is what left
/// it: one session for a close, a worktree's whole list for a delete.
/// Successor first, the earliest survivor past the last removed entry, else
/// the latest survivor before the first.
///
/// `prefer` is the removed workspace's owning project under `ring_project`,
/// and None under `ring_global` and for home.  When set, the search runs over
/// that project's entries before running over the whole ring.
///
/// A path two projects both list appears in the ring twice.  Both entries
/// carry the same `project_of` tag and name the same session, so a duplicate
/// changes no answer; indices are taken by first occurrence, the way
/// `session_ring_target` takes them.
fn ring_landing(
    ring: &[RingEntry],
    removed: &[SessionId],
    prefer: Option<&Path>,
) -> Option<(WorkspaceKey, SessionId)>
```

Survivorship is `ring` minus `removed`, so there is no separate liveness set to pass and no way for a caller or a test fixture to supply one that disagrees with the ring.

`ring_project` is a strict refinement of `ring_global`: `prefer: Some(p)` runs the same search over a subset first and then over the whole ring, so the two can only differ while `p` still holds a survivor. That is a property the tests assert directly rather than a claim in prose.

**Nothing wraps.** A removal at the end of the ring lands on the entry before it rather than on the first. `session_ring_target` wraps because cycling is a loop with no ends; a removal is not, and `close_landing` already resolves its own boundary by stepping backwards.

**The ring is captured before the removal, and only under a ring policy.** Ring order is sidebar order while `close_session`'s existing `removed_idx` indexes `self.sessions` in spawn order, and the two disagree as soon as sessions are spawned in one workspace while another already has some. Capturing costs a `Vec`, so both call sites build it only when `last_session_close` is one of the ring values; `respawn` and `navigate` allocate nothing new.

**`ring_landing` never competes with `close_landing`.** A surviving sibling in the same worktree means the workspace did not empty, `close_fallback` returns `Stay` (`app.rs:6309`), and no ring policy is consulted. The two functions answer disjoint questions on the same event.

### Carrying the verdict

`CloseFallback` (`app.rs:6255`) gains a fourth variant:

```rust
/// A session in another workspace, chosen by `ring_landing`.
ActivateSession(SessionId),
```

`apply_close_fallback` (`app.rs:1385`) handles it with `activate_session_by_id` (`app.rs:2394`), which derives the workspace from the session and inserts `active_session` before anything can re-adopt it, then requests a repaint the way the existing variants do. The id is enough; the workspace does not need carrying because that function already looks it up.

The variant rides the existing `DeferredClose` (`app.rs:6325`) path, which moves the struct by value and hands `verdict` to `apply_close_fallback` (`app.rs:2278`). One invariant changes: today `removed_worktree` is only ever `Some` alongside `verdict: Home`, and under a ring policy a deletion pairs it with `ActivateSession`. Nothing reads that combination, but the field's doc comment should stop implying it.

**Under `sidebar_focus = "follow"` the cursor's landing still wins.** The reconciler applies the carried verdict only when the cursor lands on no live session (`app.rs:2274-2280`), and that precedence is deliberate: `follow` means the landing row decides where the terminal goes. The ring verdict is what it falls back to. For the motivating case the two agree anyway, because an emptied worktree's cursor slides to the worktree row itself, which has no session, so the verdict applies.

### The project grouping

`ring_project` needs one fact the ring does not carry: which project owns a workspace. That is `project_of`, a pure function over `&[Project]` living in `sidebar_nav.rs` beside the other egui-free sidebar models:

```rust
/// The project whose worktree list contains `ws`, or None for home and for a
/// workspace no listed project owns.  A path two projects both list belongs to
/// the first in sidebar order; the session records a directory, not a project,
/// so nothing better is available.
fn project_of(projects: &[Project], ws: &WorkspaceKey) -> Option<&Path>
```

This is an extraction, not an invention: `project_main_for` (`app.rs:6376`) opens with exactly this lookup and then asks a further question of the answer, so it becomes a caller.

`RingEntry` carries the owner rather than recomputing the lookup per candidate:

```rust
struct RingEntry {
    /// The owning project's root, from `project_of`.  None for home.
    project: Option<PathBuf>,
    workspace: WorkspaceKey,
    id: SessionId,
}
```

**The tag comes from `project_of`, not from the position in the walk.** `workspace_order` (`app.rs:1621`) pushes every project's worktrees with no dedup, so a path two projects both list is already in the ring twice and `cycle_sessions` already visits it twice. Tagging by walk position would give the two entries different owners and make `ring_project`'s filter disagree with `project_of` on exactly that path. Tagging by `project_of` gives them the same owner, so the duplicate names the same session with the same group and resolves to the same landing whichever entry is reached. First occurrence wins for the anchor index, matching `session_ring_target`'s `position` (`app.rs:6467`).

Deduplicating the ring instead would be the other way to get there, and it is the wrong one: `cycle_sessions` and `cycle_workspaces` read the same order, so removing a duplicate changes cycling for a user who has configured nothing.

Home belongs to no project, so `project_of` returns `None` for it and `ring_project` on home is `ring_global`.

### Deletion

`run_pending_delete` (`app.rs:7590`) resolves the same way `close_session` does. Under a ring policy it captures the ring before the `retain` that drops the worktree's sessions, collects those ids as `removed`, and resolves `ring_landing` with the deleted worktree's project as `prefer`. The deferred branch carries `ActivateSession` instead of `Home`; the immediate branch calls `activate_session_by_id` instead of `activate_home`. An empty result keeps today's behaviour on both branches.

`respawn` and `navigate` keep today's deletion behaviour untouched. Respawning inside a directory that is being deleted is not a thing to offer, and `navigate`'s main-checkout hop is a close-time policy the deletion path has never consulted.

Under `sidebar_focus = "follow"` a deletion already lands somewhere sensible without any of this, because the cursor slides to a sibling worktree row inside the same project and that row usually has a session. The gap this closes is `preserve`, which hard-codes home, and `follow` when the project has no other live worktree.

### What this makes unreachable

`navigate`'s main-checkout hop cannot fire from either ring policy. `close_fallback` only takes that branch when the main checkout has a live session, `worktree_is_switchable` (`app.rs:6841`) keeps any worktree that has sessions, so such a main is in a ring captured before the removal and the ring search finds it. Both ring policies therefore end at home. The four values stay genuinely distinct, but this belongs in the docs so the fallback chain is not read as three stops when it is two.

## 2. Following the active session

```toml
[ui]
sidebar_follow_active = false  # default
```

When on, the projects panel scrolls the session you are looking at into view whenever it changes, for any reason: a cycling binding, a click, the command palette, an IPC or MCP request.

**Detection is a comparison, not a flag.** `AlacritreeApp` holds `last_followed: (WorkspaceKey, Option<SessionId>)` and the panel compares it against `(current_workspace, active_session)`, cloning only when they differ. A flag set by each mutation site would be the `sidebar_cursor_moved` pattern, and it would have to be added to every present and future writer of those two fields; a comparison cannot be forgotten. `sidebar_focus_overtaken` (`app.rs:6241`) already compares a stored `(cursor, workspace, active)` triple per pass for that reason and is the precedent to follow.

**The cursor scroll wins when both are live.** egui stores one scroll target per direction as a plain assignment (`egui-0.31.1/src/ui.rs:1506`) and `ScrollArea` consumes it with a single `take` (`scroll_area.rs:855`), so two `scroll_to_rect` calls in a frame resolve by paint order rather than by policy. Both reasons genuinely coincide: under `sidebar_focus = "follow"` a close moves the cursor and the terminal in the same pass. The follow scroll therefore yields whenever `cursor_moved` is set, because a cursor move is explicit navigation and the follow scroll is a consequence of it. One `scroll_to_rect` fires per frame because the closure enforces it, not because only one reason was live.

**The scroll target is the row with `is_displayed` set**, which `SessionRowData` (`app.rs:6100`) already computes as "active *and* the workspace is current" (`app.rs:7098`), precisely the session on screen.

**When that row is not painted, the target climbs to the nearest ancestor that is.** Four cases produce no session row, and the rule is one sentence rather than four: the workspace row when the session-row threshold hides it (`sidebar_session_ids` returns nothing below two unless `[ui.session_display] sidebar_always`, `app.rs:6108`), the workspace row when a search filters the session out, the project header when the project is collapsed, and nothing at all for a session whose project was removed (`Parent::Detached`). The panel resolves the target before the paint loop, so the row painters keep the signatures they have.

**The comparison is written back only after a scroll actually fires.** Otherwise a change whose row is painted nowhere that frame is consumed by the compare and never retried, so expanding the project afterwards would leave the panel where it was.

**The sidebar cursor is never written.** `sidebar_focus`'s whole premise is that the cursor is user state that survives a trip through the terminal, and dragging it along would make `preserve` mean nothing. Pressing the sidebar-focus binding still lands you where you left off, on a row the panel may have scrolled past.

## 3. Scroll alignment

```toml
[ui]
sidebar_scroll_align = "minimal"  # default, egui's scroll-just-far-enough
                       "center"   # park the row in the middle of the panel
```

`Theme` (`app.rs:73`) gains `scroll_align: Option<egui::Align>`, following `icon_tooltips` (`app.rs:115`) as the precedent for a config-derived field that is not a colour. All five `scroll_to_rect(rect, None)` calls become `scroll_to_rect(rect, theme.scroll_align)`. `Theme` is `Copy` and already reaches every site that scrolls, including `paint_git_row_cursor` (`app.rs:5805`), so nothing new is passed down.

One key governs both panels and both reasons to scroll, because it describes where a row is parked rather than why it was chosen.

Hard centring rather than a `scrolloff` row margin. egui clamps a centred target to the scroll range on both the immediate and the animated path (`scroll_area.rs:1148`, `1264`), so a short tree and the top of a long one degrade to today's behaviour instead of overscrolling, which is the case a margin would otherwise be needed for. A margin also needs row height and manual offset arithmetic where alignment is a parameter egui already takes.

`center` re-centres on every cursor step, and a click near the panel edge scrolls the clicked row out from under the pointer. That is what the setting asks for, and the doc line for the key says so plainly so nobody reads it as a scrolloff.

## 4. Config surface

| Key | Values | Default | Owns |
| --- | --- | --- | --- |
| `ui.last_session_close` | `respawn`, `navigate`, `ring_global`, `ring_project` | `respawn` | where the view goes when the on-screen workspace stops having sessions |
| `ui.sidebar_follow_active` | bool | `false` | whether the panel scrolls to the session on screen |
| `ui.sidebar_scroll_align` | `minimal`, `center` | `minimal` | where a scrolled-to row is parked |

Each gets a doc comment on its `RawUi` field, since those are the hover text the published schema carries. `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`. `docs/alacritree.md` carries the annotated `[ui]` block and the sidebar-focus prose, so all three keys are documented there; `docs/keyboard-shortcuts.md` describes what `last_session_close` does to a close and needs the two new values plus the note that they also govern deletion.

## Testing

`cargo nextest run -p alacritree`.

`ring_landing` and `project_of` are pure over `(project, workspace, id)` snapshots, the way `close_landing` and `close_fallback` are, so they test without spawning a PTY:

- successor, and predecessor when the removal sits at the tail
- a multi-entry `removed`, which is the deletion shape
- an empty ring, and a `removed` the ring does not contain, both `None`
- a path two projects both list: one owner from `project_of`, the same landing from either occurrence
- home, where `prefer: Some(_)` cannot arise
- the refinement: whenever `prefer`'s project holds no survivor, `Some(p)` and `None` return the same landing

Both worked examples from the context section go in verbatim.

The follow-scroll's target selection comes out as a pure function over the rendered rows, so the four not-painted cases and the cursor-wins precedence are testable; the scrolling itself is egui and stays untested.

Config parsing gets the defaults / all-values / invalid-falls-back trio the other `[ui]` keys have (`config.rs:2741` onward is the pattern). The schema test fails the build while the checked-in schema is stale, so it needs no case of its own.

`steady_state.rs` is not evidence for anything here. It measures `ObservedInputs::matches` and never runs the panel paint, so it passes however the follow-scroll is built. The reason to prefer a comparison over a flag is the one in section 2, not an allocation count.

## Commits

Three, one per issue, in this order. Each is code-independent, though all three edit adjacent lines in `config.rs`'s `RawUi` and `Ui`, in `schema/alacritree-config.json` and in `docs/alacritree.md`, so reverting one in isolation means resolving those.

1. `feat(sidebar): land a removal on the neighbouring session` (#63)
2. `feat(sidebar): scroll to the session navigation lands on` (#64)
3. `feat(sidebar): option to centre the scrolled-to row` (#65)

## Open decision

The unimplemented session-reorder spec (`2026-09-05-session-reorder-design.md`, issue #20) needs the same project grouping this design needs, and specifies it as `ReorderScope::Project` inside `move_range`, a function returning the workspaces a session may move through. That function already takes `projects`, and its Project-scope rule is `project_of` word for word, including how it resolves home and a path two projects both list.

So `project_of` is the shared primitive and `move_range` is a caller, which means whichever branch lands first adds it to `sidebar_nav.rs` and the other consumes it. Both specs currently claim the same base, PR 210. Deciding the order before either is set up costs nothing; discovering it at the second rebase costs a merge.
