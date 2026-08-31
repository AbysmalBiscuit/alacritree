# Close last session without respawn — design

## Problem

A workspace's last session cannot meaningfully be closed today:

- The central panel calls `ensure_active_session` every frame the current
  workspace has no active session (`app.rs:3991`), which spawns a fresh shell
  in place. Closing the last session — or the shell exiting — instantly
  respawns one.
- There is no keyboard action to close the active session.

## Desired behavior

Closing a session (any path: sidebar × row button, new keybinding, MCP/CLI
`close_session`, or the shell process exiting — any exit status) must not
auto-respawn a shell in that workspace. If the removed session was in the
**current** workspace and that workspace is now empty, navigate:

1. Current workspace is a worktree belonging to a project, and the project's
   main checkout (`project.root`) has a live session → activate that
   workspace. Do **not** spawn there.
2. Otherwise (project main has no session, the workspace *is* the project
   main, the workspace belongs to no known project, or the workspace is
   home) → activate home. `activate_home` spawns a shell if home has none —
   home is the fallback of last resort and always greets with a live shell.

No navigation when:

- The removed session was in a non-current workspace (background close via
  MCP/CLI).
- The workspace still has other sessions (existing sibling-promotion in
  `close_session` stands).

Explicit activation paths keep their spawn behavior: clicking a worktree/home
row, Ctrl+T / `SpawnNewInstance`, spawn buttons, profile spawns, and
worktree-creation open-on-done all still call the spawn-capable
`ensure_active_session`.

The sidebar keeps its two-session row threshold (`sidebar_session_ids`).
Single-session workspaces expose close only via the keybinding, shell exit,
or MCP/CLI.

## Changes

All in `alacritree/` (`app.rs`, `bindings.rs`).

### 1. Adopt-only frame recovery

Replace the per-frame `ensure_active_session` call in the central panel with
an adopt-only variant: if the current workspace has sessions but the active
id is stale, adopt one; if the workspace has none, fall through to the
existing "no session — Ctrl+T to open one" placeholder. Never spawn from the
frame loop.

### 2. Post-removal navigation

A pure decision helper (PTY-free, testable — same pattern as
`sidebar_session_ids`):

```
fn close_fallback(
    removed_ws: &WorkspaceKey,
    current_ws: &WorkspaceKey,
    remaining: &[(WorkspaceKey, SessionId)],   // sessions after removal
    projects: …roots + worktree paths…
) -> CloseFallback   // Stay | Activate(WorkspaceKey) | Home
```

Rules: `Stay` if `removed_ws != current_ws` or `remaining` still has sessions
in `removed_ws`; `Activate(Some(project.root))` if `removed_ws` is a
non-main worktree of a project and `remaining` has a session at the project
root; `Home` otherwise.

`close_session` and `reap_exited_sessions` apply the verdict after removal:
`Activate` sets `current_workspace` (session already exists — no spawn);
`Home` calls `activate_home` (spawns if empty); `Stay` does nothing beyond
today's sibling promotion.

### 3. `CloseSession` binding action

- `NamedAction::CloseSession` in `bindings.rs`; parser accepts
  `"CloseSession"`.
- Dispatch in `app.rs` calls `request_close_session(active id)` — the
  `ui.confirm_session_close` policy applies; no-op with no active session.
- Defaults: `Ctrl+Shift+W` (all platforms), `Cmd+W` (macOS). Plain Ctrl+W
  stays with the shell (readline delete-word).

### 4. MCP/CLI

No changes. IPC `CloseSession` already routes through `close_session` and
inherits the navigation.

## Testing

Unit tests on the pure helper:

- worktree close → project main when main has a session
- worktree close → home when main has none
- project-main close → home
- home close → home (spawn happens in `activate_home`, not the helper)
- background-workspace close → stay
- close with surviving siblings → stay

Bindings: parse test for `"CloseSession"`, default-binding presence test,
alongside the existing ones.

## Error handling

Nothing new. Spawn failure already surfaces via `last_error`; a worktree not
found in any project falls through to home.
