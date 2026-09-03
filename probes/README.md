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
row being watched, `Building` by default.
