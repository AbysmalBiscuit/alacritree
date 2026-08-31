# Close-behavior config: busy detection + respawn policy — design

## Problem

Two gaps after the close-last-session feature (`feat/close-last-session`,
PR #107):

1. `ui.confirm_session_close = "busy"` keys on `Session::is_busy()`, which
   only recognizes known agent CLIs (`AGENT_PROCESS_GLYPHS`) or a
   spinner-glyph title. A shell running *any* other process (a build, an
   editor, ssh) closes without confirmation even under `busy`.
2. The last-session close behavior (navigate to project main / home) is
   hardwired. Users who prefer the old respawn-in-place behavior have no
   way back.

## Desired behavior

### 1. `busy` means "something is running"

`Session::is_busy()` becomes: **a process is running in the terminal** OR
the existing spinner-title heuristic. Agent detection stays for the sidebar
glyph but is no longer what the confirm gate depends on (agent processes are
a subset of "something is running" on both probed platforms).

- **Linux:** one read of `/proc/<shell_pid>/stat` — busy when the terminal's
  foreground process group (`tpgid`, already parsed by `read_tpgid`) differs
  from the shell's own process group (`pgrp`, same stat line, field 2 after
  comm). Foreground-only: background jobs (`sleep 5 &`) don't count, same
  scope as the existing glyph probe.
- **Windows:** the throttled `windows_process_probe` snapshot already builds
  the shell's descendant tree for agent glyphs; add
  `has_descendants(shell_pid) -> bool` (tree contains any pid besides the
  shell). Any child counts — Windows has no foreground concept; this mirrors
  the glyph probe's approximation. Known trade-off: a shell with a
  persistent helper child always looks busy (same approximation kitty /
  WezTerm accept).
- **macOS:** probe not wired (unchanged) — never busy; document it.
- Caching: the result rides the existing `AGENT_CACHE_TTL` busy-cache
  cadence. No new polling.

`confirm_session_close` values and default are **unchanged**:
`never` (default) | `busy` | `always`. Only `busy`'s meaning broadens.

### 2. `ui.last_session_close = "respawn" | "navigate"`

New alacritree-only option in `[ui]`, parsed in `config.rs` beside
`confirm_session_close` (same warn-and-default handling for unknown
values, documented in the `RawUi` doc comment and the config docs).

- **`respawn`** (**default** — preserves alacritree's long-standing
  out-of-box behavior): when the on-screen workspace's last session closes,
  spawn a fresh shell in place in that workspace. The last session is
  deliberately "unclosable" — closing recycles the shell.
- **`navigate`**: the close-last-session branch behavior — go to the
  project's main checkout if it has a live session, else home.

Plumbing: decided at the single point in `close_session` where
`close_fallback` returns a non-`Stay` verdict. With `respawn`, spawn in the
closed workspace instead of navigating (spawn failure surfaces via
`last_error` like every other spawn). `Stay` verdicts (background closes,
surviving siblings) are unaffected by the policy. The diff-pane toggle-off
path routes through `close_session`, so under `respawn` a lone diff pane
toggled off yields a fresh shell — matching pre-feature behavior.
`run_pending_delete` keeps calling `activate_home` directly (the workspace
it would respawn into no longer exists).

Docs: `docs/keyboard-shortcuts.md`'s `CloseSession` bullet gains a pointer
that post-close behavior follows `ui.last_session_close`.

### 3. Update the user's own config (post-implementation step, not code)

After the feature lands, set in Lev's real `alacritree.toml`:
`last_session_close = "navigate"` and `confirm_session_close = "busy"`.

## Testing

- Pure decision tests: busy = f(has_running_process, title) with injected
  inputs, following existing `session.rs` test patterns (no live PTY).
- `config.rs`: parse tests for `last_session_close` (both values, unknown
  value falls back to `respawn` with a warning), default test; existing
  `confirm_session_close` tests unchanged.
- `close_session` policy plumbing: extend the existing fallback tests only
  if the decision helper's signature changes; otherwise the policy branch
  is thin glue verified in the GUI lab (idle shell closes silently;
  `sleep 30` prompts under `busy`; respawn recycles the shell in place;
  navigate matches the previous branch's verified scenarios).

## Error handling

Nothing new: unknown config values warn and use the default; spawn failure
under `respawn` sets `last_error` (existing path); Linux `/proc` read
failure means "not busy" (probe returns None today for the same reason).

## Delivery

New branch off master → upstream PR (separate from #107; note in the PR
that `navigate` is the #107 behavior now behind a default-off option) →
merge into `integration/all-features`. Only `alacritree/src/**` and docs
change.
