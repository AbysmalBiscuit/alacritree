# Kitty graphics protocol: design

Issue: alacritree/alacritree#382. Research: `docs/superpowers/research/2026-09-29-image-rendering-*.md` (start with the comparison). kitty's implementation is the reference wherever it and its spec differ; Ghostty is the second reference and the source of most ported tests.

## One data path

```
PTY bytes
  -> vendored vte: APC collected (256 KiB cap), handed to Perform::apc_dispatch
  -> ansi::Processor -> Handler::apc(&[u8])
  -> alacritty_terminal Term: flush, build ApcContext (cursor, screen, cell px), call Graphics
  -> alacritree_graphics::Graphics: parse keys, reassemble chunks, validate, store, place
       reply -> Term sends Event::PtyWrite
       cursor move -> Term applies it with its own primitives (linefeed honours margins)
       pixel decode -> job pool, outside the terminal lock, wakes the pane when done
  -> Term lifecycle hooks (scroll, ED2, ED3, alt screen, RIS, resize) -> Graphics moves/clears placements
  -> frame capture (terminal_view, under the lock it already takes): when the layout changed,
     Graphics fills an ImageFrame (crates/alacritree_graphics/src/frame.rs)
  -> grid_gl: one texture per Pixels, one instance buffer of ImageQuads, one instanced draw per run,
     three bands around the existing background, glyph and decoration passes
```

## Decisions

### How the graphics layer attaches to `Term`: a concrete field

`Term` gains a field `graphics: alacritree_graphics::Graphics` and `alacritty_terminal` depends on `alacritree_graphics`. The graphics crate depends on neither `alacritty_terminal` nor GL and takes plain values.

- A generic parameter would thread a second type parameter through every `Term<EventProxy<R>>` in the app for one implementation.
- An event cannot move the cursor synchronously, and the cursor has to move before the next byte is parsed.
- The renderer reads the store under the terminal lock the frame capture already takes, so the field adds no lock.

`Term` exposes `graphics()` and `graphics_mut()`. The vendored diff is the field, `Handler::apc`, `Handler::cell_size_pixels` (for `CSI 16 t`), and one call per lifecycle hook. Protocol logic stays in the graphics crate.

### How the cell size in pixels reaches `Term`

`Session::resize` already locks the terminal to call `Term::resize`. It also calls `term.graphics_mut().set_cell_pixels(width, height)` there, in device pixels.

The session's cell size becomes device pixels everywhere it leaves the app: `WindowSize` (TIOCGWINSZ, `CSI 14 t`, and the new `CSI 16 t`) and the graphics layer. It was egui points, which on a HiDPI display told programs a cell half its real size, so `kitten icat` would size every image wrong. `terminal_view` computes `cell_w * ppp` (whole pixels, since the cell is floored in pixel space).

`CSI 16 t` goes through the existing `Event::TextAreaSizeRequest` path with a closure that formats `ESC [ 6 ; <cell h> ; <cell w> t`. That keeps `14 t` and `16 t` answered from one `WindowSize`.

### Quota

320 MiB of decoded RGBA per screen buffer, kitty's figure, and not configurable. An image counts `width * height * 4` from the moment it is created, pending or not, because its size is known at parse time. A single image over the quota is `ENOMEM`. Eviction follows kitty's order: images with no placements first, then the oldest.

### Decode off the lock, replies at parse time

- At parse time, under the lock: key parsing, base64, chunk accumulation, validation, dimensions (PNG `IHDR`, or `s`/`v`), footprint, anchor, cursor movement and reply.
- On the job pool (`alacritree_common::jobs::pool()`, `Priority::Interactive`): zlib inflate, PNG decode, RGB to RGBA, file reads for `t=f`/`t=t` beyond the header.
- The image's decoded pixels live in a shared slot (an `Arc`'d `OnceLock` or equivalent). The job fills it without taking the terminal lock, bumps an atomic readiness counter the store shares, and calls the pane's waker (`Graphics::set_waker`, which `Session` points at its `EventProxy` with `Event::Wakeup`).
- A `pending` image draws nothing. The first capture after the readiness counter moves rebuilds the frame and the image appears with no new input.
- Divergence: an error only a full decode can find (a PNG body that fails after a valid header, an `o=z` payload that inflates to the wrong size) cannot be replied to, since the reply already went out. The image ends failed, draws nothing, and is logged. kitty replies with the error because it decodes in the parser.

### File paths from WSL panes: refused

A `t=f` or `t=t` path is read only when it can be opened as named. On Windows that means a drive path, so a Linux path from a WSL pane gets `EBADF:Failed to read image file`, the same reply as any file alacritree cannot read, `a=q` included. Every client that probes with `a=q,t=f` (icat, Claude Code) falls back to sending the bytes directly on an error, so refusing costs one probe. Translating would need the pane's distro in the graphics layer and a `\\wsl$` read, which is a UNC path and the one kind of path the Windows policy exists to refuse. An `OK` goes out only after the file was opened and its head read.

### XTVERSION

`CSI > q` and `CSI > 0 q` get `DCS > | alacritree <version> ST`. vte gains a `('q', [b'>'])` arm and `Handler::report_version`. `Term` answers beside DA2 from `term::Config::version_report`, which the app sets to `alacritree <version>`, since `alacritty_terminal` cannot name the app's version. The reply is a `PtyWrite` like DA1, so the replies leave in the order the queries came.

### Placement anchor

A placement stores an absolute line: `scrolled + screen line` at placement time, where `scrolled` is a per-screen counter of lines that went into history. A full-screen scroll then moves every placement by bumping the counter. Only margin scrolls, which clip or move individual placements, walk the placement list. A placement is dropped once its bottom row is above `-history_size`. The column and in-cell pixel offsets are stored as given. The source rect is stored in texels and rewritten by margin clipping.

### Hooks in `Term`

Hook the scroll primitives, not `scroll_up_relative`/`scroll_down_relative`, because IL and DL call those and must not move images:

- `Handler::scroll_up` and `Handler::scroll_down`, which LF at the bottom margin, SU, SD and RI all reach. Report the region, the signed line count, and whether lines went to history (`region.start == 0` on the main screen).
- ED2 (`ClearMode::All`): clear first, keeping only placements wholly in scrollback. On the main screen `grid.clear_viewport` then scrolls the viewport into history, so it returns how many lines it pushed and the hook reports that as a scroll.
- ED3 (`ClearMode::Saved`), `swap_alt` (entering the alternate screen clears its placements; each screen has its own store), `reset_state` (kitty's reset rules), and `resize` (kitty's `grman_resize`: shift by the lines that moved into history when rows shrink, and leave anchors alone on a column change).

## The shared record: `crates/alacritree_graphics/src/frame.rs`

This file is fixed for both halves. A change goes through the lead.

- `Band`, from z: `z < -2^30` under backgrounds, `-2^30 <= z < 0` under text, `z >= 0` over text.
- `Pixels`: RGBA8, straight alpha, rows top first, and a process-unique `PixelsKey`. New content is a new `Pixels` with a new key, so a texture keyed by it never goes stale.
- `ImageQuad` (`#[repr(C)]`, 32 bytes), the instance record:
  - `dest`: `[left, top, right, bottom]` in cells from the viewport's top-left cell corner, with fractions for pixel offsets and native sizes. The renderer multiplies by its cell size in points, so images line up with the text at any scale and a native-size image covers exactly its pixel count.
  - `src`: `[left, top, right, bottom]` in texels.
- `ImageRun`: band, `Arc<Pixels>`, span of quads.
- `ImageFrame`: quads and runs in draw order (band, z, image creation order, placement), plus a generation that moves on every rebuild. `clear` keeps capacity.

Ownership:

- The graphics layer owns images, `Pixels` and the frame's contents. It fills an `ImageFrame` only when its layout generation, the display offset, the grid size or the cell size changed.
- The renderer owns every GL object. It keeps one texture per `PixelsKey`, uploads a texture the first time a run names its key, and frees it once the `Pixels` it came from is gone. It holds a `Weak<Pixels>` per texture and sweeps them when a new frame arrives. It re-uploads the quad buffer only when the frame's generation changed.

## Rendering

- The draw order is: eframe's clear to the default background, `UnderBackgrounds`, the background pass (it already collapses default-background cells), `UnderText`, glyphs, decorations, `OverText`.
- The cursor and the egui-painted overlays (emoji, box drawing, preedit) are drawn after the callback, so they sit above every band. That differs from kitty, where images with `z >= 0` cover the cursor. The cursor stays visible, and nothing here would be simpler for copying kitty.
- Textures use clamp-to-edge and linear filtering. The fragment shader clamps the sample point to the source rect inset by half a texel, since GLSL 140 / ES 3.0 has no `CLAMP_TO_BORDER`. It premultiplies alpha, because egui blends `ONE, ONE_MINUS_SRC_ALPHA`.
- The sampler unit is set from the host. `#version 140` has no `layout(binding)`.

## Tests

- Graphics half, without a GPU: bytes driven through the patched `vte::ansi::Processor` into a real `Term`, asserting on the store, the replies (`PtyWrite` events), the cursor, and the `ImageFrame`. Port kitty's `kitty_tests/graphics.py` and Ghostty's graphics tests for chunking, deletes, scroll-margin clipping and IL/DL. Add the zellij and herdr sequences captured from their format strings (`docm info zellij`: `zellij-server/src/output/mod.rs`; `docm info herdr`: `src/kitty_graphics.rs`).
- Renderer half: the pass order as data (a test asserts the bands around backgrounds, glyphs and decorations), the texture cache's upload and free decisions, and the frame-to-instance upload rule. GL itself is verified by running the app.

## Out of scope

Unicode placeholders (#384), animation, relative placements, `t=s`, detection and `TERM_PROGRAM` (#385), sixel (#386).
