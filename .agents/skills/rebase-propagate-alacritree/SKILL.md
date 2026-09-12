---
name: rebase-propagate-alacritree
description: Replay the alacritree [n] PR stack onto upstream/master after a merge and push every branch to the fork [rpa]
disable-model-invocation: true
user-invocable: true
allowed-tools: Bash, Read, Glob, Grep, TaskCreate, TaskUpdate, TaskList, TaskGet
---
Rebase every open alacritree PR onto the merged upstream master and push the whole stack to the fork.

Takes no arguments. `--dry-run` prints the plan and writes nothing; `--test` runs each rebased branch's suite before any push; `--old-base REV` names the commit to cut below on the lowest branch when no merged PR does.

## What already happened

`.claude/skills/rebase-propagate-alacritree/scripts/rebase_propagate_alacritree.py` has **already run**. It fetched both remotes, read the stack off the `[n]` markers in the open upstream PR titles, checked every branch against the fork and against its cut-below commit, replayed each one with `--onto`, and pushed them all under leases. Its output is below. Invoking `/rebase-propagate-alacritree` authorizes those force-pushes.

Nothing is pushed until every rebase succeeds, so a `CONFLICT` means the fork is untouched and the run is resumable.

Everything you need about repo, worktree and PR state is in that output. Do not re-run `git status`, `git log`, `gh pr list`, or `gh pr view` to orient yourself or to double-check a result the script already reported.

---

!`uv run .claude/skills/rebase-propagate-alacritree/scripts/rebase_propagate_alacritree.py 2>&1 || true`

---

## Why this stack needs its own script

Every PR here targets `master` on GitHub even though the branches stack on each other, so GitHub's base field records nothing about the order. The `[n]` marker in the PR title is the only record, which is why the script reads titles rather than bases.

Upstream squash-merges. Once PR `[n]` lands, its commits are gone from `master`'s history, the merge base collapses to the commit the whole stack was cut from, and a plain `git rebase upstream/master` on `[n+1]` replays work `master` already carries, hunk by conflicting hunk. So every branch is replayed with `git rebase --onto <new parent> <old base>`, cutting below the tip its parent held before this run. For the lowest open branch, that tip is the merged PR's head branch on the fork, which the fork keeps after the squash.

The middle argument must be a true ancestor of the branch. When it is not, git does not error: it silently falls back to the real merge base and queues the entire divergent history. The script asserts the ancestry first, which is the difference between an 18-commit replay and a 93-commit conflict storm.

Local branches go stale. Several worktrees here hold pre-sweep copies that are both ahead of and behind `origin/<branch>`: ahead with commits that belong to branches further up the stack, behind by the rebase the fork already has. The fork is the authority. The script refuses to touch a branch in that state and prints the backup-and-reset pair for it.

## Act on the status

| `RPA-RESULT` | What to do |
|---|---|
| `PROPAGATED` | Done. Report the stack in one short table, flagging any branch whose patches the replay changed. Stop. |
| `NOTHING-TO-DO` | Already current. Say so in one line and stop. |
| `PLAN` | `--dry-run`. Report the plan and ask whether to run it for real. |
| `CONFLICT` | The real work. See below. |
| `STALE` | The script printed a backup ref and a reset per stale branch. Confirm with Lev before running them, since a reset discards local commits, then re-run the script. |
| `BADBASE` | The cut-below commit is not in the branch's history. Find the real one in the parent's reflog (`git reflog show <parent>`, the entry just after its rebase) and re-run with `--old-base <rev>`. |
| `NO-WORKTREE` | A stack branch has no worktree. Report which, offer the `git worktree add` the output printed. |
| `DIRTY` | Uncommitted changes predate the command. Report them and ask whether to commit or drop. Don't decide. |
| `IN-PROGRESS` | A rebase was already running in a worktree. Report the state and ask how to proceed. |
| `TEST-FAILED` | Read the tail of the output, fix the break on that branch, commit, re-run. Nothing was pushed. |
| `REJECTED` | The fork moved under a branch. Do not override. Show `git log <branch>..origin/<branch>` and ask. |
| `PUSH-FAILED` | Read the output, fix the cause, re-run. Note which branches already pushed. |
| `ERROR` | Read the message, fix the precondition, re-run. |

## Resolving a `CONFLICT`

The rebase is **in progress** in the worktree named in the output, and nothing has been pushed. The branches above it are queued behind.

1. Read each conflicted file and both sides. `git -C <worktree> log -1 REBASE_HEAD` is the commit being replayed.
2. Resolve so **both** intents survive. When two changes are genuinely incompatible and nothing in the repo, the commit messages, or the PRs disambiguates, ask Lev rather than picking a side.
3. `git -C <worktree> add <files>` then `git -C <worktree> rebase --continue`, repeating for further conflicts on that branch.
4. Re-run the script. It resumes from its state file, including the branches already rebased, and pushes the whole stack once every rebase lands. Do **not** push by hand: a partial push is the inconsistent state this avoids.
5. Later branches may conflict too. Repeat until `RPA-RESULT: PROPAGATED`.

A conflict that reaches dozens of commits deep is almost always a wrong cut-below commit rather than a genuine collision. Abort it (`git -C <worktree> rebase --abort`), check `git merge-base --is-ancestor <old base> <branch>`, and fix the base instead of resolving hunks.
