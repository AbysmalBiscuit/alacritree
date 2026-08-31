# Templated Worktree/Project Names — Design

**Date:** 2026-07-16
**Status:** Draft (pending user review)
**Note:** This spec stays untracked. It must never be committed or reach an upstream PR.

## Goal

Template-driven display names for sidebar worktree and project rows, using shell-style
`$variable` substitution via the `subst` crate. Starship-*like* (the `$branch` look and
fallback semantics), not starship itself — vendoring or depending on starship's formatter
was evaluated and rejected (47-dep tree, second git stack via gix, ANSI-shaped output).

## Branch structure

- `feat/templated-names` — based on upstream master, independent of `feat/sidebar-appearance`.
- Merges into `integration/all-features` when done. Upstream PR only when the user asks.

## Dependency

`subst = "0.3"` — required deps `memchr` + `unicode-width` are already in the workspace
lockfile, so this adds effectively one crate. Chosen over `leon` (no per-variable
fallbacks, `miette` default feature) and `minijinja` (full Jinja2 engine; wrong weight
class) after comparison.

## Config (alacritree.toml)

```toml
[ui]
worktree_name = "$name"   # default: unchanged behavior
project_name  = "$name"   # default: unchanged behavior
```

Syntax is subst's: `$var`, `${var}`, and fallbacks `${var:fallback}` (fallbacks may nest,
e.g. `${branch:$name}` — "the branch, or the worktree name when detached").

## Variables

| Row | Variable | Value |
|---|---|---|
| worktree | `$name` | git worktree name (today's display) |
| worktree | `$branch` | branch name; **unset** when detached/None, so `${branch:...}` falls back |
| worktree | `$path` | full worktree path (useful for disambiguation) |
| project | `$name` | directory name (today's default display) |
| project | `$path` | full project root path |

Variables are supplied per row as a `HashMap<String, String>`; `$branch` is simply absent
from the map when there is no branch.

## Precedence and fallback

- A manual `Project.label` (the rename feature) **wins over** the project template —
  templates only shape the *default* display name.
- Worktrees have no manual label; the template always applies.
- Any subst error — parse failure, unknown variable (e.g. a typo, or plain `$branch` on a
  detached worktree without a fallback) — logs one `warn!` per offending template string
  and falls back to the plain name. A bad template degrades to today's behavior, never to
  a blank or missing row.

## Mechanics

New module `alacritree/src/row_label.rs`:

- `pub fn render_label(template: &str, vars: &HashMap<String, String>) -> Option<String>` —
  thin wrapper over `subst::substitute`; `None` on any error (caller falls back and warns).
  Results are additionally trimmed; an empty render falls back like an error.
- Call sites: the sidebar's worktree-row and project-row label computation. Rendering
  happens at paint time from current `Worktree`/`Project` fields — substitution over a
  short string is microseconds, and branch data refreshes with project state, so no cache
  layer is warranted.
- Warn-once bookkeeping: a small `HashSet<String>` of already-warned template strings on
  the app (config strings are static per run).

## Testing

- `render_label`: plain `$name`, `${branch:$name}` with and without branch, unknown
  variable → `None`, empty render → fallback, literal text without variables passes through.
- Config parse: defaults when keys absent; custom templates round-trip.
- Precedence: manual project label beats template (pure function test on the display-name
  computation).

Manual GUI verification in the isolated lab: default config unchanged; `${branch:$name}`
shows branches on worktree rows and falls back on the main worktree if detached; a typo'd
template degrades to plain names with one log warning.

## Out of scope

- Git-status variables (`$ahead`, `$behind`, `$dirty`) — a later extension fed from the
  existing `StatusCache`, not from starship.
- Styling/color inside templates (labels are single-color egui text).
- Conditional/group syntax beyond subst's `${var:fallback}`.
- Session-row title templates (session titles come from the PTY/OSC pipeline).
