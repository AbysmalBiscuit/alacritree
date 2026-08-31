# Multiple shells per worktree (UI) — design

Date: 2026-07-12
Branch: `feat/multi-shell-ui` off `master`, worktree at
`../alacritree-worktrees/feat/multi-shell-ui`.
Source: feature 6 in `docs/specs/planned_features.md`.

## Problem

The session model already supports several sessions per workspace: Ctrl+T
spawns another shell in the current workspace, Ctrl+Tab cycles, and a 2px
tab strip above the terminal shows the sessions of the current workspace.
But the sidebar shows nothing about sessions: there is no way to see which
worktrees have shells running, spawn a shell in a non-current worktree, or
close a specific session with the mouse. This feature adds that UI — no
changes to the session model itself.

## Decisions (from brainstorming)

- Coverage: every workspace row gets the session list — all worktree rows
  **and the Home row** (the `None` workspace).
- Contents: **all sessions** of the workspace, including diff panes,
  mirroring the tab strip. Diff panes appear while they exist and vanish
  when reaped.
- Collapse: **automatic, no chevron** — the list renders only when the
  workspace has 2+ sessions. A single-session workspace row looks exactly
  as today. No new persisted or ephemeral expand state.
- Tab strip: **kept unchanged** — it is the only session indicator when
  the sidebar is hidden (Ctrl+B).
- Close confirmation: **configurable** via a new `[ui]` option (below);
  default `"never"`.
- Base: `master` (independent PR per the worktree-per-feature workflow);
  merge conflicts with `feat/prunable-worktrees` and
  `feat/focus-navigation` are accepted and resolved by whoever merges
  second.

## UI

### Session list

When a workspace has 2+ sessions, its sidebar row is followed by indented
session rows, one per session, in `self.sessions` order (spawn order —
identical to the tab strip). Under worktree rows the indent is one level
deeper than the worktree row (~28px frame margin, vs. 16 for worktrees);
under the Home row the same session-row rendering is used.

Session row anatomy (mirrors `worktree_row`):

- Status icon via `paint_row_status_icon`: this session's attention dot >
  its agent glyph > a default `▪` marker (accent-colored when the session
  is the workspace's active session, muted otherwise).
- Title label, truncated: `session.title` (directory name for fresh
  shells, whatever the program sets via OSC afterwards, `diff: <file>`
  for diff panes).
- Trailing `×` (`icon_button`), hover text "close session". Uses the same
  click-routing workaround as `worktree_row`'s delete button (frame
  interact registers after the inner button; route clicks inside the
  button rect to close).
- Row background: `row_active_bg` when this session is the current
  workspace's active session; `row_hover_bg` on hover; transparent
  otherwise. Background spans the full panel width like other rows.

Clicking a session row activates its workspace **and** that session, from
anywhere — including when the row belongs to a non-current workspace.

### Spawn affordance

- Worktree rows: a `+` icon joins the trailing icons (before the existing
  `×`), hover text "new shell".
- Home row: gets the same trailing `+` (the row moves to the
  `row_with_trailing` layout to host it).

Clicking `+` spawns a shell in that workspace and activates both the
workspace and the new session — consistent with Ctrl+T and with
worktree-creation's activate-on-done behavior.

### Signal de-duplication

When a workspace's session list is rendered (2+ sessions), the parent row
suppresses the aggregated attention dot and agent glyph — the per-session
rows carry them — and falls back to its default icon (`●`/`○`/`⌂`). This
is the same "don't double the dot" rule the project row applies when
expanded. Single-session and collapsed cases keep today's aggregate
behavior (worktree row shows the session's dot/glyph as before).

## State and data flow

No new persistent state: sessions aren't persisted, the list has no
expand state, `state.toml` is untouched.

Inside `show_project_sidebar` (and the Home row path):

- The existing up-front snapshot (attention flags, agent glyphs — kept
  outside the `iter_mut` over projects for borrow-cleanliness) grows a
  per-workspace session listing: for each workspace key, a
  `Vec<SessionRowData>` of
  `(SessionId, title, needs_attention, agent_glyph, is_active)`.
- Click results flow out through the established `Cell`-based request
  pattern. New requests:
  - `spawn_shell_request: Option<WorkspaceKey>`
  - `activate_session_request: Option<(WorkspaceKey, SessionId)>`
  - `close_session_request: Option<SessionId>`
- After the panel closure returns, the app applies them:
  - spawn → `spawn_session(ctx, ws)` (which already inserts the new id as
    the workspace's active session) + set `current_workspace = ws`.
  - activate → set `current_workspace = ws`, insert the id into
    `active_session`.
  - close → confirmation-policy check; either close immediately via
    `close_session(id)` (which already handles active-session fallback)
    or set `pending_session_close`.

## Config

New `[ui]` option in `alacritree.toml`, documented on `RawUi` in
`config.rs`:

```toml
[ui]
# When the sidebar × on a session row asks before killing the PTY.
# "never" (default) | "busy" | "always"
confirm_session_close = "never"
```

- `never`: × closes immediately (matches the app's philosophy of
  confirming only at worktree/app level).
- `busy`: prompt only when the session looks busy — it reports an agent
  glyph (`session.agent_glyph().is_some()`) or a spinner title
  (`is_spinner_title(&session.title)`).
- `always`: prompt on every session close.

Unknown values fall back to `never` with a logged warning, matching the
config module's lenient posture.

## Confirmation modal

For `busy`/`always` when the policy fires: same `modal_frame` pattern as
the delete dialog. Title "Close session `<title>`?", a hint line when the
session looks busy ("A process appears to be running."), Enter to close /
Esc to cancel, buttons right-aligned, default focus on the confirm
button. App state: `pending_session_close: Option<SessionId>`,
participating in `is_modal_open()` so terminal input and shortcuts are
suppressed while it's up.

## Edge cases and error handling

- **Spawn failure** (missing cwd, bad shell): surfaces through the
  existing `last_error` path, same as Ctrl+T. Graceful handling of
  prunable-worktree cwds belongs to `feat/prunable-worktrees`; here a
  spawn into a deleted dir just reports the error.
- **Closing the active session**: `close_session` falls back to another
  session in the same workspace or clears the map entry; next frame
  `ensure_active_session` respawns if the workspace is current. Closing
  the last session of a non-current workspace leaves it empty — the list
  disappears (below 2 sessions) and activating the workspace later
  spawns fresh.
- **Closing a diff pane via ×**: drops the `Session`, identical to
  `open_diff`'s replace path; `Drop` sends `Msg::Shutdown` and delta
  exits cleanly.
- **Stale clicks**: requests carry `SessionId`; if the session was reaped
  between paint and apply, the id lookup misses and the request is a
  no-op. Same for `pending_session_close` pointing at an exited session —
  the modal's confirm becomes a no-op and the modal closes.
- **Modal interplay**: the confirmation modal joins `is_modal_open()`
  like the other three dialogs.

## Testing

egui rendering isn't testable headlessly; tests target extracted logic,
plus a manual GUI pass.

Unit tests:

- `confirm_session_close` parsing: `"never"`/`"busy"`/`"always"`, absent
  → default `never`, invalid value → `never` (+ warning).
- Confirmation policy: pure
  `needs_close_confirmation(policy, is_busy) -> bool` across the full
  matrix; busy detection through the existing `is_spinner_title` /
  agent-glyph helpers.
- Grouping/visibility: a helper that groups sessions by workspace key and
  answers "render list?" (2+ sessions) — tested with mixed Shell/Diff
  sessions across Home and worktree keys, including the exactly-1 and 0
  cases.

Manual GUI checklist (user acceptance):

1. `+` on a worktree row spawns a shell there and switches to it; same
   for Home.
2. List appears under a row once it has 2 sessions; disappears when back
   to 1.
3. Clicking a session row of a *different* workspace switches workspace
   and session.
4. Per-session attention dot shows on the session row (run `sleep 2 &&
   printf '\a'` in a background session); parent row shows no duplicate
   dot while the list is visible; collapsed/single-session rows still
   aggregate.
5. Agent glyph (run `claude`) shows per-session; parent row de-dups the
   same way.
6. × closes a session: with `confirm_session_close` unset (immediate),
   `"always"` (modal), `"busy"` (modal only while an agent/spinner is
   active).
7. Opening a diff from the git sidebar adds a `diff: <file>` row to the
   current workspace's list; `q` in delta removes it.
8. Tab strip behavior unchanged; Ctrl+T / Ctrl+Tab unchanged.
9. Active session row highlight follows tab-strip clicks and Ctrl+Tab.

## Out of scope

- Keyboard navigation of session rows (`feat/focus-navigation` owns
  sidebar keyboard-drivability; its merge will need to extend to session
  rows as a follow-up).
- Shell launch profiles (feature 5) — the `+` always spawns the default
  configured shell.
- Persisting sessions across restarts.
