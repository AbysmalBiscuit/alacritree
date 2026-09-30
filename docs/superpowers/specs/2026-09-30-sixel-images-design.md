# Sixel images

Issue: alacritree/alacritree#386. Parent #381. Stacked on #390 (kitty graphics, #382), whose vendored vte, image store and placement pass this reuses.

## Goal

Draw `DCS P1;P2;P3 q ... ST` images, and let programs detect that alacritree can: DA1 reports attribute 4 and `XTSMGRAPHICS` answers.

## Parser

The vendored vte collects a sixel string the way it collects APC. `hook` with final `q`, no intermediates and no ignored params starts collecting; `put` appends; `unhook` hands `Handler::sixel(params: [u16; 3], data: &[u8])` the whole string. The params are P1, P2 and P3, 0 where absent. A string longer than 64 MiB is dropped whole. CAN and SUB end the string like ST, since a VT340 keeps what it drew before them.

vte hands printable text to `Handler::input` a character at a time and holds none back, so nothing is pending when the DCS starts, and the cursor the handler reads at `unhook` is the one the DCS started at.

`CSI ? Pi ; Pa ; Pv S` reaches `Handler::graphics_attribute(item, action, values)`.

## Decoder

A new `sixel` module in `alacritree_graphics`, ported rather than taken from a crate. The placement needs the image size under the terminal lock and the pixels off it, so one tokenizer drives a cheap measuring pass and a decoding pass, which a crate's single decode call does not give. The module follows Windows Terminal's `SixelParser.cpp` for the colour maths and WezTerm's `sixel.rs` for the simpler parts, and each divergence below says why.

- **Two passes.** `measure` runs under the terminal lock and returns the image size in pixels without allocating. `decode` runs on the job pool and fills straight RGBA. Both walk the same tokenizer.
- **Size.** Raster attributes (`"Pan;Pad;Ph;Pv`) that come before the first sixel fix the size to Ph×Pv, and pixels past it are clipped, as in WezTerm. Without them the size is the painted extent: the widest column reached, by 6 pixel rows per band up to the last band holding data. Each side is capped at 10000, kitty's `MAX_DIMENSION`, which the store already uses.
- **Aspect ratio.** Raster attributes give each sixel `ceil(Pan / Pad)` pixel rows, clamped to 1..=10, as Windows Terminal does. P1's aspect ratio is ignored and an image without raster attributes is 1:1. A VT340 would draw P1 = 0 at 2:1, but producers that want square pixels send `"1;1` and the ones that send nothing expect square pixels too, as in xterm, foot and WezTerm.
- **Colour registers.** 256 registers, private to each image, starting from the VT340's 16 colours and xterm's 256-colour cube and grey ramp above them, the table Windows Terminal uses. Private registers are xterm's default (mode 1070 set), and they keep each decode independent of the one before it, which matters when decodes run on a pool. `#Pc` selects register `Pc % 256`. `#Pc;Pu;Px;Py;Pz` defines it, with RGB (`Pu = 2`) as percentages rounded to 0..=255 and HLS (`Pu = 1`) with blue at 0°, red at 120° and green at 240°. A colour is resolved when it is drawn, so a register redefined later does not recolour pixels already drawn, as in WezTerm and xterm.
- **Commands.** `!Pn` repeats the next sixel Pn times, 0 or absent meaning 1, clamped to the width left. `$` returns to column 0. `-` returns to column 0 and moves down one band. Unknown bytes are skipped.
- **Background.** P2 = 1 leaves unpainted pixels transparent. Any other P2 fills the image with register 0 as it stands before the data, black by default.

The pixels land in the store's `Slot`, as a PNG decode does, and the pool wakes the pane when they are ready.

## Placement

A sixel image becomes an unnamed image (no id, no number) with one placement at native pixel size and `z = -1`. The store drops it once its placement is gone, as it does for unnamed kitty images. Placing it deletes every older sixel placement whose cells lie inside its own.

- **Scrolling mode (DECSDM reset).** The placement goes at the cursor. The cursor then moves down by the rows the image covers, through linefeeds so the scroll region scrolls, which leaves it on the line below the image in the column it started in. Windows Terminal leaves it on the last row the image touches instead. The issue asks for the line below, which is also where lsix and img2sixel expect to print next.
- **Display mode (DECSDM set, mode 80).** The placement goes at the top left of the screen, the image is clipped to the screen's height, and the cursor does not move. DECRQM reports the mode.

## Detection

- DA1 answers `CSI ? 62 ; 4 c`. A VT102's `?6c` has no room for attributes, so the class moves to VT220, as foot's does.
- `XTSMGRAPHICS` replies `CSI ? Pi ; Ps ; Pv S`:
  - `Pi = 1`, colour registers: `?1;0;256S` for read, reset, set and read-maximum, since the count is fixed.
  - `Pi = 2`, sixel geometry: read, reset and set report the text area in pixels, each side capped at 10000; read-maximum reports `10000;10000`.
  - `Pi = 3` (ReGIS) and other items: `?Pi;1S`. An unknown action: `?Pi;2S`.

## Decisions

### Text over a sixel image

In xterm, foot and Windows Terminal, text printed over a sixel image erases the cells it covers. Kitty placements stay under or over text instead.

1. **Erase the covered cells.** Every grid write that can land on an image (print, ECH, EL, ED 0/1, ICH, DCH, IL, DL) tells the store which cells it touched, the store keeps an erased-cell mask per sixel placement, and the frame emits one quad per run of unerased cells. That puts a check on `Term::input`, the hottest path in the terminal, guarded by a flag that is set only while a sixel image is on screen. ICH and DCH shift image content in Windows Terminal, and matching that needs pixel shifting too.
2. **A kitty placement with negative z.** A sixel image draws in the band between cell backgrounds and text, so text printed over it shows on top of the image and spaces leave it visible. Only ED, scrolling off and RIS remove it. This needs nothing beyond the placement. The issue words this as `z = 0`, but in #390's renderer `z = 0` draws over the text and would hide it, so the option that shows text is `z = -1`.

Programs that print an image and carry on below it (lsix, img2sixel, gnuplot, matplotlib, chafa) look the same either way. Programs that erase an image by printing spaces over it (yazi's and ranger's sixel backends) leave it behind under option 2. Both of those pick kitty graphics when a terminal answers the kitty query, which alacritree does.

Decided: option 2, at `z = -1`. It costs nothing on the input path, and the programs whose output differs have a better protocol here. One rule comes with it: a new sixel image deletes every older sixel placement whose cells it covers entirely, so a program redrawing an image in place does not stack copies. Per-cell erasing waits until a program in real use shows stale images.

### ConPTY's virtual cell

Conhost parses the same sixel stream with a fixed 10×20 pixel cell and moves its own cursor by that cell, and reports 10×20 through `16t`. Alacritree lays sixel out in its real cell, so after an image the two cursors disagree whenever the real cell is not 10×20.

1. **Real cell.** Images draw at their pixel size and the cursor moves by real rows. The conhost cursor drifts from alacritree's after a sixel image, which a native Win32 program reading the console cursor sees. WSL programs never read it.
2. **Mirror the virtual cell.** Sixel images would be laid out in 10×20 units and stretched to the real cell. For programs to size their images to fit, `14t` and `16t` would have to report the virtual cell too, and kitty clients need those in real pixels, so kitty images would come out the wrong size. Mirroring only for native Windows panes would split the reports by pane type.

Decided: option 1.

## Out of scope

ReGIS, DECDLD soft fonts, iTerm2 `OSC 1337 File=` images, shared colour registers (mode 1070 reset) and any change to how kitty placements treat text over them.

## Tests

- Decoder unit tests: colour registers (RGB, HLS, select-only, the default palette), repeat, raster attributes (size, clipping, aspect), background select (transparent and filled), and painted extent without raster attributes. Each compares RGBA.
- Through `Term` with the patched vte, in `crates/alacritree_graphics/tests/sixel.rs`: the image lands at the DCS's cursor, the cursor ends on the line below in scrolling mode and scrolls the screen at the bottom, and stays put in display mode with the image at the top left.
- DA1 carries attribute 4, and `XTSMGRAPHICS` answers each item and action above.
- Manual: `img2sixel` in a WSL pane, screenshot.
