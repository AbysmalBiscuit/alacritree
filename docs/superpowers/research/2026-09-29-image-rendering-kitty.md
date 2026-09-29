# Image rendering in kitty

kitty at commit `c73326a9d861eba56b97a068ffd9ea5fd4a75d2d`, checkout `C:\Users\Lev\.local\share\devkit\docs\kitty\master`. Every `path:line` below is relative to that checkout.

## Summary

- kitty implements the kitty graphics protocol (every action, all four media, formats 24/32/100, zlib, chunking, animation, relative placements) and Unicode placeholders, and nothing else: no sixel, no iTerm2 `File=` images.
- The I/O thread only reads PTY bytes. Parsing, base64 decoding, zlib and libpng decoding, and the GPU upload all run synchronously on the main thread inside the VT parser, with the live cursor.
- A placement is a (row, column) anchor in a per-buffer `GraphicsManager`. Index and reverse index move it, scroll margins clip it by rewriting its source rect, and erase-in-line, insert/delete lines, text and column reflow leave it where it is.
- Rendering uses one `GL_TEXTURE_2D` per image and one draw call per placement, sorted by z into three bands around the cell background and foreground passes. Placeholder cells become ordinary placements, rebuilt per dirty row at render time.
- `kitten icat` detects support with `a=q` probes followed by DA1 and exits with an error on a DA1-only answer or after a 10 second timeout. `--transfer-mode=stream` skips detection, and every transmission is sent with `q=2`, so displaying an image needs no reply.

## 1. Protocols

kitty graphics protocol:

- Recognised as APC `G` in `dispatch_apc` (`kitty/vt-parser.c:1480-1486`). Any other APC is logged and dropped.
- Actions accepted by the key parser are `T a c d f p q t` (`kitty/parse-graphics-command.h:110`). A missing `a` means transmit (`kitty/graphics.c:2639-2642`). The action switch is `grman_handle_command` (`kitty/graphics.c:2620-2726`).
- Transmission media `d f t s` (`kitty/parse-graphics-command.h:130`, `kitty/graphics.c:776-824`). `t=t` deletes the file only when its path contains `tty-graphics-protocol` (`kitty/graphics.c:811-814`); `t=s` calls `shm_unlink` (`kitty/graphics.c:815`). Files are read with `pread`, never mapped, because a client may truncate the file mid-read (`kitty/graphics.c:415-419`, `kitty/graphics.c:432-441`).
- Formats 24, 32 (default) and 100 (`kitty/graphics.c:695`, `kitty/graphics.c:894-909`). PNG is decoded with libpng (`kitty/png-reader.h:11`) up to 10000 px per side (`kitty/graphics.c:27`, `kitty/graphics.c:509`).
- Compression: only `o=z` (`kitty/parse-graphics-command.h:140`), inflated in one shot and rejected unless the output is exactly the expected size (`kitty/graphics.c:474-499`).
- Chunking with `m=1` (`kitty/graphics.c:778-793`), where continuation chunks reuse the first chunk's keys (`kitty/graphics.c:925-932`).
- Animation: frame load `a=f` (`kitty/graphics.c:1861-2017`), control `a=a` (`kitty/graphics.c:2067-2108`), compose `a=c` (`kitty/graphics.c:2160-2244`), frame delete `d=f/F` (`kitty/graphics.c:2021-2065`), default gap 40 ms (`kitty/graphics.c:1614`).
- Relative placements `P Q H V`, with the chain depth limited to 8 and `ECYCLE`/`ETOODEEP`/`ENOPARENT` errors (`kitty/graphics.c:81`, `kitty/graphics.c:1292-1364`).
- Usage hint `N=1` (transient), which only affects eviction order and disk caching (`kitty/graphics.h:12`, `kitty/graphics.c:328-330`, `kitty/graphics.c:1020`).

Unicode placeholders: virtual placements with `U=1` (`kitty/graphics.c:1414-1417`) and the per-row resolver (`kitty/screen.c:3966-4043`). See section 7.

Other image paths that are not client protocols: the window logo and background image are config features drawn through the same graphics program (`kitty/shaders.c:1767`, `kitty/shaders.c:2328`).

Not present:

- Sixel. `rg -i sixel` over `kitty/` (`*.c`, `*.h`, `*.py`) and over `docs/` returns nothing. `dispatch_dcs` handles only `+q`, `$q` and `=1s`/`=2s` (`kitty/vt-parser.c:728-760`).
- iTerm2 inline images. OSC 1337 is routed to `desktop_notify` (`kitty/vt-parser.c:587-590`), and `osc_1337` only handles `SetUserVar` (`kitty/window.py:1366-1371`). There is no `File=` handling.
- Windows named shared memory, which the spec allows for `t=s` (`docs/graphics-protocol.rst:347-353`). kitty only calls POSIX `safe_shm_open` (`kitty/graphics.c:737-742`), and `rg -i "_WIN32|windows" kitty/graphics.c` returns nothing.

## 2. Parser to model

Threads:

- The I/O thread (`io_loop`, `kitty/child-monitor.c:1810`) reads PTY bytes straight into the parser's buffer through `vt_parser_create_write_buffer` and `vt_parser_commit_write` (`kitty/child-monitor.c:1674-1691`, `kitty/vt-parser.c:1625-1657`), under the parser's mutex.
- The main thread's tick (`process_global_state`, `kitty/child-monitor.c:1563-1577`) calls `parse_input`, which calls `parse_worker` for each window (`kitty/child-monitor.c:533-541`, `kitty/child-monitor.c:543-641`). The parser drops the lock while it consumes already-read input (`kitty/vt-parser.c:1583-1621`).
- Everything after that runs on the main thread: key parsing, base64 decoding, zlib and PNG inflation, GPU upload and the reply. `upload_to_gpu` makes the window's GL context current in the middle of parsing (`kitty/graphics.c:934-942`).

Buffering and limits:

- The parser buffer is 1 MiB and the longest escape code is a quarter of that, 256 KiB (`kitty/vt-parser.c:18-21`). An APC is dispatched as soon as its ST is found (`kitty/vt-parser.c:451-461`). An unterminated one that grows past 256 KiB is logged and dropped (`kitty/vt-parser.c:463-477`).
- `parse_graphics_code` decodes the base64 payload in place, into the same buffer (`kitty/parse-graphics-command.h:201-208`), and calls `screen_handle_graphics_command` directly (`kitty/parse-graphics-command.h:237`).
- `screen_handle_graphics_command` passes the live `self->cursor` into the graphics manager (`kitty/screen.c:1916-1918`), so a placement lands at the cursor as it is when the final chunk is parsed. For `a=T` the put happens only after the load completes (`kitty/graphics.c:2658`), which is what the spec requires (`docs/graphics-protocol.rst:424-426`).
- One `LoadData` accumulator exists per `GraphicsManager` (`kitty/graphics.h:134-145`, `kitty/graphics.h:156`). For direct transmission it is preallocated to the expected size plus a small slack (`kitty/graphics.c:912-921`). Only PNG may grow it, up to `MAX_DATA_SZ` = 400,000,000 bytes; any other format that overflows gets `EFBIG` (`kitty/graphics.c:694`, `kitty/graphics.c:778-786`).
- While a direct chunked load is pending, every `t=d` command is treated as its continuation, whatever keys it carries (`kitty/graphics.c:950`, `kitty/graphics.c:983-990`). A continuation with no `a` key during an `a=f` load is routed to the frame handler (`kitty/graphics.c:2631-2637`).
- The spec's 4096-byte chunk size is not enforced. kitty's own client sends 128 KiB base64 chunks (`tools/tui/graphics/command.go:287`) without padding (`tools/tui/graphics/command.go:288`).

## 3. Replies and detection

Replies:

- An error sets `CODE:message` in a 512-byte static buffer (`kitty/graphics.c:337-347`). `finish_command_response` builds `Gi=<id>[,I=<n>][,p=<p>];OK` or the error (`kitty/graphics.c:1033-1058`).
- No reply is sent when the command has neither `i` nor `I` (`kitty/graphics.c:1040`, `kitty/graphics.c:1057`). `q=1` suppresses OK and `q=2` suppresses errors as well (`kitty/graphics.c:1037-1039`). Specifying both `i` and `I` is `EINVAL` (`kitty/graphics.c:2626-2629`).
- `a=q` loads the data with the id forced to 0 so nothing is stored, replies with the original id, then trims the unnamed image (`kitty/graphics.c:2643-2660`). A query without `i` is logged and gets no reply (`kitty/graphics.c:2647-2650`).
- The reply is written with `write_escape_code_to_child(ESC_APC, ...)` (`kitty/screen.c:1919`), which schedules a write through the I/O thread (`kitty/screen.c:1766-1784`). The DA1 reply goes through the same function from Python (`kitty/window.py:1603-1604`, `kitty/screen.c:6707`), so replies leave in parse order. Ordering is inferred from both paths using `schedule_write_to_child`, **unverified** by a test.

What a program can probe:

- DA1 answers `CSI ?62;c`, or `?62;52;c` when clipboard writing is allowed (`kitty/window.py:655-660`, `kitty/screen.c:3172-3182`). It carries no graphics attribute.
- XTVERSION answers `DCS >|kitty(<version>) ST` (`kitty/screen.c:3185-3187`).
- Environment: `TERM` from the `term` option (`kitty/child.py:382`), `KITTY_PID` (`kitty/child.py:384`), `KITTY_WINDOW_ID` (`kitty/tabs.py:778`), `TERMINFO` (`kitty/child.py:401-403`). kitty sets no `TERM_PROGRAM`; `rg TERM_PROGRAM` over `kitty/` and `docs/` returns nothing.
- The spec's detection recipe is an `a=q` query followed by DA1 (`docs/graphics-protocol.rst:456-473`).

Pixel sizes:

- `CSI 14t` answers `4;<h>;<w>t` with cell size times the grid, `CSI 14;2t` the OS window framebuffer, `CSI 16t` `6;<ch>;<cw>t`, and `CSI 18t` `8;<lines>;<cols>t` (`kitty/screen.c:3189-3223`, `kitty/vt-parser.c:1355-1357`).
- `TIOCGWINSZ` gets `xpixel`/`ypixel` from the window geometry in pixels (`kitty/window.py:1093`), applied by `resize_pty` (`kitty/child-monitor.c:694-716`).

How `kitten icat` detects support:

- `DetectSupport` sends three 1x1 RGB `a=q` probes (direct, temp file, shared memory), then `CSI c` (`kittens/icat/detect.go:52-85`). An OK marks that medium (`kittens/icat/detect.go:97-110`). The DA1 reply ends the loop (`kittens/icat/detect.go:92-95`). A timer fails the loop with a deadline error (`kittens/icat/detect.go:48-50`), and the timeout defaults to 10 seconds (`kittens/icat/main.py:83-87`).
- If direct is not supported, icat exits with "This terminal does not support the graphics protocol" (`kittens/icat/main.go:290-293`). A timeout error is returned as exit code 1 (`kittens/icat/main.go:286-289`).
- Detection runs only with `--transfer-mode=detect` (the default) or `--detect-support`, and never under tmux passthrough (`kittens/icat/main.go:285`). `--transfer-mode=stream` skips it (`kittens/icat/main.py:63-72`).
- Every transmission is sent with `q=2` (`kittens/icat/transmit.go:49`), so after detection icat reads nothing back.
- icat reads its size only from `TIOCGWINSZ` and refuses to run when `xpixel` or `ypixel` is 0 (`kittens/icat/main.go:179-191`, `kittens/icat/main.go:248-250`). `--use-window-size cols,rows,w,h` overrides it (`kittens/icat/main.go:192-221`, `kittens/icat/main.py:90-92`). It never falls back to `CSI 14t`/`16t`. `tools/tty/` holds only Linux and BSD variants (`tools/tty/tty_linux.go`, `tools/tty/tty_bsd.go`), and `GetSize` is a bare `TIOCGWINSZ` ioctl (`tools/tty/tty.go:349-356`).

Consequence for a pane whose replies are dropped (inference, **unverified** against ConPTY): icat sees the DA1 reply without any graphics reply and exits as unsupported. With `--transfer-mode=stream` it transmits blind and the image displays, provided `TIOCGWINSZ` reports non-zero pixel sizes or `--use-window-size` is given.

## 4. Image store

- Each screen has two managers, `main_grman` and `alt_grman`, and `grman` points at the active one (`kitty/screen.c:172-176`). The quota applies per manager, which the spec describes as "320MB per buffer" (`docs/graphics-protocol.rst:1052-1058`).
- Images live in a hash map keyed by an internal id from a wrapping counter (`kitty/graphics.h:171`, `kitty/graphics.c:75-80`, `kitty/graphics.c:654`). Lookup by client id is a linear scan (`kitty/graphics.c:215-219`). Lookup by number returns the newest image with that number (`kitty/graphics.c:221-230`).
- An image created with only `I` gets the smallest unused client id (`kitty/graphics.c:661-684`, `kitty/graphics.c:974-977`). Retransmitting an existing id frees its frames, texture and placements (`kitty/graphics.c:957-970`).
- Placements are `ImageRef`s in a per-image map (`kitty/graphics.h:68-91`, `kitty/graphics.h:126`). A put with an existing `p` updates that ref in place (`kitty/graphics.c:1366-1388`).
- Decoding happens on the main thread when the final chunk arrives (`kitty/graphics.c:840-884`).
- Pixels move twice. The decoded buffer is uploaded to the GPU first, then ownership of the CPU buffer moves into the disk cache (`kitty/graphics.c:1017-1023`, `kitty/graphics.c:45-53`). The cache's write thread writes entries to an anonymous temp file, XOR-encrypted with a random key when the file could not be opened securely (`kitty/disk-cache.c:416-437`, `kitty/disk-cache.c:527`, `kitty/disk-cache.c:590`). Transient images stay memory-only (`kitty/disk-cache.c:712-714`). Frames are read back from the cache for animation composition (`kitty/graphics.c:1778-1811`).
- The storage limit is 320 MiB (`kitty/graphics.c:28`, `kitty/graphics.c:88`). Over the limit, images without data or without placements go first, then transient images, then the least recently used by `atime` (`kitty/graphics.c:318-335`). Animation frames have a separate limit of five times that, measured on the cache size (`kitty/graphics.c:1891-1894`).
- Every new transmission first removes images that have no data, or no client id and no placements (`kitty/graphics.c:525-528`, `kitty/graphics.c:955`).
- Images without an id or number are freed once their last placement scrolls out of history (`kitty/graphics.c:2299-2303`).
- Textures are reference counted so that paused rendering (pending mode) can hold a snapshot of the image list (`kitty/graphics.c:137-161`, `kitty/graphics.c:272-309`).

## 5. Placement model

Anchor and size:

- A ref stores `start_row`, `start_column`, in-cell pixel offsets, requested `num_rows`/`num_cols` and computed `effective_num_rows`/`effective_num_cols` (`kitty/graphics.h:68-91`). The anchor is the cursor cell (`kitty/graphics.c:1400-1401`). In-cell offsets are clamped below the cell size (`kitty/graphics.c:1402-1403`).
- The effective size comes from the source size, or from one of `c`/`r` with the other derived from the aspect ratio (`kitty/graphics.c:1073-1101`).
- When both `c` and `r` are given, a classic placement is stretched to fill the box (`kitty/graphics.c:1543-1558`), and `test_image_put` asserts that stretched rect (`kitty_tests/graphics.py:803-815`). The spec says the image is letterboxed (`docs/graphics-protocol.rst:549-552`). Only placeholder placements are fit and centered (`kitty/graphics.c:1201-1214`).

Cursor after placement:

- Unless `C=1`, `U=1` or a parent is given, the cursor moves right by the placement's columns and down by rows minus one (`kitty/graphics.c:1418-1429`). The screen then wraps a column past the edge to the next line and scrolls when the row passes the bottom margin (`kitty/screen.c:1920-1928`). Tests: a 1x1 image leaves the cursor at (1, 0) and a 3x3 at (3, 2) (`kitty_tests/graphics.py:801`, `kitty_tests/graphics.py:821-822`).

What moves or removes a placement:

- Scrolling. `INDEX_UP` and `INDEX_DOWN` call `INDEX_GRAPHICS(-1)` and `INDEX_GRAPHICS(1)` (`kitty/screen.c:430-442`, `kitty/screen.c:445-453`, `kitty/screen.c:2493-2495`). They cover LF at the bottom margin, `SU`, `SD`, `RI` and `ED 22` (`kitty/screen.c:2511-2581`, `kitty/screen.c:2590-2595`, `kitty/screen.c:2914-2929`).
- Without margins, every non-virtual ref shifts by the amount, and a ref is removed once it lies wholly above `-historybuf->ynum` on the main screen or above row 0 on the alternate screen (`kitty/graphics.c:2308-2314`, `kitty/screen.c:436`). The limit is the history capacity, not the current line count.
- With margins, only refs wholly inside the region move. A ref that crosses a margin is clipped by shrinking its source rect and row count, and removed when it leaves the region (`kitty/graphics.c:2316-2380`). Refs that straddle a margin before the scroll are left alone. `src_pixels_per_screen_pixel` must match the render-time dest rect so scaled images clip instead of distorting (`kitty/graphics.c:2326-2338`). Tests: `kitty_tests/graphics.py:1193-1272`.
- Scrollback viewing moves nothing. The render offsets every ref by `scrolled_by` plus the pixel-scroll fraction (`kitty/graphics.c:1513`) and rebuilds the list when either changes (`kitty/graphics.c:1499-1503`).
- Resize. All placeholder refs are dropped (`kitty/screen.c:672-676`). `grman_resize` shifts refs up by the lost content lines only when the column count is unchanged and content shrank; a column change leaves refs where they were (`kitty/graphics.c:2578-2601`). Refs do not follow text reflow. A font size change recomputes each dest rect (`kitty/graphics.c:2603-2618`, `kitty/screen.c:752-758`).
- Alternate screen. Entering it with clearing wipes the alternate manager's refs (`kitty/screen.c:1944-1948`), and the active manager is swapped both ways (`kitty/screen.c:1953`, `kitty/screen.c:1961`).
- Erase. `ED 2` removes refs with any row on screen and keeps those wholly in scrollback. `ED 3` removes everything. `ED 22` pushes the screen into scrollback first (`kitty/screen.c:2961-2976`, `kitty/graphics.c:2423-2439`). `ED 0`/`ED 1`, `EL` and `ECH` do not touch placements and only invalidate placeholder refs (`kitty/screen.c:2980`, `kitty/screen.c:2873`, `kitty/screen.c:3133-3144`). This follows the spec (`docs/graphics-protocol.rst:1175-1182`).
- Reset. A hard reset keeps main-screen refs that sit wholly in scrollback and clears all alternate-screen refs (`kitty/screen.c:235-241`). `test_gr_reset` covers it (`kitty_tests/graphics.py:1276-1288`).
- Text written over a placement has no effect on it. The text-drawing path only flags rows that contain U+10EEEE (`kitty/screen.c:1457-1460`). That no other `grman_` call sits in the draw path is inferred from the full list of `grman_` call sites in `kitty/screen.c`.
- Insert and delete lines (`IL`, `DL`) do not move classic placements. They only drop placeholder refs in the region and dirty those rows (`kitty/screen.c:3001-3030`, `kitty/screen.c:3048-3059`, `kitty/screen.c:1838-1851`). `ICH`/`DCH` do nothing to images (`kitty/screen.c:3071-3131`).

z-index:

- Three bands. Below `INT32_MIN/2`, negative, and zero or above (`kitty/graphics.c:1565-1567`).
- The draw order within the list is z, then internal image id, then ref id (`kitty/graphics.c:1592-1596`). The spec says the lower image id wins on a tie (`docs/graphics-protocol.rst:560-562`). kitty compares the internal id (`kitty/graphics.c:1575`), which is creation order rather than the client's `i`.

Deletion (`a=d`, `kitty/graphics.c:2495-2574`):

- Any delete aborts a pending chunked upload (`kitty/graphics.c:2497`), as the spec requires (`docs/graphics-protocol.rst:803-804`).
- Letter mapping: `a` visible non-placeholder refs, `i` by id with optional `p`, `r` id range, `p` cell, `q` cell and z, `x` column, `y` row, `z` z-index, `c` cursor cell, `n` newest image with a number, `f` frames (`kitty/graphics.c:2530-2566`).
- Uppercase frees an image once it has no refs left. Lowercase frees it only when it has no client id (`kitty/graphics.c:2283`). A bare uppercase `I`/`N` on an image with no refs frees it directly (`kitty/graphics.c:2499-2520`).
- "Visible" means any row at or below screen row 0, independent of the scrollback view (`kitty/graphics.c:2429-2433`).
- Positional filters skip virtual and placeholder refs. Id, range and number filters also hit virtual refs (`kitty/graphics.c:2446-2492`).

## 6. Render pipeline

- API. OpenGL 3.3, or 3.1 on some builds (`kitty/data-types.h:25-30`). Shaders are written in Slang and compiled to GLSL 150 or newer at build time (`kitty/shaders/slang.py:687-691`, `kitty/shaders/graphics.slang`). There are no `.glsl` sources under `kitty/`; `fd -e glsl` over `kitty/` returns nothing.
- Textures. One `GL_TEXTURE_2D` per image, `GL_SRGB_ALPHA`, linear filtering, `GL_CLAMP_TO_BORDER` with a transparent border (`kitty/shaders.c:315-336`, `kitty/graphics.c:941`). The glyph sprites use a separate `GL_TEXTURE_2D_ARRAY` (`kitty/shaders.c:260`). No image atlas.
- Render list. `grman_update_layers` walks every image and ref, skips virtual refs, resolves parents, computes an NDC dest rect and culls refs wholly above or below the screen (`kitty/graphics.c:1487-1563`). The list is sorted and consecutive entries of the same image are grouped (`kitty/graphics.c:1592-1607`).
- Draw. `draw_graphics` binds the texture once per group, then issues one `glDrawArrays(GL_TRIANGLE_FAN, 0, 4)` per placement with `src_rect` and `dest_rect` as uniforms (`kitty/shaders.c:1070-1091`, `kitty/gl.c:131-136`). The vertex shader maps the vertex id to rect corners (`kitty/shaders/blit-common.slang`, `kitty/shaders/graphics.slang:19-24`) and the fragment shader premultiplies alpha (`kitty/shaders/graphics.slang:28-41`). There is no instancing for images.
- Order (`kitty/shaders.c:2037-2061`):
  1. default-background cells
  2. window logo
  3. images with z below `INT32_MIN/2`
  4. non-default-background cells
  5. images with negative z
  6. the cell foreground pass
  7. images with z of 0 or more
  8. visual bell, drag overlay, progress bar, scrollbar, hyperlink target and window number

  The cursor, selection colors and underlines are drawn inside the cell foreground pass (`kitty/shaders/cell.slang:168-183`, `kitty/shaders/cell.slang:281-287`), so images with z of 0 or more cover the cursor and the selection. Placeholder refs use z = -1 so the cursor stays on top (`kitty/graphics.c:1276-1277`).
- Layers. When a window has images, kitty renders it into an offscreen RGBA16 texture (`kitty/shaders.c:1116-1117`, `kitty/shaders.c:1770-1776`, `kitty/shaders.c:2383-2409`). Otherwise it draws one combined cell pass (`kitty/shaders.c:1799-1802`).
- Clipping. The viewport is set to the window's geometry (`kitty/shaders.c:2134`) and images use no scissor. That parts of a quad outside the window are removed by clip-space clipping is inferred from the NDC rects, **unverified**. Scroll-region clipping is done once on the CPU at scroll time by rewriting the source rect (`kitty/graphics.c:2340-2380`), not per frame.
- Dirty tracking. Any ref change sets `layers_dirty` (`kitty/graphics.c:245-248`). The list is rebuilt only when that flag is set or the scroll offset changed (`kitty/graphics.c:1499-1503`), from `cell_prepare_to_render` each frame (`kitty/shaders.c:1036-1058`). Per-frame cost is one draw call per visible placement plus one texture bind per image group.
- Animation. Frames live in the disk cache. On a frame change kitty composes the frame on the CPU and re-uploads the whole frame into the image's texture with `glTexImage2D` (`kitty/graphics.c:1818-1835`). `scan_active_animations` advances frames and returns the shortest remaining gap, which sets the main loop's wait (`kitty/graphics.c:2116-2150`, `kitty/child-monitor.c:925-931`). Only images that were drawn animate (`kitty/graphics.c:2110-2114`, `kitty/graphics.c:1585-1588`).

## 7. Unicode placeholders

- The placeholder is `IMAGE_PLACEHOLDER_CHAR` = U+10EEEE (`kitty/data-types.h:163`), rendered with the blank font so the glyph pass draws nothing (`kitty/fonts.c:809`).
- `a=p,U=1` (or `a=T,U=1`) creates a virtual ref at row 0, column 0 that is never drawn and never moves the cursor (`kitty/graphics.c:1413-1417`, `kitty/graphics.c:1425`, `kitty/graphics.c:1525-1528`). A virtual placement cannot have a parent (`kitty/graphics.c:1323-1326`). After any `U=1` command every screen row with placeholders is dirtied (`kitty/screen.c:1929-1932`).
- Writing U+10EEEE sets `has_image_placeholders` on the row (`kitty/screen.c:1457-1460`).
- Resolution happens at render time. `screen_update_cell_data` calls `screen_render_line_graphics` for each dirty screen row and for every visible history row on every update, because a graphics command can change what an already-scanned placeholder shows (`kitty/screen.c:4109-4125`).
- `screen_render_line_graphics` drops the row's placeholder refs and rescans it (`kitty/screen.c:3966-4043`):
  - Image id low 24 bits come from the foreground color and the placement id from the underline color, both taking the top 24 bits of the stored color, so 256-color and truecolor both work (`kitty/screen.c:3956-3961`, `kitty/screen.c:3993-3994`).
  - The first three combining marks are row, column and the id's high byte (`kitty/screen.c:3997-4001`). `diacritic_to_num` maps each mark to a 1-based number, 0 meaning absent (`kitty/rowcolumn-diacritics.c:8`), from the table in `gen/rowcolumn-diacritics.txt` (`gen/wcwidth.py:398-410`).
  - Adjacent cells with the same image id and placement id, compatible row, next column and compatible high byte merge into one run, with missing values inherited from the left (`kitty/screen.c:4006-4014`). Each run becomes one call to `grman_put_cell_image` (`kitty/screen.c:4018-4022`, `kitty/screen.c:4037-4042`).
- `grman_put_cell_image` finds the virtual ref by placement id, or the first virtual ref (`kitty/graphics.c:1150-1174`). It fits the image into the virtual box of `c` x `r` cells, preserving aspect and centering (`kitty/graphics.c:1181-1214`), derives the run's source rect, trims fully empty rows and columns (`kitty/graphics.c:1216-1274`), and creates a real ref tagged with `virtual_ref_id` at z = -1 (`kitty/graphics.c:1276-1286`).
- Placeholder refs are treated as derived data. Resize and rescale drop them all (`kitty/screen.c:672-676`, `kitty/screen.c:754-755`). Erase, `IL` and `DL` drop them in the affected rows (`kitty/screen.c:1838-1851`). The next render rebuilds them from the text. Tests: `kitty_tests/graphics.py:949-1177`.
- A relative placement whose parent is virtual takes the minimum row and column of that parent's placeholder refs (`kitty/graphics.c:1442-1456`, `kitty/graphics.c:1472-1476`).
- icat's placeholder output: a random id with non-zero high and middle bytes (`kittens/icat/transmit.go:278-289`), foreground set with `38:2` (`kittens/icat/transmit.go:223`), all three marks on every cell (`kittens/icat/transmit.go:244`). The maximum grid is the table length per side (`kittens/icat/transmit.go:298-301`).

## 8. Platform and multiplexer paths

- tmux. icat detects tmux from `TMUX` (`tools/tui/tmux.go:28-29`), wraps each command in `DCS tmux; ... ST` with ESC doubled (`kittens/icat/transmit.go:35-44`), and turns on `allow-passthrough` (`tools/tui/tmux.go:69-104`). Because replies do not come back through tmux, it disables file and memory transfer and forces Unicode placeholders (`kittens/icat/main.go:275-283`, `kittens/icat/main.go:305-309`, `kittens/icat/main.go:320-323`).
- The kitty terminal does not unwrap `DCS tmux;`; `rg tmux kitty/vt-parser.c` returns nothing, since tmux does the unwrapping.
- zellij. Not present. `rg -i zellij` over the whole checkout matches only `docs/faq.rst:448`. That FAQ says image display inside such programs depends on them supporting Unicode placeholders (`docs/faq.rst:463-466`).
- Interleaved uploads. Any direct transmission during a pending chunked load is taken as its continuation (`kitty/graphics.c:950`). Two programs streaming chunks through one PTY corrupt each other's images. The spec puts the burden on the client (`docs/graphics-protocol.rst:422-424`).
- Local media. `f`, `t` and `s` only work when client and terminal share a filesystem (`kittens/icat/main.py:70-72`). Paths are policy-checked before opening and re-checked on the open fd (`kitty/graphics.c:744-765`).
- Windows and ConPTY. Not present. `rg -i "conpty|CreatePseudoConsole"` over the whole checkout returns nothing, and `tools/tty/` has no Windows variant.

## 9. Tests and specs

- `docs/graphics-protocol.rst` is the spec. The interaction rules at `docs/graphics-protocol.rst:1172-1189` (reset, alternate screen, erase, scrolling with margins) and the placeholder section at `docs/graphics-protocol.rst:579-700` bear most on grid alignment.
- `kitty_tests/graphics.py` holds the behavior tests. It drives a `Screen` with a synthetic cell size and asserts on the render list (`image_id`, `ref_id`, `src_rect`, `dest_rect`), which ports directly to a Rust placement store:
  - `test_gr_scroll` (`kitty_tests/graphics.py:1179-1274`)
  - `test_unicode_placeholders`, `_3rd_combining_char`, `_multiple_placements`, `_scroll` (`kitty_tests/graphics.py:949-1177`)
  - `test_image_put` (`kitty_tests/graphics.py:792-823`) and `test_graphics_put_with_pixel_offsets` (`kitty_tests/graphics.py:825-830`)
  - `test_gr_delete` (`kitty_tests/graphics.py:1290-1380`) and `test_gr_reset` (`kitty_tests/graphics.py:1276-1288`)
  - `test_image_parents` (`kitty_tests/graphics.py:848-947`) and `test_image_layer_grouping` (`kitty_tests/graphics.py:832-846`)
  - loading, files and PNG (`kitty_tests/graphics.py:440-712`), quota (`kitty_tests/graphics.py:1655-1697`), animation (`kitty_tests/graphics.py:1382-1654`)
  - helpers `send_command`, `put_helpers` (`kitty_tests/graphics.py:31`, `kitty_tests/graphics.py:123`)
- `kitty_tests/parser.py:1135` (`test_graphics_command`) covers control-data parsing and malformed keys.
- `tools/tui/graphics/command_test.go` covers the client serializer, chunking and compression.
- `gen/rowcolumn-diacritics.txt` is the diacritic table to embed.
- `kittens/icat/scaling_test.go` covers icat's fit scaling.

## 10. Lessons

- Scaled images at a margin must be clipped, not distorted. The source-to-screen scale used when clipping has to match the renderer's dest rect math (`kitty/graphics.c:2326-2338`, `docs/changelog.rst:360-362`, `kitty_tests/graphics.py:1229-1272`).
- Continuation chunks carry only `m` (and `q`), so a chunked `a=f` must remember that it is a frame load. Before the fix it replaced the root frame (`kitty/graphics.c:2631-2637`, `docs/changelog.rst:388-391`).
- The frame composition mode is `X`, not `C`, and fully transparent pixels are a no-op unless `X=1` (`docs/changelog.rst:384-386`, `kitty_tests/graphics.py:1430-1440`).
- Every file-read failure returns the same `EBADF:Failed to read image file` so a remote client cannot probe the filesystem (`kitty/graphics.c:356-370`, `docs/changelog.rst:423-425`). The fd is closed on every path, since leaking it lets a client exhaust the process's descriptors (`kitty/graphics.c:727-730`).
- Upload to the GPU before handing the buffer to the cache, because the cache takes ownership and may encrypt it in place (`kitty/graphics.c:45-48`, `kitty/graphics.c:1018`, `kitty/graphics.c:2002`, `kitty/graphics.c:2219`).
- Frame composition guards every row against geometry larger than the buffers (`kitty/graphics.c:1683-1687`, `kitty/graphics.c:1703-1706`). Two CVEs came from invalid PNG data and invalid compose offsets (`docs/changelog.rst:649-651`).
- Response suppression has to apply to chunked transmissions as well (`kitty/graphics.c:2654-2655`, `docs/changelog.rst:3434-3435`).
- Placeholder and `c`/`r` images are resized with linear filtering on the GPU (`docs/changelog.rst:1843-1845`).
- Paused rendering must copy the visible ref counts, or the first paused frame picks the wrong layering path (`kitty/graphics.c:283-288`).
- Scroll counts are clamped to the screen height so `CSI 2147483647 S` cannot busy-loop through image scrolling (`kitty/screen.c:2536-2540`).
- Known limits: 10000 px per side, 400 MB of transmitted data, 256 KiB per escape code, 320 MiB per buffer, parent chains of 8, placeholder grids bounded by the diacritic table (`kitty/graphics.c:27-28`, `kitty/graphics.c:694`, `kitty/vt-parser.c:21`, `kitty/graphics.c:81`, `kittens/icat/transmit.go:298-301`).
- `rg "TODO|FIXME|XXX|HACK"` over `kitty/graphics.c`, `kitty/graphics.h`, `kitty/screen.c` and `kitty/shaders.c` returns nothing.

## Takeaways for alacritree

1. **Keep one placement store per screen buffer and drive it from the scroll primitives, not from damage.** kitty's placement is a row anchor that moves only on index and reverse index, with margin clipping done once by rewriting the source rect (section 5). alacritree needs the same hook where `alacritty_terminal` scrolls its grid: a signed delta, the region, and whether lines went to history. Deriving image movement from damaged rows would move images on redraws that are not scrolls. Match kitty's other choices too: erase-in-line, `IL`/`DL`, text and reflow leave classic placements alone, and `ED 2`/`ED 3`/alternate screen clear them.
2. **Add images as passes around the three instanced draws and split the background pass.** kitty's order needs the default-background cells drawn apart from the non-default ones so that images below `INT32_MIN/2` sit between them (section 6). In alacritree's terms: default bg, lowest band, non-default bg, negative band, glyphs and underlines, positive band. Decide where the cursor goes. kitty's cursor is part of the text pass, so images with z of 0 or more cover it, and placeholder images use z = -1 to stay under it.
3. **Use one texture per image, never egui's font atlas, and batch quads per texture.** kitty issues one uniform-driven draw per placement and still binds the texture once per group (section 6). The existing instanced buffer design fits better: one instance record per placement (dest rect, source rect) and one instanced draw per texture group from the already sorted list. kitty relies on `GL_CLAMP_TO_BORDER`, which GLES 3.0 core lacks (from GL knowledge, **unverified** here), so clamp to edge and inset the source rect by half a texel instead.
4. **Resolve Unicode placeholders from row damage, as kitty does at render time.** kitty rebuilds a row's placeholder refs whenever the row is re-rendered, and on every cell data update for visible history rows (section 7). alacritree's damaged-row upload is the same trigger: when a row is re-uploaded, scan it for U+10EEEE and rebuild that row's placeholder quads from fg color, underline color and the first three combining marks. This is the path that zellij and herdr panes, image.nvim and snacks.nvim depend on, and it survives any reflow or scroll because the text carries the image.
5. **The parser patch must hand over the payload with the cursor at the final chunk and keep one accumulator per session.** kitty decodes base64 in the parser, applies the command with the live cursor, and treats every direct transmission during a pending load as a continuation (section 2). Accept chunks well past the spec's 4096 bytes, since icat sends 128 KiB. Deferring PNG and zlib decoding off the parse thread is possible, but the cursor movement, the placement's anchor row and the reply order must still be fixed at parse time.
6. **Do not rely on replies reaching programs under ConPTY.** icat needs an `a=q` reply to pass detection and a non-zero `TIOCGWINSZ` pixel size to run at all, while its actual transmissions use `q=2` and need no reply (section 3). If ConPTY drops APC replies, icat reports "unsupported" unless run with `--transfer-mode=stream` (and `--use-window-size` if pixel sizes are missing). Test that path on Windows first, and design the reply writer so it can later be routed around ConPTY without touching the store.
