# Sidebar-search confirm/cancel actions — design

**Status:** in review (revised after codex-review round 5 — final round hit at
REVISE; the 3 residual items are folded in below but not re-reviewed by Codex,
2026-07-22)

## Problem

In the projects sidebar's fuzzy-search mode (entered with `/`), pressing `Enter`
activates the highlighted row but never scrolls it into view — after the query
clears and the full list returns, the selected row can be off-screen. Pressing
`Esc` clears the query but parks you in the sidebar with the cursor wherever the
filter left it, rather than on the session you actually have open.

Both behaviors are hardcoded inside `panel_filter::on_key` and cannot be rebound.

## Goal

Turn the two search-mode operations into named, rebindable actions dispatched
through the existing `[[keyboard.bindings]]` system, with `Enter`/`Esc` as their
defaults, and fix the missing scroll-into-view.

## Constraints

- **Preserve Arnaud's workflow: the terminal's own `Enter`/`Esc` must be
  untouched.** This is the hard constraint. The new actions are **search-scoped** —
  they act only while the *focused* panel is in search mode; their default keys
  pass straight through to the PTY when the terminal owns focus. No new config
  bool; the binding table is the gating surface.
- Mirror alacritty where possible: the action model, `is_*_scoped` gating,
  default-binding merge, and the `ReceiveChar` pass-through rule already exist.
- Both sidebars share `PanelFilter`, so the actions operate on whichever panel is
  focused and in search mode, preserving each panel's confirm-focus nuance.

## Actions

Three new `NamedAction` variants, all search-scoped:

| Action | Default key | Behavior |
|---|---|---|
| `SidebarSearchConfirm` | `Enter` | Exit search → activate the cursored row (row-type-specific, below) → scroll the acted-on row into view. |
| `SidebarSearchCancel` | `Esc` | Exit search, stay in the sidebar, move the cursor to the cancel target (below) and scroll it into view. Git panel just exits search and stays put. |
| `SidebarSearchCancelToTerminal` | *(unbound)* | Exit search and focus the terminal. |

### Row-type-specific confirm (projects panel)

`activate_sidebar_row` branches by row type; these differences are part of the
contract:

- **Session / Worktree / Home row:** switch workspace/session, then focus the
  terminal.
- **Project header row:** toggle its expansion **in place** — focus stays in the
  sidebar; no terminal focus, no workspace switch.

Confirm reuses `activate_sidebar_row` unchanged and inherits these per-row
behaviors. Tests assert each row type separately.

**Confirm scroll target** is the **acted-on row itself** (the snapshotted cursor
row) — *not* `seed`, which returns the *active* workspace's row and is wrong when
confirming a non-active project header. But the acted-on row may not survive the
search exit: search **force-expands** collapsed projects for display, so a matched
worktree/session can be visible only during search and hidden again once the query
clears. `ensure_cursor` does **not** fix this — it falls back to the first row
(Home), not the owning header (`sidebar_nav.rs:179`). So the target is resolved as:
if the acted-on child row is no longer rendered, map it to its **owning project
header** via `row_project_root` (`app.rs:4435`), then pass through `ensure_cursor`
against the rendered rows (first-row/none as the last resort). A collapsed-project
search confirm is tested for this.

### Git-panel confirm

Reuses the existing git activate: open the diff for the cursored row and **stay in
the git panel**. Then run the git filter-change recompute/ensure path (below) and
scroll the git cursor row into view.

### Cancel target (projects panel)

`sidebar_nav::seed` (active session's row when listed, else the workspace's worktree
row, else its collapsed **project header**, else **Home**) through
`sidebar_nav::ensure_cursor` against the **rendered** rows: the target when
rendered, else the **first rendered row**, else `None` (empty toggle-filtered set).
Cancel clears the query only; it does **not** clear toggles. This one rule is the
whole definition, stated identically in the action table and behavior section.

## Dispatch — one interleaved event pipeline

Two ordered passes (nav-then-shortcuts) mis-order batches and cannot thread real
dispatch effects (rounds 1–3). **Replace the three input passes
(`handle_sidebar_nav`, `handle_git_sidebar_nav`, `handle_shortcuts`) with a single
`handle_input` pass that processes each event in original order and applies its
effect against the real app — including terminal input — before the next event.**

### Event-time ownership (no deferred re-interpretation)

`dispatch_action` needs `&mut self` + `ctx`, which cannot be held inside
`ctx.input_mut`. So `handle_input` drains the event batch into a local `Vec` inside
a short `input_mut`, then iterates outside the borrow with immediate effects.

**Terminal-owned key/text events are encoded and written to the focused session
inline, at event time — they are _not_ requeued.** The current design leaves
unconsumed events in the egui queue for `terminal_view` to drain during paint
(`terminal_view.rs:146`), but that consumer checks **paint-time** focus. In a
single ordered pass a later event can change focus or open the palette, so a
requeued terminal `Enter` would be delivered under the wrong owner
(`[Enter, Ctrl+K]` feeds it to the just-opened palette; `[Enter, Ctrl+Shift+B]`
resolves it against final focus). Delivering terminal keys inline binds each event
to its event-time owner.

**Terminal keyboard, text, _and IME_ events all move into the ordered
`handle_input` stream** (reusing `input::event_to_bytes` / the `consumed_event` /
`ime.process` paths that `terminal_view` runs today). IME cannot be left for paint
while keys go inline: `terminal_view.rs:150` currently processes IME and keys as one
ordered stream, so `[Ime::Preedit, Enter]` suppresses the `Enter` during
composition; splitting them would write `Enter` before the preedit exists. The
`ime.preedit().is_none()` guard at `app.rs:6191` is pre-batch, so an `Ime::Preedit`
*within* the batch must establish preedit before a later key is interpreted — only
one ordered stream gives that. `terminal_view` keeps painting and mouse; the
preedit-advance coupling noted at `app.rs:6242`/`:6259` is satisfied because
`handle_input` now runs that drain.

**The event-time terminal session is resolved before any inline write.** Today
input is consumed inside the terminal-view paint branch, which runs
`adopt_active_session` first when the active mapping is stale (`app.rs:6254`).
`handle_input` runs earlier in `update`, so it must resolve/adopt the active session
before writing, or a stale mapping drops input that reaches the adopted session
today.

### Per-event processing (live state each step)

State is read **live per event**, never snapshotted before the loop. In particular
`modal_open` is recomputed per event: `[Ctrl+Q, Enter]` opens the quit modal on the
first event, and the modal must own the `Enter` on the second (today `modal_open` is
snapshotted once at `app.rs:6187`, so the `Enter` would both reach the PTY and
confirm the modal). Order for each event:

1. **A modal / the palette owns input** (checked live) → route the event to it;
   never to the PTY or a binding.
2. **Key press — binding match** (`all_matches`), keeping matched actions through
   **both** existing gates, applied independently:
   - `is_sidebar_scoped` survives only when the projects sidebar owns focus (the
     current `app.rs:1357` rule — must be preserved, or terminal
     Home/End/PageUp/PageDown get swallowed);
   - `is_search_scoped` survives only when the **focused** panel is in
     `Mode::Search`.
   Consume-vs-retain by alacritty's rule: **suppress only when every matched action
   is non-`ReceiveChar`; any matched `ReceiveChar` forces the raw key through**
   (`keyboard.rs:218`). Dispatch matched actions in binding order immediately;
   keyboard-routed search actions carry a **match-time `SearchPanel`** so a
   co-stacked `FocusTerminal` cannot invalidate them (dispatch-site focus gate is
   used only for palette/CLI).
3. **Precedence for unmatched / non-search keys is unchanged from today.** Only an
   authorized search-scoped match preempts search navigation; every *other* binding
   keeps the current precedence where the focused panel's filter/nav consumes
   unmodified keys first (a user's custom unmodified `ArrowDown`/`Space`/`Backspace`
   still behaves as it does now). So:
   - **focused panel in Search:** `Enter`/`Esc` are **never** routed to generic
     navigation regardless of bindings; other keys flow to `panel_filter`
     (typing/`Backspace`/arrows) unless claimed by a search binding in step 2;
   - **focused panel in Browsing:** existing nav (Enter=activate, Esc=leave,
     arrows=move);
   - **terminal focus:** unmatched key/text → encoded to the PTY inline (step above).

Because dispatch is immediate and ordered, later events see real post-dispatch
state: `[Confirm, ArrowDown]` moves nothing before Confirm acts; after a
project-header/git confirm (focus stays, mode Browsing) a following `Esc` runs
Browsing-leave, not a PTY write.

`panel_filter::on_key` loses its `Enter`/`Esc` arms; `Outcome::Activate` is deleted.

### `dispatch_action`

Gains arms for the three actions, each taking an explicit `SearchPanel` when
keyboard-routed or resolving it from live focus for palette/CLI, acting only if that
panel is in `Mode::Search`:

- `SidebarSearchConfirm`: snapshot the filtered cursor, `exit_search`, activate the
  snapshotted row, set the pending scroll target to that row (mapping a now-hidden
  child to its header via `row_project_root`, then `ensure_cursor`).
- `SidebarSearchCancel`: `exit_search`; **projects** sets the cancel-target cursor +
  pending scroll; **git** runs the git recompute/ensure path (so a following
  `ArrowDown` navigates the restored full list, not the stale filtered `git_rows`)
  and sets its pending scroll.
- `SidebarSearchCancelToTerminal`: `exit_search`, run the same panel recompute if
  the panel is git, then `focus_terminal`.

## Scroll-into-view — focus-independent pending target

The round-1 `*_cursor_moved` approach cannot scroll a project confirm: after
`focus_terminal`, the paint pass nulls `cursor_row` (`app.rs:2271`) yet consumes
`cursor_moved` (`:2276`).

Per-sidebar **pending scroll target** independent of focus/cursor:

- `pending_sidebar_scroll: Option<SidebarRow>`, `pending_git_scroll: Option<GitRow>`.
- Confirm stores the acted-on row, mapping a now-hidden child to its owning header
  via `row_project_root` before `ensure_cursor`; cancel stores the cancel target.
  The stored target is always an *already-rendered* row — **the design never
  force-expands a collapsed project** to reveal one (that would be a persisted
  `expanded` mutation, out of scope).
- The paint pass scrolls to the target when it renders that row.
- **Clearing:** cleared by the **next sidebar paint** (scrolled or row-not-found) —
  *not* unconditionally at end of frame, because sidebars paint before the palette
  (`app.rs:6218/6229` vs `:6316`) and a palette-dispatched target must survive to
  the next frame's paint. Additionally cleared at end of frame **when its sidebar is
  hidden** (confirm's `focus_terminal` hides an auto-shown sidebar, `app.rs:1293`,
  so it never paints). Shown-but-unpainted → survives; hidden → dropped; painted →
  consumed.

The existing cursor-move scroll path for arrow navigation is untouched.

## `PanelFilter` API

- Add `pub fn exit_search(&mut self)`: clears the query, rebuilds the empty pattern,
  sets `Mode::Browsing`, leaves toggles intact.
- `mode()` stays public. `Outcome::Activate` is removed from the enum and all arms.

## Defaults, config, palette

- `default_bindings()` gains `Enter → SidebarSearchConfirm`, `Esc →
  SidebarSearchCancel`. `SidebarSearchCancelToTerminal` ships unbound.
- `parse_action` accepts the three names; `config_name`/`description` cover them.
- `is_search_scoped` is true for exactly these three, false for a sampled set.
- **`command_palette::bindable_actions` gains all three; array `[NamedAction; 46]`
  → `[NamedAction; 49]`** so the unbound action appears in the palette. All three
  no-op via the `dispatch_action` gate unless a panel is in search.
- Search-scoped defaults merge like every other default and are replaceable by a
  same-trigger user binding.

## Behavior change surface

- `SidebarSearchConfirm` adds scroll-into-view; otherwise identical per-row to
  today's Enter.
- `SidebarSearchCancel` changes projects `Esc`: lands on the `seed`→`ensure_cursor`
  target and scrolls; git `Esc` equivalent to today plus an explicit rows recompute.
- **Terminal `Enter`/`Esc` reach the PTY only when the terminal owns focus**, and
  are now delivered inline at event time. With a sidebar focused in `Browsing` they
  keep their sidebar meaning; a background panel in Search never affects them.
- **The three input passes are unified into one `handle_input` pass, and terminal
  keyboard input moves into it.** Existing precedence for unmodified keys, both
  `is_*_scoped` gates, stacked bindings, and `ReceiveChar` pass-through are
  preserved and regression-tested. This is the largest-blast-radius part (open
  question #1).
- Everything outside sidebar search mode is unchanged.

### Explicit contract narrowing (printable rebindings)

Binding a search action to **any trigger egui may pair with an `Event::Text`** is
**unsupported** (the text mutates the query *and* the action fires). This covers:
bare printables; Shift+printable; **Alt+printable on macOS/Linux** (Option/xkb
compose, `input.rs:51`); and **Ctrl+Alt / AltGr printables**, which also compose
text (`input.rs:43` returns no bytes precisely because the char arrives via `Text`).
Supported: `Enter`/`Esc` (defaults), function keys, and **plain `Ctrl` (no Alt)**
printable chords, which emit no `Text`. `parse`/validation may warn on an
unsupported search binding. A follow-up may consume the paired `Text` to lift this.

## Testing

- **`panel_filter`:** `on_key(Enter)`/`on_key(Escape)` in `Search` return `None`; an
  `exit_search` test (query cleared, `Browsing`, toggles intact); keep
  `Backspace`/arrow coverage.
- **`bindings`:** the three names parse; `Enter`/`Esc` search defaults present;
  `is_search_scoped` membership exact; a same-trigger user binding replaces the
  default.
- **Automated `handle_input` pipeline tests (required — not a manual matrix).** The
  event-ownership bugs above are invisible to a pure gate helper, so `handle_input`
  must be exercised through real dispatch with `egui::Event` batches. `handle_input`
  is structured to take the drained event `Vec` + the app so it runs without a live
  frame loop. Cases: terminal focus retains/writes Enter/Esc to the PTY;
  `[Enter, Ctrl+K]` (palette) and `[Enter, Ctrl+Shift+B]` deliver Enter to the
  *terminal*, not the later owner; `[Ctrl+Q, Enter]` lets the modal own the Enter;
  terminal Home/End/PageUp/PageDown survive under terminal and git focus
  (`is_sidebar_scoped` gate intact); background-Search-while-terminal-focused
  retains; focused-Search matches + consumes; `[Confirm, ArrowDown]`;
  project-header and git `[Confirm, Esc]`; `[SidebarSearchCancel, ArrowDown]` on git
  (navigates the recomputed list); rebound/unbound Enter in Search never hard-fires
  nav; a custom unmodified `ArrowDown`/`Space` binding keeps today's precedence;
  stacked trigger with `ReceiveChar`; both panels. Also IME interleaving
  (`[Ime::Preedit, Enter]` suppresses the `Enter`; preedit/commit ordering), and a
  stale active-session mapping (input reaches the adopted session, not dropped). A
  pure match/gate helper may back the retention assertions, but the
  ordering/ownership cases run through the pipeline.
- **App-level scroll:** confirm/cancel set the pending target; palette-created target
  survives to the next sidebar paint; confirm that hides an auto-shown sidebar drops
  its target; a **collapsed-project search confirm** scrolls to the owning header
  (via `row_project_root`), not Home. Projects and git symmetric.
- **Palette:** `SidebarSearchCancelToTerminal` appears as an unbound palette row.

*If a real `AlacritreeApp` proves impractical to construct in a test (egui `Context`
is cheap; sessions/PTYs are not), `handle_input` is refactored to depend on a
narrow trait/struct over the state it touches (focus, filters, sessions' write sink,
bindings) so the pipeline tests run against a lightweight fake — the ownership
coverage is not dropped.*

## Docs

`docs/keyboard-shortcuts.md` gains a "Sidebar search" subsection: the three actions,
the search-scoped defaults, and that the defaults pass through to the terminal when
it owns focus.

## Out of scope

- Terminal scrollback search (alacritty's `SearchConfirm`/`SearchCancel`).
- Making arrow-key cursor movement or `/` entry rebindable.
- Consuming the paired `Text` event for printable-chord rebindings (narrowed above).
- Force-expanding a collapsed project to reveal a scroll target.

## Open questions

1. **Input-pass unification + inline terminal input:** merging the three passes and
   moving terminal keyboard input into `handle_input` is the riskiest part. Land it
   here, or stage the pipeline refactor as a separate PR first (search actions ride
   on top once it exists)?
2. **Test harness:** construct a real `AlacritreeApp`, or refactor `handle_input`
   behind a narrow state trait for a lightweight fake? (Either keeps the automated
   ownership coverage; this is an engineering choice, not a coverage cut.)
3. **Cancel + toggles:** clear-query-only (an active toggle can strand the cursor on
   the first rendered row / none) or also clear toggles? *(Codex: acceptable either
   way.)*
4. **`SidebarSearchCancelToTerminal` default:** unbound (chosen) or `Shift+Esc`?
   *(Codex: acceptable either way.)*
