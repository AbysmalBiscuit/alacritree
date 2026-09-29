# Image rendering research: WezTerm

WezTerm at commit `cab25161054c50fd6c705db4ceefef0f1e5a9575`, checkout `C:\Users\Lev\.local\share\devkit\docs\wezterm\main`. Every `path:line` below is relative to that checkout.

## Summary

1. WezTerm implements kitty graphics (no `a=a`, no unicode placeholders, only `d=a/A` and `d=i/I` deletes), sixel, and iTerm2 OSC 1337 `File=`.
2. Its own parser (`vtparse`) buffers APC without a size cap and emits an ordered `Action` stream; a per-pane parser thread applies it to the model under a mutex, so an image lands at whatever the cursor is when its action runs.
3. Every placement is sliced into one `ImageCell` per covered grid cell, stored in the cell's heap-allocated attributes, so images scroll, enter scrollback and move with IL/DL for free, but text written over a cell deletes that slice and reflow can tear the image.
4. Images share the single glyph atlas and render as one 272-byte quad per covered cell on three quad layers; a full atlas triggers a full atlas rebuild, then progressive image downscaling, then dropping images.
5. On Windows it runs a bundled ConPTY with no image-specific handling; the only ConPTY accommodation is client side, in `wezterm imgcat` and termwiz's probe, which pre-scroll, move the cursor explicitly, and sleep to survive reordered replies.

## 1. Protocols

Implemented: kitty graphics, sixel, iTerm2 inline images. `docs/features.md:25-27` lists all three.

Kitty graphics, parsed in `wezterm-escape-parser/src/apc.rs` and applied in `term/src/terminalstate/kitty.rs`:

- Recognition. An APC whose first byte is `G` (`wezterm-escape-parser/src/apc.rs:1072-1075`); the control data is split on `,` and `=` into a `BTreeMap` and the payload follows the first `;` (`apc.rs:1076-1087`). A missing `a=` means `t` (`apc.rs:1088`). Any unparseable key value makes the whole command `None`, which is dropped silently (`wezterm-escape-parser/src/parser/mod.rs:228-234`).
- Actions. `t`, `T`, `p`, `d`, `q`, `f`, `c` (`apc.rs:1090-1123`). `a=a` (animation control) is not present: the match has no `"a"` arm, so it falls to `_ => None` (`apc.rs:1122`).
- Transmission media. `t=d` direct base64, `t=f` file, `t=t` temp file, `t=s` shared memory (`apc.rs:107-129`). Temp files are deleted only if the path starts with `/tmp/`, `/var/tmp/`, `/dev/shm/` or `$TMPDIR` (`apc.rs:217-253`). Shared memory uses `shm_open`/`shm_unlink` on Unix (`apc.rs:265-309`) and `OpenFileMappingW`/`MapViewOfFile` on Windows, copying the bytes out (`apc.rs:320-445`). `S=` and `O=` size and offset are honoured for all three indirect media (`apc.rs:179-197`).
- Formats. `f=24`, `f=32`, `f=100` (`apc.rs:478-505`). RGB is expanded to RGBA and the length must equal `w*h*4` (`kitty.rs:778-811`). PNG is decoded with the `image` crate after a dimension probe (`kitty.rs:812-819`). No other `f=` values.
- Compression. `o=z` zlib via `miniz_oxide` (`apc.rs:514-521`, `kitty.rs:770-776`).
- Chunking. `m=1` pushes the command into an accumulator; the final chunk coalesces all chunks, base64-decoding each chunk separately (`kitty.rs:205-231`, `kitty.rs:851-924`).
- Animation. `a=f` builds and edits frames with `r=`, `c=`, `x=`, `y=`, `Z=` gap, `X=` composition mode and `Y=` background pixel (`apc.rs:977-1001`, `kitty.rs:540-741`). `a=c` composes a rectangle from one frame onto another (`apc.rs:896-921`, `kitty.rs:390-538`). Frames always loop at render time (`wezterm-gui/src/glyphcache.rs:929-991`); with `a=a` absent there is no stop, loop count or current-frame control.
- Placement keys. `x y w h X Y c r C p z` (`apc.rs:626-644`). Not present: `U=` (unicode placeholder) and `P= Q= H= V=` (relative placements). Search: `rg -n -i "10eeee|placeholder" term/src wezterm-gui/src wezterm-escape-parser/src` finds only an unrelated placeholder image in `glyphcache.rs:496`, and the key list in `apc.rs:626-644` has no such keys.
- Deletes. All specifiers `a i n c f p q x y z` and uppercase forms parse (`apc.rs:729-776`), but only `a/A` and `i/I` are applied (`kitty.rs:241-269`). Everything else logs `unhandled KittyImage::Delete` (`kitty.rs:270-272`).

Sixel: DCS with final `q` and no intermediates starts a `SixelBuilder` (`parser/mod.rs:246-247`); raster attributes, repeat, color define (RGB and HLS) and select are parsed (`wezterm-escape-parser/src/parser/sixel.rs:47-183`). The model renders it into an RGBA buffer (`term/src/terminalstate/sixel.rs:10-115`), honouring transparent background P2 (`sixel.rs:26-35`), private color registers (`sixel.rs:18-24`) and DECSDM (`sixel.rs:124-154`). Pixel aspect (`pan`/`pad`) is stored (`parser/sixel.rs:148-149`) but not used when rasterising (`terminalstate/sixel.rs:37-112` never reads it).

iTerm2: OSC 1337 `File=` only (`wezterm-escape-parser/src/osc.rs:1309-1311`), with `width`, `height`, `preserveAspectRatio`, `inline` and the WezTerm extension `doNotMoveCursor` (`osc.rs:1040-1055`, `docs/imgcat.md:21-24`). Non-inline files go to a download handler (`term/src/terminalstate/iterm.rs:11-22`). Not present: `MultipartFile`/`FilePart`/`FileEnd`. Search: `rg -n "MultipartFile|FilePart" wezterm-escape-parser/src` returns nothing.

Kitty graphics can be disabled with `enable_kitty_graphics`, default true (`config/src/config.rs:256-257`, `kitty.rs:176-178`).

## 2. Parser to model

Recognition. `vtparse` is WezTerm's own DEC-style state machine. `ApcStart` clears a `Vec<u8>`, `ApcPut` pushes every byte, `ApcEnd` hands the whole buffer to `apc_dispatch` (`vtparse/src/lib.rs:643-657`). The buffer has no size limit: `apc_data` is a plain `Vec<u8>` (`vtparse/src/lib.rs:376-377`) and `ApcPut` never checks its length (`vtparse/src/lib.rs:650-653`). OSC uses an unbounded `Vec<u8>` in std builds too (`vtparse/src/lib.rs:317-325`), which is what lets multi-megabyte OSC 1337 payloads through. Sixel data flows byte by byte through `dcs_put` into the builder (`parser/mod.rs:275-280`) and is emitted on `dcs_unhook` (`parser/mod.rs:310-312`).

Limits. Sixel caps declared raster size at 100,000,000 (`parser/sixel.rs:7`, `parser/sixel.rs:151-176`). Kitty and sixel images are refused over 100 MB of RGBA (`term/src/terminalstate/image.rs:302-319`). The kitty chunk accumulator has no chunk count or byte cap (`kitty.rs:210-212`, `kitty.rs:227-229`).

Payload into the model. The escape parser turns the APC into `Action::KittyImage(Box<KittyImage>)` (`parser/mod.rs:228-231`), sixel into `Action::Sixel` (`parser/mod.rs:312`) and OSC 1337 into an `OperatingSystemCommand` (`parser/mod.rs:320-323`). Actions carry no cursor position. The model applies actions in order (`term/src/terminal.rs:176-185`), so the cursor is the model's cursor at apply time. Placement reads `self.cursor` directly (`term/src/terminalstate/image.rs:156`, `image.rs:163`).

Print buffering caveat. The model's performer buffers printable characters and flushes them on the next control, CSI, ESC or OSC (`term/src/terminalstate/performer.rs:365-373`, `performer.rs:375-378`, `performer.rs:490-493`, `performer.rs:585-587`, `performer.rs:741-743`). Kitty images flush first (`performer.rs:281-282`). Sixel does not: `Action::Sixel(sixel) => self.sixel(sixel)` (`performer.rs:279`). **Unverified:** text printed immediately before a sixel in the same batch would land after the image, because the buffered text is flushed at a later cursor position. Reasoning: the DCS introducer emits no action for sixel (`parser/mod.rs:246-247`), so nothing flushes the buffer before `sixel()` runs.

Threads. A reader thread does blocking PTY reads and forwards bytes over a socketpair (`mux/src/lib.rs:283-349`). A per-pane parser thread (`mux/src/lib.rs:314-318`) parses and batches actions, holding them during synchronized output (`mux/src/lib.rs:162-196`) and coalescing for up to `mux_output_parser_coalesce_delay_ms`, default 3 ms, within a 128 KiB buffer (`mux/src/lib.rs:198-231`, `config/src/config.rs:1675-1681`). The same parser thread applies the batch under `Mutex<Terminal>` (`mux/src/lib.rs:122-128`, `mux/src/localpane.rs:389-391`, `localpane.rs:126`) and then notifies the GUI (`mux/src/lib.rs:128`). So kitty PNG decode, zlib inflate and sixel rasterisation all run on the parser thread while holding the terminal lock (`kitty.rs:770-819`, `terminalstate/sixel.rs:10-115`).

## 3. Replies and detection

Writing replies. The model writes into a `BufWriter<ThreadedWriter>` (`term/src/terminalstate/mod.rs:370`). `ThreadedWriter` sends each write over a channel to a dedicated thread that writes to the PTY, so the model never blocks on the PTY (`term/src/terminalstate/mod.rs:455-507`).

Kitty replies. `kitty_send_response` formats `ESC _ G i=<id>[,I=<no>];<msg> ESC \` and honours `q=1`/`q=2` (`kitty.rs:351-388`). What gets a reply:

- `a=q` replies `OK` if the data loads and `ERROR:<reason>` otherwise, always verbose (`kitty.rs:181-200`, `apc.rs:1063`). It only loads the bytes; it does not decode them (`kitty.rs:181`). **Unverified:** a query with `t=t` deletes the temp file, since `load_data` unlinks it (`apc.rs:212-254`).
- A successful transmit replies `OK` only when `I=` was given (`kitty.rs:838-846`). A transmit or place with `i=` alone gets no `OK`.
- Transmit and place failures return `Err`, which the performer only logs (`performer.rs:283-285`); no `ERROR` reply is sent. Only frame operations reply `ENOENT` (`kitty.rs:395-440`, `kitty.rs:578-593`). `i=` and `I=` together is refused with a TODO to send `EINVAL` (`kitty.rs:749-752`).

Detection signals.

- DA1 replies `CSI ? 65 ; 4 ; 6 ; 18 ; 22 ; 52 c`, advertising sixel (`term/src/terminalstate/mod.rs:1407-1418`).
- XTSMGRAPHICS reports 65536 color registers and the text area pixel size as sixel geometry (`mod.rs:1459-1497`).
- XTVERSION replies `DCS >| WezTerm <version> ST` (`mod.rs:1440-1447`, `mux/src/domain.rs:619-624`).
- Env: `TERM` defaults to `xterm-256color`, plus `COLORTERM=truecolor`, `TERM_PROGRAM=WezTerm`, `TERM_PROGRAM_VERSION` (`config/src/config.rs:1617-1622`, `config.rs:1751-1753`). On Windows and WSL these are added to `WSLENV` so they cross into WSL (`config.rs:1606-1613`).
- There is no kitty-specific DA or env signal; kitty clients detect support with `a=q`.

Pixel sizes.

- CSI 14t reports the text area in pixels, CSI 16t the cell size as `pixel_width / cols` and `pixel_height / rows`, CSI 18t the text area in cells (`mod.rs:2145-2174`).
- On Unix the PTY size carries `ws_xpixel`/`ws_ypixel` through TIOCSWINSZ (`pty/src/unix.rs:26-31`, `unix.rs:181-186`).
- On Windows `ResizePseudoConsole` takes only a cell `COORD`; the pixel fields are stored locally and never reach the child (`pty/src/win/conpty.rs:53-72`). Programs under ConPTY have to ask with CSI 14t/16t.
- Placement divides the same totals by the grid size and refuses to place when the result is zero (`term/src/terminalstate/image.rs:70-88`).

## 4. Image store

Data types. `ImageDataType` is `EncodedFile(Vec<u8>)`, `EncodedLease(BlobLease)` (bytes on disk), `Rgba8` or `AnimRgba8`, each decoded variant carrying a SHA-256 per frame (`wezterm-cell/src/image.rs:216-260`). `ImageData` wraps it in a `Mutex` with an immutable content hash taken at creation, and equality is hash equality (`wezterm-cell/src/image.rs:548-582`).

Decoding, by protocol.

- Kitty RGB/RGBA/PNG and sixel are decoded eagerly on the parser thread into `Rgba8` (`kitty.rs:778-820`, `terminalstate/sixel.rs:114-115`).
- iTerm2 probes dimensions only (`iterm.rs:24-35`) and keeps the encoded file unless it must be downscaled and is not GIF, PNG or WebP, in which case it resizes with CatmullRom on the parser thread (`iterm.rs:104-122`).
- Encoded images are moved to an on-disk blob by `swap_out` (`wezterm-cell/src/image.rs:393-404`) when blob storage is initialised, which the GUI does in the cache directory (`wezterm-gui/src/main.rs:416-418`).
- The GUI decodes encoded images lazily on a spawned thread per image (`wezterm-gui/src/glyphcache.rs:235-259`), storing each decoded frame as a blob (`glyphcache.rs:329-356`). The render waits up to 125 ms, or one frame interval if longer, for the first frame and shows an 8x8 black placeholder until then (`glyphcache.rs:1001-1007`, `glyphcache.rs:385-406`).

Ids and numbers. `i=` is used as given. `I=` allocates `max_image_id + 1` and records the mapping (`kitty.rs:758-762`, `kitty.rs:831`). Neither means id 0 (`kitty.rs:753-756`), and placing id 0 does not replace earlier id-0 placements (`kitty.rs:103-105`). Re-transmitting an id replaces its data (`kitty.rs:37-44`). A 16-entry LRU in `TerminalState`, keyed by content hash, returns the existing `Arc<ImageData>` for repeated identical data (`term/src/terminalstate/image.rs:285-299`, `term/src/terminalstate/mod.rs:583`).

Quota and eviction.

- Kitty keeps `id_to_data` with a 320 MB budget marked `FIXME: make this configurable`. Over budget, it frees ids that have no placement, in hash-map order, until under budget (`kitty.rs:46-68`). `len()` counts encoded leases as 0 bytes and animated images as frames times first-frame size (`wezterm-cell/src/image.rs:606-614`).
- **Unverified:** placements are removed only by delete, re-place or delete-all (`kitty.rs:312-349`), never when their rows leave scrollback, so an image whose placement scrolled away still counts as referenced and cannot be pruned. Reasoning: `prune_unreferenced` builds its referenced set from `self.placements.keys()` (`kitty.rs:49`), and no scrollback or erase path touches `placements` (`rg -n "kitty_img" term/src` hits only `kitty.rs` and `performer.rs:283`).
- Pixels referenced from cells stay alive through the cells' `Arc`s, in scrollback too, until the lines are dropped.

Where pixels live. CPU memory, or disk for leases, in the model. On first render each frame is copied into the shared glyph atlas and cached by frame hash (`glyphcache.rs:917-927`, `glyphcache.rs:972-982`). The GUI keeps an LFU of decoded images, sized by `glyph_cache_image_cache_size`, default 256 (`glyphcache.rs:564`, `glyphcache.rs:610-615`, `config/src/config.rs:849-850`, `config.rs:2102-2104`). Atlas sprites are never evicted individually. The only eviction is rebuilding the whole `GlyphCache` when the atlas fills (`glyphcache.rs:558-559`, `wezterm-gui/src/renderstate.rs:757-778`).

## 5. Placement model

Anchoring. `assign_image_to_cells` computes the cell span from the image or source rect and the cell pixel size, or from `c=`/`r=` (`term/src/terminalstate/image.rs:121-155`). It then writes one `ImageCell` per covered cell holding texture coordinates for that cell's slice, per-cell padding, z-index, image id and placement id (`image.rs:188-252`, `wezterm-cell/src/image.rs:80-115`). The `ImageCell` lives in the cell's boxed `FatAttributes.image: Vec<Box<ImageCell>>` (`wezterm-cell/src/lib.rs:91-104`). Kitty appends to that list in z order (`wezterm-cell/src/lib.rs:394-407`). Sixel and iTerm2 replace it (`wezterm-cell/src/lib.rs:370-375`, `image.rs:237-242`). For kitty deletes the model also records `PlacementInfo { first_row: StableRowIndex, rows, cols }` per `(image_id, placement_id)` (`image.rs:12-17`, `kitty.rs:135-137`).

What moves it. Because the image is cell data, anything that moves cells moves it:

- Scrolling, scrollback and scroll regions. Placement advances rows with `new_line` (`image.rs:249-251`), so a tall image scrolls the screen, and slices go to scrollback with their lines.
- IL, DL, ICH and DCH move the slices with their cells. **Unverified:** inferred from the storage above; no image-specific code exists in those paths (`rg -n "image" term/src/terminalstate/mod.rs` hits only the cache and state fields).
- Resize and reflow. `Screen::resize` rewraps lines carrying their cells (`term/src/terminalstate/mod.rs:995-1001`). **Unverified:** a reflow that re-wraps image rows tears the image, and `PlacementInfo.first_row` goes stale. Reasoning: slices are ordinary cells and `first_row` is only written at placement (`kitty.rs:135-137`).
- Erase and clear. Blank cells are built from `pen.clone_sgr_only()`, which drops `fat` except colors (`wezterm-cell/src/lib.rs:426-466`, `term/src/terminalstate/mod.rs:1170`, `mod.rs:1186`), so ED, EL and ECH delete the covered slices.
- Text written over it. `set_cell_grapheme` builds a fresh cell from the pen (`wezterm-surface/src/line/line.rs:760-793`), so printing over an image cell deletes that cell's slice. This differs from kitty, where text and images are independent layers.
- Alternate screen. `kitty_img` is one struct on `TerminalState` (`term/src/terminalstate/mod.rs:377`), shared by both screens, while removal edits the active screen (`kitty.rs:292-310`). **Unverified:** deleting a primary-screen placement while the alternate screen is active edits alternate-screen rows with the same stable indices.
- RIS erases both screens (`performer.rs:724-730`) but does not reset `kitty_img`, so ids, data and placement records survive a reset (`performer.rs:679-731` has no kitty call).

z-index. Documented as negative below text, non-negative above text, and below `i32::MIN/2` under non-default cell backgrounds (`wezterm-cell/src/image.rs:197-203`). The renderer only tests `< 0` and `>= 0` (`wezterm-gui/src/termwindow/render/screen_line.rs:490`, `screen_line.rs:685`). Search: `rg -n "i32::MIN|INT32_MIN"` over `wezterm-gui`, `term` and `wezterm-cell` finds only that doc comment, so the `i32::MIN/2` tier is not present.

Cursor after placement. Kitty and iTerm2 move the cursor right by the width, plus one if padding spills over, leaving it on the last image row (`image.rs:254-276`). Sixel leaves the cursor below-left unless `sixel_scrolls_right` is set (`image.rs:262-268`). Kitty `C=1` does not move the cursor and clips the image at the bottom of the screen instead of scrolling (`image.rs:167-171`, `image.rs:195-199`). DECSDM places sixels at the top left and restores the cursor (`terminalstate/sixel.rs:124-154`).

Deletion. `d=i/I` removes matching placements by clearing their `ImageCell`s from the recorded row range and, for `I`, frees the data (`kitty.rs:241-263`, `kitty.rs:292-338`). `d=a/A` removes every placement, including those in scrollback, and `A` clears all data and numbers (`kitty.rs:264-269`, `kitty.rs:340-349`). Every other specifier is not handled (`kitty.rs:270-272`).

Memory per cell. Each covered cell holds a `Box<ImageCell>` inside a `Box<FatAttributes>`. **Unverified:** the exact size; by field count an `ImageCell` is about 56 bytes (two `NotNan<f32>` pairs, an `Arc`, an `i32`, four `u16`, two `Option<u32>`, `wezterm-cell/src/image.rs:92-115`), so a 40x20-cell image carries around 800 boxed slices per placement.

## 6. Render pipeline

Graphics API. glium (OpenGL) or WebGPU, with one glyph shader program and one atlas texture, sampled through nearest and linear samplers (`wezterm-gui/src/termwindow/render/draw.rs:211-222`, `draw.rs:237-272`). The atlas is an sRGB RGBA8 texture checked against `max_texture_size` (`wezterm-gui/src/renderstate.rs:70-95`).

Texture strategy. Images and glyphs share the one atlas. `cached_image` allocates each frame with padding equal to the next power of two of the larger cell dimension (`render/mod.rs:456-471`, `glyphcache.rs:917-927`). When the atlas fills, the paint loop first rebuilds at the same size, then grows it. If that fails it retries with images downscaled 2x, 4x and 8x, then disables images (`wezterm-gui/src/termwindow/render/paint.rs:37-105`). A rebuild keeps the decoded-image cache so animations do not restart (`renderstate.rs:769-774`).

Quads. Every covered cell emits one quad per attached image (`render/mod.rs:440-521`). Texture coordinates are computed in floating point from the sprite origin and the cell's normalised slice, to avoid rounding seams (`render/mod.rs:476-496`). A quad is 4 vertices of 68 bytes, 272 bytes total (`wezterm-gui/src/quad.rs:33-43`, `quad.rs:401`). Draws are indexed quads, not instanced (`draw.rs:257-267`).

Draw order. Each render layer holds three quad sub-layers drawn in order 0, 1, 2 (`draw.rs:237-271`, `renderstate.rs:481-495`). Per line (`screen_line.rs`):

- Sub-layer 0: cell backgrounds (`screen_line.rs:230-257`), underlines (`screen_line.rs:261-281`), selection background (`screen_line.rs:287-304`), block cursor (`screen_line.rs:368-376`), then images with z < 0 (`screen_line.rs:488-503`).
- Sub-layer 1: glyphs (`screen_line.rs:659-678`).
- Sub-layer 2: bar cursor (`screen_line.rs:369-372`), then images with z >= 0 (`screen_line.rs:683-693`, `screen_line.rs:719-731`).

**Unverified:** within a sub-layer, later allocations draw on top. That would mean negative-z images cover the block cursor and the selection background, and non-negative images cover glyphs and the bar cursor. Reasoning: each sub-layer is one draw call over quads in allocation order (`draw.rs:257-267`).

Clipping. Only visible lines are rendered, and images are clipped to whole cells because each cell draws only its own slice. Padding shrinks a cell's quad for `X=`/`Y=` offsets and the partial last row and column (`render/mod.rs:507-514`, `image.rs:190-210`). Within a scroll region, placement scrolls the region with `new_line` like text.

Per-frame cost and dirty tracking. The whole frame's quads are rebuilt every paint (`paint.rs:164-186`). Each line's quads are cached in `line_quad_cache`, keyed by pane, selection, cursor, generations and a shape hash of the line (`wezterm-gui/src/termwindow/render/pane.rs:426-443`). A hit copies the cached quads into the GPU buffers (`pane.rs:445-467`). Image cells contribute a shape hash of their layout and image identity, not their pixels (`wezterm-cell/src/image.rs:126-141`, `wezterm-cell/src/lib.rs:106-120`). **Unverified:** an in-place `a=f` edit of frame 1 keeps the line's cache key, so the edited pixels may not show until something else invalidates the line. Reasoning: `ImageData.hash` is fixed at creation (`wezterm-cell/src/image.rs:548-556`) and `compute_shape_hash` hashes it (`wezterm-cell/src/image.rs:133`).

Animation. Frame advance happens during render. Each frame's due time is its duration rounded up to the `max_fps` interval, and a 0 ms root frame is skipped (`glyphcache.rs:929-991`, `glyphcache.rs:593`). A line holding an animated image expires its cache entry at the next due time (`pane.rs:531-551`), and the paint loop schedules a timer to invalidate the window only while it has focus (`paint.rs:118-146`). Every distinct frame gets its own atlas sprite in `frame_cache` (`glyphcache.rs:974-982`), which is cleared only when the atlas is rebuilt (`glyphcache.rs:558-559`).

## 7. Unicode placeholders

Not present. Searches: `rg -n -i "10eeee|10EEEE|unicode.placeholder|virtual.placement"` across all Rust sources and `rg -n -i "unicode placeholder|10EEEE|U=1" docs` return nothing relevant, and the placement key parser has no `U` key (`wezterm-escape-parser/src/apc.rs:626-644`). A U+10EEEE cell would print as an ordinary private-use glyph.

## 8. Platform and multiplexer paths

Windows and ConPTY.

- WezTerm prefers a sideloaded `conpty.dll` plus `OpenConsole.exe` shipped beside the binary, falling back to the system one (`pty/src/win/pseudocon.rs:43-58`, `assets/windows/conhost/README.md`, `wezterm-gui/build.rs:18`). The README gives mouse reporting as the reason; the latest refresh is `4accc376` (`docs/changelog.md:313`).
- It creates the console with `INHERIT_CURSOR | RESIZE_QUIRK | WIN32_INPUT_MODE` (`pseudocon.rs:80-90`). `PSEUDOCONSOLE_PASSTHROUGH_MODE` is defined but marked `dead_code` and never used (`pseudocon.rs:28-29`).
- The terminal model has no ConPTY image handling. `enable_conpty_quirks` only changes wrap marking (`performer.rs:170-192`, `mux/src/domain.rs:628-630`). Search `rg -n -i "apc|\\x1b_" pty/src` returns nothing, so no path special-cases APC in either direction.
- The client side compensates. `wezterm imgcat` assumes ConPTY on any Windows build (`wezterm/src/main.rs:514-517`) and, when it knows the cell pixel size, prints newlines to pre-scroll, moves back up, emits the image, then moves the cursor down explicitly (`main.rs:519-587`). The changelog says tmux and ConPTY "do not natively understand image protocols" and would otherwise let the prompt overwrite the image (`docs/changelog.md:449-452`).
- termwiz's probe sleeps 100 ms under tmux or Windows because both reorder the DA1 reply ahead of the pixel-size reply. It also tolerates XTVERSION traffic that ConPTY itself generates (`termwiz/src/caps/probed.rs:152-167`, `probed.rs:185-190`).
- Kitty APC replies cross ConPTY's input side unmodified in WezTerm's code. **Unverified:** whether they survive ConPTY; nothing in this checkout tests or works around it.

tmux. WezTerm does not unwrap `DCS tmux;` passthrough, which is tmux's job. Its clients wrap for tmux: imgcat's `--tmux-passthru` (`wezterm/src/main.rs:572-576`) and termwiz's probe (`probed.rs:8`, `probed.rs:91-95`, `probed.rs:145-150`). The changelog notes tmux wipes such images on redraw (`docs/changelog.md:442-447`). WezTerm's own tmux control mode (`DCS 1000 p`) is described as incomplete (`docs/escape-sequences.md:373`, `parser/mod.rs:258-263`). zellij: not present (`rg -l -i zellij --type rust` over the checkout returns no files).

WezTerm's own multiplexer. Lines sent to a mux client have their images stripped and replaced by `SerializedImageCell` records carrying the data hash (`codec/src/lib.rs:950-1008`, `codec/src/lib.rs:1065-1085`). The client fetches missing data by hash with `GetImageCell` and caches it in a 128-entry LRU (`wezterm-client/src/pane/renderable.rs:640-680`). `docs/imgcat.md:14-15` still says images are not fully handled over mux sessions.

What breaks images between program and terminal: missing pixel size (refused, `image.rs:75-88`), tmux redraws, ConPTY cursor desync (client-side workaround only), and any unsupported kitty feature, which fails silently because errors are not replied (Section 3).

## 9. Tests and specs

Tests worth reading or porting:

- `term/src/test/image.rs`: kitty zero-size, valid PNG, zero pixel size, and in-place frame edit keeping the hash current (`image.rs:15-126`). These are model-level tests that feed raw escapes, the shape alacritree needs.
- `wezterm-escape-parser/src/apc.rs:1219-1279`: kitty control-data parsing, including `a=f` keys.
- `wezterm-escape-parser/src/parser/mod.rs:903-970`: kitty through the full parser, `a=q` with direct and file data.
- `vtparse/src/lib.rs:1104-1135`: APC and DCS at the state machine level.
- `wezterm-escape-parser/src/parser/sixel.rs:190-306`: sixel parsing with the Wikipedia "HI" example.
- `wezterm-escape-parser/src/osc.rs:1979-2016`: OSC 1337 `File=` round trip, and `parser/mod.rs:1104-1108` for an out-of-bounds OSC 1337 regression.

Docs: `docs/imgcat.md`, `docs/cli/imgcat.md`, `docs/escape-sequences.md:372` (sixel status), and `docs/changelog.md`, whose image entries cover kitty fixes (`changelog.md:410`, `changelog.md:1749`, `changelog.md:1775`), sixel OOM (`changelog.md:1756`) and imgcat tmux/ConPTY handling (`changelog.md:436-458`). There is no internal design doc for images. `rg -l -i "image|sixel|kitty" docs-internal` finds none.

## 10. Lessons

- Zero pixel size or zero draw size used to divide by zero and crash the pane. Both are now refused, with the reasons in comments and tests (`term/src/terminalstate/image.rs:75-88`, `image.rs:108-119`, `term/src/test/image.rs:11-74`, issue 6344).
- Frame hashes must be refreshed after in-place edits, or the dedup and atlas caches serve stale pixels (`wezterm-cell/src/image.rs:222-223`, `kitty.rs:490`, `kitty.rs:642`, `term/src/test/image.rs:95-126`).
- Frame durations are rounded up to the render interval so neighbouring cells of one image never show different frames mid-paint (`glyphcache.rs:941-950`, issue 3260).
- Truncated or corrupted on-disk frame blobs are replaced by blank frames, because a size mismatch would panic deep in the renderer (`glyphcache.rs:1041-1064`).
- Allocating a texture larger than the GPU maximum fails silently at bind time, so WezTerm checks the maximum itself (`renderstate.rs:74-88`).
- Sixel HLS hue angles are rotated 120 degrees relative to standard HSL (`terminalstate/sixel.rs:87-102`, issue 775). Huge sixel repeat counts once caused OOM (`changelog.md:1756`).
- ConPTY and tmux reorder query replies and ConPTY emits its own XTVERSION queries, so probes need a delay and a tolerant reader (`termwiz/src/caps/probed.rs:152-167`, `probed.rs:185-190`).
- Open FIXME and TODO items: the 320 MB kitty budget is hard-coded (`kitty.rs:47`), `EINVAL` is never sent (`kitty.rs:750`), glyph clipping is not implemented (`screen_line.rs:531`), and ConPTY detection is a `cfg!(windows)` guess (`wezterm/src/main.rs:514-517`).
- Serialisation bugs in `to_keys`, the encoder used by `Display`: `a=T` is written as `a=Q` (`apc.rs:1145`), the file offset is written to `S` instead of `O` (`apc.rs:146-147`, `apc.rs:156-157`, `apc.rs:166-167`), and `d=q` is written as `d=p` (`apc.rs:820-821`). Parsing is unaffected; any code that re-emits kitty commands through `Display` sends wrong keys.
- The iTerm2 path avoids resampling GIF, PNG and WebP so animations survive (`iterm.rs:104-113`).

## Takeaways for alacritree

1. Keep images out of the cell. WezTerm gets scrolling, scrollback and IL/DL for free by storing a boxed slice per cell (Section 5). It pays with a heap allocation per covered cell, one 272-byte quad per cell per image (Section 6), slices deleted by overprinted text, and placement records that go stale on reflow. alacritree's 12-byte per-cell instance record has no room for this and should not grow to hold it. Use a placement table anchored to an absolute grid line and column, updated from the grid's scroll and resize events, and draw one instanced quad per visible placement, clipped to the viewport in the shader. That matches kitty's layered semantics, where text does not erase images.
2. Give images their own textures. WezTerm's shared atlas couples image memory to glyph caching: a big image forces atlas growth or a full rebuild, the fallback downscales every image, and each animation frame occupies a sprite until the next rebuild (Section 6). alacritree samples egui's font atlas, which it does not own, so it needs its own image textures regardless. Use one GL texture per image, or per frame for animations, keyed by content hash like WezTerm's `frame_cache`. Upload once, evict independently, and draw in dedicated passes before and after the existing glyph pass according to z.
3. Deliver APC in stream order through the handler rather than capturing the cursor separately. WezTerm never records a cursor at parse time. It relies on applying actions strictly in order, so `self.cursor` is correct when the image action runs (Section 2). A vte patch that calls an `apc_dispatch` hook synchronously from `advance` gets the same property for free. Two WezTerm gaps to avoid: flush any buffered printable text before placing an image, which WezTerm skips for sixel (Section 2), and cap both the APC buffer and the kitty chunk accumulator, which WezTerm leaves unbounded (Sections 2 and 4).
4. Decode off the terminal lock and show a placeholder. WezTerm decodes kitty PNGs, inflates zlib and rasterises sixel on the parser thread while holding `Mutex<Terminal>`. iTerm2 images instead keep their encoded bytes and decode on a worker thread with a 125 ms first-frame wait (Section 4). alacritree's PTY thread holds the `alacritty_terminal` lock while parsing, so the iTerm2 pattern fits better: record the placement at parse time with known dimensions, decode on a worker, then request a repaint through `EventProxy`.
5. Treat ConPTY as a cursor-desync and reply-loss problem, and do not expect WezTerm to have solved it. WezTerm's terminal side does nothing for images under ConPTY. Its only fixes are in its own client, which pre-scrolls and moves the cursor explicitly, and in a 100 ms probe delay for reordered replies (Section 8). For alacritree on Windows, test `kitten icat` and yazi through ConPTY early, confirm whether APC replies reach the client, and check whether `PSEUDOCONSOLE_PASSTHROUGH_MODE`, which WezTerm defines but never enables, changes either result. That flag's effect is unverified here.
6. Unicode placeholders need another reference. WezTerm has none (Section 7), and they are the only kitty path zellij and herdr panes can carry. Take the U+10EEEE design from kitty or ghostty. The Section 1 table of implemented kitty keys and delete specifiers is still a usable minimum-viable subset, as long as alacritree replies with errors, which WezTerm does not (Section 3).
