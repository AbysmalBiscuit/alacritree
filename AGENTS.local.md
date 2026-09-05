Whenever working on features, the goal is to preserve the original behavior so Arnaud's workflow isn't affected.

So new UX/UI features need to provide config options that are used to enable them.

## Working on features/bugfixes

Every feature and bugfix gets its own worktree and branch, created by `devkit
issue setup` and living at `../alacritree-worktrees/<branch>`, a sibling of this
checkout.

**Branches stack: branch off the newest open PR, never off `master`.** Two
branches cut from `master` collide when the second one merges, and that
collision lands after review, on whoever merges last. Stacking front-loads it
into your own worktree, where you resolve it once. Read the tip fresh each time,
since the stack grows while specs sit unimplemented.

Stack branches that look independent, too. A clean merge between them today says
nothing about what `master` moving under both does to them later.

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
```

Take the entry whose title carries the highest `[n]` marker; its `headRefName` is
your base and `n + 1` is your marker. PR titles carry that marker:
`feat(logging): record why alacritree died [8]`.

The slug is the whole branch name, type prefix included, and the GitHub issue
number comes first:

```sh
devkit issue setup 41 --slug feat/decoration-metrics
```

`devkit issue setup` cuts every branch from `origin/master` and takes no base
flag, so a stacked branch is re-pointed once, before it has any commits of its
own:

```sh
git -C ../alacritree-worktrees/feat/decoration-metrics reset --hard origin/<base>
```

The base moves under you. A branch that sat unimplemented for a few days is
based on a commit its own PR has since rebased away, and nothing errors: the
hash is simply gone from the base's history. Check before opening the PR.

```sh
git -C <worktree> merge-base --is-ancestor <recorded base> origin/<base>
```

When that fails, the live commit usually carries the same subject at a new hash,
and replaying onto the base fixes it:

```sh
git -C <worktree> rebase --onto origin/<base> <recorded base> <branch>
```

`devkit issue status` lists what exists, `devkit issue end` removes a finished
worktree.

## Running tests

`cargo nextest run -p alacritree` is the test command in this checkout, in
place of the `cargo test` line in `AGENTS.md`. That file keeps naming `cargo
test` because Arnaud's CI runs it and nextest is installed only here.

## devkit

This checkout is a devkit project. `devkit.local.toml` configures it and is
untracked, so it rides on the `docs/specs-and-plans` branch with the other local
files.

`worktree_include` names the untracked instructions copied into each new
worktree, so an agent working there reads the same rules as one working here.
After editing `AGENTS.local.md` or `CLAUDE.local.md`, push the change into
worktrees that already exist:

```sh
devkit issue sync-includes --overwrite
```

Several agents share this checkout, so claim a file with `lockm acquire` before
editing it. `docm` resolves alacritty, kitty, ghostty, wezterm and zed at the
versions this project pins; read those checkouts rather than recalling how they
behave.

## Specs and plans

Specs go in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`,
always in the main checkout. Written into a worktree they die with it.

`.git/info/exclude` keeps `docs/superpowers/` untracked, so specs and plans stay
off feature branches and out of PRs; PR descriptions carry the context instead.
The one branch that tracks them is `docs/specs-and-plans`, which holds no code
and exists so they survive worktree deletion and reach another machine.

The branch also carries the untracked local files this repository needs and git
ignores: `AGENTS.local.md`, `CLAUDE.local.md`, `devkit.local.toml`,
`install.local.py` and `sync.local.py`.

`sync.local.py` moves both kinds of file between the main checkout and that
branch, and works from either one. It needs the worktree to exist first:

```sh
git worktree add ../alacritree-worktrees/docs/specs-and-plans docs/specs-and-plans
python3 sync.local.py --dry-run
python3 sync.local.py --push
```

Direction is decided per file. One side missing it gets a copy; both sides
holding different content sends the newer one, so writing a spec here pushes it
onto the branch and pulling the branch on a new machine seeds this checkout.
After a clone stamps every file at once, `--to-branch` or `--to-main` overrides
that. Anything reaching the branch is committed there, one commit per logical
change, and `--trailer` adds a `Co-Authored-By` line for an agent's commits.

Merging the branch into a feature branch puts working documents into a PR.

## Soft-wrapped prose

Issue bodies, PR bodies, review comments and Markdown docs are soft-wrapped: one line per paragraph, one line per bullet, and the renderer wraps them. Hard wrapping at a column renders as ragged text on GitHub and makes every later edit a rewrap. Commit messages are the exception and stay wrapped at 72.

## Git Commits

Git commits you and/or your subagents make must have a commit trailer like: `Co-Authored-By: MODEL <EMAIL>`
The `<EMAIL>` should follow standard practices for the model/harness being used.

Example for Claude Opus 5: `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` or `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`

## Opening PRs

Whenever I ask to open a PR, or push open PR, etc. You need to push the branch to my fork/remote. The PR must be opened against upstream mathix420/Arnaud's repo.

The GitHub base is always `master`, even though the branch descends from the
previous PR in the stack rather than from `master`.

Open the PR with `devkit issue pr create`, not `gh pr create`. Its `pr_body`
template in `devkit.local.toml` renders the body shape every PR here uses: a
human TL;DR, the `Closes` lines, then the Claude summary under a rule, ending in
the model attribution. Writing the body by hand reproduces that shape by memory
and drifts from it.

```sh
devkit issue pr create --ready \
  --pr-title 'feat(render): draw the grid on the GPU [3]' \
  --pr-body "$(cat summary.md)" \
  --arg tldr="$(cat tldr.md)" \
  --arg closes="12 41" \
  --arg stacked_on=203
```

`devkit issue review request` is a different command. It requests review on a PR
that already exists, and it is mine to run, not yours. Never pass `--to` to `pr
create` either: without it the command adds no reviewer and sends no Slack, so
it opens the PR and stops.

`--ready` opens a real PR. Without it devkit opens a draft, and drafts get no
review-bot coverage. Add `--no-push` only when the branch is already pushed.

`--pr-body` is the Claude summary and `--arg tldr` the human half; pass both
through files rather than inline, since either can run long. The first `Closes`
line comes from the worktree's own issue, so `--arg closes` carries only the
extra ones, whitespace separated. `--arg stacked_on` takes the PR number this
branch sits on and emits the review-order note; leave it off for a branch that
really does descend from `master`. `--arg model` overrides the attribution when
a different model did the work.

The template renders only when the command creates a PR. Editing an open one is
still `gh pr edit`, and the shape has to be preserved by hand there.


After opening PR, merge in the features into the `all-features` branch. Then run the `install.local.py` script.

## Tracking features

Features I plan to work are tracked via GitHub issues on my fork: `https://github.com/AbysmalBiscuit/alacritree/issues`

## Upstreaming features for vendored crates

Never propose to upstream features for vendored crates. This is an AI/vibe coded project, so nothing will be upstreamed to vendored crates.
The only upstreaming PRs that we will do are to Arnaud's fork (`alacritree`).

## Agent skills

### Issue tracker

GitHub issues on the fork `AbysmalBiscuit/alacritree`, always with an explicit `-R`.
See `docs/agents/issue-tracker.local.md`.

### Triage labels

The five canonical roles, each label string equal to its name.
See `docs/agents/triage-labels.local.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root.
See `docs/agents/domain.local.md`.
