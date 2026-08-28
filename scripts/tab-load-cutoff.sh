#!/usr/bin/env bash
# Does a shell reach its prompt under load, or only once the load stops?
#
# The complaint is that a new tab "won't start until the load goes down".  That
# is a different failure from being slow: a shell that is merely starved still
# finishes while the machine is busy, whereas one waiting on the load ending
# produces nothing until the moment it does.  The two are told apart by cutting
# the load at a known instant and printing when the screen changed relative to
# it.
#
#   tab-load-cutoff.sh 16 20            # 16 burners, cut 20s after the tab opens
#   tab-load-cutoff.sh 16 20 //wsl.localhost/kali/home/me/repo
#
# A run where every change lands after the cut, repeatedly, is the claim
# confirmed.  Changes on both sides of it mean the shell was making progress
# all along and the load only slowed it down.
#
# The default load is `echo_probe --burn`, which spins on the ALU and touches
# nothing else, so a negative result with it bounds what spinning processes can
# cause rather than what a build can.  `LOAD_CMD` replaces it with any command,
# which is how the reference case gets tested with the thing that provoked it:
#
#   LOAD_CMD='cargo build -p alacritree --release' tab-load-cutoff.sh 0 40
#
# Like the other benches this opens on GlazeWM workspace 1 and only ever talks
# to the window it started.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
quiet="${QUIET_MS:-4000}"
workspace="${GLAZE_WORKSPACE:-1}"
logs="${LOCALAPPDATA:-$APPDATA}/alacritree"

load="${1:?usage: tab-load-cutoff.sh <load> <cut-seconds> [workspace]}"
cut="${2:?usage: tab-load-cutoff.sh <load> <cut-seconds> [workspace]}"
arm="${3:-home}"
case "$arm" in
  //*) arm="$(printf '%s' "$arm" | tr '/' '\\')" ;;
esac

load_cmd="${LOAD_CMD:-}"

app=""
holder=""
listing=""
killer=""
load_pid=""
# Killing the launcher is not enough for a build, whose work is all in its
# children, and burners survive a hard kill of their holder only until their
# stdin closes — which a tree kill does.
stop_load() {
  [ -n "$load_pid" ] && taskkill //T //F //PID "$load_pid" >/dev/null 2>&1 || true
  [ -n "$holder" ] && kill "$holder" 2>/dev/null || true
}
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null || true
  [ -n "$killer" ] && kill "$killer" 2>/dev/null || true
  stop_load
  rm -f "$listing"
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
before="$(pipes)"

glazewm command focus --workspace "$workspace" >/dev/null
ALACRITREE_FRAME_LOG=1 "$exe" >/dev/null 2>&1 &
app=$!

sock=""
for _ in $(seq 1 90); do
  fresh="$(comm -13 <(printf '%s\n' "$before") <(pipes))"
  [ "$(printf '%s' "$fresh" | grep -c . || true)" = 1 ] && { sock="$fresh"; break; }
  sleep 1
done
[ -n "$sock" ] || { echo "the window never started listening" >&2; exit 1; }

ask() {
  MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*" "$exe" --socket "$sock" "$@"
}
now_ms() { date +%s%3N; }

for _ in $(seq 1 90); do
  ask session list >/dev/null 2>&1 && break
  sleep 1
done

# The load has to be steady before the tab opens, so it starts here and the cut
# is scheduled from when the tab was asked for rather than from now.  A command
# load is started through PowerShell for its Windows pid: `$!` names the Git
# Bash wrapper, and killing that leaves the build running.
load_pid=""
if [ -n "$load_cmd" ]; then
  starter="$(mktemp --suffix=.ps1)"
  printf '%s\n' \
    "\$p = Start-Process -FilePath cmd.exe -ArgumentList '/c','$load_cmd' -WorkingDirectory '$(cygpath -w "$here/..")' -PassThru -WindowStyle Hidden" \
    '$p.Id' > "$starter"
  load_pid="$(powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$starter")" | tr -d '\r')"
  rm -f "$starter"
  echo "load: $load_cmd (pid $load_pid)"
else
  "$probe" --load "$load" --hold "$((cut + 30))" >/dev/null 2>&1 &
  holder=$!
fi
sleep 3

start="$(now_ms)"
if [ "$arm" = home ]; then
  reply="$(ask session create 2>&1)" || { echo "$reply" >&2; exit 1; }
else
  reply="$(ask session create --workspace "$arm" 2>&1)" || { echo "$reply" >&2; exit 1; }
fi
id="${reply##* }"
cut_at=$((cut * 1000))
echo "tab $id in $arm, ${load_cmd:-$load burners}, cut at ${cut_at}ms"

# The cut is a whole process tree: a build's work is in its children, so
# killing only the launcher would leave the load running.
( sleep "$cut"; stop_load ) &
killer=$!

last_hash=""
last_change=0
deadline=$((cut_at + 60000))
while :; do
  hash="$(ask session read-screen "$id" 2>/dev/null | md5sum | cut -d' ' -f1)" || hash="$last_hash"
  at=$(( $(now_ms) - start ))
  if [ "$hash" != "$last_hash" ]; then
    last_hash="$hash"
    last_change="$at"
    if [ "$at" -lt "$cut_at" ]; then
      printf '  %6sms  changed   (loaded)\n' "$at"
    else
      printf '  %6sms  changed   (cut %sms ago)\n' "$at" "$((at - cut_at))"
    fi
  fi
  if [ "$((at - last_change))" -gt "$quiet" ] || [ "$at" -gt "$deadline" ]; then
    break
  fi
  sleep 0.2
done

if [ "$last_change" -lt "$cut_at" ]; then
  echo "settled ${last_change}ms in, while loaded"
else
  echo "settled ${last_change}ms in, $((last_change - cut_at))ms after the cut"
fi
ask session read-screen "$id" 2>/dev/null | grep -v '^ *$' | tail -3 || true
