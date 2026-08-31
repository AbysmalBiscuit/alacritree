# Sidebar-search confirm/cancel actions — surgical design

**Status:** implemented. Scoped-down alternative to
`2026-07-21-sidebar-search-confirm-cancel-actions-design.md` (kept as the record
of the pipeline-unification approach; not superseded, just not the path we build).
2026-07-22, confirm semantics revised 2026-07-23: confirm **selects** the
highlighted row instead of activating it, which also retired the pending-scroll
machinery. The input architecture is untouched either way — what confirm *does* is
independent of how events are routed.

## Why this exists

The reviewed design grew an input-architecture refactor — collapsing
`handle_sidebar_nav` / `handle_git_sidebar_nav` / `handle_shortcuts` into one
`handle_input` pass and moving terminal keyboard/text/IME input into it. That is
the largest-blast-radius change in `app.rs` and is *not* the feature. This spec
delivers the same user-facing feature — rebindable search confirm/cancel + the
scroll-into-view fix — while leaving the input architecture, terminal delivery,
and IME path exactly as they are today.

The pivot: the reviewed design had to solve IME ordering
(`terminal_view.rs:150`), inline event-time terminal delivery, live `modal_open`
recomputation, and stale-session adoption **only because it relocated terminal
input into a shared ordered pass.** Keep terminal input where it lives (consumed
in the terminal-view paint drain) and none of those problems arise.

## Problem

Same as the reviewed design. In the projects sidebar's fuzzy-search mode (`/`),
`Enter` activates the highlighted row but never scrolls it into view — after the
query clears and the full list returns, the row can be off-screen. `Esc` clears
the query but parks the cursor wherever the filter left it. Both behaviors are
hardcoded in `panel_filter::on_key` (`panel_filter.rs:99-108`) and can't be
rebound.

## Goal

Turn the two search-mode operations into named, rebindable actions with
`Enter`/`Esc` as defaults, and fix the missing scroll-into-view — **without
touching the terminal input path, the IME path, or the nav/shortcuts pass
structure.**

## Constraints

- **Preserve Arnaud's workflow: terminal `Enter`/`Esc` untouched.** The hard
  constraint. Terminal input is never moved; the terminal keeps consuming raw
  events in its paint drain exactly as today.
- Mirror alacritty: reuse the existing `NamedAction` model, `all_matches`
  binding lookup, and the `is_sidebar_scoped` gating pattern verbatim.
- No new config bool: the binding table's search-scoping is the gating surface.

## Key architectural decision — dispatch in the focused panel's nav pass

The three search actions are dispatched **inside the focused panel's existing nav
handler** (`handle_sidebar_nav` / `handle_git_sidebar_nav`), in event order,
during the pass that already runs there. They are **not** routed through
`handle_shortcuts`, and terminal input is **not** relocated.

Why the nav handler and not `handle_shortcuts`:

1. **In-order correctness for free.** The nav handler processes `Event::Text` and
   `Event::Key` in a single ordered drain (`app.rs:1390-1410`). Handling the
   search action there keeps it ordered with the filter's own arrow/typing steps:
   `[Enter, ArrowDown]` confirms the cursor *then* moves; `[ArrowDown, Enter]`
   moves *then* confirms. Routing confirm through the later `handle_shortcuts`
   pass would reorder it after same-batch arrow steps — a regression from today,
   where `panel_filter` handles `Enter` in the nav pass.
2. **The nav handler only runs when a sidebar owns focus** (`app.rs:6197-6201`).
   Anything handled there physically cannot touch terminal input.

`handle_shortcuts` gets exactly one change: **drop search-scoped actions from its
matched set** (one line, mirroring the existing `is_sidebar_scoped` filter at
`app.rs:1357-1363`). That protects the default `Enter → SidebarSearchConfirm`
binding when the terminal owns focus: `all_matches(Enter)` returns the search
action, the filter drops it, the event is retained, and the terminal receives
`Enter` as it does today. The same drop handles "a background panel is still in
search while the terminal is focused" — the binding never fires because the nav
handler didn't run and `handle_shortcuts` discards it.

## Actions

Three new `NamedAction` variants, all search-scoped:

| Action | Default key | Behavior |
|---|---|---|
| `SidebarSearchConfirm` | `Enter` | Exit search → land the cursor on the highlighted row → scroll it into view. Stays in the sidebar; never activates. |
| `SidebarSearchCancel` | `Esc` | Exit search, stay in the sidebar, move the cursor to the cancel target and scroll it in. Git panel exits search, recomputes rows, stays put. |
| `SidebarSearchCancelToTerminal` | `Shift+Esc` | Exit search and focus the terminal. |

### Confirm selects, it never activates (both panels)

Confirm does **not** call `activate_sidebar_row` and does **not** open a diff. It
is uniform across row types: exit search, put the cursor on the highlighted row,
scroll it in, keep focus in the sidebar. Selecting a project header does not
toggle it; selecting a worktree or session does not switch workspace, does not
switch session, and does not focus the terminal. Activation stays a second,
explicit step — a following browsing-mode `Enter`.

This makes confirm the twin of cancel: both exit search, stay in the sidebar, and
land a target row. They differ only in which row — confirm the highlighted one,
cancel the `seed`.

**Reveal, so the selected row survives the exit.** `filtered_rows`
(`sidebar_nav.rs:153`) lists matched worktrees and sessions regardless of their
project's `expanded` flag, so a child matched under a collapsed project vanishes
the moment the query clears. Before exiting search, confirm expands that project
(`set_project_expanded(root, true)` — **expand-only**), so the row stays
selectable. Header rows are excluded from this, which is what keeps confirming a
project from behaving as a toggle.

Do **not** gate the reveal on `rows.contains(row)`: `current_project_rows` takes
the `filtered_rows` path whenever *any* filter is active, including the `s`/`a`
toggles that confirm/cancel deliberately leave intact. A child can therefore look
visible while its project is still persistently collapsed.

### Cancel target (projects panel)

`sidebar_nav::seed` (`sidebar_nav.rs:106` — active session's row when listed, else
the workspace's worktree row, else its collapsed project header, else Home)
through `sidebar_nav::ensure_cursor` against the **rendered** rows.

**Cancel clears the query only; it does not clear the `s`/`a` toggle filters**
(`allowed_toggles` for the projects panel, `app.rs:1504-1505`). This matches
today's behavior — the deleted `Escape` search arm called `clear_query`, leaving
toggles (`panel_filter.rs:104-108`) — so the fuzzy query and the toggle filters
stay independent: cancelling the search you just typed does not silently drop a
sessions/attention filter you set earlier. `exit_search` therefore only touches the
query and mode, never the toggle set.

## PanelFilter API changes

`panel_filter.rs`:

- **Remove the `Enter` and `Escape` arms from the `Mode::Search` match**
  (`panel_filter.rs:99-108`). In search mode `on_key` now returns `None` for both,
  so the nav handler no longer consumes them there and they become bindable.
- **Add `pub fn exit_search(&mut self)`:** clears the query, rebuilds the empty
  pattern, sets `Mode::Browsing`, leaves toggles intact. (Replaces the
  clear-and-switch that the deleted `Enter`/`Escape` arms did inline.)
- **Remove `Outcome::Activate`** from the enum (`panel_filter.rs:29`) and the arm
  that consumed it (`app.rs:1431-1435`).
- `mode()` stays public — the nav handler and the `dispatch_action` gate read it.

## Nav-handler changes (both panels)

In `handle_sidebar_nav` (`app.rs:1386`) and `handle_git_sidebar_nav`
(`app.rs:1651`), add a search-mode check for **every** `Event::Key { pressed:
true, modifiers, .. }` event — regardless of modifiers, so a modified default like
`Shift+Esc` is caught — evaluated **before** the existing `modifiers.is_none()`
filter/nav arm:

```
if filter.mode() == Mode::Search {
    // search-scoped actions bound to this exact (key, modifiers)
    let hits = all_matches(&self.config.bindings, key, modifiers)
        .filter(is_search_scoped);
    if !hits.is_empty() {
        for a in hits { steps.push(SidebarNavStep::SearchAction(a)); }
        return false; // consume
    }
}
// unchanged: modifiers.is_none() → filter.on_key / on_text / is_sidebar_nav_key
```

- The lookup reuses `bindings::all_matches(&bindings, key, *modifiers)` with the
  event's real modifiers, keeping only actions where
  `NamedAction::is_search_scoped()` is true. This lands search actions **in event
  order within the nav pass** — `[Enter, ArrowDown]` confirms *then* moves,
  `[ArrowDown, Enter]` moves *then* confirms — with no cross-pass reordering.
- Only **search-scoped** matches are consumed here. A modified key bound to a
  non-search action (e.g. `Ctrl+B → ToggleLeftSidebar`) is not caught, falls
  through, and reaches `handle_shortcuts` exactly as today — the "nav consumes
  only what it owns" contract (`app.rs:1383-1385`) is preserved.
- Because this runs before `filter.on_key`, a rebound unmodified trigger wins over
  the filter's own handling of that key. The defaults `Enter`/`Esc`/`Shift+Esc`
  don't collide with typing/backspace/arrows.
- `SidebarNavStep` (`app.rs:4177`) gains a `SearchAction(NamedAction)` variant. The
  apply loops (`app.rs:1413`, `:1678`) route it to `dispatch_action` (or a thin
  `apply_search_action`) in order with the other steps.

**Fallback nuance.** `is_sidebar_nav_key` (`app.rs:4216`) includes `Enter`,
`Space`, `Escape`. With the default bindings the search-action check consumes
`Enter`/`Esc` first, so they never hit the fallback in search mode. Restrict the
`is_sidebar_nav_key` fallback to `Mode::Browsing` **except `Space`**, which stays
consumed as a no-op in both modes to keep the fake-click guard (`app.rs:4225-4229`
— egui fake-clicks the natively-focused terminal view on `Space`). Effect: if a
user *unbinds* the search confirm/cancel, an unbound `Enter`/`Esc` in search falls
through rather than hard-firing a browsing activate (matches the reviewed design's
"unbound Enter never hard-fires nav").

## handle_shortcuts change (one filter)

In `handle_shortcuts` (`app.rs:1357-1363`), extend the per-action filter to also
drop search-scoped actions unconditionally — they are owned by the nav pass:

```
.filter(|a| {
    (sidebar_focused || !is_sidebar_scoped(a))
        && !is_search_scoped(a)
})
```

This is the sole terminal-safety mechanism for the default keys, and it is the
existing pattern, not a new one.

## dispatch_action arms

`dispatch_action` (`app.rs:1828`) gains three arms. Each resolves the **focused**
panel (`PaneFocus::ProjectsSidebar` → `project_filter`; `PaneFocus::GitSidebar` →
`git_filter`) and acts **only if that panel is in `Mode::Search`** — a defensive
gate so a palette/CLI-originated run no-ops when no panel is searching (the
keyboard path already guarantees search mode via the nav-handler check):

Confirm and cancel share one landing primitive per panel:

```rust
fn finish_project_search_at(&mut self, requested: Option<SidebarRow>) {
    self.project_filter.exit_search();
    let rows = self.current_project_rows();
    self.sidebar_cursor = sidebar_nav::ensure_cursor(&rows, requested.as_ref());
    self.sidebar_cursor_moved = true;   // forced, see "Scroll-into-view"
}
```

`finish_git_search_at` is the git counterpart (`exit_search` → `recompute_git_rows`
→ `git_nav::ensure_cursor` → force `git_cursor_moved`). Keep them as two helpers:
the row types and recompute needs differ, and one generic abstraction buys
nothing.

- `SidebarSearchConfirm`: snapshot the cursor, `reveal_search_row` it (expand-only,
  children only), then `finish_*_search_at(snapshot)`.
- `SidebarSearchCancel`: compute `seed`, then `finish_project_search_at(seed)`;
  git passes its current cursor. `seed` reads no filter state, so computing it
  before `exit_search` is equivalent.
- `SidebarSearchCancelToTerminal`: `exit_search`, run the git recompute if the
  panel is git, then `focus_terminal`.

## Scroll-into-view — the ordinary cursor path, forced

No focus-independent scroll target is needed. That machinery would only have been
required because confirm called `focus_terminal`, after which the paint pass nulls
`cursor_row` and consumes `cursor_moved`. Confirm now keeps sidebar focus and
guarantees its row renders, so the existing cursor-move scroll does the job.

The one requirement: **force `*_cursor_moved = true` unconditionally**, rather
than only when the cursor's identity changes. Restoring the unfiltered list can
move the very same row far off-screen, and `set_sidebar_cursor` /
`after_git_filter_changed` both flag a move only on identity change — so a
same-row confirm or cancel would silently fail to scroll.

The cursor-move scroll path for arrow navigation is otherwise untouched.

## What this deliberately does NOT do (vs the reviewed design)

- **No `handle_input` unification.** `handle_sidebar_nav`, `handle_git_sidebar_nav`,
  and `handle_shortcuts` stay three passes.
- **No terminal input relocation.** Terminal keyboard/text/IME stay in the
  terminal-view paint drain (`terminal_view.rs:146-150`). The IME preedit guard
  (`app.rs:6191`), stale-session adoption (`app.rs:6254`), and live `modal_open`
  recomputation problems from the reviewed design do not apply.
- **No inline event-time terminal delivery.** The reviewed design's `[Enter,
  Ctrl+K]` / `[Enter, Ctrl+Shift+B]` misrouting cases can't occur, because terminal
  `Enter` is never matched by a binding (dropped in `handle_shortcuts`) and is
  delivered by the unchanged paint drain.

## Contract narrowing (search-action rebinding)

The nav-handler check matches on the event's real modifiers, so both modified and
unmodified triggers work: `Enter`, `Esc`, `Shift+Esc` (default), function keys,
and plain-`Ctrl`/`Shift` chords all dispatch correctly.

- **Unsupported: any trigger that also produces an `Event::Text`** — bare
  printables, `Shift`+printable, `Alt`+printable on macOS/Linux (compose,
  `input.rs:51`), and `Ctrl+Alt`/AltGr printables (`input.rs:43`). The text mutates
  the query *and* the action would fire — same footgun the reviewed design
  excludes. `parse`/validation may warn on such a binding.
- Modified-chord rebinding to non-text keys therefore already works as a side
  effect of the `Shift+Esc` mechanism. The deferred **follow-up (open question #3)
  is only its polish** — a validation warning on text-producing triggers, docs, and
  dedicated tests — not core functionality.

## Defaults, config, palette

- `default_bindings()` (`bindings.rs:266`) gains `Enter → SidebarSearchConfirm`,
  `Esc → SidebarSearchCancel`, and `Shift+Esc → SidebarSearchCancelToTerminal`.
  Plain `Esc` (no mods) and `Shift+Esc` are distinct triggers under
  `matches_exact` (`bindings.rs:453-455`), so both resolve unambiguously in search
  mode.
- `parse_action` accepts the three names; `config_name`/`description` cover them.
- `NamedAction::is_search_scoped()` is true for exactly these three.
- `command_palette::bindable_actions` (`command_palette.rs:112`) gains all three;
  `[NamedAction; 46]` → `[NamedAction; 49]`, so the unbound action is discoverable.
  All three no-op via the `dispatch_action` gate unless a panel is in search.

## Testing

- **`panel_filter`:** `on_key(Enter)`/`on_key(Escape)` in `Search` return `None`; an
  `exit_search` test (query cleared, `Browsing`, toggles intact); keep the existing
  `Backspace`/arrow/toggle coverage. Delete the `Outcome::Activate` test
  (`panel_filter.rs:200-208`).
- **`bindings`:** the three names parse; `Enter`/`Esc` search defaults present;
  `is_search_scoped` membership exact; a same-trigger user binding replaces the
  default (existing `parse_bindings` replacement, `bindings.rs:255-258`).
- **`drain_search_or_nav`:** the existing step-level coverage (a search-scoped
  binding produces `SidebarNavStep::SearchAction` and consumes the event; an
  unmatched key falls through) stands unchanged — the dispatch layer is untouched.
- **Reveal decision (`search_reveal_root`):** a worktree and a session each resolve
  to their owning project root; a **project header resolves to `None`**, which is
  the invariant that keeps confirming a header from toggling it; Home resolves to
  `None`.
- **Palette:** `SidebarSearchCancelToTerminal` appears as an unbound palette row and
  no-ops when run with no panel in search.

**Not covered by automated tests, deliberately.** The effect-level assertions the
earlier draft of this spec listed (confirm leaves workspace/active-session/focus
unchanged, cancel lands on the seed, scroll fires for a same-identity target) need
a live `AlacritreeApp`, and `AlacritreeApp::new` takes an eframe `CreationContext`
that cannot be built in a unit test. That is why the original list was never
delivered rather than merely unfinished. The reveal decision is extracted into the
free function above precisely so the one non-obvious rule is testable; the rest is
verified by hand in the GUI. Making these testable would mean splitting the
sidebar state out of `AlacritreeApp` — a real refactor, tracked separately.

## Docs

`docs/keyboard-shortcuts.md` gains a "Sidebar search" subsection: the three
actions, the `Enter`/`Esc` defaults, that the defaults pass through to the terminal
when it owns focus, and the unmodified-key rebinding limitation.

## Out of scope

- Terminal scrollback search (alacritty's `SearchConfirm`/`SearchCancel`).
- Making arrow-key cursor movement or `/` entry rebindable.
- Modified-chord / printable rebinding of search actions (narrowed above).
- Collapsing anything: the reveal step expands only, and never touches a header.
- Unifying the three input passes or relocating terminal input (the reviewed
  design's territory; explicitly declined here).

## Decisions (resolved 2026-07-22)

1. **Cancel + toggles:** clear query only; leave the `s`/`a` toggles intact
   (preserves today's behavior).
2. **`SidebarSearchCancelToTerminal` default:** `Shift+Esc`.
3. **Modified-chord rebinding:** works for free via the `Shift+Esc` mechanism; only
   its validation/docs/tests polish is deferred to a follow-up issue. The
   text-producing-key exclusion stands.
4. **Bookkeeping:** keep the reviewed pipeline-unification spec as the record; build
   this surgical spec now.

## Open questions

None blocking. Follow-up issue to file: validation warning + docs + tests for
rebinding search actions to modified non-text chords (question 3's polish).
