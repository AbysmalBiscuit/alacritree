#!/usr/bin/env bash
# Does the loop wake for a keystroke that came from a keyboard?
#
# `load-latency-gui.sh` types from inside the app, so the frame carrying a
# character is one the app asked a timer for.  Its lateness therefore measures
# the timer wake, and says nothing about a key that arrives as a window message
# — which can wake a loop the timer would not, and cannot wake one blocked in
# the swap.
#
# So this types from outside with SendInput, and reads the answer off the frame
# rate rather than off a new metric: keys arrive at a known cadence, and a loop
# that wakes for each of them runs about that many frames a second.  A loop
# still running at the rate it managed without them is not being woken at all.
#
#   real-keys-gui.sh 64 125          # 64 burners, 125s of typing
#
# The window opens and its shell reaches a prompt before the load starts, for
# the same reason as the synthetic bench: the complaint is about a terminal
# that was already open when the machine got busy.
#
# The window must hold focus throughout; the typist checks before every
# character and reports how many it withheld, so a run that lost focus says so
# instead of quietly measuring nothing.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
keys_every="${KEYS_EVERY_MS:-60}"
settle="${SETTLE_SECONDS:-20}"
workspace="${GLAZE_WORKSPACE:-1}"
logs="${LOCALAPPDATA:-$APPDATA}/alacritree"

load="${1:?usage: real-keys-gui.sh <load> <seconds>}"
seconds="${2:?usage: real-keys-gui.sh <load> <seconds>}"

app=""
holder=""
typist=""
listing=""
cleanup() {
  [ -n "$typist" ] && kill "$typist" 2>/dev/null || true
  [ -n "$app" ] && kill "$app" 2>/dev/null || true
  [ -n "$holder" ] && kill "$holder" 2>/dev/null || true
  rm -f "$listing"
}
trap cleanup EXIT INT TERM

# The window is found by the pipe that appeared when it launched, not by its
# log: a busy daily-driver window always owns the most recently written one,
# and typing into that is the one mistake this bench must not make.
listing="$(mktemp --suffix=.ps1)"
printf '%s\n' \
  '$dir = [char]92 + [char]92 + "." + [char]92 + "pipe" + [char]92' \
  '[System.IO.Directory]::GetFiles($dir) | Where-Object { $_ -like "*alacritree-*.sock" }' \
  > "$listing"
pipes() {
  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$listing")" | tr -d '\r' | sort
}

before_pipes="$(pipes)"
before_log="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1 || true)"

glazewm command focus --workspace "$workspace" >/dev/null
ALACRITREE_FRAME_LOG=1 "$exe" &
app="$!"

sock=""
for _ in $(seq 1 90); do
  fresh="$(comm -13 <(printf '%s\n' "$before_pipes") <(pipes))"
  [ "$(printf '%s' "$fresh" | grep -c . || true)" = 1 ] && { sock="$fresh"; break; }
  sleep 1
done
[ -n "$sock" ] || { echo "the window never started listening" >&2; exit 1; }
pid="$(printf '%s' "$sock" | sed 's/.*alacritree-//; s/\.sock$//')"
echo "instance pid $pid, typing every ${keys_every}ms for ${seconds}s"

# Window up and shell at its prompt on an idle machine, then the load.
sleep "$settle"
"$probe" --load "$load" --hold "$((seconds + 30))" &
holder=$!
sleep 3

powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$here/real-keys.ps1")" \
  -TargetPid "$pid" -Seconds "$seconds" -EveryMs "$keys_every" &
typist=$!

wait "$typist" || true
typist=""
kill "$app" 2>/dev/null || true
wait "$app" 2>/dev/null || true
cleanup; app=""; holder=""

after="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1)"
if [ "$after" = "$before_log" ]; then
  echo "no new session log — is [debug] gpu_timing or persistent_logging on?"
  exit 1
fi
# The windows covering startup and the idle settle carry no keystrokes.
rg -N "frames:" "$after" | tail -n "+$(((settle + 3) / 5 + 1))" || echo "no frame reports"
