"""Run an isolated dev alacritree, drive it through its CLI, and capture it.

Windows only: it places the window with user32 and records it with ffmpeg's
`gfxcapture` filter, which reads the window itself through Windows Graphics
Capture. The capture holds the window's pixels only, so no other window and
no part of the desktop can reach a take. `SKILL.md` beside it shows a scenario.
"""

import ctypes
import ctypes.wintypes
import json
import os
import re
import shutil
import subprocess
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

PROMPT = "❯"
DOWN = "\x1b[B"
UP = "\x1b[A"
ESC = "\x1b"

# Variables of the agent's own terminal, which a fresh window does not have.
# Inherited, they strip the prompt's colours and point `hc` at the human's core.
SESSION_VARS = (
    "ALACRITREE_SOCKET", "ALACRITREE_SESSION_ID", "COLORTERM", "DEVKIT_SESSION", "NO_COLOR",
    "TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION",
)
SESSION_PREFIXES = ("CLAUDE", "HC_", "STARSHIP_", "TRANSIENT_PROMPT_")

# The config every take starts from: the human's font settings only.
BASELINE = Path(__file__).parent / "baseline"
# The scenarios wait for the pwsh prompt glyph, and the human's own shell is nu.
SHELL = 'terminal.shell={program="pwsh",args=["-NoLogo"]}'

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetProcessDPIAware()
user32.SetWindowPos.argtypes = [ctypes.wintypes.HWND, ctypes.wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                ctypes.c_int, ctypes.c_int, ctypes.c_uint]
user32.GetWindowRect.argtypes = [ctypes.wintypes.HWND, ctypes.POINTER(ctypes.wintypes.RECT)]
# Move and size only. The window keeps its place in the z-order and never
# takes focus, so a take covers neither the human's windows nor another take's.
SWP_NOZORDER_NOACTIVATE = 0x0004 | 0x0010
SW_SHOWNOACTIVATE = 4


def _ffmpeg():
    """The first `ffmpeg` on PATH that has `gfxcapture`, which arrived in ffmpeg 8."""
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(directory) / "ffmpeg.exe"
        if candidate.is_file():
            probe = subprocess.run([str(candidate), "-hide_banner", "-h", "filter=gfxcapture"],
                                   capture_output=True, text=True)
            if "gfxcapture AVOptions" in probe.stdout:
                return candidate
    raise RuntimeError("no ffmpeg on PATH has gfxcapture; install ffmpeg 8 or later")


FFMPEG = _ffmpeg()
FFPROBE = FFMPEG.with_name("ffprobe.exe")


def window_of(pid):
    """The first visible top-level window of process `pid`, or None."""
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM)
    def visit(hwnd, _):
        owner = ctypes.wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(visit, 0)
    return found[0] if found else None


class Demo:
    """One dev window with its own config, state and, optionally, its own core."""

    def __init__(self, work, worktree, core=False, overrides=(), rect=(80, 80, 1440, 900)):
        """`overrides` holds `key=value` settings, passed to the app as `-o` over the baseline config."""
        self.work = Path(work).resolve()
        self.app = Path(worktree) / "target" / "debug" / "alacritree.exe"
        self.core = core
        self.rect = rect
        self.process = None
        self.socket = None
        self.launches = 0
        self.recorder = None
        self.segments = []

        config = self.work / "config"
        state = self.work / "app-state"
        # A take starts from nothing, so a failed earlier take leaves no projects or terminals.
        for leftover in (state, self.work / "hc-state"):
            shutil.rmtree(leftover, ignore_errors=True)
        for directory in (config, state, self.work / "hc-state", self.work / "hc-config"):
            directory.mkdir(parents=True, exist_ok=True)
        for name in ("alacritty.toml", "alacritree.toml"):
            shutil.copyfile(BASELINE / name, config / name)

        self.env = {k: v for k, v in os.environ.items()
                    if k not in SESSION_VARS and not k.startswith(SESSION_PREFIXES)}
        if core:
            # A pipe per demo, since takes that share a core adopt each other's
            # terminals. hc needs its backslashes, else os error 123.
            tag = uuid.uuid4().hex[:12]
            self.env["HC_SOCKET"] = "\\\\.\\pipe\\alacritree-proof-" + tag
            self.env["HC_SESSION"] = "alacritree-proof-" + tag
            self.env["XDG_STATE_HOME"] = str(self.work / "hc-state")
            self.env["XDG_CONFIG_HOME"] = str(self.work / "hc-config")
        self.args = [str(self.app), "--config-dir", str(config)]
        for setting in (f"general.state_dir='{state.as_posix()}'", SHELL, *overrides):
            self.args += ["-o", setting]

    def __enter__(self):
        try:
            if self.core:
                self.hc("server", "start")
            self._launch()
        except BaseException:
            # `with` runs `__exit__` only once `__enter__` returns.
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *_):
        self._stop_segment()
        # The window goes first: stopping the core under it makes every
        # session print the lost-connection notice.
        if self.process and self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        if self.core:
            self.hc("server", "stop")

    def _launch(self):
        log = self.work / ("alacritree.log" if self.launches == 0 else f"alacritree-{self.launches}.log")
        self.launches += 1
        self.process = subprocess.Popen([*self.args, "--log-file", str(log)], env=self.env, cwd=self.work,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.socket = f"\\\\.\\pipe\\alacritree-{self.process.pid}.sock"
        deadline = time.time() + 60
        while not self.window() or not self._answers():
            if self.process.poll() is not None:
                raise RuntimeError(f"the dev window exited with {self.process.returncode}; see {log}")
            if time.time() > deadline:
                raise TimeoutError("the dev window never opened its IPC pipe")
            time.sleep(0.5)
        self.pin()

    def relaunch(self):
        """Kill the window and start it again on the same config, state and core.

        A take that is recording goes on. The capture follows one window, so
        the kit ends the segment before the kill and starts one on the new window.
        """
        recording = self.recorder is not None
        self._stop_segment()
        self.process.kill()
        self.process.wait()
        self._launch()
        if recording:
            self._start_segment()

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
        # The core's own directory is where a terminal opens when nothing names one.
        return subprocess.run(["hc", *args], env=self.env, cwd=self.work, capture_output=True, text=True).stdout

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
        return window_of(self.process.pid)

    def pin(self):
        """Give the window the size of `rect`, so every segment of a take has one frame size."""
        hwnd = self.window()
        x, y, w, h = self.rect
        user32.ShowWindow(hwnd, SW_SHOWNOACTIVATE)
        # The app applies its configured size after the window first shows, so
        # repeat until the window holds the rect.
        actual = ctypes.wintypes.RECT()
        for _ in range(40):
            user32.SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER_NOACTIVATE)
            time.sleep(0.25)
            user32.GetWindowRect(hwnd, ctypes.byref(actual))
            if (actual.right - actual.left, actual.bottom - actual.top) == (w, h):
                time.sleep(0.5)
                user32.GetWindowRect(hwnd, ctypes.byref(actual))
                if (actual.right - actual.left, actual.bottom - actual.top) == (w, h):
                    return
        raise RuntimeError(f"the window would not hold {self.rect}")

    def _capture(self):
        # A client area can have an odd side, and yuv420p needs even sides.
        # A negative size rounds the side down to a multiple of 2.
        source = f"gfxcapture=hwnd={self.window()}:capture_cursor=0:max_framerate=30:width=-2:height=-2"
        return ["-f", "lavfi", "-i", source, "-vf", "hwdownload,format=bgra"]

    def screenshot(self, path):
        subprocess.run([str(FFMPEG), "-hide_banner", "-loglevel", "error", "-y", *self._capture(),
                        "-frames:v", "1", str(path)], check=True)

    def _start_segment(self):
        path = self.work / f"segment-{len(self.segments)}.mkv"
        self.segments.append(path)
        self.recorder = subprocess.Popen(
            [str(FFMPEG), "-hide_banner", "-loglevel", "error", "-y", *self._capture(),
             "-c:v", "libx264", "-preset", "ultrafast", "-crf", "18", "-pix_fmt", "yuv420p", str(path)],
            stdin=subprocess.PIPE,
        )
        time.sleep(1.5)

    def _stop_segment(self):
        if self.recorder is not None:
            self.recorder.communicate(b"q")
            self.recorder = None

    @contextmanager
    def record(self, path):
        """Record the window into `path`, across relaunches. MKV survives an interrupted take."""
        self.segments = []
        self._start_segment()
        try:
            yield
            time.sleep(2)
        finally:
            self._stop_segment()
            listing = self.work / "segments.txt"
            listing.write_text("".join(f"file '{s.as_posix()}'\n" for s in self.segments), encoding="utf-8")
            subprocess.run([str(FFMPEG), "-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0",
                            "-i", str(listing), "-c", "copy", str(path)], check=True)

    # Review

    @staticmethod
    def contact_sheet(video, path, every=5, columns=3, rows=3):
        """Tile one frame per `every` seconds into one image to review a take at a glance."""
        subprocess.run([str(FFMPEG), "-hide_banner", "-loglevel", "error", "-y", "-i", str(video), "-vf",
                        f"fps=1/{every},scale=720:-1,tile={columns}x{rows}", "-frames:v", "1", str(path)],
                       check=True)

    @staticmethod
    def frame(video, at, path, crop=None):
        """One frame at `at` seconds (negative counts from the end), optionally `w:h:x:y` cropped."""
        seek = ["-sseof", str(at)] if at < 0 else ["-ss", str(at)]
        filters = ["-vf", f"crop={crop}"] if crop else []
        subprocess.run([str(FFMPEG), "-hide_banner", "-loglevel", "error", "-y", *seek, "-i", str(video),
                        "-frames:v", "1", *filters, str(path)], check=True)

    @staticmethod
    def idle_spans(video, idle=1.0):
        """`(start, end)` seconds of every stretch at least `idle` long where no 8x8 block changes.

        `mpdecimate` compares blocks, so one typed character counts as a change.
        A whole-frame measure such as `freezedetect` misses it.
        """
        out = subprocess.run([str(FFMPEG), "-hide_banner", "-i", str(video), "-vf", "mpdecimate,showinfo",
                              "-map", "0:v", "-f", "null", "-"], capture_output=True, text=True,
                             encoding="utf-8", errors="replace")
        kept = [float(t) for t in re.findall(r"pts_time:([0-9.]+)", out.stderr)]
        duration = float(subprocess.run(
            [str(FFPROBE), "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(video)],
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
        subprocess.run([str(FFMPEG), "-hide_banner", "-loglevel", "error", "-y", "-i", str(mkv), "-vf", filters,
                        "-c:v", "libx264", "-preset", "slow", "-crf", "24", "-pix_fmt", "yuv420p",
                        "-movflags", "+faststart", str(mp4)], check=True)
