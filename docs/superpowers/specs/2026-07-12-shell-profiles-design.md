# Shell launch profiles — design

Date: 2026-07-12
Branch: `feat/shell-profiles`, stacked on `feat/wsl-support`
Feature: planned_features.md §5 — Shell launch configurations

## Goal

Named shell profiles in config: each profile specifies a program + args
(e.g. a WSL distro, PowerShell). Profiles are launchable from a menu on the
session tab strip, bindable to keyboard shortcuts, and selectable as a
per-project shell override. A configured default profile is what plain
"new session" uses.

## Context and constraints

- `feat/wsl-support` already ships the machinery this builds on:
  `ShellChoice` (per-project override, persisted in state.toml as
  `"windows"` / `"wsl:<distro>"`), the right-click "Open in…" menu on
  project rows, `resolve_shell()` in app.rs, and a `shell_override:
  Option<Shell>` parameter on `Session::spawn` with Windows argv quoting
  (`escape_args`).
- `feat/rebindable-app-shortcuts` is a sibling branch (not an ancestor).
  This design must not depend on it; binding dispatch for `Named` actions
  already exists on master, so profile shortcuts work on the wsl-support
  base and merge with the rebindable branch as a small `parse_action`
  conflict.
- Profiles have no upstream alacritty equivalent, so per repo convention
  they are alacritree-only config under `[ui]` in `alacritree.toml`.

## Config (`config.rs`)

```toml
[ui]
default_profile = "ubuntu"

[[ui.profiles]]
name = "ubuntu"
program = "wsl.exe"
args = ["-d", "ubuntu"]

[[ui.profiles]]
name = "pwsh"
program = "pwsh"
args = ["-NoLogo"]
```

- `[[ui.profiles]]` is an ordered array of tables: `name` (string,
  required), `program` (string, required), `args` (string array, default
  empty). Order drives menu order and the `SpawnProfileN` indices.
- `[ui] default_profile` names the profile used by plain new-session.
- Parsed into `Config { profiles: Vec<Profile>, default_profile:
  Option<String> }`; `Profile { name, program, args }` lives next to
  `ShellConfig` in config.rs.
- Load-time validation warns and degrades, never panics (matching the
  config module's posture):
  - empty `name` or empty `program` → drop the entry, warn
  - duplicate `name` → first wins, warn
  - `default_profile` naming no surviving profile → ignored, warn

## Shell resolution (`app.rs`)

`resolve_shell` precedence for a workspace becomes:

1. Per-project override (`ShellChoice`), now including `Profile(name)`
2. WSL auto-by-location (unchanged — location correctness beats a global
   preference)
3. Default profile (new)
4. Config shell → OS fallback (unchanged, inside `Session::spawn`)

The home tab (`WorkspaceKey = None`), which today always uses the config
shell, also picks up the default profile at step 3: "plain new session
uses the default profile" holds everywhere.

Explicit spawns (menu item, `SpawnProfileN` shortcut) bypass the chain:
they spawn the named profile into the current workspace regardless of any
override.

Profile-built `Shell`s go through the existing `shell_override` parameter
of `Session::spawn` and therefore get `escape_args = true` on Windows.
Deliberate divergence from `[terminal.shell]` (which stays raw to match
upstream alacritty): profiles have no upstream contract, and a TOML array
element containing a space must survive as a single argument.

## Per-project override (`wsl.rs`, `state.rs`, `app.rs`)

- `ShellChoice` gains `Profile(String)`, persisted as `"profile:<name>"`
  alongside `"windows"` / `"wsl:<distro>"`. `parse` / `to_state_string`
  extend accordingly.
- The "Open in…" context menu lists configured profiles after the
  Auto/Windows/WSL entries, with the same selection mark. The menu now
  shows whenever distros **or** profiles exist (today it is hidden when no
  distros are registered).
- A persisted override naming a since-removed profile warns and falls back
  to auto at resolve time — same handling as an unregistered distro.

## Explicit spawn UI (`app.rs`)

A `+` button at the end of the session tab strip (`show_tab_strip`):

- Left-click spawns exactly like Ctrl+T: full resolution chain into the
  current workspace.
- Right-click opens a context menu (same egui `context_menu` mechanism as
  the Open-in menu) listing the configured profiles by name; clicking one
  spawns that profile into the current workspace.
- No profiles configured → plain + button, no context menu.
- WSL distros are *not* implicitly listed: a distro entry is one two-line
  profile away, and synthesizing implicit profiles would duplicate the
  override menu's job.

## Keyboard shortcuts (`bindings.rs`)

- `NamedAction::SpawnProfile(u8)`, 1-indexed into `[[ui.profiles]]` order.
- Parsed from action names `"SpawnProfile1"` … `"SpawnProfile9"`,
  mirroring alacritty's `SelectTab1..9`. No default keys shipped; users
  bind e.g. `key = "2", mods = "Control|Shift", action = "SpawnProfile2"`.
- Index past the configured profile list → warning + `last_error` toast.

## WSL interplay (feature 2 pairing)

A WSL profile is just `program = "wsl.exe", args = ["-d", "<distro>"]`.
`working_directory` is still passed to the PTY and wsl.exe itself maps a
Windows cwd into the distro, so static profiles need no `--cd`. The
`--cd`-building auto path (`wsl::shell_invocation`) remains the
auto-selection mechanism for WSL-resident projects; profiles do not
replace it.

## Error handling

Every failure degrades with a log warning, plus the existing `last_error`
toast for user-initiated spawns:

- unknown name in `default_profile` (load time)
- stale `"profile:<name>"` override (resolve time → auto fallback)
- `SpawnProfileN` with no profile N (dispatch time)
- spawn failure (existing path)

Nothing panics on bad config or stale state.

## Testing

- `config.rs`: profile parsing (full/minimal entries), validation — empty
  name/program dropped, duplicate names deduped, dangling
  `default_profile` ignored.
- `bindings.rs`: `SpawnProfile1..9` parse to `SpawnProfile(n)`;
  `SpawnProfile0`/`SpawnProfile10` are `Unsupported`.
- `wsl.rs`: `ShellChoice::Profile` round-trips through
  `parse`/`to_state_string`; `"profile:"` (empty name) rejected.
- `app.rs`: extract the precedence logic into a pure, testable function
  (override + profiles + location classification in, shell decision out)
  and pin the chain: override wins, WSL auto beats default profile,
  default profile applies to the home tab, stale profile falls back to
  auto.
- Manual GUI checklist: + button spawn, dropdown spawn, override menu
  shows/marks profiles, `SpawnProfileN` binding spawns, toast on
  out-of-range index, default profile used by Ctrl+T and first-launch
  session.

## Out of scope

- Per-worktree session list UI (feature 6).
- Profile-specific env vars, colors, or titles — a profile is program +
  args only until a concrete need appears.
- Implicit per-distro profiles.
- More than 9 shortcut-addressable profiles (menu still lists all).
