# Specs and plans

Design specs and implementation plans for alacritree, kept on the
`docs/specs-and-plans` branch so they survive worktree deletion and can be
shared between machines.

`docs/superpowers/` is listed in `.git/info/exclude`, which keeps these files
untracked on every feature branch. That is deliberate: they are working
documents and are never upstreamed. This branch is the one place they are
tracked, so adding a file here needs `git add -f`.

- `specs/` — designs produced by the brainstorming skill, named
  `YYYY-MM-DD-<topic>-design.md`.
- `plans/` — implementation plans produced by the writing-plans skill, named
  `YYYY-MM-DD-<topic>.md`, plus their progress files.

The branch carries no code changes. Never merge it into a feature branch;
pull from it, or copy files out of it.
