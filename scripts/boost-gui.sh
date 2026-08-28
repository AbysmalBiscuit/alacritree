#!/usr/bin/env bash
# Does `[ui] shell_priority_boost` fix typing latency in the real app?
#
# The headless probe already showed a boosted shell echoing in 10 ms where an
# unboosted one took seconds, and the wiring check showed the option reaching
# the child.  This is the composition: the window, its sidebars, its git polls
# and a keyboard typing into it, with the load running.
#
#   boost-gui.sh 64 60 off on
#
# Arguments are the burner count, the seconds of typing per arm, and one arm
# per setting.  `echo` in the frame report is the number that moves: it brackets
# the keystroke's write to the PTY and the answer coming back, which is the
# whole of what the load stretches.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
logs="${LOCALAPPDATA:-$APPDATA}/alacritree"
settle="${SETTLE_SECONDS:-15}"
workspace="${GLAZE_WORKSPACE:-1}"

load="${1:?usage: boost-gui.sh <load> <seconds> <arm>...}"
seconds="${2:?usage: boost-gui.sh <load> <seconds> <arm>...}"
shift 2

app=""; holder=""; bench=""; listing=""
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null || true
  [ -n "$holder" ] && kill "$holder" 2>/dev/null || true
  rm -f "$listing"
  [ -n "$bench" ] && rm -rf "$bench" || true
}
trap cleanup EXIT INT TERM

listing="$(mktemp --suffix=.ps1)"
printf '%s\n' \
  '$dir = [char]92 + [char]92 + "." + [char]92 + "pipe" + [char]92' \
  '[System.IO.Directory]::GetFiles($dir) | Where-Object { $_ -like "*alacritree-*.sock" }' \
  > "$listing"
pipes() {
  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$listing")" | tr -d '\r' | sort
}

for arm in "$@"; do
  echo "=== shell_priority_boost = $arm, $load burners ==="
  bench="$(mktemp -d)"
  cp -r "$APPDATA/alacritty" "$bench/"
  cfg="$bench/alacritty/alacritree.toml"
  # Inside the existing [ui] table rather than appended: a second [ui] header
  # is a duplicate key and the whole file fails to parse.
  if [ "$arm" = on ]; then
    awk 'BEGIN{d=0} {print} /^\[ui\]/ && !d {print "shell_priority_boost = true"; d=1}' "$cfg" > "$cfg.new"
    mv "$cfg.new" "$cfg"
  fi

  before="$(pipes)"
  before_log="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1 || true)"
  glazewm command focus --workspace "$workspace" >/dev/null
  APPDATA="$(cygpath -w "$bench")" ALACRITREE_FRAME_LOG=1 "$exe" 2>/dev/null &
  app="$!"

  sock=""
  for _ in $(seq 1 90); do
    fresh="$(comm -13 <(printf '%s\n' "$before") <(pipes))"
    [ "$(printf '%s' "$fresh" | grep -c . || true)" = 1 ] && { sock="$fresh"; break; }
    sleep 1
  done
  [ -n "$sock" ] || { echo "  the window never started listening"; kill "$app" 2>/dev/null || true; continue; }
  pid="$(printf '%s' "$sock" | sed 's/.*alacritree-//; s/\.sock$//')"

  # Window up and shell at its prompt on an idle machine, then the load.
  sleep "$settle"
  "$probe" --load "$load" --hold "$((seconds + 40))" &
  holder=$!
  sleep 3

  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$here/real-keys.ps1")" \
    -TargetPid "$pid" -Seconds "$seconds" -EveryMs "${KEYS_EVERY_MS:-60}" | tail -2

  kill "$app" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  kill "$holder" 2>/dev/null || true
  app=""; holder=""
  sleep 2

  after="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1)"
  [ "$after" = "$before_log" ] && { echo "  no new session log"; continue; }
  rg -N -o "frames: .*echo n=[1-9][0-9]* p50 [^ ]+ p95 [^ ]+" "$after" \
    | sed 's/frames: \([0-9]*\) in \([^ ]*\).*echo/frames \1\/\2 echo/' \
    | tail -n "$(( (seconds / 5) + 1 ))"
  rm -rf "$bench"; bench=""
done
