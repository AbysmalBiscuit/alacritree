"""Run an isolated dev alacritree, drive it through its CLI, and capture it.

Windows only: it pins the window with user32 and records with ffmpeg's
gdigrab. `SKILL.md` beside it shows a scenario.
"""

import ctypes
import ctypes.wintypes
import json
import os
import re
import shutil
import subprocess
import time
from contextlib import contextmanager
from pathlib import Path

PROMPT = "❯"
DOWN = "\x1b[B"
UP = "\x1b[A"
ESC = "\x1b"

# Variables that tie a child process to the agent session running this
# script. A Claude Code started inside the demo inherits them otherwise and
# warns that its transcript is off.
SESSION_VARS = (
    "CLAUDECODE", "CLAUDE_AFK_TIMEOUT_MS", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED", "CLAUDE_CODE_SESSION_ID", "CLAUDE_EFFORT", "CLAUDE_PID",
    "HC_TERMINAL_ID", "ALACRITREE_SOCKET", "ALACRITREE_SESSION_ID", "DEVKIT_SESSION",
)

ALACRITTY_TOML = """\
[window]
dimensions = { columns = 100, lines = 28 }
padding = { x = 5, y = 1 }
decorations_theme_variant = "Dark"

[font]
size = 12

[colors.primary]
foreground = "#e5e5e5"
background = "#0B111A"

[terminal]
shell = { program = "pwsh", args = ["-NoLogo"] }
"""

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetProcessDPIAware()
# Without these, ctypes passes HWND_TOPMOST (-1) as a 32-bit int and the call fails.
user32.SetWindowPos.argtypes = [ctypes.wintypes.HWND, ctypes.wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                ctypes.c_int, ctypes.c_int, ctypes.c_uint]
user32.GetWindowRect.argtypes = [ctypes.wintypes.HWND, ctypes.POINTER(ctypes.wintypes.RECT)]
HWND_TOPMOST = ctypes.wintypes.HWND(-1)


class Demo:
    """One dev window with its own config, state and, optionally, its own core."""

    def __init__(self, work, worktree, core=False, alacritree_toml="", alacritty_toml=ALACRITTY_TOML,
                 rect=(80, 80, 1440, 900)):
        self.work = Path(work).resolve()
        self.app = Path(worktree) / "target" / "debug" / "alacritree.exe"
        self.core = core
        self.rect = rect
        self.process = None
        self.socket = None

        config = self.work / "config"
        state = self.work / "app-state"
        # A take starts from nothing, so a failed earlier take leaves no projects or terminals.
        for leftover in (state, self.work / "hc-state"):
            shutil.rmtree(leftover, ignore_errors=True)
        for directory in (config, state, self.work / "hc-state", self.work / "hc-config"):
            directory.mkdir(parents=True, exist_ok=True)
        (config / "alacritty.toml").write_text(alacritty_toml, encoding="utf-8")
        (config / "alacritree.toml").write_text(alacritree_toml, encoding="utf-8")

        self.env = {k: v for k, v in os.environ.items() if k not in SESSION_VARS}
        if core:
            # hc needs backslash paths: a forward-slash path under a verbatim
            # prefix fails with os error 123.
            self.env["HC_SOCKET"] = "\\\\.\\pipe\\alacritree-proof-" + self.work.name
            self.env["HC_SESSION"] = "alacritree-proof"
            self.env["XDG_STATE_HOME"] = str(self.work / "hc-state")
            self.env["XDG_CONFIG_HOME"] = str(self.work / "hc-config")
        self.args = [
            str(self.app), "--config-dir", str(config), "--log-file", str(self.work / "alacritree.log"),
            "-o", f"general.state_dir='{state.as_posix()}'",
        ]

    def __enter__(self):
        try:
            if self.core:
                self.hc("server", "start")
            self.process = subprocess.Popen(self.args, env=self.env, cwd=self.work,
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.socket = f"\\\\.\\pipe\\alacritree-{self.process.pid}.sock"
            deadline = time.time() + 60
            while not self.window() or not self._answers():
                if time.time() > deadline:
                    raise TimeoutError("the dev window never opened its IPC pipe")
                time.sleep(0.5)
            self.pin()
        except BaseException:
            # `with` runs `__exit__` only once `__enter__` returns.
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *_):
        # The window goes first: stopping the core under it makes every
        # session print the lost-connection notice.
        if self.process and self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        if self.core:
            self.hc("server", "stop")

    def _answers(self):
        try:
            self.cli("session", "list")
            return True
        except RuntimeError:
            return False

    # Driving

    def cli(self, *args):
        out = subprocess.run([str(self.app), "--socket", self.socket, *args], env=self.env,
                             capture_output=True, text=True, encoding="utf-8")
        if out.returncode != 0:
            raise RuntimeError(f"alacritree {' '.join(args)}: {out.stderr.strip()}")
        return out.stdout

    def hc(self, *args):
        if not self.core:
            raise RuntimeError("this demo has no core of its own")
        return subprocess.run(["hc", *args], env=self.env, capture_output=True, text=True).stdout

    def hc_terminals(self):
        return json.loads(self.hc("ls", "--all", "--json"))["terminals"]

    def sessions(self):
        """`(id, focused, line)` for every session `session list` prints."""
        rows = []
        for line in self.cli("session", "list").splitlines():
            focused = line.startswith("*")
            rows.append((int(line.lstrip("* ").split()[0]), focused, line))
        return rows

    def add_project(self, path):
        self.cli("project", "add", str(path))

    def select_workspace(self, path):
        """Focus a worktree and return the session that selecting it opens."""
        self.cli("workspace", "select", str(path))
        name = Path(path).name
        for _ in range(60):
            for session, focused, line in self.sessions():
                if focused and name in line:
                    return session
            time.sleep(0.3)
        raise TimeoutError(f"no focused session in {path}")

    def new_session(self, workspace=None):
        args = ["session", "create"] + (["--workspace", str(workspace)] if workspace else [])
        return int(self.cli(*args).strip().split()[-1])

    def screen(self, session):
        return self.cli("session", "read-screen", str(session))

    def wait_for(self, session, predicate, timeout=30, what="screen"):
        deadline = time.time() + timeout
        while time.time() < deadline:
            text = self.screen(session)
            if predicate(text):
                return text
            time.sleep(0.3)
        raise TimeoutError(f"{what}:\n{self.screen(session)}")

    def wait_prompt(self, session, timeout=30):
        def at_prompt(screen):
            lines = screen.rstrip().splitlines()
            return bool(lines) and PROMPT in lines[-1]

        return self.wait_for(session, at_prompt, timeout, "prompt")

    def type(self, session, text, pause=0.04, enter=True):
        """Type one character at a time, so the video shows typing."""
        for ch in text:
            self.cli("session", "send-text", str(session), ch)
            time.sleep(pause)
        if enter:
            self.cli("session", "send-text", str(session), "", "--enter")

    def keys(self, session, sequence):
        """Send raw bytes such as `DOWN` or `ESC`. Python passes them through argv intact."""
        self.cli("session", "send-text", str(session), sequence)

    # Capture

    def window(self):
        """The dev window's handle, or None while it is not up."""
        found = []

        @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM)
        def visit(hwnd, _):
            pid = ctypes.wintypes.DWORD()
            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value == self.process.pid and user32.IsWindowVisible(hwnd):
                found.append(hwnd)
            return True

        user32.EnumWindows(visit, 0)
        return found[0] if found else None

    def pin(self):
        """Put the window on top at `rect`. Call it again after anyone moves it."""
        hwnd = self.window()
        x, y, w, h = self.rect
        user32.ShowWindow(hwnd, 9)
        user32.SetForegroundWindow(hwnd)
        # The app applies its configured size after the window first shows, so
        # repeat until the window holds the rect; a smaller one leaks desktop.
        actual = ctypes.wintypes.RECT()
        for _ in range(40):
            user32.SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, 0x0040)
            time.sleep(0.25)
            user32.GetWindowRect(hwnd, ctypes.byref(actual))
            got = (actual.left, actual.top, actual.right - actual.left, actual.bottom - actual.top)
            if got == self.rect:
                time.sleep(0.5)
                user32.GetWindowRect(hwnd, ctypes.byref(actual))
                if (actual.right - actual.left, actual.bottom - actual.top) == (w, h):
                    return
        raise RuntimeError(f"the window would not hold {self.rect}")

    def _grab(self):
        x, y, w, h = self.rect
        return ["-f", "gdigrab", "-draw_mouse", "0", "-offset_x", str(x), "-offset_y", str(y),
                "-video_size", f"{w}x{h}", "-i", "desktop"]

    def screenshot(self, path):
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-framerate", "30",
                        *self._grab(), "-frames:v", "1", str(path)], check=True)

    @contextmanager
    def record(self, path):
        """Record the window region. MKV survives an interrupted take."""
        recorder = subprocess.Popen(
            ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-framerate", "30", *self._grab(),
             "-c:v", "libx264", "-preset", "ultrafast", "-crf", "18", "-pix_fmt", "yuv420p", str(path)],
            stdin=subprocess.PIPE,
        )
        time.sleep(1.5)
        try:
            yield
            time.sleep(2)
        finally:
            recorder.communicate(b"q")

    # Review

    @staticmethod
    def contact_sheet(video, path, every=5, columns=3, rows=3):
        """Tile one frame per `every` seconds into one image to review a take at a glance."""
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(video), "-vf",
                        f"fps=1/{every},scale=720:-1,tile={columns}x{rows}", "-frames:v", "1", str(path)],
                       check=True)

    @staticmethod
    def frame(video, at, path, crop=None):
        """One frame at `at` seconds (negative counts from the end), optionally `w:h:x:y` cropped."""
        seek = ["-sseof", str(at)] if at < 0 else ["-ss", str(at)]
        filters = ["-vf", f"crop={crop}"] if crop else []
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *seek, "-i", str(video),
                        "-frames:v", "1", *filters, str(path)], check=True)

    @staticmethod
    def idle_spans(video, idle=1.0):
        """`(start, end)` seconds of every stretch at least `idle` long where no 8x8 block changes.

        `mpdecimate` compares blocks, so one typed character counts as a change.
        A whole-frame measure such as `freezedetect` misses it.
        """
        out = subprocess.run(["ffmpeg", "-hide_banner", "-i", str(video), "-vf", "mpdecimate,showinfo",
                              "-map", "0:v", "-f", "null", "-"], capture_output=True, text=True,
                             encoding="utf-8", errors="replace")
        kept = [float(t) for t in re.findall(r"pts_time:([0-9.]+)", out.stderr)]
        duration = float(subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(video)],
            capture_output=True, text=True, check=True).stdout)
        edges = kept + [duration]
        return [(a, b) for a, b in zip(edges, edges[1:]) if b - a >= idle]

    @staticmethod
    def finish_video(mkv, mp4, idle=1.0, hold=0.8, end_hold=2.5):
        """Cut each idle stretch down to `hold` seconds and encode a take small enough for a PR.

        The last stretch keeps `end_hold`, so the final state stays readable.
        """
        spans = Demo.idle_spans(mkv, idle)
        holds = [hold] * (len(spans) - 1) + [end_hold] if spans else []
        cuts = [(a + h, b) for (a, b), h in zip(spans, holds) if b - a > h]
        drop = "+".join(f"between(t,{a:.3f},{b - 0.001:.3f})" for a, b in cuts)
        filters = f"select='not({drop})',setpts=N/FRAME_RATE/TB" if cuts else "null"
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(mkv), "-vf", filters,
                        "-c:v", "libx264", "-preset", "slow", "-crf", "24", "-pix_fmt", "yuv420p",
                        "-movflags", "+faststart", str(mp4)], check=True)
