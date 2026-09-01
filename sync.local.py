#!/usr/bin/env python3
"""Move the untracked local files and the working docs between the main
checkout and the `docs/specs-and-plans` branch.

Both sides hold files git ignores: the `.local.*` instructions and config at
the repository root, and everything under `docs/superpowers/`.  The branch is
the one place they are tracked, so a second machine can fetch them; the main
checkout is where they are read and written.  Nothing about that arrangement
tells git to carry a change from one side to the other, which is what this
does.

Direction is decided per file, not per run, so it does the right thing from
either checkout.  A file only one side has is copied to the other.  A file both
sides have that differs goes whichever way its modification time points, so
writing a spec in the main checkout pushes it onto the branch and pulling the
branch on a new machine seeds the checkout.  `--to-branch` and `--to-main`
override that when a modification time is not the truth, after a clone that
stamped every file at once.

Anything that reaches the branch is committed there, grouped so one run that
touches an instruction file and two specs produces three commits rather than
one describing all of it.  A file can also arrive on the branch's disk without
this script moving it, in which case both sides agree and only the commit is
missing; that is committed too, so the order the tools run in does not decide
whether the branch ends up carrying the change.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

BRANCH = "docs/specs-and-plans"

WORKING_DOCS = ("docs/superpowers/specs", "docs/superpowers/plans")

#: Root files git ignores that the branch carries, grouped by the commit each
#: one lands in.  A run that touches two groups produces two commits, because
#: an instruction file and a build script are not one change.
#:
#: Each entry is (scope, conventional-commit type, what to call them, files).
PAYLOAD = (
    ("agents", "docs", "the local agent instructions", ("AGENTS.local.md", "CLAUDE.local.md")),
    ("devkit", "chore", "the local devkit config", ("devkit.local.toml",)),
    ("scripts", "chore", "the local scripts", ("sync.local.py", "install.local.py")),
    ("superpowers", "docs", "the working-docs README", ("docs/superpowers/README.md",)),
)

TO_BRANCH = "to-branch"
TO_MAIN = "to-main"

#: Which checkout the running copy of this script sits in.  A third value is
#: possible: the script is itself part of the payload, so every worktree cut
#: after a sync carries a copy of it, and one of those can be the copy that
#: runs.
SIDE_MAIN = "main"
SIDE_BRANCH = "branch"


@dataclass(frozen=True)
class Move:
    """One file that differs, and which way it goes."""

    relative: str
    direction: str
    #: The destination has no such file, which is what separates "add" from
    #: "update" in the commit subject.
    created: bool


def git(repo: Path, *args: str, check: bool = True) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, encoding="utf-8"
    )
    if check and result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed in {repo}:\n{result.stderr.strip()}")
    return result.stdout


def worktrees(start: Path) -> list[tuple[Path, str | None]]:
    """Every worktree of the repository `start` sits in, main checkout first."""
    entries: list[tuple[Path, str | None]] = []
    path: Path | None = None
    branch: str | None = None
    for line in git(start, "worktree", "list", "--porcelain").splitlines():
        if line.startswith("worktree "):
            path, branch = Path(line[len("worktree ") :]), None
        elif line.startswith("branch refs/heads/"):
            branch = line[len("branch refs/heads/") :]
        elif not line.strip() and path is not None:
            entries.append((path, branch))
            path, branch = None, None
    if path is not None:
        entries.append((path, branch))
    return entries


def locate(start: Path) -> tuple[Path, Path, str | None]:
    """The main checkout, the branch's worktree, and which of them `start` is.

    Direction is decided per file, so the side a run starts from changes
    nothing about what moves.  It is worth naming anyway: a copy of this
    script sits in both checkouts, and a run that reports only the two paths
    reads the same from either, which is exactly when a surprising result is
    hardest to account for.  `None` means neither, which is what running a
    copy carried into some other worktree looks like.
    """
    entries = worktrees(start)
    if not entries:
        sys.exit(f"{start} is not inside a git worktree")
    # `git worktree list` names the main checkout first, run from wherever.
    main = entries[0][0].resolve()
    branch_path = next((path.resolve() for path, name in entries if name == BRANCH), None)
    if branch_path is None:
        sys.exit(
            f"no worktree is checked out on {BRANCH}. Create one with:\n"
            f"  git worktree add ../alacritree-worktrees/{BRANCH} {BRANCH}"
        )
    side = SIDE_MAIN if start == main else SIDE_BRANCH if start == branch_path else None
    return main, branch_path, side


def tracked_files(main: Path, branch: Path) -> list[str]:
    """Every path either side offers, as repository-relative strings."""
    names = [name for _, _, _, group in PAYLOAD for name in group]
    for directory in WORKING_DOCS:
        found: set[str] = set()
        for root in (main, branch):
            found.update(path.name for path in (root / directory).glob("*.md"))
        names.extend(f"{directory}/{name}" for name in sorted(found))
    return names


def content(path: Path) -> bytes:
    """A file's bytes with line endings normalized.

    The two checkouts can disagree about line endings without disagreeing
    about anything that matters: `core.autocrlf` decides them per checkout, so
    a file freshly checked out on the branch differs from a Windows main
    checkout on every line.  Comparing raw bytes would read that as an edit
    and copy the file back and forth on alternate runs, each copy looking like
    a change to commit.
    """
    return path.read_bytes().replace(b"\r\n", b"\n")


def compare(main: Path, branch: Path, relative: str, forced: str | None) -> Move | None:
    """How `relative` differs across the two checkouts, if it does."""
    here, there = main / relative, branch / relative
    if not here.exists() and not there.exists():
        return None
    if not there.exists():
        return Move(relative, TO_BRANCH, True)
    if not here.exists():
        return Move(relative, TO_MAIN, True)
    if content(here) == content(there):
        return None
    if forced:
        return Move(relative, forced, False)
    return Move(relative, TO_BRANCH if here.stat().st_mtime > there.stat().st_mtime else TO_MAIN, False)


def uncommitted(branch: Path, names: list[str]) -> list[Move]:
    """Payload the branch's worktree holds on disk but has not committed.

    `devkit issue sync-includes` copies the instruction files into every
    worktree, this branch's included, so a file can reach the branch without
    passing through here.  Both sides then agree and `compare` reports nothing,
    which would leave the change sitting uncommitted where the next run cannot
    see it either.
    """
    if not names:
        return []
    changed = set(git(branch, "diff", "--name-only", "HEAD", "--", *names).split())
    return [Move(name, TO_BRANCH, False) for name in names if name in changed]


def apply(move: Move, main: Path, branch: Path) -> None:
    source, destination = (main, branch) if move.direction == TO_BRANCH else (branch, main)
    target = destination / move.relative
    target.parent.mkdir(parents=True, exist_ok=True)
    # `copy2` carries the modification time across, so the next run sees two
    # files that agree rather than one that looks newer for having been copied.
    shutil.copy2(source / move.relative, target)


def topic(relative: str) -> str:
    """The piece of work a working document belongs to.

    `2026-08-31-decoration-metrics-design.md` and
    `2026-08-31-decoration-metrics.md` are one piece of work, so they land in
    one commit rather than two describing halves of it.
    """
    stem = Path(relative).stem
    stem = re.sub(r"^\d{4}-\d{2}-\d{2}-", "", stem)
    return re.sub(r"-(design|progress)$", "", stem)


def payload_subject(scope: str, kind: str, description: str, moves: list[Move]) -> str:
    verb = "add" if all(move.created for move in moves) else "update"
    return f"{kind}({scope}): {verb} {description}"


def docs_subject(slug: str, moves: list[Move]) -> str:
    kinds = [
        label
        for directory, label in zip(WORKING_DOCS, ("spec", "plan"))
        if any(move.relative.startswith(f"{directory}/") for move in moves)
    ]
    verb = "add" if all(move.created for move in moves) else "update"
    subject = f"docs: {verb} the {slug.replace('-', ' ')} {' and '.join(kinds)}"
    # A long topic can push the subject past the 72-column limit, and the
    # topic is the part worth keeping.
    return subject if len(subject) <= 72 else f"docs: {verb} the {slug.replace('-', ' ')} docs"


def commits(moves: list[Move]) -> list[tuple[str, list[Move]]]:
    """Group what reached the branch into one commit per logical change."""
    pending = {move.relative: move for move in moves if move.direction == TO_BRANCH}
    grouped: list[tuple[str, list[Move]]] = []

    for scope, kind, description, group in PAYLOAD:
        claimed = [pending.pop(name) for name in group if name in pending]
        if claimed:
            grouped.append((payload_subject(scope, kind, description, claimed), claimed))

    by_topic: dict[str, list[Move]] = {}
    for move in pending.values():
        by_topic.setdefault(topic(move.relative), []).append(move)
    for slug in sorted(by_topic):
        claimed = sorted(by_topic[slug], key=lambda move: move.relative)
        grouped.append((docs_subject(slug, claimed), claimed))

    return grouped


def message(subject: str, moves: list[Move], trailers: list[str]) -> str:
    parts = [subject]
    if len(moves) > 1:
        parts.append("\n".join(f"- {move.relative}" for move in moves))
    if trailers:
        parts.append("\n".join(trailers))
    return "\n\n".join(parts)


def commit(branch: Path, subject: str, moves: list[Move], trailers: list[str]) -> None:
    paths = [move.relative for move in moves]
    # Everything here is ignored on every branch, this one included, so it
    # never stages without `-f`.
    git(branch, "add", "-f", "--", *paths)
    staged = subprocess.run(
        ["git", "-C", str(branch), "diff", "--cached", "--quiet", "--", *paths]
    )
    if staged.returncode == 0:
        return
    git(branch, "commit", "-m", message(subject, moves, trailers))


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "-n", "--dry-run", action="store_true", help="report what would move, write nothing"
    )
    direction = parser.add_mutually_exclusive_group()
    direction.add_argument(
        "--to-branch",
        action="store_true",
        help="the main checkout wins every difference, whatever the modification times say",
    )
    direction.add_argument(
        "--to-main", action="store_true", help="the branch wins every difference"
    )
    parser.add_argument(
        "--no-commit", action="store_true", help="copy onto the branch but leave it uncommitted"
    )
    parser.add_argument("--push", action="store_true", help="push the branch when it has commits")
    parser.add_argument(
        "--trailer",
        action="append",
        default=[],
        metavar="LINE",
        help="append a commit trailer, repeatable (an agent passes its Co-Authored-By here)",
    )
    args = parser.parse_args()

    forced = TO_BRANCH if args.to_branch else TO_MAIN if args.to_main else None
    here = Path(__file__).resolve().parent
    main_checkout, branch, side = locate(here)
    running = "  <- running here"
    print(f"main   {main_checkout}{running if side == SIDE_MAIN else ''}")
    print(f"branch {branch}{running if side == SIDE_BRANCH else ''}")
    if side is None:
        print(f"\nrunning from {here}, which is neither; it syncs the two above")
    print()

    names = tracked_files(main_checkout, branch)
    moves = [
        move
        for move in (compare(main_checkout, branch, relative, forced) for relative in names)
        if move is not None
    ]
    moved = {move.relative for move in moves}
    pending = [move for move in uncommitted(branch, names) if move.relative not in moved]
    if not moves and not pending:
        print("nothing to sync")
        return 0

    for move in moves:
        arrow = "->" if move.direction == TO_BRANCH else "<-"
        print(f"  {'add ' if move.created else 'edit'} {arrow} {move.relative}")
    for move in pending:
        print(f"  keep -> {move.relative}  (already on the branch, uncommitted)")

    if args.dry_run:
        print("\ndry run, nothing written")
        return 0

    for move in moves:
        apply(move, main_checkout, branch)

    if args.no_commit:
        print("\ncopied; the branch is left uncommitted")
        return 0

    grouped = commits(moves + pending)
    if grouped:
        print()
    for subject, group in grouped:
        commit(branch, subject, group, args.trailer)
        print(f"  {subject}")

    if args.push and grouped:
        git(branch, "push")
        print("\npushed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
