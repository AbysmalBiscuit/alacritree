# Configurable worktree location — design

Date: 2026-07-12
Status: approved

## Problem

New worktrees are always created at `~/.alacritree/worktrees/<project>-<hash>/<branch>`,
hardcoded in `alacritree/src/worktree.rs` (`project_worktree_dir`). Users cannot
choose where worktrees live — e.g. a faster disk, a shorter path, or a directory
their tooling expects.

## Goals

- Let the user configure the base directory new worktrees are created under,
  globally and per project, via `alacritree.toml`.
- Preserve the existing `<project>-<hash>/<branch>` layout beneath every base
  directory (default, global, or override).
- Leave existing worktrees untouched: discovery goes through `git worktree list`,
  so changing the config only affects where *new* worktrees land.

## Non-goals

- Path templates / placeholders (`{project}`, `{branch}`).
- Setting the location from the app UI; overrides are hand-edited config only.
- Migrating existing worktrees when the config changes.

## Config schema

A new `[workspace]` table, alacritree-only (belongs in `alacritree.toml`,
alongside `[ui]`):

```toml
[workspace]
worktree_dir = "~/dev/worktrees"          # optional; default: ~/.alacritree/worktrees

[[workspace.overrides]]                    # optional, repeatable
project = "~/Git/github/alacritree"
worktree_dir = "D:/wt"
```

Because alacritree merges config arrays by concatenation (matching alacritty's
merge semantics), `[[workspace.overrides]]` entries accumulate across config
files, consistent with `[[keyboard.bindings]]`.

## Path semantics

- A leading `~` / `~/` expands via the `home` crate at config-conversion time.
  No environment-variable expansion.
- After expansion the path must be absolute. A relative `worktree_dir` (global
  or override) logs a warning and the entry is ignored, falling back to the
  next tier — never resolved against the process CWD.

## Resolution

Precedence when creating a worktree for a project:

1. The first `[[workspace.overrides]]` entry whose `project` matches the
   project root. Matching compares both sides after `fs::canonicalize`
   (falling back to the raw path when canonicalization fails, e.g. the path
   does not exist).
2. The global `workspace.worktree_dir`.
3. The built-in default `~/.alacritree/worktrees`.

The chosen base directory always receives the existing
`<project>-<hash>/<branch>` subtree; hashing and branch-name sanitization are
unchanged.

## Implementation

Resolution lives in `app.rs` (UI thread, where `Config` already lives);
`worktree.rs` stays decoupled from config types.

- `config.rs`
  - `RawConfig` gains `workspace: RawWorkspace`.
  - `RawWorkspace { worktree_dir: Option<String>, overrides: Vec<RawWorktreeOverride> }`,
    `RawWorktreeOverride { project: String, worktree_dir: String }`.
  - `Config` gains `workspace: WorkspaceConfig`;
    `WorkspaceConfig { worktree_dir: Option<PathBuf>, overrides: Vec<WorktreeOverride> }`,
    `WorktreeOverride { project: PathBuf, worktree_dir: PathBuf }`.
  - Tilde expansion + absolute-path validation happen in `into_config`;
    invalid entries are dropped with `log::warn!` (same never-panic posture as
    the rest of `config.rs`).
- `config.rs` also hosts the resolution helper,
  `WorkspaceConfig::base_dir_for(&self, project_root: &Path) -> Option<PathBuf>`
  (override → global → `None`), so it is unit-testable without constructing
  the egui app.
- `app.rs`
  - When the create modal confirms, call `base_dir_for(&project_root)` on the
    UI thread and set the result on `CreateRequest`.
- `worktree.rs`
  - `CreateRequest` gains `base_dir: Option<PathBuf>`.
  - `project_worktree_dir(repo, base: Option<&Path>)` uses the provided base,
    keeping `~/.alacritree/worktrees` as the `None` fallback.

## Error handling

- Bad config values: warn at load, drop the entry, defaults apply.
- Unwritable/uncreatable configured dir: the existing `run_create` error path
  already reports `failed to create <dir>` in the create-progress modal; no
  new handling.

## Testing

Unit tests (first tests in the crate; `cargo test -p alacritree`):

- tilde expansion (expands `~/x`, leaves absolute paths alone),
- relative-path rejection (entry dropped, warning logged),
- override precedence (override beats global beats default; first match wins),
- `project_worktree_dir` honors a provided base dir and falls back when `None`.

Manual verification: set `worktree_dir` in `alacritree.toml`, create a worktree
from the sidebar, confirm it lands under the configured directory.

## Documentation

- README configuration section: mention `[workspace]`.
- `docs/alacritree.md`: worktree-location paragraph in the worktree section.
- `CLAUDE.md`: amend the claim that alacritree-only options live only under `[ui]`.
