#!/usr/bin/env python3
"""Replay the alacritree PR stack onto `upstream/master` after a merge, and
push every branch to the fork.

Usage: rebase_propagate_alacritree.py [--dry-run] [--test] [--old-base REV]

The stack is the open PRs on the upstream repository whose titles carry an
`[n]` marker, ordered by that marker.  Each branch lives in its own worktree,
is replayed with `--onto` because upstream squash-merges, and is pushed to the
fork under a lease.  Nothing is pushed until every rebase lands.

The last line of output is always `RPA-RESULT: <STATUS>`.  Why `--onto`, what
the preflight guards against, and what to do about each status: the SKILL.md
beside this script.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

FORK = "origin"
UPSTREAM = "upstream"
BASE = "master"

#: Where a branch with no worktree of its own is replayed.  One reused
#: worktree, detached when the run ends, rather than a permanent Rust checkout
#: per branch for a rebase that takes seconds.
SCRATCH = Path("../alacritree-worktrees/.rebase-propagate")

#: PR titles end in the stack position: `perf(jobs): bound the pool [6]`.
MARKER = re.compile(r"\[(\d+)\]\s*$")

TERMINAL = ("PROPAGATED", "NOTHING-TO-DO", "PLAN", "DIRTY", "ERROR")


def say(text: str = "") -> None:
    print(text, flush=True)


def run(*args: str, cwd: Path | None = None) -> tuple[int, str]:
    result = subprocess.run(
        args,
        cwd=None if cwd is None else str(cwd),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return result.returncode, (result.stdout + result.stderr).strip()


def git(*args: str, cwd: Path | None = None) -> str:
    code, out = run("git", *args, cwd=cwd)
    if code != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{out}")
    return out


def git_ok(*args: str, cwd: Path | None = None) -> bool:
    return run("git", *args, cwd=cwd)[0] == 0


def upstream_repo(root: Path) -> str:
    url = git("-C", str(root), "remote", "get-url", UPSTREAM)
    slug = re.sub(r"\.git$", "", url.rsplit(":", 1)[-1])
    return "/".join(slug.rstrip("/").split("/")[-2:])


def prs(repo: str, state: str, limit: int) -> list[dict]:
    code, out = run(
        "gh", "pr", "list", "--repo", repo, "--state", state, "--limit", str(limit),
        "--json", "number,title,headRefName",
    )
    if code != 0:
        sys.exit(f"gh pr list --state {state} failed:\n{out}")
    return json.loads(out)


def marked(rows: list[dict]) -> dict[int, dict]:
    """PRs that carry a stack marker, keyed by it.

    `gh` lists newest first and the first row of a marker wins, so a PR
    reopened under a marker that already shipped loses to the live one.
    """
    found: dict[int, dict] = {}
    for row in rows:
        hit = MARKER.search(row["title"])
        if hit:
            found.setdefault(int(hit.group(1)), row)
    return found


@dataclass
class Branch:
    marker: int
    name: str
    pr: int
    worktree: Path | None = None
    #: The tip this branch had when the run started, which is what the branch
    #: above it cuts below.
    old_tip: str = ""
    #: The fork's tip at fetch time, which becomes the push lease.
    lease: str = ""
    old_base: str = ""
    parent: str = ""
    rebased: bool = False
    #: No local ref yet, so the run creates one from the fork.
    create: bool = False
    note: str = ""

    @property
    def ref(self) -> str:
        """What to read the branch through before `prepare` has created it."""
        return f"{FORK}/{self.name}" if self.create else self.name


@dataclass
class Run:
    root: Path
    state: Path
    scratch: Path
    stack: list[Branch] = field(default_factory=list)

    def finish(self, status: str, code: int = 0) -> None:
        if status in TERMINAL:
            self.state.unlink(missing_ok=True)
        say("")
        say(f"RPA-RESULT: {status}")
        sys.exit(code)

    # ------------------------------------------------------------ state file

    def write_state(self) -> None:
        rows = [
            "\t".join([b.name, b.old_tip, b.old_base, b.parent, str(b.pr)])
            for b in self.stack
        ]
        self.state.write_text("\n".join(rows) + "\n", encoding="utf-8")

    def load_state(self) -> dict[str, tuple[str, str]]:
        """Each branch's recorded old tip and cut-below commit, by name."""
        try:
            text = self.state.read_text(encoding="utf-8")
        except OSError:
            return {}
        found: dict[str, tuple[str, str]] = {}
        for line in text.splitlines():
            parts = line.split("\t")
            if len(parts) >= 3:
                found[parts[0]] = (parts[1], parts[2])
        return found

    # ------------------------------------------------------------ discovery

    def discover(self, repo: str) -> None:
        open_prs = marked(prs(repo, "open", 50))
        merged = marked(prs(repo, "merged", 40))
        if not open_prs:
            say(f"no open PR on {repo} carries an [n] marker")
            self.finish("ERROR", 3)

        self.stack = [
            Branch(marker=n, name=open_prs[n]["headRefName"], pr=open_prs[n]["number"])
            for n in sorted(open_prs)
        ]

        bottom = self.stack[0]
        bottom.parent = f"{UPSTREAM}/{BASE}"
        below = merged.get(bottom.marker - 1)
        if below:
            bottom.old_base = f"{FORK}/{below['headRefName']}"
            bottom.note = f"cut below merged [{bottom.marker - 1}] #{below['number']}"
        for above, under in zip(self.stack[1:], self.stack):
            above.parent = under.name

    def locate(self) -> None:
        """Find each branch's worktree, and the tips this run reasons from."""
        checkouts: dict[str, Path] = {}
        path: Path | None = None
        for line in git("-C", str(self.root), "worktree", "list", "--porcelain").splitlines():
            if line.startswith("worktree "):
                path = Path(line[len("worktree "):])
            elif line.startswith("branch refs/heads/") and path is not None:
                checkouts[line[len("branch refs/heads/"):]] = path
        for b in self.stack:
            b.worktree = checkouts.get(b.name)
            code, tip = run("git", "-C", str(self.root), "rev-parse", "--verify", f"{FORK}/{b.name}")
            if code != 0:
                say("")
                say(f"[{b.marker}] PR #{b.pr} names head branch {b.name}, which {FORK} does not have")
                self.finish("ERROR", 3)
            b.lease = tip
            code, tip = run("git", "-C", str(self.root), "rev-parse", "--verify", b.name)
            b.create = code != 0
            b.old_tip = b.lease if b.create else tip

    # ------------------------------------------------------------ preflight

    def check_worktrees(self) -> None:
        """Refuse to start in any worktree the run would have to rebase in."""
        for b in self.stack:
            if b.worktree is None:
                say(f"[{b.marker}] {b.name}: no worktree, replaying in the scratch one")
                continue
            self.check_clean(b.worktree, f"[{b.marker}] {b.name}")
        if self.scratch.is_dir():
            self.check_clean(self.scratch, "the scratch worktree")

    def check_clean(self, tree: Path, label: str) -> None:
        marker = next(
            (
                name
                for name in ("rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD")
                if Path(git("-C", str(tree), "rev-parse", "--git-path", name)).exists()
            ),
            "",
        )
        if marker:
            say("")
            say(f"{label}: an operation is already in progress ({marker}) in {tree}")
            say(git("-C", str(tree), "status", "--short", "--branch"))
            say("")
            say("resolve it, 'git rebase --continue', then re-run this script")
            self.finish("IN-PROGRESS", 4)

        if git("-C", str(tree), "status", "--porcelain", "--untracked-files=no"):
            say("")
            say(f"{label}: uncommitted changes in {tree}")
            say(git("-C", str(tree), "status", "--short", "--untracked-files=no"))
            say("")
            say("commit or drop them, then re-run")
            self.finish("DIRTY", 5)

    def prepare(self) -> None:
        """Create the refs and the scratch worktree the rebase needs."""
        for b in self.stack:
            if b.create:
                git("-C", str(self.root), "branch", b.name, f"{FORK}/{b.name}")
                say(f"[{b.marker}] {b.name}: created at {FORK}/{b.name} ({b.lease[:9]})")
        if any(b.worktree is None for b in self.stack) and not self.scratch.is_dir():
            git(
                "-C", str(self.root), "worktree", "add", "--detach",
                str(self.scratch), f"{UPSTREAM}/{BASE}",
            )
            say(f"scratch worktree: {self.scratch}")
        for b in self.stack:
            if b.worktree is None:
                b.worktree = self.scratch

    def release_scratch(self) -> None:
        """Leave no branch checked out in the scratch worktree.

        A branch checked out anywhere is locked against every other worktree,
        and the scratch one is not where anybody works.
        """
        if self.scratch.is_dir():
            run("git", "-C", str(self.scratch), "checkout", "--detach", "--quiet")

    def check_freshness(self) -> None:
        """Make every local branch agree with the fork before anything replays.

        Several local copies here predate the last sweep: they carry commits
        belonging to branches further up the stack and are missing the rebase
        the fork already has.  Replaying one of those is the conflict storm
        this guards against.
        """
        stale = []
        for b in self.stack:
            if b.create or b.old_tip == b.lease:
                continue
            ahead, behind = git(
                "-C", str(self.root), "rev-list", "--left-right", "--count",
                f"{b.name}...{FORK}/{b.name}",
            ).split()
            if int(ahead) and not int(behind):
                b.note = f"{ahead} local commit(s) the fork has not seen"
                continue
            stale.append((b, ahead, behind))

        if not stale:
            return
        say("")
        say("== stale locals ==")
        for b, ahead, behind in stale:
            say(f"[{b.marker}] {b.name}: {ahead} ahead / {behind} behind {FORK}/{b.name}")
            say(f"      local {b.old_tip[:9]}   fork {b.lease[:9]}")
        say("")
        say("The fork carries the swept branch, so a local copy that is behind it is a")
        say("pre-sweep leftover whose extra commits belong to branches above it.")
        say("Keep a backup ref and take the fork's copy, per branch:")
        for b, _, _ in stale:
            backup = b.name.replace("/", "-")
            say(f"  git -C {self.root} branch backup/{backup}-stale {b.old_tip[:9]}")
            say(f"  git -C {b.worktree} reset --hard {FORK}/{b.name}")
        say("")
        say("then re-run this script.")
        self.finish("STALE", 6)

    def check_ancestry(self) -> None:
        bottom = self.stack[0]
        if git_ok("-C", str(self.root), "merge-base", "--is-ancestor", bottom.parent, bottom.ref):
            return
        if not bottom.old_base:
            say("")
            say(f"nothing merged at [{bottom.marker - 1}], so the commit to cut below is unknown")
            say("pass it: --old-base <rev>")
            self.finish("BADBASE", 7)
        if not git_ok(
            "-C", str(self.root), "rev-parse", "--verify", f"{bottom.old_base}^{{commit}}"
        ):
            say("")
            say(f"no such ref: {bottom.old_base}")
            self.finish("BADBASE", 7)
        if not git_ok(
            "-C", str(self.root), "merge-base", "--is-ancestor", bottom.old_base, bottom.ref
        ):
            say("")
            say(f"[{bottom.marker}] {bottom.name} does not descend from {bottom.old_base}")
            say("")
            say("git would ignore that argument and replay the whole divergent history")
            say("instead of erroring. Find the commit this branch was really cut above")
            say(f"({FORK}/<parent>, or the parent's reflog) and pass it: --old-base <rev>")
            self.finish("BADBASE", 7)

    # ------------------------------------------------------------ rebase

    def rebase_all(self, resumed: dict[str, tuple[str, str]]) -> None:
        say("")
        say("== rebase ==")
        for i, b in enumerate(self.stack):
            if i:
                b.old_base = self.stack[i - 1].old_tip

            if git_ok("-C", str(self.root), "merge-base", "--is-ancestor", b.parent, b.name):
                say(f"[{b.marker}] {b.name}: already on {b.parent}")
                continue

            if not git_ok(
                "-C", str(self.root), "merge-base", "--is-ancestor", b.old_base, b.name
            ):
                say("")
                say(f"[{b.marker}] {b.name} does not descend from {b.old_base[:9]}")
                say("refusing the rebase: git would replay the whole divergent history")
                self.finish("BADBASE", 7)

            say(f"[{b.marker}] {b.name}: --onto {b.parent} {b.old_base[:9]}")
            code, out = run(
                "git", "rebase", "--onto", b.parent, b.old_base, b.name, cwd=b.worktree
            )
            if code != 0:
                self.report_conflict(b, out)
            b.rebased = True
            new_tip = git("-C", str(self.root), "rev-parse", b.name)
            say(f"      {b.old_tip[:9]} -> {new_tip[:9]}  {self.drift(b, new_tip)}")

    def drift(self, b: Branch, new_tip: str) -> str:
        """Whether the replayed patches came out identical to the originals."""
        code, out = run(
            "git", "-C", str(self.root), "range-diff", "--no-color",
            f"{b.old_base}..{b.old_tip}", f"{b.parent}..{new_tip}",
        )
        if code != 0:
            return "range-diff unavailable"
        rows = [line for line in out.splitlines() if re.match(r"^\s*\d+:", line)]
        changed = [line for line in rows if not re.match(r"^\s*\d+:\s+\S+\s+=\s", line)]
        if not changed:
            return f"{len(rows)} commit(s), every patch unchanged"
        return f"{len(rows)} commit(s), {len(changed)} patch(es) CHANGED by the replay"

    def report_conflict(self, b: Branch, out: str) -> None:
        say("")
        say(out)
        say("")
        say(f"conflicted files in {b.worktree}:")
        say(git("-C", str(b.worktree), "diff", "--name-only", "--diff-filter=U"))
        say("")
        stopped = git("-C", str(b.worktree), "log", "--oneline", "-1", "REBASE_HEAD")
        say(f"stopped applying: {stopped}")
        waiting = [f"[{x.marker}] {x.name}" for x in self.stack[self.stack.index(b) + 1:]]
        say(f"still queued behind it: {', '.join(waiting) or 'none'}")
        say("")
        say("NOTHING HAS BEEN PUSHED. The fork is untouched.")
        say(f"resolve, 'git -C {b.worktree} add <files>', then")
        say(f"'git -C {b.worktree} rebase --continue', then re-run this script; it resumes")
        say("from its state file and pushes once every branch has landed.")
        self.finish("CONFLICT", 10)

    # ------------------------------------------------------------ test

    def test_all(self) -> None:
        say("")
        say("== test ==")
        for b in self.stack:
            if not b.rebased:
                say(f"[{b.marker}] {b.name}: unchanged, skipping")
                continue
            code, out = run("devkit", "run", "task", "test", "--dir", str(b.worktree))
            summary = next(
                (line for line in reversed(out.splitlines()) if "tests run" in line), ""
            )
            say(f"[{b.marker}] {b.name}: {summary or ('ok' if code == 0 else 'FAILED')}")
            if code != 0:
                say("")
                say(out[-4000:])
                say("")
                say("NOTHING HAS BEEN PUSHED.")
                self.finish("TEST-FAILED", 11)

    # ------------------------------------------------------------ push

    def push_all(self) -> None:
        say("")
        say("== push ==")
        pushed: list[str] = []
        for b in self.stack:
            if git("-C", str(self.root), "rev-parse", b.name) == b.lease:
                say(f"[{b.marker}] {b.name}: fork already matches")
                continue
            code, out = run(
                "git", "-C", str(self.root), "push", FORK, b.name,
                f"--force-with-lease={b.name}:{b.lease}",
            )
            say(f"[{b.marker}] {b.name}: {out.splitlines()[-1] if out else 'pushed'}")
            if code != 0:
                say("")
                say(out)
                say("")
                say(f"pushed so far: {', '.join(pushed) or 'none'}")
                if any(hint in out for hint in ("stale info", "fetch first", "non-fast-forward")):
                    say(f"the fork moved since the fetch, so the lease refused {b.name}")
                    self.finish("REJECTED", 20)
                self.finish("PUSH-FAILED", 21)
            pushed.append(b.name)

    # ------------------------------------------------------------ report

    def report(self) -> None:
        say("")
        say("== result ==")
        bottom = self.stack[0]
        clean = run(
            "git", "-C", str(self.root), "merge-tree", "--write-tree",
            f"{UPSTREAM}/{BASE}", bottom.name,
        )[0]
        for b in self.stack:
            tip = git("-C", str(self.root), "log", "--oneline", "-1", b.name)
            say(f"  [{b.marker}] #{b.pr} {b.name}  {tip}")
        say("")
        say(
            f"[{bottom.marker}] {bottom.name} merges into {UPSTREAM}/{BASE} "
            f"{'cleanly' if clean == 0 else 'WITH CONFLICTS'}"
        )


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--dry-run", "-n", action="store_true", help="print the plan, write nothing")
    parser.add_argument("--test", action="store_true", help="run the suite per branch before pushing")
    parser.add_argument(
        "--old-base",
        metavar="REV",
        help="the commit to cut below on the lowest branch, when no merged PR names it",
    )
    args = parser.parse_args()

    code, root_out = run("git", "rev-parse", "--show-toplevel")
    if code != 0:
        say(f"not inside a git repository: {Path.cwd()}")
        say("RPA-RESULT: ERROR")
        sys.exit(3)
    root = Path(root_out)
    common = Path(git("-C", str(root), "rev-parse", "--path-format=absolute", "--git-common-dir"))
    session = Run(
        root=root,
        state=common / "rebase-propagate-alacritree.state",
        scratch=(root / SCRATCH).resolve(),
    )
    resumed = session.load_state()

    # A dry run fetches too: every check below weighs a local branch against a
    # remote one, and stale remote refs make the plan a guess.
    say("== fetch ==")
    for remote in (FORK, UPSTREAM):
        say(f"{remote}: {run('git', '-C', str(root), 'fetch', '--prune', remote)[1] or 'ok'}")

    repo = upstream_repo(root)
    session.discover(repo)
    if args.old_base:
        session.stack[0].old_base = args.old_base
        session.stack[0].note = "cut below the commit given on the command line"
    session.locate()

    say("")
    say("== stack ==")
    tip = git("-C", str(root), "log", "--oneline", "-1", f"{UPSTREAM}/{BASE}")
    say(f"upstream {repo}, base {UPSTREAM}/{BASE} ({tip})")
    for b in session.stack:
        cut = f"  cut below {b.old_base}" if b.old_base else ""
        say(f"  [{b.marker}] #{b.pr} {b.name} -> {b.parent}{cut}")
        if b.create:
            say(f"        no local branch, taking {FORK}/{b.name} ({b.lease[:9]})")
        if b.note:
            say(f"        {b.note}")

    session.check_worktrees()
    session.check_freshness()
    session.check_ancestry()

    if resumed:
        say("")
        say(f"resuming the run recorded in {session.state.name}")
        for b in session.stack:
            if b.name in resumed:
                b.old_tip = resumed[b.name][0]
    if args.dry_run:
        session.finish("PLAN", 0)

    say("")
    say("== prepare ==")
    session.prepare()
    session.write_state()
    session.rebase_all(resumed)
    session.release_scratch()

    if not any(b.rebased for b in session.stack) and all(
        git("-C", str(root), "rev-parse", b.name) == b.lease for b in session.stack
    ):
        session.finish("NOTHING-TO-DO", 0)

    if args.test:
        session.test_all()
    session.push_all()
    session.report()
    session.finish("PROPAGATED", 0)


if __name__ == "__main__":
    main()
