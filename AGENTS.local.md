Whenever working on features, the goal is to preserve the original behavior so Arnaud's workflow isn't affected.

So new UX/UI features need to provide config options that are used to enable them.

## Code comments and doc strings

Follow the guidelines from the `/unslop` skill and the agent-guard hook messages.

Don't add multiple spaces sporadically in doc strings/comments. The ones that exist in the code base are a mistake and need to be cleaned up.

### Content

1. **Significance inflation.** "pivotal moment", "testament to", "evolving landscape", "setting the stage for", "indelible mark", "deeply rooted". Cut puffery, state what happened.
2. **Notability name-dropping.** Listing media outlets without context. Pick one, say what was said.
3. **Superficial -ing phrases.** "highlighting...", "ensuring...", "reflecting...", "showcasing...", "fostering...". Delete or expand with real sources.
4. **Promotional language.** "nestled", "vibrant", "breathtaking", "groundbreaking", "renowned", "stunning", "must-visit". Use neutral descriptions.
5. **Vague attributions.** "Experts believe", "Industry reports suggest", "Some critics argue". Name the source or delete.
6. **Formulaic challenges.** "Despite challenges... continues to thrive." Replace with specific facts.

### Language

7. **AI vocabulary.** Additionally, crucial, delve, enduring, enhance, fostering, garner, interplay, intricate, landscape (abstract), pivotal, showcase, tapestry (abstract), testament, underscore, vibrant. Replace with plain words.
8. **Copula avoidance.** "serves as", "stands as", "boasts", "features". Just say "is" or "has".
9. **Negative parallelisms.** "It's not just X, it's Y." State the point directly.
10. **Rule of three.** Forcing ideas into groups of three. Use the natural number.
11. **Synonym cycling.** Protagonist, main character, central figure, hero all in one paragraph. Pick one, repeat it.
12. **False ranges.** "from X to Y" where X and Y aren't on a meaningful scale. List topics directly.

### Style

13. **Em dash overuse.** Avoid em dashes entirely. Use periods or commas only (no parentheses, no en dashes, no hyphen-as-dash substitutes). Em dashes are an AI tell, and reaching for parentheses instead just trades one tell for another. If a thought needs separation, end the sentence or use a comma.
14. **Colon overuse.** Colons are fine before a list or example. Not as mid-sentence connectors. "If you're coming from traditional automation: instead of registering event handlers, you describe conditions" adds nothing with the colon. Rewrite to let the point stand on its own without comparison framing. "Describing when the scheduler should fire works best as plain English." Same meaning, no crutch punctuation.
15. **Boldface overuse.** Don't bold every proper noun or acronym.
16. **Inline-header lists.** The tell is a bold label and colon that restates the line: "**Performance:** Performance improved...". Convert those to prose. A bold lead-in that ends in a period, names the item, and is followed by genuinely new detail ("**Schema in TypeScript.** Tables live in one file.") is fine, not a tell.
17. **Title case headings.** Use sentence case.
18. **Decorative emojis.** Remove from headings and bullets.
19. **Curly quotes.** Replace with straight quotes.

## Working on features/bugfixes

Every feature and bugfix gets its own worktree and branch, created by `devkit issue setup` and living at `../alacritree-worktrees/<branch>`, a sibling of this checkout.

**Branches stack: branch off the newest open PR, never off `master`.** Two branches cut from `master` collide when the second one merges, and that collision lands after review, on whoever merges last. Stacking front-loads it into your own worktree, where you resolve it once. Read the tip fresh each time, since the stack grows while specs sit unimplemented.

Stack branches that look independent, too. A clean merge between them today says nothing about what `master` moving under both does to them later.

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
```

Take the entry whose title carries the highest `[n]` marker; its `headRefName` is your base and `n + 1` is your marker. PR titles carry that marker: `feat(logging): record why alacritree died [8]`.

The slug is the whole branch name, type prefix included, and the GitHub issue number comes first:

```sh
devkit issue setup 41 --slug feat/decoration-metrics
```

`devkit issue setup` cuts every branch from `origin/master` and takes no base flag, so a stacked branch is re-pointed once, before it has any commits of its own:

```sh
git -C ../alacritree-worktrees/feat/decoration-metrics reset --hard origin/<base>
```

The base moves under you. A branch that sat unimplemented for a few days is based on a commit its own PR has since rebased away, and nothing errors: the hash is simply gone from the base's history. Check before opening the PR.

```sh
git -C <worktree> merge-base --is-ancestor <recorded base> origin/<base>
```

When that fails, the live commit usually carries the same subject at a new hash, and replaying onto the base fixes it:

```sh
git -C <worktree> rebase --onto origin/<base> <recorded base> <branch>
```

The middle argument has to be a real ancestor of the branch. When it is not, git does not error: it falls back to the merge base and queues the whole divergent history, which looks like a conflict in the code and is really a conflict in the arguments. Once a PR in the stack merges, `/rebase-propagate-alacritree` does this for every open branch at once and checks that first.

`devkit issue status` lists what exists, `devkit issue end` removes a finished worktree.

## Running commands

Build, test, lint and format through devkit rather than by typing cargo directly, so every run carries the flags this checkout needs instead of the ones CI happens to use:

```sh
devkit run task fmt      # nightly rustfmt
devkit run task check    # cargo check
devkit run task test     # nextest
devkit run task build
devkit run task clippy
```

`devkit config tasks` lists them, `--dry-run` prints the argv without running it, and `--dir <worktree>` runs one in a worktree.

Two are not the command you would otherwise type. `fmt` runs nightly rustfmt, because ten of the fifteen options in `rustfmt.toml` are nightly-only and stable rustfmt ignores every one of them after a warning, reformatting files the change never touched. `test` runs nextest, which is installed only here, which is why `AGENTS.md` still names `cargo test` for Arnaud's CI.

<critical>
Never disable rustc cache via env var prefix.
Caching works correctly on this system.
</critical>

## Shared checkout

Several agents work here at once, and `[harness] enforce_writes` refuses a write to any path the session has not claimed. Claim a file before editing it, under the identity the write harness matches on:

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"
```

Without `DEVKIT_SESSION` the holder falls back to the parent pid, which differs between shell invocations, so the next write is refused by a lock you hold yourself. `lockm status` names the holder of each claim. Force-releasing is for a lock stranded under your own dead pid, never for one another agent is using.

## devkit

This checkout is a devkit project. `devkit.local.toml` configures it and is untracked, so it rides on the `docs/specs-and-plans` branch with the other local files.

`worktree_include` names the untracked instructions copied into each new worktree, so an agent working there reads the same rules as one working here. It seeds them when the worktree is cut and nothing refreshes them afterwards, so after editing `AGENTS.local.md` or `CLAUDE.local.md` the change has to be pushed into the worktrees that already exist. A bare `sync.local.py` run does that as its last step; `devkit issue sync-includes --overwrite` does it on its own:

```sh
devkit issue sync-includes --overwrite
```

`docm` resolves alacritty, kitty, ghostty, wezterm and zed at the versions this project pins; read those checkouts rather than recalling how they behave.

## Specs and plans

Specs go in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`, always in the main checkout. A sync run collects one written into a worktree back, but only when the main checkout has no file by that name, so write them here rather than relying on that.

`.git/info/exclude` keeps `docs/superpowers/` untracked, so specs and plans stay off feature branches and out of PRs; PR descriptions carry the context instead. The one branch that tracks them is `docs/specs-and-plans`, which holds no code and exists so they survive worktree deletion and reach another machine.

The branch also carries the untracked local files this repository needs and git ignores: `AGENTS.local.md`, `CLAUDE.local.md`, `devkit.local.toml`, `install.local.py` and `sync.local.py`.

`sync.local.py` moves both kinds of file between the main checkout and that branch, and works from either one. It needs the worktree to exist first:

```sh
git worktree add ../alacritree-worktrees/docs/specs-and-plans docs/specs-and-plans
uv run sync.local.py -d
uv run sync.local.py
```

A bare run is the whole round trip and takes no flags to be one: it collects the working documents the feature worktrees hold and the main checkout does not, commits whatever already sits on the branch uncommitted, rebases the branch onto its remote, moves files both ways, commits what landed, copies the instruction files back out to every worktree whose copy differs, and pushes. `-d` (or `-n`) reports all of that and writes nothing. `--no-commit` and `--no-push` stop at the earlier steps.

Direction is decided per file. One side missing it gets a copy; both sides holding different content sends the newer one, so writing a spec here pushes it onto the branch and pulling the branch on a new machine seeds this checkout. After a clone stamps every file at once, `--to-branch` or `--to-main` overrides that. Anything reaching the branch is committed there, one commit per logical change, and `--trailer` adds a `Co-Authored-By` line for an agent's commits.

Both ends of that round trip reach past the two checkouts. Working documents come back from every feature worktree, because an agent writes them where it is standing. A worktree copy of a document the main checkout already has is left where it is: it is as likely to be a leftover from when the worktree was cut as an edit worth keeping. The instruction files go the other way once the run has settled which copy of them wins, which is what keeps an old worktree from reading rules this checkout stopped following.

The push goes on whether the branch is ahead, not on whether that run is what put it there, so a commit made in the worktree by hand still reaches the remote. A rebase that conflicts is left standing for you to resolve rather than aborted.

Merging the branch into a feature branch puts working documents into a PR.

## Soft-wrapped prose

Issue bodies, PR bodies, review comments and Markdown docs are soft-wrapped: one line per paragraph, one line per bullet, and the renderer wraps them. Hard wrapping at a column renders as ragged text on GitHub and makes every later edit a rewrap. Commit messages are the exception and stay wrapped at 72.

## Git Commits

Git commits you and/or your subagents make must have a commit trailer like: `Co-Authored-By: MODEL <EMAIL>` The `<EMAIL>` should follow standard practices for the model/harness being used.

Example for Claude Opus 5: `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` or `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`

## Opening PRs

Whenever I ask to open a PR, or push open PR, etc. You need to push the branch to the upstream repo (mathix420/Arnaud's repo).

The base/target branch should be `master`, unless you're setting up/working with stacked PRs, then it should be the preceding PR's branch.

Open the PR with `devkit issue pr create`, not `gh pr create`. Its `pr_body` template in `devkit.local.toml` renders the body shape every PR here uses: my TL;DR, the `Closes` lines, then the Claude summary under a rule, ending in the model attribution. Writing the body by hand reproduces that shape by memory and drifts from it.

```sh
devkit issue pr create --ready \
  --pr-title 'feat(render): draw the grid on the GPU [3]' \
  --pr-body "$(cat summary.md)" \
  --arg closes="12 41" \
  --arg stacked_on=203
```

The TL;DR is mine and it is never yours to write. The template emits the heading with nothing under it, and I fill it in myself once the PR is open. There is no `--arg tldr` to pass. The variable does not exist and the template never renders one. Pass only `--pr-body`, `--arg closes` and `--arg stacked_on`.

`devkit issue review request` is a different command. It requests review on a PR that already exists, and it is mine to run, not yours. Never pass `--to` to `pr create` either: without it the command adds no reviewer and sends no Slack, so it opens the PR and stops.

`--ready` opens a real PR. Without it devkit opens a draft, and drafts get no review-bot coverage. Add `--no-push` only when the branch is already pushed.

`--pr-body` carries the Claude summary; pass it through a file rather than inline, since it can run long. The first `Closes` line comes from the worktree's own issue, so `--arg closes` carries only the extra ones, whitespace separated. `--arg stacked_on` takes the PR number this branch sits on and emits the review-order note; leave it off for a branch that really does descend from `master`. `--arg model` overrides the attribution when a different model did the work.

The template renders only when the command creates a PR. Editing an open one is still `gh pr edit`, and the shape has to be preserved by hand there.


<critical>
The all-featurse integration only applies when not doing major refactors. While working on issue #70 or any of its children DO NOT INTEGRATE INTO all-features. A new branch for all-features will be cut from master after the refactor is done.
</critical>
Before opening PR or whenever I ask you, cherry-pick your changes/features into the `all-features` branch (it's normally checked out in a worktree). Then run the `install.local.py` script inside the `all-features` worktree.
This is important so I test features before a PR.

## Tracking features

Features I plan to work are tracked via GitHub issues on my fork: `https://github.com/AbysmalBiscuit/alacritree/issues`

## Upstreaming features for vendored crates

Never propose to upstream features for vendored crates. This is an AI/vibe coded project, so nothing will be upstreamed to vendored crates. The only upstreaming PRs that we will do are to Arnaud's fork (`alacritree`).


## Alacritree config

When adding new entries to the config, always add default values so they are included in the generated spec.

## Agent skills

### Repository skills

`.agents/skills/` holds the skills this repository ships to its own agents, and every worktree reaches them at `.claude/skills/` through a link `sync.local.py` makes. They ride on `docs/specs-and-plans` with the other untracked local files.

`/rebase-propagate-alacritree` replays the whole open `[n]` stack onto `upstream/master` after a PR merges and pushes every branch to the fork.

### Issue tracker

GitHub issues on the fork `AbysmalBiscuit/alacritree`, always with an explicit `-R`. See `docs/agents/issue-tracker.local.md`.

### Triage labels

The five canonical roles, each label string equal to its name. See `docs/agents/triage-labels.local.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.local.md`.
