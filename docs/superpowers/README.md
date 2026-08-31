# Specs and plans

Design specs and implementation plans for alacritree. `specs/` holds designs
from the brainstorming skill, `plans/` holds implementation plans and their
progress files, both named `YYYY-MM-DD-<topic>[-design].md`.

They are working documents and are never upstreamed, so `.git/info/exclude`
keeps `docs/superpowers/` untracked on every other branch. This branch is the
one place they are tracked, which is what lets them outlive a worktree and reach
another machine. It carries no code.

`AGENTS.local.md` in this repo root, under "Specs and plans", is the procedure
for adding to the branch and pulling from it.
