#!/usr/bin/env bash
# Sweep the headless echo probe across load levels.
#
# The shape of the curve is the diagnostic: smooth degradation points at
# per-keystroke cost, a cliff points at contention.  cmd.exe is the control —
# a child that does almost nothing — so any arm that degrades while it does
# not is degrading in the shell, not in the PTY or the scheduler.
set -euo pipefail

probe="$(dirname "$0")/../target/release/examples/echo_probe.exe"
keys="${KEYS:-20}"
spawns="${SPAWNS:-4}"

for load in "$@"; do
  "$probe" \
    --shell cmd.exe \
    --shell nu.exe \
    --shell "pwsh -NoLogo" \
    --load "$load" --keys "$keys" --spawns "$spawns" 2>/dev/null
done
