---
name: record-proof
description: Record a video or take a screenshot of a dev alacritree build as proof of a change. Use when an issue's done-when list asks for a video or screenshot, or when asked to show a change working in the real window.
---

# Record proof

A take is one recorded run of a scenario script against an isolated dev window. `proof_kit.py` beside this file holds everything a take needs: it launches the dev build, drives it through the alacritree CLI, records the window with ffmpeg, and cuts the idle stretches. Windows only, and it needs an ffmpeg 8 or later on PATH, since older builds have no `gfxcapture`.

## Steps

1. Build the worktree with `devkit run task build`. The kit runs `target/debug/alacritree.exe`.
2. Write the scenario as a `.py` file in your scratchpad. The write harness cannot follow an inline `python -c` that imports the kit.
3. Run the scenario. A failed take raises. Fix the scenario and run it again, since every `Demo` starts from empty state.
4. Review the take. Read a contact sheet with `Demo.contact_sheet`, then single frames with `Demo.frame` wherever the sheet is unclear. The take is done when each claim the issue names is readable in a frame you looked at.
5. Check the take's privacy. The window shows only what the scenario made: no project, session or path the scenario did not create. A frame that fails the check ends the take: delete it and record again.
6. Run `Demo.finish_video(mkv, mp4)`. It cuts each idle stretch to a short hold, keeps a longer hold on the last one, and encodes an MP4 small enough to attach to a PR.
7. Put the MP4 and any screenshots under the worktree's `.devkit/proof/` and report their paths. Never attach media that failed step 5. An upload stays readable at its URL after the PR stops showing it.
8. Confirm that no dev window is left: `Get-Process alacritree` lists no process from `target\debug`.

## Scenario

```python
import subprocess, sys
from pathlib import Path

sys.path.insert(0, r"C:\Users\Lev\Git\github\alacritree\.agents\skills\record-proof")
from proof_kit import Demo

here = Path(__file__).parent
repo = here / "repo"
repo.mkdir(exist_ok=True)
subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)

with Demo(work=here / "run", worktree=r"<worktree>", core=True,
          overrides=["integrations.herdr_core.session_backend='herdr_core'"]) as demo:
    demo.add_project(repo)
    with demo.record(here / "take.mkv"):
        first = demo.select_workspace(repo)
        demo.wait_prompt(first)
        demo.type(first, "hc ls")
        demo.wait_for(first, lambda s: "running" in s, what="hc ls")
    demo.screenshot(here / "after.png")
Demo.contact_sheet(here / "take.mkv", here / "sheet.png")
```

`Demo.cli` runs any `alacritree` subcommand against the dev window: `project`, `session`, `workspace`, `action`. `alacritree --help` lists them. Use `wait_for` on `read-screen` text for every step that takes time, so the scenario does not rely on a fixed delay.

`demo.relaunch()` kills the window and starts it again on the same config, state and core. Inside `record`, the take goes on across it: the kit records one segment per window and joins them into the take.

## Config

Every take starts from `baseline/`, which holds the human's font settings and nothing else. Without them the prompt's Nerd Font glyphs and the sidebar text draw wrong. When the human changes their fonts, copy the new `[font]` and `[ui.font]` tables into `baseline/`.

Pass every other setting a scenario needs in `overrides`, one `key=value` string per setting, in the syntax of the app's `-o` flag. The kit passes each one as `-o`, after its own `general.state_dir` and `terminal.shell` settings. Do not edit `baseline/` to set up a scenario.

## Isolation

The kit keeps the take away from the human's own alacritree and herdr-core:

- `--config-dir` points the dev build at a scratch config. That flag does not move `state.toml`, so the kit also sets `-o general.state_dir=...`. Without it the dev window loads the human's projects and can write their state.
- With `core=True`, the kit starts a core of its own on its own `HC_SOCKET` pipe, `HC_SESSION` and XDG homes. Each `Demo` gets a unique pipe, so concurrent takes by other agents never share a core. Run `hc` only through `Demo.hc`. A bare `hc server stop` stops the human's core.
- The kit removes the variables your own terminal set, so the dev window starts with the environment of a fresh one. `NO_COLOR` strips the prompt's colours, `STARSHIP_*` loads the prompt of your shell, `HC_*` points `hc` at the human's core, and a Claude Code started in the take reads `CLAUDE_*` and warns that its transcript is off. When the window draws differently from the human's, look for a variable the kit still passes on.
- ffmpeg's `gfxcapture` records the dev window itself through Windows Graphics Capture. A frame holds that window's pixels only, so other windows and the desktop never reach a take, even when they cover the window. Never record with `gdigrab` or any other desktop capture.
- The kit sizes the window but leaves it where it is in the z-order and never focuses it, so a take does not cover the human's windows or another agent's take. Concurrent takes do not see each other.

If a dev window ever shows the human's projects, kill it at once with `Stop-Process -Force`. A hard kill writes no state.

## What the screen shows

- The prompt glyph is `❯`, not `>`. `wait_prompt` checks the last screen line for it.
- `workspace select` opens a session in that worktree. `select_workspace` returns it.
- The sidebar lists session rows only when a worktree holds two or more sessions. Open a second session before you show a row's cwd, foreground or agent status.
- Closing the last session of a selected worktree opens a new one.
- Claude Code's status line shows the human's email and spending. Start it with `claude --settings <file>`, where the file sets `{"statusLine":{"type":"command","command":"echo demo"}}`.
- Claude Code asks whether to trust a new folder, and its default answer exits. Send `DOWN`, then an empty `--enter`.
- PSReadLine shows a gray history suggestion while the kit types. It is not part of the command.
- When someone resizes the window during a take, call `demo.pin()` and record the take again. The capture keeps its first frame size and adds black bars around a smaller window.

## Tools

- `mpdecimate` finds idle stretches because it compares 8x8 blocks, so one typed character counts as a change. `freezedetect` measures the whole frame and calls typing idle.
- ProofShot records a browser session and cannot capture a native window.
- The Bash tool collapses `\\` in command text, so a pipe name such as `\\.\pipe\...` loses a backslash. Build pipe names in Python.
- The Write tool saves CRLF line endings. A shell file sourced from bash then carries a `\r` into every value.
