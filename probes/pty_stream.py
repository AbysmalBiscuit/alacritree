"""Record what a pty sends, with the time each read arrived.

The counterpart of `alacritree/examples/conpty_stream.rs` for platforms with a
real pty, so a stream recorded on either can be compared against the other and
against the grid it was parsed into.  A pty forwards a child's bytes unchanged,
which is exactly what makes it the control: whatever a recording here is missing,
the program did not write.

Each line of the log is one read, as JSON: {"t": seconds since start, "b": the
bytes, decoded latin-1 so the mapping back to bytes is exact}.

Usage: python3 pty_stream.py <log> <cols> <rows> <program> [args...]
"""

import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time

log_path, cols, rows = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
command = sys.argv[4:]

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

child = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
os.close(slave)

start = time.monotonic()
records = []
while True:
    readable, _, _ = select.select([master], [], [], 0.2)
    if readable:
        try:
            data = os.read(master, 65536)
        except OSError:
            # The child closed its end; on Linux that surfaces as EIO rather
            # than as a zero-length read.
            break
        if not data:
            break
        records.append({"t": round(time.monotonic() - start, 6), "b": data.decode("latin-1")})
    elif child.poll() is not None:
        break

child.wait()
os.close(master)

with open(log_path, "w") as log:
    for record in records:
        log.write(json.dumps(record) + "\n")

if records:
    print(f"{len(records)} reads over {records[-1]['t']:.2f}s -> {log_path}")
else:
    print(f"no output captured -> {log_path}")
