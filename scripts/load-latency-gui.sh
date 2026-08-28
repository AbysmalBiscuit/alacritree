#!/usr/bin/env bash
# Run the app under synthetic load and synthetic typing, one ablation per arm.
#
# The headless probe measures the child's half of a keystroke.  This measures
# the app's: `frame_log`'s echo covers the wait for a frame to run before the
# byte reaches the PTY, and its period and phase breakdown say what the frame
# was doing instead.
#
#   load-latency-gui.sh 16 none sidebars gitpoll jobs grid repaint=8
#
# The first argument is the burner count; the rest are ALACRITREE_ABLATE
# values, run in turn.  The terminal pane has to keep focus for the synthetic
# keystrokes to reach the session, so nothing else may take it while a run is
# in flight.
#
# The window opens before the load rather than underneath it.  The complaint is
# about typing into a terminal that was already sitting there when a build
# started; a window that has to create its GL context and start its shell under
# saturation is a cold start, which is a different failure with a different
# cause.  `SETTLE_SECONDS` is how long it gets to itself, and the typist is
# held back for that plus the load's ramp.
#
# Every arm opens on GlazeWM workspace 1, tiled beside what is already there.
# Grid size decides how much there is to paint, so a window that lands on
# whatever workspace happened to be focused is a different measurement each
# run.  Workspace 1 is also where the other benches on this machine ran, which
# keeps their numbers comparable with these.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
seconds="${SECONDS_PER_ARM:-40}"
settle="${SETTLE_SECONDS:-20}"
keys_every="${KEYS_EVERY_MS:-60}"
workspace="${GLAZE_WORKSPACE:-1}"
logs="${LOCALAPPDATA:-$APPDATA}/alacritree"

load="$1"; shift

# An interrupted run must not leave burners behind: they are busy loops with
# nothing to stop them, and a machine quietly missing several cores is worse
# than a lost measurement.
app=""
burners=()
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null || true
  for pid in "${burners[@]:-}"; do kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT INT TERM

# Anything the typist sends before the load is up lands on an idle machine, so
# it waits out the settle and the ramp together.
quiet_for=$((settle + 3))

for arm in "$@"; do
  echo "=== ablate=$arm load=$load ==="

  burners=()
  before="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1 || true)"

  # A new window opens on whatever workspace has focus, so the focus moves
  # first rather than the window moving afterwards — moving it afterwards
  # would retile twice and change the grid size mid-run.
  glazewm command focus --workspace "$workspace" >/dev/null

  ALACRITREE_FRAME_LOG=1 \
  ALACRITREE_SYNTH_KEYS="$keys_every" \
  ALACRITREE_SYNTH_DELAY="$quiet_for" \
  ALACRITREE_ABLATE="$([ "$arm" = none ] && echo "" || echo "$arm")" \
    "$exe" &
  app="$!"

  # Window up, GL context built, shell at its prompt, all on an idle machine —
  # the state the terminal is in when a build starts.
  sleep "$settle"

  # One holder owns every burner.  Spawning them from here instead would give
  # each an inherited stdin that is already at EOF, and the watchdog that
  # stops them outliving a hard kill would then stop them immediately.
  "$probe" --load "$load" --hold "$((seconds + 30))" &
  burners+=("$!")
  # The report covers five-second windows, so the first one has to land after
  # the load is already steady or it averages the ramp in.
  sleep 3

  sleep "$seconds"
  kill "$app" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  cleanup; app=""; burners=()
  wait 2>/dev/null || true

  after="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1)"
  if [ "$after" = "$before" ]; then
    echo "  no new session log — is [debug] gpu_timing or persistent_logging on?"
    continue
  fi
  # Everything before the load is a different measurement, so the windows
  # covering the settle go with the one carrying startup.
  rg -N "frames:" "$after" | tail -n "+$((quiet_for / 5 + 1))" || echo "  no frame reports"
done
