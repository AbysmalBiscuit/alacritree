#!/usr/bin/env python3
"""Build the all-features alacritree in release mode and install it into
`~/.local/bin`.

On Windows the binary travels with the vendored console host it loads by name,
and a running alacritree pins both.  The install writes to a temporary name and
moves it into place; when the move is refused because the target is running, the
target is renamed aside first, which Windows allows even for a live executable.
The leftovers are swept on a later run, once the process holding them has
exited.  Elsewhere a replace over a running binary just works and the aside path
is never taken.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

DEFAULT_BRANCH = "integration/all-features"

# The same markers as alacritree/src/stale_exe.rs, so either side sweeps the
# other's leftovers and neither name is ever picked up by a PATH lookup.
STALE_MARKER = ".stale-"
TEMP_MARKER = ".tmp-"

WINDOWS_PAYLOAD = ("alacritree.exe", "conpty.dll", "OpenConsole.exe")
UNIX_PAYLOAD = ("alacritree",)


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout


def worktree_for(start: Path, branch: str) -> Path:
    path: Path | None = None
    for line in git(start, "worktree", "list", "--porcelain").splitlines():
        if line.startswith("worktree "):
            path = Path(line[len("worktree ") :])
        elif line == f"branch refs/heads/{branch}" and path is not None:
            return path.resolve()
    sys.exit(f"no worktree is checked out on {branch} (git worktree add one first)")


def ocargo() -> Path | None:
    """Locate `ocargo`, the cargo wrapper that turns on the optimized profile.

    It is a Python script, so callers run it through this interpreter rather
    than through PATH: a bare `ocargo.py` is only executable where PATHEXT
    says so.
    """
    installed = Path.home() / ".local" / "bin" / "ocargo.py"
    if installed.is_file():
        return installed
    found = shutil.which("ocargo.py")
    return Path(found) if found else None


def build_command(worktree: Path, cargo: str | None) -> list[str]:
    manifest = str(worktree / "Cargo.toml")
    arguments = ["build", "-p", "alacritree", "--release", "--manifest-path", manifest]
    if cargo:
        return [*cargo.split(), *arguments]
    wrapper = ocargo()
    if wrapper:
        return [sys.executable, str(wrapper), *arguments]
    return ["cargo", *arguments]


def sweep(directory: Path, payload: tuple[str, ...]) -> None:
    """Delete leftovers from an earlier install.

    One whose process is still running refuses deletion and waits for the next
    sweep, which is why nothing here treats a failure as an error.
    """
    names = "|".join(re.escape(name) for name in payload)
    markers = f"({re.escape(STALE_MARKER)}|{re.escape(TEMP_MARKER)})"
    leftover = re.compile(f"^({names}){markers}")
    for path in sorted(directory.iterdir()):
        if not path.is_file() or not leftover.match(path.name):
            continue
        try:
            path.unlink()
        except OSError:
            continue
        print(f"  swept {path.name}")


def free_name(target: Path, marker: str) -> Path:
    for attempt in range(1000):
        candidate = target.with_name(f"{target.name}{marker}{os.getpid()}-{attempt}")
        if not candidate.exists():
            return candidate
    sys.exit(f"could not find a free name beside {target}")


def install(source: Path, target: Path) -> None:
    # Staged under a temporary name first: a copy that fails partway must not
    # leave the destination without a working binary.
    temp = free_name(target, TEMP_MARKER)
    shutil.copy2(source, temp)
    try:
        os.replace(temp, target)
    except PermissionError:
        aside = free_name(target, STALE_MARKER)
        os.rename(target, aside)
        os.replace(temp, target)
        print(f"  {target.name} was in use, moved aside as {aside.name}")
    print(f"  installed {target.name}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--branch", default=DEFAULT_BRANCH, help="worktree to build from"
    )
    parser.add_argument(
        "--destination",
        type=Path,
        default=Path.home() / ".local" / "bin",
        help="install directory",
    )
    parser.add_argument(
        "--skip-build", action="store_true", help="install what is already built"
    )
    parser.add_argument(
        "--cargo",
        help='build command to use instead of ocargo or cargo, e.g. "cargo +nightly"',
    )
    args = parser.parse_args()

    payload = WINDOWS_PAYLOAD if os.name == "nt" else UNIX_PAYLOAD
    worktree = worktree_for(Path(__file__).resolve().parent, args.branch)
    release = worktree / "target" / "release"

    if not args.skip_build:
        command = build_command(worktree, args.cargo)
        print(f"building {args.branch} ({worktree})")
        if subprocess.run(command).returncode != 0:
            sys.exit("cargo build failed")

    sources = [release / name for name in payload if (release / name).exists()]
    if not any(source.name == payload[0] for source in sources):
        sys.exit(f"no {payload[0]} in {release}")

    args.destination.mkdir(parents=True, exist_ok=True)
    print(f"installing into {args.destination}")
    sweep(args.destination, payload)
    for source in sources:
        install(source, args.destination / source.name)

    stamp = datetime.fromtimestamp((args.destination / payload[0]).stat().st_mtime)
    print(f"done - {stamp:%Y-%m-%d %H:%M:%S}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
