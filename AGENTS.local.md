Whenever working on features, the goal is to preserve the original behavior so Arnaud's workflow isn't affected.

So new UX/UI features need to provide config options that are used to enable them.

## Working on features/bugfixes

Always use worktrees + branches to work on features and bugfixes. Worktrees live
at `../alacritree-worktrees/<branch>`, a sibling of this checkout.

**Branches stack: branch off the newest open PR, never off `master`.** Resolve
the tip at branch time — the stack grows while specs sit unimplemented, and a
stale base silently forks the chain.

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
```

Take the entry whose title carries the highest `[n]` marker; its `headRefName` is
your base and `n + 1` is your marker. Then:

```sh
git worktree add ../alacritree-worktrees/<branch> -b <branch> origin/<base>
```

PR titles carry that marker: `feat(logging): record why alacritree died [8]`.

## Specs and plans

Specs go in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`,
always in the main checkout. Written into a worktree they die with it.

`.git/info/exclude` keeps `docs/superpowers/` untracked, so specs and plans stay
off feature branches and out of PRs; PR descriptions carry the context instead.
The one branch that tracks them is `docs/specs-and-plans`, which holds no code
and exists so they survive worktree deletion and reach another machine.

After writing a spec or plan, copy it onto that branch and push. The file is
still excluded there, so it needs `git add -f`:

```sh
W=../alacritree-worktrees/docs/specs-and-plans
git worktree add $W docs/specs-and-plans   # once per machine
cp docs/superpowers/specs/<file>.md $W/docs/superpowers/specs/
git -C $W add -f docs/superpowers
git -C $W commit -m "docs: add <topic> spec" && git -C $W push
```

Pull from that branch or copy files out of it. Merging it into a feature branch
puts working documents into a PR.

## Git Commits

Git commits you and/or your subagents make must have a commit trailer like: `Co-Authored-By: MODEL <EMAIL>`
The `<EMAIL>` should follow standard practices for the model/harness being used.

Example for Claude Opus 5: `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` or `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`

## Opening PRs

Whenever I ask to open a PR, or push open PR, etc. You need to push the branch to my fork/remote. The PR must be opened against upstream mathix420/Arnaud's repo.

The GitHub base is always `master`, even though the branch descends from the
previous PR in the stack rather than from `master`.


After opening PR, merge in the features into the `all-features` branch. Then run the `install.local.ps1` script.

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
