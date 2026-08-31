# Rebindable app shortcuts — design

Branch: `feat/rebindable-app-shortcuts`. Local-only spec (excluded via
`.git/info/exclude`); the PR description carries this context upstream.

## Problem

Nine app-level shortcuts are hardcoded in `app.rs::handle_shortcuts` via
`consume_exact`, which consumes the key events before both user bindings and
the terminal see them. Consequences:

- Ctrl+B (tmux prefix), Ctrl+G (readline abort), and Ctrl+T (readline
  transpose) never reach the PTY and cannot be reclaimed.
- Users cannot move app actions to other keys.
- Alt+Left/Right workspace cycling shadows word-motion escape sequences.

## Decision summary

- **Config surface:** extend the existing `[[keyboard.bindings]]` action
  vocabulary with alacritree-only action names. No new table or syntax.
  Alacritree-only names are documented as belonging in `alacritree.toml`
  (real alacritty warns on unknown actions in the shared `alacritty.toml`;
  the array-concatenating merge makes this safe).
- **Defaults unchanged:** current keys stay the out-of-box defaults so the
  upstream PR is a pure refactor + new capability. Moving defaults off
  colliding keys is a possible follow-up PR.
- **Architecture:** fold the hardcoded shortcuts into the bindings system
  entirely; delete the `consume_exact` pass.

## Behavior

Default bindings (all current behavior, now expressed as bindings):

| Default        | Action                    | Status   |
| -------------- | ------------------------- | -------- |
| Ctrl+B         | `ToggleLeftSidebar`       | new      |
| Ctrl+G         | `ToggleRightSidebar`      | new      |
| Ctrl+Tab       | `SelectNextTab`           | existing |
| Ctrl+Shift+Tab | `SelectPreviousTab`       | existing |
| Alt+Right      | `SelectNextWorkspace`     | new      |
| Alt+Left       | `SelectPreviousWorkspace` | new      |
| Ctrl+Shift+O   | `AddProject`              | new      |
| Ctrl+T         | `SpawnNewInstance`        | existing |
| Ctrl+Q         | `Quit`                    | existing |

Rebinding idioms (no bespoke code — they fall out of the binding system):

```toml
# Move the sidebar toggle to F1 and give Ctrl+B back to tmux:
[[keyboard.bindings]]
key = "F1"
action = "ToggleLeftSidebar"

[[keyboard.bindings]]
key = "B"
mods = "Control"
action = "ReceiveChar"   # forward to the PTY; "None" would swallow instead
```

### Precedence

A user binding whose key+mods equal a default binding's **replaces** that
default at parse time — alacritty's `triggers_match` semantics, minus modes
(mode-bindings are already dropped in this fork). This also fixes a latent
bug: today a user rebind of e.g. Ctrl+Shift+V runs both the user action and
the default Paste, because defaults are appended unconditionally and
`all_matches` fires every hit.

Multiple *user* bindings on one trigger still all run (alacritty runs all
matching bindings; the ClearLogNotice + `chars` stacking pattern relies on
it).

Replacement matching is exact on modifiers: a user Ctrl+Shift+Tab binding
does not remove the default Ctrl+Tab binding.

### Modal behavior

Unchanged. `update()` skips shortcut handling while a modal is open; that
gate stays where it is.

## Implementation

All changes in `alacritree/` (vendored crates untouched).

### `bindings.rs`

- Add `NamedAction` variants: `ToggleLeftSidebar`, `ToggleRightSidebar`,
  `SelectNextWorkspace`, `SelectPreviousWorkspace`, `AddProject`; wire into
  `parse_action`.
- Append the nine app-shortcut defaults to `default_bindings()`.
- In `parse_bindings`, before appending defaults, drop each default whose
  key+mods match any user binding. Fix the stale "matches returns the first
  hit" comment.

### `app.rs`

- Delete the `consume_exact` block from `handle_shortcuts`; merge what
  remains with `dispatch_user_bindings` (drop the `is_empty` early return —
  defaults make the binding list never empty). Remove `consume_exact` if
  unused afterward.
- New `dispatch_action` arms: sidebar toggles flip the flag and call
  `persist()`; workspace cycling calls `cycle_workspaces(ctx, ±1)`;
  `AddProject` calls `add_project_via_dialog()`.

### `config.rs`

- Doc-comment note only: alacritree-only action names belong in
  `alacritree.toml`.

## Error handling

Unknown action names already map to `BindingAction::Unsupported` with a
debug log — unchanged. No new failure modes.

## Testing

TDD; unit tests in `bindings.rs`:

1. Empty config yields the app defaults (Ctrl+B → `ToggleLeftSidebar`, …).
2. Same-trigger replacement: user Ctrl+B binding removes the default; only
   the user action matches. (RED first — this is the behavior change.)
3. Replacement requires exact mods: user Ctrl+Shift+Tab keeps default
   Ctrl+Tab intact.
4. New action names parse; unknown names still map to `Unsupported`.
5. `ReceiveChar` on a default trigger replaces it (the "free Ctrl+B for
   tmux" path).

`dispatch_action` arms are one-line calls to existing methods and need an
egui `Context`; verified manually in the running app instead of unit tests.

## Out of scope

- Changing default keys (follow-up PR candidate).
- Mode-dependent bindings (vi/search) — still dropped.
- Focus-navigation shortcuts (planned feature 7 builds on this work).
