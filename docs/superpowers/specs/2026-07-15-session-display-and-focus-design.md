# Session-display visibility & directional focus — design

Date: 2026-07-15
Status: approved (brainstorm concluded)

Two features for the `alacritree` crate plus edits to the user's personal
config. All code changes live in `alacritree/`; the vendored crates are
untouched.

## Feature A: session-display visibility

### Problem

A workspace with a single session shows neither its indented session row in
the left sidebar (`sidebar_session_ids` returns empty below 2 sessions,
`app.rs`) nor a tab-strip segment above the terminal (`show_tab_strip` guards
on `indices.len() >= 2`). The user wants the option to always show both.

### Config

New alacritree-only sub-table in `alacritree.toml`:

```toml
[ui.session_display]
sidebar_always = true   # default false — sidebar session rows for 1 session
tabs_always = true      # default false — tab-strip segment for 1 session
```

- `config.rs`: `RawSessionDisplay { sidebar_always: Option<bool>, tabs_always: Option<bool> }`
  nested in `RawUi` as `session_display`, converted to
  `SessionDisplay { sidebar_always: bool, tabs_always: bool }` on `UiTheme`
  (alongside `notifications` / `confirm_session_close`). Doc comments on the
  raw struct per repo convention.
- Missing table or keys → `false` (current behavior). Deep-merge treats the
  sub-table like any table: key-by-key merge of `alacritree.toml` over
  `alacritty.toml`.

### Runtime state & toggles

- `AlacritreeApp` holds two runtime bools initialized from config at startup.
  Rendering reads these, not config.
- Two new `NamedAction`s: `ToggleSessionRows`, `ToggleSessionTabs`. Each
  flips its runtime bool. Default unbound.
- **Persistence: none.** Config is the startup default; toggles are
  runtime-only and reset on restart. Nothing is written to `state.toml`.
- Global — no per-workspace override.
- Stacking both actions on one key works already: `all_matches`
  (`bindings.rs`) returns every matching binding and `handle_shortcuts`
  dispatches all of them.

### Rendering changes

- `sidebar_session_ids` stays pure and gains `always: bool`, lowering the
  list threshold from 2 to 1. Zero sessions still yields no rows. Its doc
  comment (two-session threshold) is updated.
- Downstream fallbacks need no change: the workspace row's attention flag
  and agent glyph only show when the session-row list is empty, so with rows
  forced on they move onto the session row — same as multi-session
  workspaces today.
- `show_tab_strip`: guard becomes `indices.len() >= 2 || tabs_always`. A
  single session renders one full-width segment with the existing
  click/hover/attention behavior. The `+` segment is unchanged.

## Feature B: directional focus with TUI-aware passthrough

### Problem

Focus movement between panels is today limited to
`ToggleSidebarFocus`/`FocusProjectsSidebar`/`FocusTerminal`, the git (right)
sidebar cannot take keyboard focus at all, and there is no way to integrate
focus movement with a TUI running inside the terminal (nvim/tmux own the same
navigation key and should win until they hit their own edge — the pattern the
user already runs in wezterm + Navigator.nvim + tmux).

### Actions

Two new `NamedAction`s: `FocusLeft`, `FocusRight`. Default unbound; users
bind any key (the user will bind Ctrl+Left/Right).

### Focus model

- `PaneFocus` gains `GitSidebar`.
- Panel order: `ProjectsSidebar ↔ Terminal ↔ GitSidebar`.
- Moving toward a closed/absent panel does nothing. No wrap. Focus movement
  never opens a panel.
- Git-sidebar focus is **visual only**: the panel gets the same focus
  highlight treatment the projects sidebar has; no keyboard interaction
  inside the panel yet (row navigation is possible future work).
- `ToggleRightSidebar` while the git sidebar is focused returns focus to the
  terminal — mirroring the existing `ToggleLeftSidebar` guard.

### TUI-aware passthrough

Applies only when the terminal has focus; sidebars always do pure panel
movement. Decision for `FocusLeft`/`FocusRight` while the terminal is
focused, in order:

1. **tmux edge protocol.** If the active session's title matches
   `^tmux:([LRUD]*)`, the inner stack (tmux, with nvim edges folded in via
   the user's `@nvim_edges` autocmds) publishes which walls it is against.
   Direction letter absent → the inner stack can still move → write the
   Ctrl+Arrow byte sequence (CSI `1;5D` / `1;5C`, the same encoding
   `input.rs` produces for Ctrl+Left/Right) to the PTY instead of moving
   focus. Direction letter present → alacritree moves focus.
2. **nvim fallback.** No tmux prefix but the title looks like nvim
   (`^n?vim` prefix match — the title is the only cross-platform signal;
   reading the PTY's foreground process is not worth the platform surface)
   → always pass the bytes through, matching the wezterm fallback.
3. **Plain shell.** Neither pattern → alacritree moves focus. The shell
   loses Ctrl+Arrow word-jump on whatever key the user binds — the same
   tradeoff the user accepted in wezterm.

The bytes are synthesized in the dispatch path (the binding consumed the key
event before the terminal view saw it), reusing the arrow-encoding logic from
`input.rs` rather than duplicating it.

The decision logic (`title × direction × panel visibility × focus →
passthrough | move | nothing`) is a pure function unit-tested without PTYs.

## User config edits (implementation-time, after the build works)

In `%APPDATA%\alacritty\alacritree.toml` (alacritree-only bindings stay out
of the shared `alacritty.toml`):

```toml
[[keyboard.bindings]]
key = "T"
mods = "Control|Shift"
action = "SpawnNewInstance"      # new shell in current workspace (exists today)

[[keyboard.bindings]]
key = "PageUp"
mods = "Control|Shift"
action = "SelectPreviousTab"     # cycle sessions in workspace (exists today)

[[keyboard.bindings]]
key = "PageDown"
mods = "Control|Shift"
action = "SelectNextTab"

[[keyboard.bindings]]
key = "Left"
mods = "Control"
action = "FocusLeft"             # new (Feature B)

[[keyboard.bindings]]
key = "Right"
mods = "Control"
action = "FocusRight"
```

Notes:
- Bindings are global, not focus-gated; the cycle bindings also fire while a
  sidebar is focused. Accepted — cycling only switches the displayed session.
- Claude's permission settings currently deny `%APPDATA%\Roaming\alacritty\`;
  the edit will prompt, or the user allows the path first.

## Testing

- `sidebar_session_ids`: `always = true` cases — single session shown, zero
  sessions still empty; existing threshold cases unchanged.
- Config parse: `[ui.session_display]` present/absent/partial, and merge of
  `alacritree.toml` over `alacritty.toml` for the sub-table.
- Bindings: the four new action names
  (`ToggleSessionRows`, `ToggleSessionTabs`, `FocusLeft`, `FocusRight`) added
  to the existing action-name round-trip test table in `bindings.rs`.
- Passthrough decision function: table-driven cases covering tmux edges
  (blocked/unblocked per direction), nvim fallback, plain shell, sidebar
  focus, and closed panels.
- Manual verification: run the app; single-session workspace with the config
  on shows row + segment; toggles flip them; Ctrl+Left/Right moves focus and
  defers to a running nvim/tmux until its edge.

## Out of scope

- Per-workspace overrides for any of these options.
- Persisting toggle state across restarts.
- Keyboard navigation inside the git sidebar (visual focus only).
- Up/Down focus movement (the layout is a single horizontal row of panels).
- Focus-gating key bindings to the terminal pane.
