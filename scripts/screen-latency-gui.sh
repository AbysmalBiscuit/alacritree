#!/usr/bin/env bash
# How long after a keypress does the character actually appear on screen?
#
# Every other bench here measures from inside the process, and every one of
# them stops before the pixels exist: `echo` ends when the PTY thread sees the
# byte, the frame log counts calls to `update`, the GPU timer covers the draw.
# None include the driver's queue or DWM composing the frame, so an app whose
# frames are piling up reads as healthy while the screen runs seconds behind.
# This one captures the window and times the pixels changing, which is what a
# person is actually complaining about.
#
#   screen-latency-gui.sh 64 30 stock vsync=true
#
# Arguments are the burner count, the number of keystrokes to time, and one arm
# per `[ui]` override to apply.  `stock` runs the config as it is installed.
#
# The config is copied rather than used in place: an arm has to be able to
# change a setting without touching the config the machine's real terminal is
# reading.  On Windows the only place looked at is `%APPDATA%\alacritty`, so
# pointing APPDATA at a copy redirects both the config and the state file, and
# the copy carries the real state so the sidebar has the same work to do.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
settle="${SETTLE_SECONDS:-20}"
workspace="${GLAZE_WORKSPACE:-1}"

load="${1:?usage: screen-latency-gui.sh <load> <samples> <arm>...}"
samples="${2:?usage: screen-latency-gui.sh <load> <samples> <arm>...}"
shift 2
[ "$#" -gt 0 ] || set -- stock

app=""
holder=""
listing=""
bench_appdata=""
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null || true
  stop_load
  rm -f "$listing"
  [ -n "$bench_appdata" ] && rm -rf "$bench_appdata" || true
}
trap cleanup EXIT INT TERM

bench_appdata="$(mktemp -d)"
cp -r "$APPDATA/alacritty" "$bench_appdata/"
[ -d "$APPDATA/alacritree" ] && cp -r "$APPDATA/alacritree" "$bench_appdata/"
config="$bench_appdata/alacritty/alacritree.toml"

listing="$(mktemp --suffix=.ps1)"
printf '%s\n' \
  '$dir = [char]92 + [char]92 + "." + [char]92 + "pipe" + [char]92' \
  '[System.IO.Directory]::GetFiles($dir) | Where-Object { $_ -like "*alacritree-*.sock" }' \
  > "$listing"
pipes() {
  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$listing")" | tr -d '\r' | sort
}

# Burners spin the ALU and touch nothing else, so a negative result with them
# bounds what a spinning process can do rather than what a build can.
# `LOAD_CMD` replaces them with the real thing, which also allocates, faults and
# hits the filesystem.  It goes through PowerShell for its Windows pid: `$!`
# would name the Git Bash wrapper, and killing that leaves the build running.
load_pid=""
start_load() {
  if [ -z "${LOAD_CMD:-}" ]; then
    "$probe" --load "$load" --hold "$((samples * 20 + 120))" &
    holder=$!
    return
  fi
  local starter
  starter="$(mktemp --suffix=.ps1)"
  printf '%s\n' \
    "\$p = Start-Process -FilePath cmd.exe -ArgumentList '/c','$LOAD_CMD' -WorkingDirectory '$(cygpath -w "$here/..")' -PassThru -WindowStyle Hidden" \
    '$p.Id' > "$starter"
  load_pid="$(powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$starter")" | tr -d '\r')"
  rm -f "$starter"
  echo "  load: $LOAD_CMD (pid $load_pid)"
}
# A build's work is all in its children, so the kill has to take the tree.
stop_load() {
  [ -n "$load_pid" ] && taskkill //T //F //PID "$load_pid" >/dev/null 2>&1 || true
  [ -n "$holder" ] && kill "$holder" 2>/dev/null || true
  load_pid=""
  holder=""
}

# An override replaces the key where it already exists and is appended to [ui]
# where it does not, so an arm names a setting rather than a file layout.
apply_override() {
  local key="${1%%=*}" value="${1#*=}"
  if rg -q "^\s*${key}\s*=" "$config"; then
    sed -i "s|^\s*${key}\s*=.*|${key} = ${value}|" "$config"
  else
    sed -i "0,/^\[ui\]/s||[ui]\n${key} = ${value}|" "$config"
  fi
  echo "  ${key} = ${value}"
}

for arm in "$@"; do
  echo "=== $arm, $load burners ==="
  cp "$APPDATA/alacritty/alacritree.toml" "$config"
  [ "$arm" = stock ] || apply_override "$arm"

  before="$(pipes)"
  glazewm command focus --workspace "$workspace" >/dev/null
  APPDATA="$(cygpath -w "$bench_appdata")" ALACRITREE_FRAME_LOG=1 ALACRITREE_ABLATE="${ABLATE:-}" "$exe" &
  app="$!"

  sock=""
  for _ in $(seq 1 90); do
    fresh="$(comm -13 <(printf '%s\n' "$before") <(pipes))"
    [ "$(printf '%s' "$fresh" | grep -c . || true)" = 1 ] && { sock="$fresh"; break; }
    sleep 1
  done
  [ -n "$sock" ] || { echo "  the window never started listening"; kill "$app" 2>/dev/null || true; continue; }
  pid="$(printf '%s' "$sock" | sed 's/.*alacritree-//; s/\.sock$//')"

  # Window up and shell at its prompt on an idle machine, then the load - the
  # state the terminal is in when a build starts.
  sleep "$settle"
  start_load
  sleep 3

  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$here/screen-latency.ps1")" \
    -TargetPid "$pid" -Samples "$samples" \
    -Burst "${BURST:-1}" -BurstGapMs "${BURST_GAP_MS:-30}" \
    -ClickFraction "${CLICK_FRACTION:-0}" \
    -DumpPath "$(cygpath -w "$here/../target/bench/$arm")" || true

  kill "$app" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  stop_load
  app=""
  wait 2>/dev/null || true
done
