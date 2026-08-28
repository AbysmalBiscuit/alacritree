#!/usr/bin/env bash
# Sweep the headless echo probe across load levels.
#
# The shape of the curve is the diagnostic: smooth degradation points at
# per-keystroke cost, a cliff points at contention.
#
# The arms isolate one thing each.  cmd.exe is the floor, a child that does
# almost nothing.  nushell runs as configured, then with
# `highlight_resolved_externals` off, then with no config at all — that option
# resolves the first token against PATH on every keystroke, which idle costs
# the whole difference between the first and last arm.
set -euo pipefail

probe="$(dirname "$0")/../target/release/examples/echo_probe.exe"
keys="${KEYS:-20}"
spawns="${SPAWNS:-3}"

for load in "$@"; do
  "$probe" \
    --shell cmd.exe \
    --shell nu.exe \
    --shell nu.exe --setup '$env.config.highlight_resolved_externals = false' \
    --shell "nu.exe -n" \
    --load "$load" --keys "$keys" --spawns "$spawns" 2>/dev/null
done
