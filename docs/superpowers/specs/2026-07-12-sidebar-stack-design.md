# Design — `integration/sidebar-stack`

Date: 2026-07-12
Branch: `integration/sidebar-stack` (worktree at `../alacritree-worktrees/integration/sidebar-stack`)
Status: design approved, spec under user review

## Goal

Stack `feat/multi-shell-ui` on top of `feat/focus-navigation` and the input
branches, then close the gap between them: sidebar keyboard navigation must
treat the per-workspace session rows as first-class cursor stops, so the user
can step onto a shell row and open or close it from the keyboard instead of
stepping over it.

Today the two features are independent branches. `focus-navigation`'s cursor
model (`SidebarRow { Home, Project, Worktree }`) knows nothing about the
session rows `multi-shell-ui` paints under a workspace, so Up/Down skips them.

## Part A — Branch construction (linear rebase stack)

Build a fresh branch and replay copies of each layer onto it, leaving every
original feature branch untouched (they remain independent upstream PRs):

```
master (e27e3a0d)
  → fix/input-encoding     (5:  control bytes, xterm CSI, alt-prefix, kitty disambig, mouse wheel)
  → fix/ime-input          (7 unique: shares the 3-commit input base; replay only the IME commits)
  → feat/focus-navigation  (15: feat/rebindable-app-shortcuts 6 + nav 9 — already stacked together)
  → feat/multi-shell-ui    (10: session rows, spawn/close buttons, confirm-close policy)
  → integration commits    (the new work in Part B)
```

Mechanics: create the branch at `fix/input-encoding`, then `git rebase --onto`
each subsequent layer's *unique* commits onto the growing tip. `fix/ime-input`
shares the input base (`76556c52`, `19c4f3fe`, `06642c6b`) with
`fix/input-encoding`, so only its 7 IME commits replay — no duplicates.

Expected conflicts:
- `input.rs` / `bindings.rs`: light (ime vs input-encoding; rebindable vs nav).
- **`app.rs`: heavy at the `multi-shell-ui → focus-navigation` step** — both
  rewrite `home_row` / `worktree_row` and the sidebar render loop. Resolve by
  hand, preserving *both* the cursor-outline rendering (focus-nav) and the
  session-row rendering (multi-shell). This merged render loop is the
  foundation Part B builds on.

## Part B — Nav learns about session rows

### `sidebar_nav.rs`

- Add `SidebarRow::Session(SessionId)`.
- `visible_rows` gains a sessions argument: the per-workspace listed session-id
  lists (as produced by `multi-shell-ui`'s `sidebar_session_ids`, which applies
  the 2+ threshold). It appends `Session` rows immediately after their owning
  `Home` / `Worktree` row, so the cursor list is exactly what the panel paints.
- `left_target`: a `Session` cursor returns its owning `Home`/`Worktree` row
  (the nearest preceding workspace row).
- `seed`: when the current workspace shows a session list, land on the active
  session's row rather than the workspace row.
- `step` is unchanged — it walks whatever `visible_rows` returns.

Threshold consistency: session rows are navigable exactly when they are
rendered (2+ sessions in the workspace). With a single session there is no
child row; the workspace row *is* the session and activating it already
activates that session — existing behavior, unchanged.

### `app.rs` — `apply_sidebar_nav` + render

- Up / Down: work automatically once session rows are in `visible_rows`.
- Enter / Right on a `Session`: activate it — set `current_workspace` +
  `active_session`, the same path the mouse `activate_session_request` uses.
- Left on a `Session`: jump to the owning workspace row (`left_target`).
- `session_row` gains an `is_cursor` flag → paints the same
  `paint_cursor_outline` and honors the `sidebar_cursor_moved` auto-scroll the
  worktree/project rows use.
- Cursor invalidation: a cursor on a session that vanishes (closed, or the list
  drops below the 2-session threshold) falls back to its owning workspace row
  rather than Home. `visible_rows` no longer containing it is the trigger.

### Close — `CloseSession`, default `Ctrl+Shift+W`, rebindable

- New `NamedAction::CloseSession`, exposed as a default binding and parseable
  from config, following the `rebindable-app-shortcuts` pattern already in the
  stack.
- Context-sensitive, matching the Linux-terminal "close the current tab"
  convention:
  - Sidebar focused with the cursor on a `Session` row → close that session.
  - Otherwise → close the current workspace's on-screen (active) session.
- Both paths route through `request_close_session`, which honors the
  `confirm_session_close` policy (never / busy / always) from `multi-shell-ui`.

### Out of scope (deferred)

- Keyboard spawn of a new shell on the cursored workspace. `Ctrl+T` already
  spawns a session in the current workspace; the `+` button stays mouse-only
  for now.

## Testing

`sidebar_nav.rs` stays pure, so extend its unit tests:
- `visible_rows` interleaves session rows after their workspace row and honors
  the 2+ threshold (0/1 sessions → no rows; 2+ → rows).
- `left_target` returns the owning workspace row for a `Session` cursor.
- `step` moves onto and off session rows and clamps correctly.
- `seed` lands on the active session row when the list is shown, falls back to
  the workspace row otherwise.

No GUI/integration tests (the crate has none). Manual acceptance is the user's,
per the fork workflow.

## Deliverable / workflow

- Branch `integration/sidebar-stack` in a sibling worktree.
- This spec and the implementation plan stay local and uncommitted
  (`docs/superpowers/` is git-excluded). PR context, if pushed, lives in the PR
  description.
- Nothing pushed unless the user asks.

## Open questions

None outstanding. Resolved during brainstorming:
- Branch name: `integration/sidebar-stack`.
- Close scope: context-sensitive (sidebar cursor when focused, else on-screen
  session).
- Keyboard spawn: deferred to `Ctrl+T`.
- Seed: onto the active session row when the list is shown.
