# Image rendering in Ghostty

Ghostty at commit `0538f7535be0cbca6bbe54e6fde654d5c628f1f2`, checkout `C:\Users\Lev\.local\share\devkit\docs\ghostty\main`. Every `path:line` below is relative to that checkout.

## Summary

- Ghostty implements only the kitty graphics protocol, including unicode placeholders, relative placements and animation. Sixel and iTerm2 OSC 1337 images are absent.
- APC bytes are parsed and executed synchronously on the IO reader thread under the terminal lock, so a placement reads the cursor at the moment the final chunk's APC ends and pins it to a tracked `PageList.Pin`.
- Pixels live decoded in CPU memory in a per-screen `ImageStorage` with a 320 MB default quota, and the renderer copies them into one GL texture per image, keyed by a process-wide generation stamp.
- The frame draws kitty images in three z bands around the cell background and text passes, with one tiny vertex buffer and one draw call per placement per frame.
- Unicode placeholders cost a per-frame viewport rescan, gated by a per-row flag set at print time, and nothing in Ghostty handles tmux, zellij or ConPTY specially.

## 1. Protocols

Kitty graphics is the only image protocol. The APC handler recognizes `G` as kitty graphics and one other APC protocol, a Ghostty "glyph" protocol that registers glyph outlines and is not an image protocol, `src/terminal/apc.zig:60-103`.

- Kitty unicode placeholders (U+10EEEE) are implemented, `src/terminal/kitty/graphics_unicode.zig:15-16`.
- Sixel is not present. `rg -i sixel src` matches only the DA1 feature enum entry `sixel = 4` in `src/terminal/device_attributes.zig:53`, and the DCS handler dispatches only tmux control mode, XTGETTCAP and DECRQSS, `src/terminal/dcs.zig:53-99`.
- iTerm2 OSC 1337 images are not present. The OSC 1337 parser recognizes `File`, `FilePart`, `FileEnd` and `MultipartFile` as keys but routes them to a "unimplemented OSC 1337" branch that returns invalid, `src/terminal/osc/parsers/iterm2.zig:157-195`.

Kitty graphics details:

- Actions: `q`, `t`, `T`, `p`, `d`, `f`, `a`, `c` are all parsed and executed; an absent `a` means `t`, `src/terminal/kitty/graphics_command.zig:224-243`, `src/terminal/kitty/graphics_exec.zig:69-120`.
- Transmission media: `d`, `f`, `t`, `s` are parsed, `src/terminal/kitty/graphics_command.zig:579-588`. The Ghostty app enables file, temporary file and shared memory with the system temp dir, `src/termio/Termio.zig:276`, while the library default is direct only, `src/terminal/kitty/graphics_image.zig:101-105`. Shared memory is refused on Windows and Android, `src/terminal/kitty/graphics_image.zig:204-208`.
- Formats: `f=24` RGB, `f=32` or `f=0` RGBA, `f=100` PNG. An unknown `f` is deferred so the EINVAL reply can carry the image id, `src/terminal/kitty/graphics_command.zig:564-577`, `src/terminal/kitty/graphics_image.zig:122-124`. PNG may decode to gray or gray+alpha, `src/terminal/kitty/graphics_command.zig:518-527`.
- Compression: only `o=z` (zlib), anything else is `InvalidFormat`, `src/terminal/kitty/graphics_command.zig:618-624`.
- Chunking: `m=1` continues a load, and `m` is ignored for non-direct media because mpv relies on that with shared memory, `src/terminal/kitty/graphics_command.zig:626-639`. Continuation chunks inherit `q` from the first chunk unless they set `q>=1`, `src/terminal/kitty/graphics_exec.zig:74-89`.
- Animation: frames (`a=f`), control (`a=a`), composition (`a=c`) and frame deletion (`d=f/F`) are supported. Frames are composed eagerly into full RGBA buffers instead of kitty's delta chain, `src/terminal/kitty/graphics_animation.zig:1-22`.
- Relative placements (`P=`, `Q=`, `H=`, `V=`) are supported with a chain limit of 8, `src/terminal/kitty/graphics_exec.zig:311-345`, `src/terminal/kitty/graphics_storage.zig:706-709`.
- Usage hint `N` (transient) is parsed and drives eviction priority, `src/terminal/kitty/graphics_command.zig:541-552`.

Left out: sixel, iTerm2 inline images, compression other than zlib, and shared memory on Windows. No other kitty graphics key was found unhandled; unknown keys are ignored, `src/terminal/kitty/graphics_command.zig:9-13`.

## 2. Parser to model

Recognition. The VT parser has one `sos_pm_apc_string` state that emits `apc_start`, `apc_put` and `apc_end`, `src/terminal/Parser.zig:275-313`. The stream bulk-consumes APC payload bytes with a SIMD scan and dispatches them as one `apc_put_slice`, stopping at CAN, SUB, ESC or bytes of 0x80 and above, `src/terminal/stream.zig:873-900`, `src/terminal/stream.zig:1056-1098`. The termio stream handler forwards these to `apc.Handler`, `src/termio/stream_handler.zig:367-370`.

Identification. A first byte of `G` switches immediately into the kitty command parser; anything else buffers up to `;` for other identifiers, `src/terminal/apc.zig:60-124`.

Buffering and limits.

- Control keys are single letters stored in a 52-slot value table plus a presence bitmap, `src/terminal/kitty/graphics_command.zig:31-54`. Values over 11 characters switch to an ignore state, `src/terminal/kitty/graphics_command.zig:292-301`.
- The payload of one APC accumulates in an `ArrayList` capped at `max_bytes`, `src/terminal/kitty/graphics_command.zig:70-79`, `src/terminal/kitty/graphics_command.zig:177-180`. The default cap is 65 MiB per APC, `src/terminal/apc.zig:401-405`. Exceeding it drops the APC, `src/terminal/apc.zig:126-131`.
- Base64 is decoded in place over the encoded buffer when the APC ends, `src/terminal/kitty/graphics_command.zig:262-290`.
- Across chunks the `LoadingImage` accumulates decoded bytes up to 400 MB, taken from kitty, `src/terminal/kitty/graphics_image.zig:22-23`, `src/terminal/kitty/graphics_image.zig:537-554`. Width and height are capped at 10000, `src/terminal/kitty/graphics_image.zig:19-20`.

Reaching terminal state with the cursor. `apcEnd` calls `Terminal.kittyGraphics` directly, which runs `graphics_exec.execute`, `src/termio/stream_handler.zig:497-515`, `src/terminal/Terminal.zig:3803-3810`. A pin placement tracks the cursor's `page_pin` at execution time, `src/terminal/kitty/graphics_exec.zig:297-309`. Because execution happens inline in stream order, the cursor is exactly where the preceding bytes left it. For a chunked `a=T`, the display parameters are captured from the first chunk, `src/terminal/kitty/graphics_image.zig:136`, but the placement is created when the last chunk completes, `src/terminal/kitty/graphics_exec.zig:223-234`, so the anchor is the cursor at the final chunk.

Threads.

- An `io-gather` thread drains the pty into rotating buffers and an `io-reader` thread runs `processOutput` (lock, VT parse, state update, render scheduling); Windows keeps a single serial loop, `src/termio/Exec.zig:1264-1299`.
- `processOutput` takes `renderer_state.mutex` for the whole batch, `src/termio/Termio.zig:675-681`. Base64 decode, zlib inflate and PNG decode therefore all run on the reader thread while it holds the terminal lock, `src/terminal/kitty/graphics_exec.zig:1057-1063`, `src/terminal/kitty/graphics_image.zig:557-600`.
- The renderer thread reads the storage under the same mutex during `updateFrame`, `src/renderer/generic.zig:1387-1388`, `src/renderer/generic.zig:1479-1492`.

## 3. Replies and detection

Replies. `Response.encode` writes `ESC _ G i=..,I=..,p=..,r=..;MESSAGE ESC \` and writes nothing when neither `i` nor `I` is set, `src/terminal/kitty/graphics_command.zig:331-370`. The stream handler encodes into a 1024-byte buffer and queues it on the termio writer, which writes it to the pty, `src/termio/stream_handler.zig:504-514`.

Reply rules:

- `q=0` replies to everything with an id, `q=1` suppresses OK, `q>=2` suppresses all, `src/terminal/kitty/graphics_command.zig:245-253`, `src/terminal/kitty/graphics_exec.zig:122-133`.
- No reply until the last chunk, and none for images with an implicit id, `src/terminal/kitty/graphics_exec.zig:236-241`.
- Delete never replies on success, `src/terminal/kitty/graphics_exec.zig:982-983`.
- Error strings are errno-style, such as `ENOENT`, `EINVAL`, `ENODATA`, `ENOMEM`, `ENOPARENT`, `ECYCLE` and `ETOODEEP`, `src/terminal/kitty/graphics_exec.zig:1075-1093`, `src/terminal/kitty/graphics_exec.zig:330-337`.
- `a=q` performs a full load and decode and then discards the image, `src/terminal/kitty/graphics_exec.zig:138-182`.
- With the storage limit at zero the whole protocol goes silent, queries included, `src/terminal/kitty/graphics_exec.zig:31-37`.

Detection.

- DA1 replies `CSI ? 62 ; 22 ; 52 c`, or `62;22` without clipboard, and carries no image attribute (no `4`), `src/termio/stream_handler.zig:796-807`. The expected detection path is `a=q` followed by a DA1 query. **Unverified** that the reply ordering holds: both replies go through the same `messageWriter` path and are produced in stream order, `src/termio/stream_handler.zig:504-514`, `src/termio/stream_handler.zig:799-807`, but I did not read the mailbox to confirm it is FIFO.
- Env: `TERM=xterm-ghostty` by default, falling back to `xterm-256color` when no terminfo is bundled, plus `COLORTERM=truecolor`, `TERM_PROGRAM=ghostty` and `TERM_PROGRAM_VERSION`, `src/config/Config.zig:3875`, `src/termio/Exec.zig:637-662`, `src/termio/Exec.zig:750-751`. The bundled terminfo carries no image capability; `rg -i 'kitty|graphics|image' src/terminfo/ghostty.zig` matches only underline and keyboard notes.

Pixel sizes.

- `CSI 14 t` replies `CSI 4;height;width t`, `CSI 16 t` replies `CSI 6;cell_h;cell_w t`, `CSI 18 t` replies `CSI 8;rows;cols t`, and mode 2048 in-band reports carry rows, cols and pixel sizes, `src/terminal/size_report.zig:41-81`, `src/termio/stream_handler.zig:1792-1796`. A 2048 report is sent on every resize when the mode is on, `src/termio/Termio.zig:525-528`.
- On POSIX, `TIOCGWINSZ` carries `ws_xpixel` and `ws_ypixel` from the screen size, `src/termio/Exec.zig:1144-1149`.
- The terminal model keeps `width_px = cols * cell_width` and `height_px = rows * cell_height`, `src/terminal/Terminal.zig:4068-4081`. Placement geometry divides these back into a cell size, `src/terminal/kitty/graphics_storage.zig:1875-1876`.
- On Windows, `ResizePseudoConsole` takes only columns and rows, so no pixel size reaches the child through the pty, `src/pty.zig:474-483`.

## 4. Image store

Decoding.

- zlib goes through wuffs with a size hint from the declared dimensions, `src/terminal/kitty/graphics_image.zig:635-669`.
- PNG goes through a swappable `sys.decode_png` hook, which defaults to wuffs in the app and to null in libghostty, where PNG is unsupported, `src/terminal/sys.zig:15-54`. PNG decode runs through a `LimitedAllocator` capped at 400 MB, `src/terminal/kitty/graphics_image.zig:672-707`.
- Decoding happens at load completion on the reader thread under the terminal lock (see section 2). The stored `Image` is always raw pixels: never compressed, never PNG, `src/terminal/kitty/graphics_image.zig:710-716`.
- `Image.data` can also be `pending`, a reserved length whose bytes arrive later. This path exists for snapshot restore and lets placements exist before pixels do, `src/terminal/kitty/graphics_image.zig:779-812`, `src/terminal/kitty/graphics_storage.zig:136-166`.

Ids and numbers.

- Images are keyed by u32 id in a hash map, placements by `(image_id, PlacementId)`, `src/terminal/kitty/graphics_storage.zig:72-74`, `src/terminal/kitty/graphics_storage.zig:1735-1759`.
- `i` and `I` together are rejected before any mutation, `src/terminal/kitty/graphics_exec.zig:49-67`.
- A number-only transmit gets the smallest free id starting at 1, matching kitty; an image with neither gets an id from the upper half of the u32 range and never receives replies, `src/terminal/kitty/graphics_storage.zig:233-265`, `src/terminal/kitty/graphics_image.zig:729-736`.
- `imageByNumber` returns the newest image with that number by generation, `src/terminal/kitty/graphics_storage.zig:919-935`.
- Retransmitting an id deletes the old image and its placements when the new transmission begins, not when it completes, `src/terminal/kitty/graphics_exec.zig:1018-1025`.
- Placement id `p=0` gets an internal id, so one image can have many unnamed placements, `src/terminal/kitty/graphics_storage.zig:381-398`.

Quota and eviction.

- The default is 320 MB per screen, which doubles per surface because primary and alternate screens each have one, `src/config/Config.zig:2515-2522`. libghostty defaults to 10 MB, `src/terminal/Terminal.zig:296-305`.
- A single image larger than the limit fails with ENOMEM, `src/terminal/kitty/graphics_storage.zig:279-280`.
- Eviction picks the lowest of four classes: transient unused, then unused, then transient used, then used. Ties go to the oldest generation, `src/terminal/kitty/graphics_storage.zig:1600-1701`. Animation frames count against the same quota, `src/terminal/kitty/graphics_image.zig:858-864`.

Where pixels live.

- The decoded CPU copy stays in the terminal's `ImageStorage` for the image's lifetime.
- On each rebuild the renderer copies the pixels into its own buffer, converting to RGBA with a wuffs swizzle, only when the `(id, generation)` pair is new, `src/renderer/image.zig:696-751`, `src/renderer/image.zig:960-983`.
- On the next draw, `upload` creates a texture from that copy and frees it, `src/renderer/image.zig:59-92`, `src/renderer/image.zig:1070-1105`.
- Steady state is one CPU copy in the terminal plus one GPU texture.

## 5. Placement model

Anchor. A pin placement stores a tracked `*PageList.Pin`, which names a page node plus `x` and `y`. The PageList rewrites every tracked pin when pages change, `src/terminal/kitty/graphics_storage.zig:1782-1797`, `src/terminal/PageList.zig:459-460`, `src/terminal/PageList.zig:5668-5693`, `src/terminal/PageList.zig:7113-7123`. Size is not stored in cells. Grid and pixel extents are recomputed from `c`, `r`, the source rect and the current cell size whenever they are needed, `src/terminal/kitty/graphics_storage.zig:1890-2002`. A placement with `c`/`r` therefore rescales with the font, and a native-size one keeps its pixel size while its cell footprint changes. The latter is an inference from `pixelSize` returning the source size when neither is set, `src/terminal/kitty/graphics_storage.zig:1905-1911`.

What moves it:

- Full-screen scrolling: the pin follows its row into scrollback by pin tracking, with no image-specific work, `src/terminal/Terminal.zig:2489-2513`. Test: `src/terminal/kitty/graphics_storage.zig:4112-4145`.
- Scrolling with margins (IND, RI, SU, SD inside a region): `scrollMarginsBegin` records each placement's final row before the row operation and restores pins afterwards. Placements wholly inside the region move and get clipped at the margin by shrinking their source rect, and are deleted when fully clipped. Placements straddling or outside the region stay put, `src/terminal/kitty/graphics_storage.zig:419-589`, `src/terminal/kitty/graphics_storage.zig:2004-2070`. The fast paths pay only a placement-count check, `src/terminal/Terminal.zig:2410-2444`.
- Insert and delete lines leave placements where they are, matching kitty, `src/terminal/Terminal.zig:2649-2652`. Test: `src/terminal/kitty/graphics_storage.zig:4069-4110`.
- Scrollback pruning: pins on a pruned page become `garbage`, `src/terminal/PageList.zig:4074-4084`. The renderer skips garbage pins, `src/renderer/image.zig:560-561`, and storage reaps them lazily on the next `addPlacement`, `src/terminal/kitty/graphics_storage.zig:375-379`, `src/terminal/kitty/graphics_storage.zig:591-622`.
- Resize and reflow: reflow copies tracked pins to their cell's new location, `src/terminal/PageList.zig:1686-1815`, and resize marks the image state dirty, `src/terminal/Screen.zig:2118-2119`.
- Alternate screen: each `Screen` owns its own `ImageStorage`, `src/terminal/Screen.zig:78-81`. Entering 1049 erases the alternate display, which clears its placements, `src/terminal/Terminal.zig:4866-4870`. A switch marks the new screen's images dirty, `src/terminal/Terminal.zig:4796-4801`.
- Erase: ED2 and the scroll-clear variant call `clearScreen`, which deletes placements intersecting the active area and frees every image left without a placement, `src/terminal/Terminal.zig:3603-3683`, `src/terminal/kitty/graphics_storage.zig:1166-1191`. ED0, ED1 and EL leave placements alone, `src/terminal/Terminal.zig:3685-3717`. ED3 erases history, `src/terminal/Terminal.zig:3719`. **Unverified** that ED3 garbage-marks pins through the same path as pruning; I read the pruning path, not `eraseHistory`.
- RIS: `Screen.reset` rebuilds the storage empty but keeps its limits, `src/terminal/Screen.zig:428-438`, `src/terminal/Terminal.zig:15856`.
- Text written over a placement: not present as a deletion. `rg -n kitty_images src/terminal/Terminal.zig` shows no hit in the print path; the only image work in `print` sets the placeholder row flag, `src/terminal/Terminal.zig:1746-1753`. The image persists and draw order decides what shows.

z-index. `z` is a signed i32 that defaults to 0, `src/terminal/kitty/graphics_command.zig:667`. The renderer sorts by `z`, then by image id, and splits the list at `minInt(i32)/2` and at 0 into below-background, below-text and above-text bands, `src/renderer/image.zig:491-525`. Ties sort by image id rather than creation order, `src/renderer/image.zig:503-504`, `src/renderer/image.zig:829-850`.

Cursor movement after placement. With `C=0` the cursor moves right by the placement's columns and down by its rows minus one, through `terminal.index()`, so the scroll region is honoured. It wraps once at the right edge. The row count is bounded to one screen past the region bottom, because a hostile `r` could otherwise spin `index` up to 2^32 times, `src/terminal/kitty/graphics_exec.zig:382-433`. Relative and virtual placements never move the cursor, `src/terminal/kitty/graphics_exec.zig:382-386`.

Deletion (`a=d`).

- Every delete first aborts an in-flight chunked load, `src/terminal/kitty/graphics_exec.zig:968-972`.
- Specifiers `a/A`, `i/I`, `n/N`, `c/C`, `p/P`, `q/Q`, `r/R`, `x/X`, `y/Y`, `z/Z` and `f/F` are parsed, `src/terminal/kitty/graphics_command.zig:1022-1200`, and executed in `ImageStorage.delete`, `src/terminal/kitty/graphics_storage.zig:1193-1408`.
- Uppercase also frees images left unused. `d=a` touches only placements visible in the active area, `src/terminal/kitty/graphics_storage.zig:1410-1447`. Virtual placements are exempt from `d=z`, `src/terminal/kitty/graphics_storage.zig:1338-1357`.
- Relative children of deleted placements are removed transitively, `src/terminal/kitty/graphics_storage.zig:1394-1407`.

## 6. Render pipeline

Graphics API. A generic renderer over Metal or OpenGL, `src/renderer.zig:17-41`. The OpenGL backend requires OpenGL 4.3 and `#version 430 core`, `src/renderer/OpenGL.zig:38-40`, `src/renderer/shaders/glsl/common.glsl:1`. It binds extra buffers as SSBOs, `src/renderer/opengl/RenderPass.zig:109-122`, and the cell background pass is a full-screen triangle that reads a per-cell color buffer, `src/renderer/opengl/shaders.zig:17-21`, `src/renderer/generic.zig:1924-1929`.

Texture strategy. One 2D texture per image, never an atlas or array, `src/renderer/image.zig:1089-1095`. Textures use RGBA8 with an sRGB internal format, linear min/mag filtering, no mipmaps (a TODO) and clamp-to-edge wrapping, `src/renderer/OpenGL.zig:386-407`. A content change replaces the texture outright instead of calling `replaceRegion`, `src/renderer/image.zig:1098-1104`. Removal marks an entry `unload_*` during the rebuild, and the next `upload` pass frees it, `src/renderer/image.zig:270-293`, `src/renderer/image.zig:71-78`.

Image shader. Each placement is one instance of `{grid_pos, cell_offset, source_rect, dest_size}`, `src/renderer/opengl/shaders.zig:262-268`. The vertex shader builds a quad from `gl_VertexID` over a 4-vertex triangle strip, positions it at `cell_size * grid_pos + cell_offset`, and normalizes the source rect with `textureSize`, `src/renderer/shaders/glsl/image.v.glsl:12-46`. The fragment shader samples, optionally unlinearizes, and premultiplies alpha, `src/renderer/shaders/glsl/image.f.glsl:9-20`. Blending is `ONE, ONE_MINUS_SRC_ALPHA`, `src/renderer/opengl/RenderPass.zig:124-126`.

Draw order in one pass, `src/renderer/generic.zig:1885-1973`:

1. Background color or background image.
2. Kitty images with `z < -2^30`.
3. Cell backgrounds.
4. Kitty images with `-2^30 <= z < 0`, which includes all unicode placeholder placements, forced to `z = -1`, `src/renderer/image.zig:680-693`.
5. Text. The cursor is part of the text instances, drawn from dedicated cursor lists, `src/renderer/cell.zig:68-107`, so `z >= 0` images cover the cursor.
6. Kitty images with `z >= 0`.
7. Debug overlay.

Selection has no separate pass in this list; **unverified** that it is folded into cell colors, since I did not read `rebuildCells`.

Clipping. Placements entirely outside the viewport are culled on the CPU, `src/renderer/image.zig:582-584`. There is no scissor anywhere in the renderer (`rg -i scissor src/renderer` finds nothing), so a partly visible placement is clipped only by the framebuffer. Scroll-region clipping is done in terminal state by `clipTop`/`clipBottom`, not at draw time (section 5). **Unverified**: a placement hanging past the last row may draw into the window padding, since the vertex position has no padding clamp, `src/renderer/shaders/glsl/image.v.glsl:43-46`.

Per-frame cost and dirty tracking.

- `ImageStorage.dirty` flags any change, geometry included, and `generation` changes only when content changes, `src/terminal/kitty/graphics_storage.zig:76-104`, `src/terminal/kitty/graphics_storage.zig:188-200`. Scrolls, viewport moves, resizes and screen switches set `dirty` directly, `src/terminal/Screen.zig:927-930`, `src/terminal/Screen.zig:1629-1634`, `src/terminal/Terminal.zig:2391-2394`.
- `kittyUpdate` runs only when `dirty` is set or placeholders exist. It clears and rebuilds the whole placement list, resolves every pin to a screen row ("expensive but necessary"), sorts, and takes the renderer's `draw_mutex`, `src/renderer/image.zig:233-526`, `src/renderer/image.zig:568-572`, `src/renderer/generic.zig:1472-1492`.
- `draw` builds a one-element vertex buffer per placement and issues one draw per placement every frame, with a `future(mitchellh)` note about grouping, `src/renderer/image.zig:119-179`.

Animation frames. `animationTick` runs on the renderer thread inside `updateFrame`, under the terminal lock. It advances at most one frame per tick with no catch-up, skips unplaced images, and returns the next deadline so the render thread can schedule a wakeup, `src/renderer/generic.zig:1445-1470`, `src/terminal/kitty/graphics_storage.zig:1057-1164`, `src/renderer/Thread.zig:597`. Each frame change stamps a new generation, `src/terminal/kitty/graphics_storage.zig:969-976`, so every displayed frame is a full CPU copy plus a new texture, `src/renderer/image.zig:753-785`.

## 7. Unicode placeholders

Model side.

- `U=1` creates a `virtual` placement with no screen location, `src/terminal/kitty/graphics_exec.zig:293-295`.
- Printing U+10EEEE sets `kitty_virtual_placeholder` on the row, `src/terminal/Terminal.zig:1746-1753`, `src/terminal/page.zig:2054-2059`. The placeholder is kept off the batched print fast paths so this bookkeeping always runs, `src/terminal/Terminal.zig:659-662`, `src/terminal/Terminal.zig:749-751`.
- Reflow carries the flag with the cell, `src/terminal/PageList.zig:18843-18927` (tests), and a complete EL clears it, `src/terminal/Terminal.zig:14168` (test).

Resolution. While any virtual placement exists, the renderer rebuilds every frame, `src/renderer/image.zig:244-248`, and walks viewport rows, skipping rows without the flag, `src/terminal/kitty/graphics_unicode.zig:36-99`. For each placeholder cell:

- The image id's low 24 bits come from the foreground color (palette index or RGB), `src/terminal/kitty/graphics_unicode.zig:525-539`.
- The placement id comes from the underline color, `src/terminal/kitty/graphics_unicode.zig:432-439`.
- Diacritics give the row, the column and the id's high 8 bits, looked up by binary search in kitty's diacritic table. Invalid diacritics count as absent, `src/terminal/kitty/graphics_unicode.zig:441-479`, `src/terminal/kitty/graphics_unicode.zig:542-558`.
- Adjacent cells merge into a one-row run when id, placement and row match and the column is continuous or omitted, following kitty's rule, `src/terminal/kitty/graphics_unicode.zig:494-501`.
- A run names its virtual placement by explicit placement id, or else by a deterministic preference among the image's virtual placements, external ids first, then the lowest id, `src/terminal/kitty/graphics_storage.zig:882-912`, `src/terminal/kitty/graphics_storage.zig:1751-1758`.

Tile geometry. `renderPlacement` fits the whole image into the virtual placement's `c` x `r` grid, deriving any missing dimension from the image size, `src/terminal/kitty/graphics_unicode.zig:353-390`. It preserves aspect ratio and centers the image, `src/terminal/kitty/graphics_unicode.zig:153-190`, then cuts out the source rect and destination offset for this run's cells, trimming the letterbox margins, `src/terminal/kitty/graphics_unicode.zig:212-350`. Each run becomes one image placement at `z = -1`, `src/renderer/image.zig:637-694`.

Text side. The shaper replaces the placeholder with a space, so no glyph is drawn, `src/font/shaper/run.zig:264-268`, `src/font/shaper/run.zig:326-332`. Relative placements rooted at a virtual placement anchor at the minimum x/y of that placement's on-screen placeholder cells, `src/renderer/image.zig:400-489`.

## 8. Platform and multiplexer paths

- tmux passthrough (`DCS tmux; ... ST`): not present. The DCS handler accepts only tmux control mode (`DCS 1000 p`), XTGETTCAP and DECRQSS, `src/terminal/dcs.zig:53-99`. `rg -i tmux src/terminal/kitty src/renderer/image.zig` finds nothing. Passthrough is tmux's job; Ghostty sees whatever tmux forwards.
- zellij: not present. `rg -i zellij src` matches only `src/apprt/embedded.zig`.
- Placeholders are the path that survives multiplexers, because they are ordinary cells with colors and combining marks. That follows from section 7; no code in Ghostty targets it.
- Windows ConPTY:
  - `CreatePseudoConsole` is called with flags `0`, so no passthrough mode is requested, `src/pty.zig:443-449`.
  - Resizes carry no pixel size, `src/pty.zig:474-483`.
  - The read loop stays serial on Windows, `src/termio/Exec.zig:1272`.
  - Shared memory is refused, `src/terminal/kitty/graphics_image.zig:204-208`.
  - File paths get extra Windows checks against UNC, device and reserved names, `src/terminal/kitty/windows.zig:1-35`, `src/terminal/kitty/graphics_image.zig:350-359`.
  - `rg -i 'conpty|pseudoconsole' src` shows no handling of APC replies being dropped on the input side; that problem is not addressed.
- Over SSH, the opt-in `ssh-env` shell integration feature rewrites TERM to `xterm-256color` and propagates `TERM_PROGRAM`, and `ssh-terminfo` installs the terminfo remotely so `xterm-ghostty` can be kept, `src/config/Config.zig:2960-2983`. A client on the remote side then has only `TERM_PROGRAM` and an `a=q` probe to go on (section 3).

## 9. Tests and specs

Specs and docs: no design doc for images in the tree. `rg -l -i 'kitty graphics|graphics protocol' -g '*.md'` finds only `README.md:84` and `example/c-vt-kitty-graphics/README.md`, which is a C example that installs a PNG decoder and writes one image. The module headers carry the design: `src/terminal/kitty/graphics.zig:1-12` and `src/terminal/kitty/graphics_animation.zig:1-22`.

Tests worth reading or porting, as behavioral specs of kitty semantics:

- `src/terminal/kitty/graphics_command.zig:1226-1872`: parser edge cases, including i32 keys, overflow, unknown keys, `m` with non-direct media, and delete ranges.
- `src/terminal/kitty/graphics_exec.zig:1095-3603`: execution. Covers chunked quiet handling, id/number rules, retransmit semantics, cursor movement after tall images, relative placements and animation.
- `src/terminal/kitty/graphics_storage.zig:2126-4664`: deletes by every specifier, eviction order, generation stamps, pending images, and scroll-with-margins clipping (`3821-4243`).
- `src/terminal/kitty/graphics_unicode.zig:858-1349`: placeholder run parsing and tile geometry against `testdata/dog.png` (`1174-1349`).
- `src/renderer/image.zig:1155-1519`: renderer-side placement building, relative placements and animation upload.
- `src/terminal/kitty/graphics_image.zig:911-2205`: loading from every medium, with raw test vectors in `src/terminal/kitty/testdata/`.
- `src/terminal/PageList.zig:18843-18927` and `src/terminal/Terminal.zig:6992`, `14168`, `15856`: placeholder flag through print, reflow, erase and reset.
- Benchmarks: `src/benchmark/ApcParser.zig` and the corpus generator `src/synthetic/Kitty.zig` measure APC parse throughput apart from decode.

## 10. Lessons

Hard-won decisions in comments:

- The subsystem's own header admits it is slow: extra allocations, C code, repeated lookups, `src/terminal/kitty/graphics.zig:6-12`.
- Retransmitting an id must delete at the start of the transmission, not at completion, `src/terminal/kitty/graphics_exec.zig:1018-1025`; commit `b8222f4a8` "clear placements on image retransmit".
- `m` must be ignored for non-direct media because mpv depends on it, `src/terminal/kitty/graphics_command.zig:626-639`.
- `C=0` cursor movement had to be bounded against untrusted row counts, `src/terminal/kitty/graphics_exec.zig:398-418`; commit `afb61e1b6` "place cursor after tall images properly".
- Scroll-with-margins is two-phase because different scroll implementations move tracked pins inconsistently, `src/terminal/kitty/graphics_storage.zig:444-458`; commit `90bce0d2d`.
- Orphaned relative placements are reaped eagerly because "our renderer never mutates terminal state", `src/terminal/kitty/graphics_storage.zig:667-672`. `animationTick` does mutate storage from the renderer thread, under the terminal lock, `src/renderer/generic.zig:1451-1470`, so that sentence is now only mostly true.
- `dirty` versus `generation`: an unchanged generation guarantees unchanged content, so texture caches key off it. Geometry-only events must not bump it, `src/terminal/kitty/graphics_storage.zig:76-104`, `src/terminal/kitty/graphics_storage.zig:188-196`. The stamp is process-global so it stays unique across screens and resets, `src/terminal/kitty/graphics_storage.zig:21-32`; commit `bdc0b6c19` replaced a transmit-time key.
- The renderer's image rebuild must hold `draw_mutex`, since `drawFrame` reads the same state, `src/renderer/generic.zig:1480-1483`; commit `548930a74`.
- Animation frames are composed eagerly into full RGBA buffers, trading memory for having no reference chains to maintain, `src/terminal/kitty/graphics_animation.zig:5-13`.
- Unplaced animated images do not tick, so a never-placed animation cannot wake the renderer forever, `src/terminal/kitty/graphics_storage.zig:1077-1081`.

Known limits and TODOs:

- One vertex buffer and one draw per placement per frame, `src/renderer/image.zig:138`.
- No mipmaps, so heavily downscaled images alias, `src/renderer/OpenGL.zig:396-398`.
- A texture is replaced rather than updated in place, `src/renderer/image.zig:1100-1102`.
- Decode runs under the terminal lock on the reader thread (section 2).
- Placeholders force a rebuild every frame while any virtual placement exists, `src/renderer/image.zig:244-248`.
- Windows has no shared memory, `src/terminal/kitty/graphics_image.zig:204-205`.

Bug-fix history in the log (`git log --grep` over this checkout) is dense in spec conformance: `f8b40a023` deletion mismatches, `8524cb593` point deletion math for `d=p`/`d=c`, `abd77067d` exact `S` size, `c5a3c7e2e` offsets clamped to the cell, `52190a5d8` source rect intersected before sizing, `83145c0a3` conflicting identifiers, `e5747cf0b` aborting incomplete loads, `402b9227d` reclaiming pruned placements, and `d0c516f8f` releasing replaced placement pins.

## Takeaways for alacritree

1. **Execute at APC end, anchored to content.** Ghostty executes each kitty command when the APC terminates, on the parser thread, and reads the cursor then. For `a=T` that is the cursor at the final chunk (section 2). The vte patch should hand over the complete payload plus the cursor at string end, and chunk reassembly belongs above the parser, as in Ghostty's `LoadingImage`. The anchor has to follow content into scrollback and through reflow, which Ghostty gets from tracked pins (section 5). **Unverified** for alacritty_terminal, which I did not read: it offers no tracked-pin facility that I know of, so alacritree would need its own anchor, such as an absolute line index adjusted on scroll, resize and reflow. Copy Ghostty's margin-scroll clipping rules and its "IL/DL do not move images" rule from the tests in section 9.

2. **Key textures by a content generation and keep geometry dirt separate.** Ghostty's `generation` (content) versus `dirty` (geometry) split lets the renderer skip re-uploads on scroll and rebuild only positions (sections 4 and 6). This fits a damage-tracked grid: scrolling re-derives image quads, and textures are touched only when `(id, generation)` changes. Do not copy the full-texture replacement per animation frame. `glTexSubImage2D` into the existing texture is the obvious improvement Ghostty itself leaves as a TODO.

3. **Batch placements as instances, which Ghostty does not.** Ghostty allocates a vertex buffer and issues a draw per placement per frame (section 6). The image shader itself is portable to alacritree's GLSL 140 / ES 3.0 setup: a `gl_VertexID` quad plus four per-instance attributes and `textureSize`. **Unverified**: `layout(binding = 0)` on the sampler needs a newer GLSL, so set the sampler unit from the host instead. Keep one texture per image as Ghostty does. Upload all visible placements into one instance buffer, sorted by z then texture, and issue one instanced draw per texture run.

4. **Slot three z bands around the existing passes.** Ghostty's bands are `z < -2^30` before cell backgrounds, `-2^30 <= z < 0` between backgrounds and text, and `z >= 0` after text (section 6). alacritree's order would be background images, then the backgrounds instanced draw, then below-text images, then glyphs and underlines, then above-text images. Ghostty's cursor lives in the text pass, so `z >= 0` images cover it. Decide deliberately where alacritree's cursor and selection sit relative to the top band.

5. **Treat placeholders as the multiplexer path and budget for them.** zellij, herdr and tmux users reach images only through U+10EEEE cells. Ghostty handles them with a per-row flag set at print time and a per-frame scan of flagged viewport rows while any virtual placement exists, drawing the tiles at `z = -1` and shaping the placeholder as a space (section 7). alacritree needs an equivalent cheap "which rows hold placeholders" index, because an unconditional per-frame grid scan defeats damage tracking. The diacritic table, the run-merge rule and the fit-and-center tile math can be ported with their tests.

6. **Do not copy the Windows and threading gaps.** On Windows Ghostty gets no pixel size through ConPTY, requests no passthrough, refuses shared memory, and has nothing for APC replies lost on ConPTY's input side (sections 3 and 8). alacritree should answer `CSI 14 t` and `CSI 16 t` itself, since clients like chafa and yazi can fall back to them, and should not rely on `a=q` replies surviving ConPTY. Ghostty also inflates and decodes PNG on the reader thread under the terminal lock (section 2). A 400 MB-capped PNG decode there stalls the pane. **Unverified** how alacritty_terminal's PTY thread lock would behave the same way; decoding off the lock with a pending-image slot, which Ghostty already models as `Image.Data.pending`, avoids it.
