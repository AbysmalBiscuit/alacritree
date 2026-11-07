# herdr-core sessions use a demux thread, not a fake `EventedPty`

A herdr-core connection carries many terminals over one framed stream, but `alacritty_terminal`'s event loop expects one pollable source per session. alacritree routes each channel from a demux thread into a byte channel per session, and a parser thread per session feeds its `Term` under the `FairMutex` lease and sends CREDIT after it parses. A fake `EventedPty` would need a second copy of the fragile IOCP readiness emulation on Windows, and a plain byte pipe loses the frame kinds: AUTOMATIC_INPUT, FOCUS, and CREDIT tied to parsed bytes.

## Considered Options

- A custom `EventedPty` per session over a socketpair or pipe. This is easy on Unix and hard on Windows, and it hides the frame semantics.
- Parsing on the demux thread. This puts every session on a side behind every other session's `Term` lock.
- Parsing on the UI thread, as herdr-app and kitty do. A flood of output then stalls the egui frame, and native sessions already parse off the UI thread.

## Consequences

The parser thread reproduces the native event loop's read limits, synchronized-update timeout, child-exit handling and wakeups through `EventProxy`. A change to those in the native loop must also go to the parser thread.
