# Image rendering across terminals, and what alacritree should build

A snapshot comparison of how four terminals render inline images, written as input to the design for alacritree/alacritree#382 (kitty image protocol). Each row below is backed by the per-terminal report beside this file, which carries the `path:line` citations:

| Terminal | Commit | Report |
|---|---|---|
| kitty | `c73326a9` | `2026-09-29-image-rendering-kitty.md` |
| Ghostty | `0538f753` | `2026-09-29-image-rendering-ghostty.md` |
| WezTerm | `cab25161` | `2026-09-29-image-rendering-wezterm.md` |
| Windows Terminal (with conhost and ConPTY) | `f1ddcbe4` | `2026-09-29-image-rendering-windows-terminal.md` |

To refresh a checkout, run `docm sync <name>`. `docm info <name>` prints its path and commit.

## Comparison

| | kitty | Ghostty | WezTerm | Windows Terminal |
|---|---|---|---|---|
| Protocols | kitty graphics (all of it), unicode placeholders | kitty graphics, unicode placeholders, animation | kitty graphics (partial: no `a=a`, no placeholders, few deletes), sixel, iTerm2 `File=` | sixel only |
| Where the escape runs | main thread, inside the parser, live cursor | reader thread under the terminal lock, cursor read at APC end | parser thread emits ordered actions, applied under the terminal mutex | parser thread, cursor captured at the DCS header |
| Decode | in the parser (base64, zlib, libpng) | in the parser, with a `pending` image state already modelled | kitty PNG in the parser; iTerm2 on a worker with a 125 ms placeholder | char by char into row slices |
| Store | per-screen `GraphicsManager`, quota | per-screen `ImageStorage`, 320 MB default quota | per-cell `ImageCell` in heap-allocated cell attributes, 320 MB budget, records never pruned | pixels inside each row's `ImageSlice`, no quota |
| Anchor | (row, column), moved only by index and reverse index | tracked `PageList.Pin` that follows content | the cells themselves | the rows themselves |
| Text written over an image | image stays | image stays | that cell's slice is deleted | that part of the image is erased |
| IL/DL, erase-in-line, reflow | leave placements alone; ED2, ED3 and alternate screen clear them | same rules as kitty, with tests | move with the cells; reflow can tear an image | move with the rows; reflow drops continuation rows |
| Textures | one `GL_TEXTURE_2D` per image | one GL texture per image, keyed by a generation stamp | shared glyph atlas; when it fills: rebuild, then downscale, then drop | shared glyph atlas, rasterized with Direct2D |
| Draw | one draw per placement, three z bands around the bg and text passes | one draw per placement, same three bands | one 272-byte quad per covered cell, on three layers | one quad per row slice, in the glyph batch, above text |
| Unicode placeholders | rebuilt per dirty row at render time | per-row flag set at print time, viewport rescan | none | none |
| Windows/ConPTY | n/a | no passthrough flag, no pixel size, no reply handling | bundled ConPTY; `PSEUDOCONSOLE_PASSTHROUGH_MODE` defined, never used; client-side workarounds only | is ConPTY; see below |

## Where they agree

kitty and Ghostty were built independently and arrived at the same architecture. That shared design is the default for alacritree:

- Images live outside the grid, in a store per screen buffer (main and alternate). Each image has an id, a quota, and placements. A placement is anchored to a grid row and column, and the scroll primitives move it. Damage does not move it.
- Text and images are separate layers. Printing text over an image leaves the image in place, and so do erase-in-line and IL/DL. Clearing the screen (ED2, ED3) and switching to the alternate screen remove images.
- Each image gets its own GL texture, uploaded once, and re-uploaded only when its content changes. Position changes rebuild only quad geometry.
- Placements are drawn in three z bands. Lowest band: `z < -2^30`, between default-background cells and cells with a non-default background. Middle band: `-2^30 <= z < 0`, between backgrounds and text. Top band: `z >= 0`, after text. The cursor is part of the text pass, so top-band images cover it, and placeholder images use `z = -1` to stay beneath it.
- Unicode placeholder cells are resolved from the text itself, row by row, when a row changes. That is why they survive scrolling, reflow and multiplexers.

## What to avoid

- Storing images in the cells. WezTerm and Windows Terminal do this. It makes grid operations move images for free, but it costs memory and draw work per covered cell. Text written over an image destroys that part of it, and reflow tears images apart. alacritree's 12-byte cell record should not grow to carry images.
- Sharing the glyph atlas. In WezTerm and Windows Terminal, images evict glyphs, and a full atlas forces rebuilds and downscaling. alacritree samples egui's font atlas, which it does not own, so it needs its own image textures regardless.
- Decoding while holding the terminal lock. kitty, Ghostty and WezTerm (for kitty images) all inflate and decode PNGs in the parser. alacritree's PTY thread holds the terminal lock while it parses, so a large PNG stalls both the pane and the paint. WezTerm's iTerm2 path shows the alternative: record the placement at parse time with its known size, decode on a worker, then repaint.
- Unbounded buffers. WezTerm caps neither the APC buffer nor the chunk accumulator. kitty caps each escape at 256 KiB, and its own client sends 128 KiB chunks, far past the spec's suggested 4096 bytes.
- Silent failures. WezTerm sends no kitty `ERROR` replies. Clients rely on errors to fall back.

## ConPTY and Windows

The Windows Terminal report settles most of the earlier ConPTY question. These rules come from the code; none has been tested at runtime:

- Output passes through. With `ENABLE_VIRTUAL_TERMINAL_PROCESSING` and `ENABLE_PROCESSED_OUTPUT` set, conhost forwards the child's bytes verbatim, APC and DCS included, however they are chunked. The parser patch needs no Windows special case.
- Replies need VT input mode. An APC reply written into ConPTY reaches the child only when the child has set `ENABLE_VIRTUAL_TERMINAL_INPUT`. Otherwise the payload is dropped and `ESC \` arrives as an Alt+`\` keypress. DCS and CSI replies (DA1, `14t`) pass back in either mode. WSL most likely sets VT input mode, but that is unverified because WSL's source is not in the checkout.
- No pixel size crosses ConPTY. Its resize signal carries only cell counts, so a WSL program most likely sees `ws_xpixel = 0`. alacritree has to answer `CSI 14 t` and `CSI 16 t` itself. chafa and yazi fall back to those queries. `kitten icat` does not: it needs a non-zero `TIOCGWINSZ` pixel size or `--use-window-size`.
- DA1. ConPTY sends its own DA1 at startup and swallows the first reply, and conhost changes behaviour based on the attributes it sees (attribute 28 switches it to DECCRA). Advertise only attributes alacritree implements.
- Conhost parses sixel too. It moves its own cursor in rows of a virtual 20-pixel cell. That matters only if alacritree adds sixel, and only for Win32 clients that read the console cursor.

How the target clients fare through ConPTY:

- `kitten icat` by default sends an `a=q` probe and gives up on a DA1-only answer or after 10 seconds. With `--transfer-mode=stream`, every transmission uses `q=2` and needs no reply.
- yazi and chafa probe, then fall back on `14t`/`16t`.
- Codex picks a protocol from an environment-variable allowlist (`TERM`, `TERM_PROGRAM`, known terminal names). It shows images only for pets, and refuses under zellij and tmux.
- Claude Code: unknown. Its source is not available here.

## Recommended architecture for alacritree

This is a draft for the design conversation, not a decision:

1. Parser patch. Vendor vte, then add an APC dispatch to `Perform`. Forward it through `ansi::Processor` to a `Handler::apc` method, and implement that on alacritty_terminal's `Term`. Calling the hook synchronously from `advance` gives the cursor for free, the way WezTerm's ordered actions do. Before placing an image, flush any text still buffered. Cap each APC at kitty's 256 KiB.
2. Kitty command layer, in a new crate kept separate from alacritty_terminal. It parses keys and reassembles chunks, keeping one accumulator per session. It validates and produces replies. Replies leave through the existing `PtyWrite` event path.
3. Image store, one per screen buffer. Ids and numbers, a quota with eviction, and `pending` images that a worker decodes (PNG, zlib, raw 24/32) outside the terminal lock and then announces through `EventProxy`. The cursor movement, the anchor and the reply order are all fixed at parse time.
4. Placement anchors. Record each anchor as an absolute line (history size plus screen line) and a column. A hook in vendored alacritty_terminal where the grid scrolls reports a signed delta, the scroll region, and whether lines went to history. Placements follow kitty's rules: margin scrolls clip a placement by rewriting its source rect, IL/DL and erase-in-line leave it alone, and ED2/ED3 clear it. alacritty_terminal has no tracked-pin facility like Ghostty's, so this hook is the equivalent.
5. The `grid_gl` passes:
   - One `GL_TEXTURE_2D` per image, uploaded when its generation changes, with `glTexSubImage2D` for animation frames.
   - One instanced placement pass per band, over one instance buffer of visible placements. Each record holds a destination rect, a source rect and an image, sorted by z and then texture, with one draw per run of the same texture.
   - Clamp to the edge, and inset the source rect by half a texel, since GLES 3.0 has no `CLAMP_TO_BORDER`.
   - For the lowest band, the background pass needs to skip default-background cells. Either discard them in the shader or split the pass.
6. Unicode placeholders. Whenever a damaged row is rebuilt, scan it for U+10EEEE and rebuild that row's placeholder quads from the foreground colour, underline colour and diacritics. Keep a per-row flag so undamaged rows cost nothing. Port kitty's diacritic table, its run-merge rule, and the tests for both.
7. Detection. Answer `a=q`, DA1 (honestly), and `14t`/`16t`/`18t`. Also decide how to advertise support to clients that check environment variables, such as Codex: a `TERM_PROGRAM` value, or a PR to Codex's allowlist.

## Open questions

- Does WSL set `ENABLE_VIRTUAL_TERMINAL_INPUT`, and what `ws_xpixel`/`ws_ypixel` does a WSL program see? Finding out needs a runtime probe in a WSL pane.
- Does `PSEUDOCONSOLE_PASSTHROUGH_MODE` change reply delivery or pixel sizes? Nothing in these checkouts uses it.
- Is sixel in scope? Windows Terminal's users and Codex's pets use sixel, and conhost already answers sixel-capable clients' expectations. The parser patch for DCS would sit beside the APC one.
- How far should the vendored alacritty_terminal be changed? At minimum it needs `Handler::apc` and a scroll hook. The kitty layer and the store can live outside it.
