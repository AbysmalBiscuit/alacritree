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
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
exe="$here/../target/release/alacritree.exe"
probe="$here/../target/release/examples/echo_probe.exe"
seconds="${SECONDS_PER_ARM:-40}"
keys_every="${KEYS_EVERY_MS:-60}"
logs="${LOCALAPPDATA:-$APPDATA}/alacritree"

load="$1"; shift

for arm in "$@"; do
  echo "=== ablate=$arm load=$load ==="

  burners=()
  for _ in $(seq "$load"); do
    "$probe" --burn &
    burners+=($!)
  done
  # The report covers five-second windows, so the first one has to land after
  # the load is already steady or it averages the ramp in.
  sleep 3

  before="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1 || true)"

  ALACRITREE_FRAME_LOG=1 \
  ALACRITREE_SYNTH_KEYS="$keys_every" \
  ALACRITREE_ABLATE="$([ "$arm" = none ] && echo "" || echo "$arm")" \
    "$exe" &
  app=$!

  sleep "$seconds"
  kill "$app" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  for pid in "${burners[@]}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true

  after="$(ls -t "$logs"/alacritree-*.log 2>/dev/null | head -1)"
  if [ "$after" = "$before" ]; then
    echo "  no new session log — is [debug] gpu_timing or persistent_logging on?"
    continue
  fi
  # The first window still carries startup, so it is dropped.
  rg -N "^\[.*frames:" "$after" | tail -n +2 || echo "  no frame reports"
done
