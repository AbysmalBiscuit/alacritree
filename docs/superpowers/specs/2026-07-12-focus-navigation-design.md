# Keyboard focus navigation — design

Branch: `feat/focus-navigation`, stacked on `feat/rebindable-app-shortcuts`
(worktree at `../alacritree-worktrees/feat/focus-navigation`; the PR merges
after the rebindable one). Local-only spec (excluded via `.git/info/exclude`);
the PR description carries this context upstream.

## Problem

The projects sidebar is mouse-only. Rows react to `Sense::click()` alone, and
the terminal re-requests egui focus every frame (`terminal_view.rs`, the
`allow_focus` block), so no widget outside a modal can hold keyboard focus.
Switching worktrees therefore always requires the mouse — or blind Alt+Left/
Right cycling through every workspace.

## Decision summary

- **Focus model:** an app-owned `PaneFocus` enum (`Terminal` |
  `ProjectsSidebar`) on `AlacritreeApp`, not egui's native widget focus. The
  terminal's `allow_focus` gains a `focus == Terminal` condition — the one
  flag already gates both its focus re-grab and its key consumption.
- **Navigation:** a dedicated event-interception pass (the `handle_shortcuts`
  retain pattern) consumes six unmodified keys while the sidebar owns focus:
  Up, Down, Left, Right, Enter, Escape.
- **Config surface:** three new `NamedAction`s in the bindings vocabulary,
  wired like the other app shortcuts. One new default key; no existing
  default changes; no new `[ui]` options.
- **Cursor:** a stable row key, not an index, so git-status refreshes and
  worktree changes cannot silently retarget it.

## Behavior

### Actions and defaults

| Action                 | Default      | Behavior                                   |
| ---------------------- | ------------ | ------------------------------------------ |
| `ToggleSidebarFocus`   | Ctrl+Shift+B | flip focus between terminal and sidebar    |
| `FocusProjectsSidebar` | none         | focus the sidebar (shows it if hidden)     |
| `FocusTerminal`        | none         | return focus to the terminal               |

All three are rebindable via `[[keyboard.bindings]]`; a user binding on a
default's key+mods replaces it (existing precedence rules). Ctrl+Shift+B
pairs mnemonically with Ctrl+B (visibility toggle): plain shows/hides,
+Shift moves focus.

### Hidden-sidebar round trip

Focusing a hidden sidebar shows it and sets a `sidebar_auto_shown` flag.
When focus returns to the terminal — Enter, Escape, toggle, directional
action, or a click on the terminal — an auto-shown sidebar hides again, so a
full keyboard round trip leaves the layout untouched. A manual visibility
toggle (Ctrl+B) clears the flag: a sidebar the user deliberately opened
never auto-hides.

### Keys while the sidebar owns focus

- **Up / Down** — previous/next visible row, clamped at both ends (no wrap).
  Visible rows, in render order: Home, then per project its header and — if
  expanded — its worktrees.
- **Right / Left on a project header** — expand / collapse (persisted, same
  as clicking the arrow).
- **Left on a worktree row** — jump to its project header. Right on a
  worktree or Home row: no-op.
- **Enter on a worktree row** — `activate_worktree`, focus returns to the
  terminal.
- **Enter on Home** — `activate_home`, focus returns to the terminal.
- **Enter on a project header** — toggle expansion; focus stays in the
  sidebar.
- **Escape** — return focus without activating anything.

All other keys fall through: app shortcuts (Ctrl+B, Ctrl+Q, the focus
toggle itself) keep working, and stray typing reaches nothing — the
terminal is not focused, so no bytes reach the PTY.

### Cursor seeding

When focus enters the sidebar the cursor lands on the current workspace's
row; if that worktree's project is collapsed, on the project's header row;
for the home workspace (or when the row cannot be found), on Home.

### Mouse

Unchanged and always live. Clicking the terminal grid returns focus to the
terminal. Clicking a sidebar row activates the workspace as today and does
not move keyboard focus into the sidebar.

## Implementation

All changes in `alacritree/` (vendored crates untouched).

### New module: `sidebar_nav.rs`

Pure cursor logic, unit-testable without an egui context:

- `enum SidebarRow { Home, Project(PathBuf), Worktree(PathBuf) }` — stable
  keys (project root / worktree path).
- `visible_rows(&[Project]) -> Vec<SidebarRow>` — flattening in render
  order, honoring `expanded`.
- Step and seed functions over that list: up/down with clamping,
  left-from-worktree → owning header, vanished-cursor fallback → Home,
  seed-from-`WorkspaceKey`.

### `app.rs`

- New fields: `focus: PaneFocus`, `sidebar_cursor: Option<SidebarRow>`,
  `sidebar_auto_shown: bool`, plus a per-frame "cursor moved" flag for
  scroll-into-view. None are persisted; focus starts on the terminal.
- New interception pass in `update()`, before `handle_shortcuts`, gated on
  `focus == ProjectsSidebar && !modal_open`: consumes the six nav keys and
  applies the behavior table above.
- `dispatch_action` arms for the three new actions. Enter/toggle/Escape and
  the directional actions share two helpers: `focus_sidebar(ctx)` (show if
  hidden → set flag, seed cursor) and `focus_terminal()` (clear focus, hide
  if auto-shown).
- Terminal call site: `allow_focus` becomes
  `!modal_open && focus == Terminal`; a click on the terminal response calls
  `focus_terminal()`.
- Row rendering: the cursor row paints a 1 px `theme.accent` stroke around
  the existing full-width background rect (distinct from, and combinable
  with, the active row's lightened background). On cursor-moved frames the
  row scrolls into view via `scroll_to_rect`; other frames never touch the
  user's scroll position. No new theme colors.

### `bindings.rs`

- `NamedAction` variants `ToggleSidebarFocus`, `FocusProjectsSidebar`,
  `FocusTerminal`; `parse_action` arms; Ctrl+Shift+B default in the
  app-shortcut block of `default_bindings()`.

### Docs

- `docs/keyboard-shortcuts.md`: Ctrl+Shift+B in the defaults table, the
  three action names in the supported-actions list, a short paragraph on
  sidebar navigation keys.

## Error handling

- Cursor row no longer visible (worktree removed, project collapsed
  elsewhere, refresh): next keypress falls back to Home. No panics; the
  flatten-then-step sequence never indexes stale data.
- No projects: the row list is `[Home]`; navigation clamps there and Enter
  activates Home.
- A modal opening while the sidebar owns focus suspends the nav pass (same
  `!modal_open` gate as shortcuts); focus state survives the modal.
- Workspace switched by other means (Alt+Left/Right) while the sidebar owns
  focus: the cursor stays put; only entering the sidebar re-seeds it.

## Integration note

`feat/prunable-worktrees` (sibling branch) renders prunable worktrees as
non-activatable rows. Whichever branch rebases onto the other must make the
cursor skip (or Enter no-op on) prunable rows — resolve at rebase time.

## Testing

TDD. Unit tests in `sidebar_nav.rs`:

1. `visible_rows` honors `expanded` and render order (Home first).
2. Up/Down clamp at both ends; stepping never wraps.
3. Left on a worktree yields its project header; Right/Left no-ops where
   specified.
4. Vanished cursor falls back to Home.
5. Seeding: current worktree visible → worktree row; project collapsed →
   header; home/unknown → Home.

Unit tests in `bindings.rs`:

6. The three action names parse; Ctrl+Shift+B default maps to
   `ToggleSidebarFocus`; a user binding on Ctrl+Shift+B replaces it.

Manual GUI acceptance checklist (egui-dependent paths): focus toggle both
directions, auto-show/auto-hide round trip, Ctrl+B clearing the auto-shown
flag, Enter activation returning focus, cursor highlight + scroll-into-view,
terminal click reclaiming focus, typing while sidebar focused reaching
neither PTY nor UI.

## Out of scope

- Right (git-status) sidebar focus — `PaneFocus` leaves room for it.
- Fuzzy worktree switcher / command palette.
- Vi mode or any terminal-mode tracking.
- Keyboard access to row trailing buttons (delete, refresh, new worktree).
- Moving any existing default key.
