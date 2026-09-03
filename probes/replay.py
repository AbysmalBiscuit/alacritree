"""Replay a recorded pty stream to stdout with its original timing.

Feeding a recording back through a terminal is how the same bytes can be put to
more than one reader.  A live build is never twice the same, so measuring the
stream and the grid against each other needs one stream both saw.

Reads the format `pty_stream.py` and `conpty_stream.rs` write.

Usage: python3 replay.py <log> [speed]
"""

import json
import sys
import time

records = [json.loads(line) for line in open(sys.argv[1])]
speed = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0

out = sys.stdout.buffer
start = time.monotonic()
for record in records:
    due = record["t"] / speed
    while True:
        remaining = due - (time.monotonic() - start)
        if remaining <= 0:
            break
        time.sleep(min(remaining, 0.001))
    out.write(record["b"].encode("latin-1"))
    # Each record was one read at the far end, so it has to leave as one write.
    out.flush()
