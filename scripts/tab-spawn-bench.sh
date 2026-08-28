#!/usr/bin/env bash
# Measure how long a new tab takes to become a usable shell, under load.
#
# A tab appears the instant it is asked for; the shell inside it can take much
# longer, and how much longer seems to depend on where the tab opens.  So the
# arms are locations: one workspace path per arm, plus `home` for a tab with no
# workspace at all.  A UNC path is given with forward slashes, because the
# literal backslash spelling does not survive every shell that might launch
# this:
#
#   tab-spawn-bench.sh 16 4 home 'C:\Users\me\repo' //wsl.localhost/kali/home/me/repo
#
# `ALACRITREE_FRAME_LOG` makes the app report per tab:
#
#   resolve        picking the shell argv, including the WSL distro lookup
#   pty            building the PTY, with the UI thread blocked throughout
#   open           resolve, pty, the checkout guard, and the Doppler scope sync
#   first-output   from the PTY existing to the child writing its first byte
#
# First output is a poor finish line: a shell answers a terminal query long
# before it has a prompt.  So the bench adds `ready`, the point at which the
# screen stopped changing, read over IPC.  That costs one CLI process per
# sample, which competes with the shell being timed — it biases `ready` upward,
# and is the price of measuring what is on screen rather than what the PTY did.
#
# Tabs are left open rather than closed between rounds: closing the last tab in
# a workspace opens a replacement, which would land in the table as a tab nobody
# asked for.  It is also the truer shape, since the complaint is about opening a
# tab in a window that already has several.
#
# The bench only ever talks to the window it started, found as the pipe that
# appeared when it launched, so a daily-driver alacritree running alongside
# never receives a tab.
#
# Every arm opens on GlazeWM workspace 1, matching the other benches here: grid
# size decides how much there is to paint, and a window that lands on whatever
# workspace happened to be focused is a different measurement each run.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
quiet="${QUIET_MS:-2500}"
workspace="${GLAZE_WORKSPACE:-1}"
logs="${LOCALAPPDATA:-$APPDATA}/alacritree"

load="${1:?usage: tab-spawn-bench.sh <load> <tabs-per-arm> <arm>...}"
tabs="${2:?usage: tab-spawn-bench.sh <load> <tabs-per-arm> <arm>...}"
shift 2
if [ "$#" -eq 0 ]; then
  echo "no arms given; pass 'home' and/or worktree paths from:" >&2
  "$exe" project list >&2 || true
  exit 2
fi

arms=()
for arm in "$@"; do
  case "$arm" in
    //*) arms+=("$(printf '%s' "$arm" | tr '/' '\\')") ;;
    *) arms+=("$arm") ;;
  esac
done

# An interrupted run must not leave burners behind: they are busy loops with
# nothing to stop them, and a machine quietly missing several cores is worse
# than a lost measurement.
app=""
holder=""
listing=""
ready_file="$(mktemp)"
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null || true
  [ -n "$holder" ] && kill "$holder" 2>/dev/null || true
  rm -f "$listing" "$ready_file"
}
trap cleanup EXIT INT TERM

# The window has to outlive every arm: restarting it per arm would re-measure
# startup, and the first tab of a cold process is not the tab being asked about.
seconds=$((${#arms[@]} * tabs * 30 + 180))
"$probe" --load "$load" --hold "$seconds" &
holder=$!
sleep 3

# The instance is identified by the pipe it did not have before.  Log files are
# no good for it: a busy window's log is always the most recently written one,
# so picking the newest finds a daily driver rather than the window just opened.
listing="$(mktemp --suffix=.ps1)"
printf '%s\n' \
  '$dir = [char]92 + [char]92 + "." + [char]92 + "pipe" + [char]92' \
  '[System.IO.Directory]::GetFiles($dir) | Where-Object { $_ -like "*alacritree-*.sock" }' \
  > "$listing"
pipes() {
  powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$listing")" | tr -d '\r' | sort
}
before="$(pipes)"

glazewm command focus --workspace "$workspace" >/dev/null
ALACRITREE_FRAME_LOG=1 "$exe" &
app=$!

sock=""
for _ in $(seq 1 90); do
  fresh="$(comm -13 <(printf '%s\n' "$before") <(pipes))"
  count="$(printf '%s' "$fresh" | grep -c . || true)"
  if [ "$count" = 1 ]; then
    sock="$fresh"
    break
  elif [ "$count" -gt 1 ]; then
    echo "several new instances appeared; cannot tell which is the bench" >&2
    exit 1
  fi
  sleep 1
done
if [ -z "$sock" ]; then
  echo "the window never started listening — is [general] ipc_socket off?" >&2
  exit 1
fi
pid="$(printf '%s' "$sock" | sed 's/.*alacritree-//; s/\.sock$//')"
log="$(ls -t "$logs"/alacritree-*-"$pid".log 2>/dev/null | head -1 || true)"
if [ -z "$log" ]; then
  echo "no session log for pid $pid — is [debug] gpu_timing or persistent_logging on?" >&2
  exit 1
fi
echo "instance pid $pid, log $(basename "$log")"

ask() {
  MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*" "$exe" --socket "$sock" "$@"
}

now_ms() { date +%s%3N; }

# When the tab stopped changing, which is when a person would call it ready.
# The gap that counts as stopped has to clear a prompt drawn in pieces, so it
# is generous; what it returns is the last change seen, not the wait for it.
settled() {
  local id="$1" start last_hash="" last_change deadline hash now
  start="$(now_ms)"
  last_change="$start"
  deadline=$((start + 120000))
  while :; do
    hash="$(ask session read-screen "$id" 2>/dev/null | md5sum | cut -d' ' -f1)" || hash="$last_hash"
    if [ "$hash" != "$last_hash" ]; then
      last_hash="$hash"
      last_change="$(now_ms)"
    fi
    now="$(now_ms)"
    if [ "$((now - last_change))" -gt "$quiet" ] || [ "$now" -gt "$deadline" ]; then
      break
    fi
    sleep 0.2
  done
  echo "$((last_change - start))"
}

# Discovery walks every project in the sidebar, so a workspace argument is not
# accepted until the worktree it names is known.  Retrying costs nothing: a
# rejected request never reaches a spawn, so it cannot land in the numbers.
for _ in $(seq 1 90); do
  ask session list >/dev/null 2>&1 && break
  sleep 1
done

# Everything the window did while starting belongs to startup, not to a tab.
mark="$(grep -c "" "$log")"

for arm in "${arms[@]}"; do
  echo "=== $arm ==="
  for _ in $(seq 1 "$tabs"); do
    if [ "$arm" = home ]; then
      reply="$(ask session create 2>&1)" || { echo "  $reply"; continue; }
    else
      reply="$(ask session create --workspace "$arm" 2>&1)" || { echo "  $reply"; continue; }
    fi
    id="${reply##* }"
    echo "$id $(settled "$id")" >> "$ready_file"
  done
done

kill "$app" 2>/dev/null || true
wait "$app" 2>/dev/null || true

# One row per tab.  `resolve`, `pty` and `open` run in order on the UI thread;
# `first-output` closes whenever the child answers, which is why it is matched
# by session rather than by position.
tail -n "+$((mark + 1))" "$log" | awk -v ready_file="$ready_file" '
  BEGIN {
    while ((getline line < ready_file) > 0) {
      split(line, f, " ")
      ready[f[1]] = f[2] "ms"
    }
    printf "%-5s %9s %9s %9s %13s %9s\n", "tab", "resolve", "pty", "open", "first-output", "ready"
  }
  /spawn resolve:/ { resolve = valof($0); next }
  /spawn pty \[/   { id = idof($0); pty[id] = valof($0); res[id] = resolve; next }
  /spawn open \[/  { id = idof($0); open[id] = valof($0); order[++n] = id; next }
  /spawn first-output \[/ { first[idof($0)] = valof($0) }
  END {
    for (i = 1; i <= n; i++) {
      id = order[i]
      printf "%-5s %9s %9s %9s %13s %9s\n", id, res[id], pty[id], open[id],
             (id in first ? first[id] : "-"), (id in ready ? ready[id] : "-")
    }
  }
  function idof(line) { match(line, /\[[0-9]+\]/); return substr(line, RSTART + 1, RLENGTH - 2) }
  function valof(line,  f) { split(line, f, ": "); return f[2] }
'
