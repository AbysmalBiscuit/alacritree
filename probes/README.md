# Flicker probes

Measurement tools for the cargo progress-line flicker. They exist so the
numbers in the issue can be rerun rather than taken on trust; nothing here is
built or shipped by alacritree.

The chain each one covers, from what a program wrote to what a viewer saw:

| probe | answers |
|---|---|
| `alacritree/examples/flicker_repro.rs` | what cargo's write pattern looks like with load and scrolling as separate knobs, and no compiler in the way |
| `alacritree/examples/conpty_stream.rs` | what a pseudoconsole emits, and when (Windows) |
| `probes/pty_stream.py` | the same for a real pty |
| `probes/replay.py` | feeds a recording from either back to a reader with its original timing |
| `alacritree/examples/grid_probe.rs` | whether the grid holds what the stream sent, sampled in process under the lock the painter takes |
| `alacritree/examples/stream_presence.rs` | the same question asked of a `conpty_stream` recording, parsed by `alacritty_terminal` rather than by a hand-written model |

A gap that shows up in a recording but not in the grid is ours. One that shows
up in neither, but on screen, is the painter's.

```sh
cargo run --release -p alacritree --example flicker_repro -- --width 117 --secs 20
cargo run --release -p alacritree --example grid_probe -- --log grid.tsv cargo build -p alacritree
python3 probes/pty_stream.py stream.jsonl 117 30 cargo build -p alacritree
```

Measure with release builds.  A debug `grid_probe` spends long enough scanning
the grid that the sampling interval stops being the thing that sets the rate.

`grid_probe` prints its summary itself.  `--match` sets the text that marks the
row being watched, `Building` by default.  `--no-rearm` drops the Windows pty
wrapper, so the reader that ships can be compared against the one underneath it.

Calibrate before trusting a number.  `flicker_repro` has a known presence in
both its arms, so running a probe against it says what that probe reads for an
output already known:

```sh
grid_probe -- flicker_repro --width 117 --secs 20 --scroll 3                    # ~100% present
grid_probe -- flicker_repro --width 117 --secs 20 --scroll 3 --throttled-redraw # ~84% present
```

`stream_presence` and `grid_probe` do not measure the same quantity at the same
grain.  A recording holds each read's state until the next read arrives, while
the reader batches reads under one lock, so states between reads often never
reach the grid.  Calibrated against the arms above, the recording reads about
four points more absence than the grid does for identical output.  Compare the
two only in that light, and treat the cursor figures as incomparable outright.

A recording and a grid sample only compare at one geometry.  `conpty_stream`
opens the console at a fixed size and `stream_presence` has to parse it at that
same size: replaying a wide recording into a narrow terminal wraps the rows,
and text left over from an earlier line reads as the row still being present.
The failure is silent and it inflates presence, so set both explicitly.

Load is a variable, not a constant.  Presence during a `cargo build` depends on
`-j`, so record it with the measurement.
