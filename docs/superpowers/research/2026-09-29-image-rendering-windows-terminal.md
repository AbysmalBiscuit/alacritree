# Image rendering in Windows Terminal

Windows Terminal (microsoft/terminal, including conhost/OpenConsole and ConPTY) at commit `f1ddcbe4df3d0cf77c9de981e7ae766295ce584e`, checkout `C:\Users\Lev\.local\share\devkit\docs\terminal\main`. Every `path:line` below is relative to that checkout.

## Summary

- Windows Terminal implements sixel only, as a VT340-style emulation with a fixed virtual cell of 10x20 pixels and 256 colors. It has no kitty graphics, no Unicode placeholders and no iTerm2 `File=` images.
- The sixel decoder runs inside the shared VT adapter on the parser thread, char by char, and writes pixels straight into a per-row `ImageSlice` owned by the text buffer, so every grid operation (scroll, erase, insert, copy, reflow) moves or erases image pixels like text.
- AtlasEngine copies a row's slice when its revision changes, rasterizes it with Direct2D into the shared glyph atlas scaled to the real cell size, and draws it as one quad per row in the same instanced batch as glyphs, on top of that row's text.
- ConPTY forwards child output verbatim (APC and DCS included) whenever `ENABLE_VIRTUAL_TERMINAL_PROCESSING` and `ENABLE_PROCESSED_OUTPUT` are set, answers no queries itself, and passes APC replies back to the child only when the child enabled `ENABLE_VIRTUAL_TERMINAL_INPUT`. Without it the payload is dropped and `ESC \` becomes an Alt+`\` key.
- Windows Terminal answers DA1 with `?61;4;...c` (sixel) and reports 14t and 16t in virtual sixel pixels (`20;10` per cell). ConPTY's resize signal carries only cell counts, so no pixel size crosses the pseudoconsole.

## 1. Protocols

Sixel (DCS `q`):

- Recognised as DCS final `q` (`src/terminal/parser/OutputStateMachineEngine.hpp:179`) and dispatched to `DefineSixelImage` with P1 (macro/aspect), P2 (background select) and P3 (VT240 background color) (`src/terminal/parser/OutputStateMachineEngine.cpp:715-719`). The parser object is created lazily on the first image (`src/terminal/adapter/adaptDispatch.cpp:3919-3930`).
- Commands handled: sixel data `?`..`~`, `#` color introducer, `!` repeat, `$` graphics CR, `-` graphics NL, `"` raster attributes (level 3 and up), and the undocumented VT240 `+` home (`src/terminal/adapter/SixelParser.cpp:111-186`).
- Conformance defaults to level 9 (`src/terminal/adapter/SixelParser.hpp:29`), which gives a 10x20 virtual cell (`src/terminal/adapter/SixelParser.cpp:20-29`) and 256 colors (`src/terminal/adapter/SixelParser.cpp:31-43`). Colors are HLS or RGB percent (`src/terminal/adapter/SixelParser.cpp:544-552`).
- DECSDM (private mode 80) switches between clamp-at-bottom display mode and scrolling mode (`src/terminal/adapter/adaptDispatch.cpp:1842-1846`, `src/terminal/adapter/SixelParser.cpp:70-81`), and DECRQM reports it (`src/terminal/adapter/adaptDispatch.cpp:1996-1997`).

What sixel leaves out:

- More than 256 colors. The index type is `uint8_t` and the comment names the change needed (`src/terminal/adapter/SixelParser.hpp:40-44`).
- Private color registers and sixel-scrolling toggles from xterm. Not present: `1070|PrivateColorRegisters|8452` in `src/terminal/adapter/DispatchTypes.hpp` has no match; mode 80 is the only sixel mode.
- XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`). Not present: `VTID("?S")` has no match in `src/terminal/parser/OutputStateMachineEngine.hpp`.
- The color table persists across images (initialised once in the constructor, `src/terminal/adapter/SixelParser.cpp:45-57`; `_initColorMap` only resets the number-to-entry mapping, `src/terminal/adapter/SixelParser.cpp:480-527`). At levels 3 and below palette changes also recolor text (`src/terminal/adapter/SixelParser.cpp:638-651`).
- Pixel-exact output. Pixels are stored at 10x20 per cell and rescaled to the font cell at draw time (section 6).

kitty graphics protocol: not present. `rg -i kitty` over `src` and `doc` hits only the kitty keyboard protocol (`src/terminal/input/terminalInput.cpp`, `src/terminal/input/terminalInput.hpp`, settings strings). APC strings are ignored by the output parser (`src/terminal/parser/stateMachine.cpp:1827-1831`).

kitty Unicode placeholders: not present (section 7).

iTerm2 OSC 1337: only `SetMark` is handled; every other action, including `File=`, goes to `UnknownSequence` (`src/terminal/adapter/adaptDispatch.cpp:3657-3682`, dispatch at `src/terminal/parser/OutputStateMachineEngine.cpp:892`).

DECDLD soft fonts also use sixel encoding but are glyph replacement, not images (`src/terminal/adapter/adaptDispatch.cpp:3932-3992`).

## 2. Parser to model

- Recognition. `ESC P` enters DCS; on the final byte the engine returns a `StringHandler` closure and the state machine enters `DcsPassThrough` (`src/terminal/parser/stateMachine.cpp:726-742`). Each payload char is handed to the closure (`src/terminal/parser/stateMachine.cpp:1803-1817`); an ESC calls the handler with `ESC` to end the string (`src/terminal/parser/stateMachine.cpp:651-661`, `src/terminal/parser/stateMachine.cpp:1871-1875`). A closure returning false moves the parser to `DcsIgnore` (`src/terminal/parser/stateMachine.cpp:1808-1810`); `SixelParser` returns false after any exception (`src/terminal/adapter/SixelParser.cpp:92-103`).
- Buffering. The raw payload is never buffered. Chunk boundaries do not matter because the output engine does not cache partial runs in string states (`src/terminal/parser/stateMachine.cpp:2076-2082`) and parsing resumes mid-sequence on the next `ProcessString` (`src/terminal/parser/stateMachine.cpp:1985-1990`). Parameters are capped at 5 per command (`src/terminal/adapter/SixelParser.cpp:188-211`).
- Decoded buffer and size limit. Pixels are decoded into `_imageBuffer`, a vector of 2-byte `IndexedPixel` (transparent flag and color index) sized `height * _imageMaxWidth` (`src/terminal/adapter/SixelParser.hpp:45-49`, `src/terminal/adapter/SixelParser.cpp:675-683`). Width is clamped to the space right of the origin (`src/terminal/adapter/SixelParser.cpp:760`, `_imageMaxWidth = _availablePixelWidth` at `src/terminal/adapter/SixelParser.cpp:659`). Height stops growing once the image passes the bottom in display mode (`src/terminal/adapter/SixelParser.cpp:245-256`); in scrolling mode rows that scrolled above the page top are dropped from the buffer after each flush (`src/terminal/adapter/SixelParser.cpp:960-968`). There is no byte-count cap on the sequence itself.
- Cursor position at parse time. `DefineImage` captures the origin when the DCS header is dispatched: the page's top-left in display mode, the live cursor otherwise, and rejects an origin outside the margins (`src/terminal/adapter/SixelParser.cpp:267-308`, `_imageOriginCell = _textCursor` at `src/terminal/adapter/SixelParser.cpp:656`). The text cursor is hidden while the image streams (`src/terminal/adapter/SixelParser.cpp:300-306`).
- Reaching terminal state. `_maybeFlushImageBuffer` scrolls the text buffer with real line feeds if needed (`src/terminal/adapter/SixelParser.cpp:410-458`, `src/terminal/adapter/SixelParser.cpp:900-907`), converts indexed pixels to BGRA through the palette and writes them into each covered row's `ImageSlice`, skipping transparent pixels so earlier content shows through (`src/terminal/adapter/SixelParser.cpp:911-953`), then calls `TriggerRedraw` on those rows (`src/terminal/adapter/SixelParser.cpp:955-958`). It flushes on end of sequence, when 500 ms have passed, or when a newline ends the current write (`src/terminal/adapter/SixelParser.cpp:886-899`), so an image appears progressively.
- Threads in Windows Terminal. The connection's output callback takes the terminal write lock and calls `Terminal::Write` (`src/cascadia/TerminalControl/ControlCore.cpp:2302-2309`); the same `AdaptDispatch` and `OutputStateMachineEngine` as conhost are built in `src/cascadia/TerminalCore/Terminal.cpp:55-57`. Decoding therefore runs on the connection output thread under the lock. The render thread copies slices under the console lock in `_PaintFrame` (`src/renderer/base/renderer.cpp:402-411`) and the GPU work happens in `Present` on a background thread without locks (`src/renderer/atlas/AtlasEngine.r.cpp:12`, `src/renderer/atlas/AtlasEngine.r.cpp:32-53`).
- Threads in conhost. `WriteCharsVT` runs the state machine on the thread serving the client's write, with the console lock held (`src/host/_stream.cpp:380-394`, lock note at `src/host/_stream.cpp:450-451`).

## 3. Replies and detection

- Reply path in Windows Terminal. `Terminal::ReturnResponse` calls the write-input callback (`src/cascadia/TerminalCore/TerminalApi.cpp:23-29`); `ControlCore` sends pending responses to the connection after the write (`src/cascadia/TerminalControl/ControlCore.cpp:2311-2315`).
- Reply path in conhost under ConPTY. None. `ConhostInternalGetSet::ReturnResponse` returns early in VT I/O mode with the comment "ConPTY should not respond to requests. That's the job of the terminal." (`src/host/outputStream.cpp:50-58`). The query itself passes through to the terminal (section 8).
- DA1. `?61;4;6;7;14;21;22;23;24;28;32;42c`, plus `;52` when clipboard write is enabled; 4 is sixel (`src/terminal/adapter/adaptDispatch.cpp:1435-1462`). DA2 is `>0;10;1c` and DA3 `!|00000000` (`src/terminal/adapter/adaptDispatch.cpp:1471-1484`).
- Pixel sizes. CSI 14t reports the page size times the virtual 10x20 cell, and CSI 16t reports the virtual cell itself, as `4;h;wt` and `6;20;10t` (`src/terminal/adapter/adaptDispatch.cpp:3490-3522`, enum values at `src/terminal/adapter/DispatchTypes.hpp:591-593`). The comment explains why the physical size is never reported: apps divide 14t by 18t to get a cell size, and it must match the sixel emulation (`src/terminal/adapter/adaptDispatch.cpp:3512-3518`). CSI 18t reports characters (`src/terminal/adapter/adaptDispatch.cpp:3509-3510`).
- Environment. The ConPTY connection sets `WT_SESSION` and `WT_PROFILE_ID` and appends them to `WSLENV` (`src/cascadia/TerminalConnection/ConptyConnection.cpp:61-64`, `src/cascadia/TerminalConnection/ConptyConnection.cpp:87-95`). `TERM` and `TERM_PROGRAM` are not set: `TERM_PROGRAM|L"TERM"` has no match in `src/cascadia/TerminalConnection/ConptyConnection.cpp`.
- Graphics queries. XTSMGRAPHICS and kitty `a=q` are not present (section 1). DECRQM on mode 80 is the only sixel-specific query.

## 4. Image store

- Decoding. Hand-written sixel decoder, no image library (`src/terminal/adapter/SixelParser.cpp`). It runs synchronously in the parser thread per character, with branch-free hot loops (`src/terminal/adapter/SixelParser.cpp:784-837`). Palette lookup to BGRA happens at flush time (`src/terminal/adapter/SixelParser.cpp:933-937`, `src/terminal/adapter/SixelParser.cpp:628-636`).
- Ids. There are no image ids. Each `ImageSlice` carries a global `uint64_t` revision drawn from a static atomic counter, never 0 so the renderer can use 0 as "empty" (`src/buffer/out/ImageSlice.cpp:67-81`). Every mutable access bumps it: `ROW::GetMutableImageSlice` calls `BumpRevision` (`src/buffer/out/Row.cpp:984-993`).
- Where pixels live. On the CPU, one `ImageSlice` per `ROW`, holding a BGRA buffer exactly one virtual cell tall (20 px) and covering the used column range (`src/buffer/out/ImageSlice.hpp:21-57`, `src/buffer/out/Row.hpp:320`). The range grows on demand and existing pixels are re-laid out (`src/buffer/out/ImageSlice.cpp:114-154`). At the default level that is 10x20x4 = 800 bytes per covered cell, and pixels in scrollback rows are kept at that cost.
- Quota and eviction. There is no quota. A slice is freed when its row is reset (`src/buffer/out/Row.cpp:226-233`), when all its cells are erased (`src/buffer/out/ImageSlice.cpp:287-300`, `src/buffer/out/ImageSlice.cpp:302-310`), or when the line rendition changes (`src/buffer/out/textBuffer.cpp:914-925`). Scrollback eviction happens implicitly when the circular row buffer reuses a row.
- Renderer copies. AtlasEngine keeps a second CPU copy per visible row in `ShapedRow::bitmap` and refreshes it only when the revision differs (`src/renderer/atlas/common.h:451-464`, `src/renderer/atlas/AtlasEngine.cpp:566-591`). Rows whose slice was not painted this frame drop the copy in `EndPaint` (`src/renderer/atlas/AtlasEngine.cpp:270-276`).
- GPU. The slice is rasterized into the glyph atlas on first draw and looked up by revision (`src/renderer/atlas/BackendD3D.cpp:1895-1933`). An atlas reset clears all glyph and bitmap entries (`src/renderer/atlas/BackendD3D.cpp:819`), after which slices are re-rasterized from the CPU copy. The atlas is capped at twice the swap chain area, which the comment says "fits a screen full of glyphs and sixels" (`src/renderer/atlas/BackendD3D.cpp:766-773`).
- Conhost under ConPTY runs its own `SixelParser` and stores its own slices, because it parses every output string before forwarding it (`src/host/_stream.cpp:394`). Nothing in `src/host/VtIo.cpp` serialises those slices back out (`ImageSlice` has no match there), so the copy only keeps conhost's buffer and cursor model consistent.

## 5. Placement model

Pixels belong to text rows, so an image is not an object with an anchor. Each grid operation moves or erases the pixels of the cells it touches.

- Scrolling and scrollback. Full-width scrolls rotate row storage and the slices ride along (`src/terminal/adapter/adaptDispatch.cpp:560-564`), so images enter scrollback with their rows. Partial-width scrolls copy text cell by cell and then copy image cells with `ImageSlice::CopyBlock` (`src/terminal/adapter/adaptDispatch.cpp:566-585`, `src/buffer/out/ImageSlice.cpp:156-179`). Revealed rows are erased with `_FillRect` (`src/terminal/adapter/adaptDispatch.cpp:588-593`).
- Insert and delete lines go through the same vertical scroll. ICH, DCH and horizontal scrolls copy image cells too (`src/terminal/adapter/adaptDispatch.cpp:607-636`). Insert-mode text shifts image cells right and erases the inserted range (`src/buffer/out/textBuffer.cpp:548-580`). DECCRA copies image content between pages (`src/terminal/adapter/adaptDispatch.cpp:1175-1187`).
- Erase and clear. `TextBuffer::FillRect` erases image cells (`src/buffer/out/textBuffer.cpp:627-634`), which covers ED, EL, ECH and DECERA. DECSERA erases image cells of unprotected cells (`src/terminal/adapter/adaptDispatch.cpp:830-845`). Console APIs erase too (`src/host/_output.cpp:253`, `src/host/output.cpp:427`, `src/host/directio.cpp:560`).
- Text written over an image erases the image cells under it: `TextBuffer::Replace` calls `ImageSlice::EraseCells` for the written range (`src/buffer/out/textBuffer.cpp:539-546`). Text and image never share a cell once text is written after the image. Text already in a cell when the image lands stays underneath.
- Resize and reflow. A row's slice is copied only onto the first new row that starts at old column 0 (`src/buffer/out/textBuffer.cpp:2866-2870`); continuation rows produced by rewrapping carry no image. The slice keeps its full pixel width. Clipping of a slice wider than the new viewport happens only at the render target edge (unverified: no explicit clamp is visible in `src/renderer/atlas/BackendD3D.cpp:1935-1944`).
- Alternate screen. Windows Terminal creates a fresh `TextBuffer` for the alt screen and drops it on exit (`src/cascadia/TerminalCore/TerminalApi.cpp:254`, `src/cascadia/TerminalCore/TerminalApi.cpp:299`), so alt-screen images vanish on exit and main-screen images survive untouched.
- Double-width and double-height lines. Setting a line rendition deletes the row's slice (`src/buffer/out/textBuffer.cpp:914-925`); copies between rows of different rendition act as erase (`src/buffer/out/ImageSlice.cpp:187-197`).
- Layering. The image is painted after the row's text (`src/renderer/base/renderer.cpp:1150-1155`) and after its gridlines (`src/renderer/atlas/BackendD3D.cpp:1208-1216`), so it sits above text and underlines. Transparent sixel pixels are never written (`src/terminal/adapter/SixelParser.cpp:934-938`) and erased pixels are zeroed to alpha 0 (`src/buffer/out/ImageSlice.cpp:317-323`), so text shows through those areas.
- Cursor movement. In scrolling mode (DECSDM reset) the cursor ends on the row intersected by the top of the last sixel band, at the column where the image started (`src/terminal/adapter/SixelParser.cpp:460-478`). In display mode the cursor does not move and the image starts at the page's top-left (`src/terminal/adapter/SixelParser.cpp:271-279`). Scrolling needed to fit the image is done with real line feeds, which may pan the viewport (`src/terminal/adapter/SixelParser.cpp:410-432`).

## 6. Render pipeline

- Graphics API. AtlasEngine uses Direct3D 11 with a Direct2D render target for rasterizing into the atlas (`src/renderer/atlas/BackendD3D.cpp:1906-1926`). The GDI engine has its own `PaintImageSlice` that scales with GDI blits (`src/renderer/gdi/paint.cpp:669-709`).
- Texture strategy. No per-image texture. Each visible row's slice becomes one rectangle in the shared glyph atlas, allocated with stb_rect_pack (`src/renderer/atlas/BackendD3D.cpp:1901-1905`, `src/renderer/atlas/BackendD3D.cpp:1706-1721`) and keyed by revision in `_glyphAtlasBitmaps` (`src/renderer/atlas/BackendD3D.h:183-212`, `src/renderer/atlas/BackendD3D.h:299`). The rectangle is `cellSize.x * targetWidth` by `cellSize.y` real pixels, and the 10x20-per-cell source is resampled into it with `D2D1_BITMAP_INTERPOLATION_MODE_LINEAR` (`src/renderer/atlas/BackendD3D.cpp:1926`). Image resolution therefore follows the font size.
- Draw. One `QuadInstance` per row slice with shading type `TextPassthrough` (`src/renderer/atlas/BackendD3D.cpp:1935-1944`); the pixel shader samples the atlas and uses its alpha as blend weight (`src/renderer/atlas/shader_ps.hlsl:152-157`). It goes into the same instance buffer and draw as glyphs.
- Draw order. Background, cursor background, then per row text, gridlines and bitmap, then the batch flush, which also draws the cursor foreground (`src/renderer/atlas/BackendD3D.cpp:232-236`, `src/renderer/atlas/BackendD3D.cpp:991-1001`). Cursor inversion skips `TextPassthrough` quads (`src/renderer/atlas/BackendD3D.cpp:2160-2163`), so an opaque image hides a cursor under it. `AtlasEngine::PaintSelection` is a no-op (`src/renderer/atlas/AtlasEngine.cpp:600-603`), so selection is expressed through cell colors and an opaque image covers the highlight (unverified: inferred from the no-op and the draw order, not traced through selection coloring).
- Clipping. None beyond the viewport row range and the render target. Horizontal offset accounts for the viewport's left column and horizontal scroll (`src/renderer/atlas/AtlasEngine.cpp:593-595`, `src/renderer/atlas/BackendD3D.cpp:1935`).
- Per-frame cost and dirty tracking. Only rows intersecting the dirty region are repainted (`src/renderer/base/renderer.cpp:1100-1108`). A row whose slice revision is unchanged costs one atlas lookup and one quad. A changed revision costs a CPU memcpy of the slice (`src/renderer/atlas/AtlasEngine.cpp:569-591`) and a D2D bitmap creation and draw into the atlas. Because any mutable access bumps the revision (`src/buffer/out/Row.cpp:991`), erasing even one cell of a row re-rasterizes the whole row slice.

## 7. Unicode placeholders

Not present. `10EEEE` (case-insensitive) has no match under `src`, and the kitty graphics protocol that placeholders reference is absent (section 1).

## 8. Platform and multiplexer paths

### ConPTY answer 1: child output, APC and DCS

ConPTY forwards the child's output string verbatim to the hosting terminal after parsing it itself. In `WriteCharsVT`:

```cpp
stateMachine.ProcessString(str);

if (writer)
{
    ...
    const auto write = [&](size_t beg, size_t end) {
        const auto chunk = til::safe_slice_abs(str, beg, end);
        if (disableNewlineTranslation)
        {
            writer.WriteUTF16(chunk);
        }
        else
        {
            writer.WriteUTF16TranslateCRLF(chunk);
        }
    };
    ...
    write(offset, std::wstring_view::npos);
    writer.Submit();
}
```

(`src/host/_stream.cpp:394-433`). The only edits are injected mode re-enables after RIS and similar sequences (`src/host/_stream.cpp:414-430`) and optional LF to CRLF translation.

- Mode dependence. The VT path runs only when both `ENABLE_VIRTUAL_TERMINAL_PROCESSING` and `ENABLE_PROCESSED_OUTPUT` are set: `if (WI_IsAnyFlagClear(screenInfo.OutputMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT)) { WriteCharsLegacy(...) }` (`src/host/_stream.cpp:460-467`). ConPTY turns VT processing on by default (`src/host/screenInfo.cpp:30-37`). In the legacy path control characters, ESC included, are mapped to OEM glyphs before being written (`src/host/_stream.cpp:337-361`), or replaced with spaces when processed output is off (`src/host/_stream.cpp:233-238`, `src/host/VtIo.cpp:650-670`), so no escape sequence survives.
- `DISABLE_NEWLINE_AUTO_RETURN` clear means `WriteUTF16TranslateCRLF`, which inserts a CR before every lone LF anywhere in the chunk, inside string payloads too (`src/host/VtIo.cpp:611-648`). Base64 APC payloads contain no LF, so kitty payloads are unaffected; a sixel payload with LFs gains CRs, which sixel ignores.
- Chunked APC. Each write is forwarded as it arrives and `Submit` flushes unless corked (`src/host/VtIo.cpp:458-471`). Conhost's own parser keeps its state across writes (`src/host/_stream.cpp:383`, `src/terminal/parser/stateMachine.cpp:1985-1990`) and ignores APC content (`src/terminal/parser/stateMachine.cpp:1827-1831`), so a split APC reaches the terminal as the same byte stream. Only the active screen buffer produces output (`src/host/consoleInformation.cpp:134-141`).
- Consequence for cursor state (unverified inference). Conhost moves its own cursor for sixel using its 10x20 virtual cell (section 5) and does not move it for APC. Conhost writes its cursor back to the terminal as absolute CUP in `SetConsoleCursorPosition` (`src/host/getset.cpp:744-750`), `WriteConsoleOutput` (`src/host/VtIo.cpp:771-778`) and screen buffer switches (`src/host/getset.cpp:488-492`, `src/host/VtIo.cpp:868`). A Win32 console client that mixes those APIs with images will see conhost's idea of the cursor, not the terminal's. A pure VT client such as a WSL program never triggers those paths.

### ConPTY answer 2: APC written into ConPTY's input

Both parser engines enter `SosPmApcString` on `ESC _` (`src/terminal/parser/stateMachine.cpp:1143-1146`) and ignore its characters (`src/terminal/parser/stateMachine.cpp:1827-1831`). The terminating `ESC \` is dispatched as an escape sequence (`src/terminal/parser/stateMachine.cpp:442-448`), and the input engine decides:

```cpp
bool InputStateMachineEngine::ActionEscDispatch(const VTID id)
{
    if (_expectingStringTerminator && id == VTID("\\"))
    {
        _expectingStringTerminator = false;
        return false;
    }

    if (_pDispatch->IsVtInputEnabled())
    {
        return false;
    }
    ...
            modifierState = WI_SetFlag(modifierState, LEFT_ALT_PRESSED);
            _WriteSingleKey(wch, vk, modifierState);
```

(`src/terminal/parser/InputStateMachineEngine.cpp:323-361`). A false return makes `_SafeExecute` call `FlushToTerminal` (`src/terminal/parser/stateMachine.cpp:2170-2181`), which hands the cached partial sequence plus the current run to `ActionPassThroughString` (`src/terminal/parser/stateMachine.cpp:1939-1966`), which writes it raw into the input buffer (`src/terminal/parser/InputStateMachineEngine.cpp:306-313`, `src/terminal/adapter/InteractDispatch.cpp:100-104`, `src/host/inputBuffer.cpp:569-579`).

- With `ENABLE_VIRTUAL_TERMINAL_INPUT` set on the input buffer (`src/host/outputStream.cpp:434-437`, `src/host/inputBuffer.cpp:794-797`), the whole `ESC _ G ... ESC \` reaches the child verbatim. The run spans the whole sequence because `_EnterEscape` does not reset it (`src/terminal/parser/stateMachine.cpp:775-781`), and a reply split across reads survives because the input engine caches unfinished runs (`src/terminal/parser/stateMachine.cpp:2084-2095`). A chunk ending exactly on `ESC _` is caught by the Alt-key heuristic (`src/terminal/parser/stateMachine.cpp:2056-2074`), but under VT input that dispatch also returns false and flushes the two chars raw, so the byte stream is unchanged. The comment at `src/terminal/parser/stateMachine.cpp:2045-2048` says WSL input arrives in 16-byte chunks. That WSL sets `ENABLE_VIRTUAL_TERMINAL_INPUT` is unverified here, since WSL's source is not in this checkout.
- Without VT input (a Win32 client reading `INPUT_RECORD`s), the APC payload is dropped and `ESC \` becomes an Alt+`\` key event.
- DCS replies differ. The input engine arms `_expectingStringTerminator` for any DCS (`src/terminal/parser/InputStateMachineEngine.cpp:545-552`), so a DCS reply is flushed raw in both modes. CSI replies such as DA1 and 14t fall to `default: return false` (`src/terminal/parser/InputStateMachineEngine.cpp:531-533`) and are also passed raw in both modes.

### ConPTY answer 3: DA1

Conhost does not answer DA1 under ConPTY; `ReturnResponse` returns early:

```cpp
    // ConPTY should not respond to requests. That's the job of the terminal.
    if (gci.IsInVtIoMode())
    {
        return;
    }
```

(`src/host/outputStream.cpp:54-58`). The child's `CSI c` is forwarded to the terminal with the rest of the output (answer 1), so the child sees whatever the hosting terminal replies. With Windows Terminal hosting, that is `?61;4;6;7;14;21;22;23;24;28;32;42c` and it advertises sixel (`src/terminal/adapter/adaptDispatch.cpp:1454-1461`). Standalone conhost (not ConPTY) would give the same answer from the same code.

- ConPTY sends its own DA1 at startup along with focus and win32-input-mode requests (`src/host/VtIo.cpp:200-204`). The input engine swallows the first DA1 reply it sees and records the attributes (`src/terminal/parser/InputStateMachineEngine.cpp:492-520`); later replies pass through. The terminal must answer DA1, or a client started with `PSEUDOCONSOLE_INHERIT_CURSOR` waits up to 1 second (`src/host/VtIo.cpp:209-217`, flag at `src/host/VtIo.cpp:26`).
- The recorded attributes change conhost's behavior. When the terminal advertises 28, conhost translates `ScrollConsoleScreenBuffer` into DECCRA and DECFRA (`src/host/getset.cpp:1005-1059`). The attributes are stored only on the inherit-cursor path (`src/host/VtIo.cpp:216`). A terminal that advertises 28 must implement DECCRA.

### ConPTY answer 4: CSI 14t, 16t, 18t and TIOCGWINSZ

Conhost parses these through the same `AdaptDispatch::WindowManipulation` but its `ReturnResponse` is a no-op under ConPTY (quoted in answer 3), and the query bytes are forwarded to the terminal (answer 1). The terminal answers, and its CSI reply passes back to the child raw (answer 2). Hosted by Windows Terminal, the answers are virtual:

```cpp
    case DispatchTypes::WindowManipulationType::ReportTextSizeInPixels:
        ...
        reportSize(_pages.VisiblePage().Size() * SixelParser::CellSizeForLevel());
        break;
    case DispatchTypes::WindowManipulationType::ReportCharacterCellSize:
        reportSize(SixelParser::CellSizeForLevel());
```

(`src/terminal/adapter/adaptDispatch.cpp:3512-3522`).

TIOCGWINSZ: the ConPTY resize signal carries only character counts:

```cpp
        struct ResizeWindowData
        {
            unsigned short sx;
            unsigned short sy;
        };
```

(`src/host/PtySignalInputThread.hpp:47-51`), fed by `ConptyResizePseudoConsole(HPCON, COORD)` (`src/winconpty/winconpty.cpp:288-296`, `src/winconpty/winconpty.cpp:509-515`). No pixel size crosses the pseudoconsole. What `ws_xpixel`/`ws_ypixel` a WSL program sees is unverified: WSL's relay is not in this checkout, and with only cells available it has no terminal pixel size to report, so 0 is the likely value.

### tmux and zellij

Not present. `tmux|zellij` has no match in `src/terminal`, `src/host` or `src/renderer`. An unknown DCS such as tmux's `ESC Ptmux;` goes to `UnknownSequence` (`src/terminal/parser/OutputStateMachineEngine.cpp:745-747`) and its content is ignored.

## 9. Tests and specs

- `src/terminal/adapter/ut_adapter/adapterTest.cpp:1746` `DeviceAttributesTests` checks both DA1 strings (`:1753`, `:1759`).
- `src/terminal/adapter/ut_adapter/adapterTest.cpp:3869` `WindowManipulationTypeTests` checks 18t, 14t and 16t against the virtual cell (`:3878-3888`).
- `src/terminal/parser/ut_parser/OutputEngineTest.cpp:989` `TestSosPmApcString` and the ESC and C1 ST cases (`:178-179`, `:1073-1097`) pin APC ignoring in the output parser.
- No sixel image or `ImageSlice` tests: `TEST_METHOD\(.*(sixel|image)` has no match under `src`, and `ImageSlice|Sixel` has no match in `src/buffer/out/ut_textbuffer`. The sixel code in `src/tools/RenderingTests/main.cpp:303` builds a DECDLD soft font, not an image.
- No design doc for images: `sixel|kitty graphic|inline image|graphics protocol` has no match under `doc`.

## 10. Lessons

- Report virtual pixels, not physical ones. Apps derive the cell size from 14t divided by 18t, so 14t must agree with the sixel emulation's cell (`src/terminal/adapter/adaptDispatch.cpp:3512-3518`). The fixed 10x20 cell also keeps image geometry independent of font and DPI.
- Throttle partial flushes. The parser renders a partial image only every 500 ms or when a write ends on a newline, so video-rate sixel does not repaint per band (`src/terminal/adapter/SixelParser.cpp:886-899`). A palette change on the last character of a write also flushes, to support palette animation (`src/terminal/adapter/SixelParser.cpp:570-576`).
- Hardware-matched quirks are documented where they diverge from the spec: raster attributes trigger a carriage return (`src/terminal/adapter/SixelParser.cpp:404-407`), aspect ratio rounds up (`src/terminal/adapter/SixelParser.cpp:378-380`), background size is in device pixels (`src/terminal/adapter/SixelParser.cpp:388-390`), and omitted raster parameters are 0 (`src/terminal/adapter/SixelParser.cpp:371-373`).
- Stop decoding what cannot be seen. In display mode, extra rows past the bottom are discarded to save memory (`src/terminal/adapter/SixelParser.cpp:245-249`); in scrolling mode rows above the page top are dropped after each flush (`src/terminal/adapter/SixelParser.cpp:960-968`).
- Revision 0 is reserved as the renderer's "no bitmap" sentinel (`src/buffer/out/ImageSlice.cpp:76-80`), and the renderer notes that two rows can hold the same revision only when scrolling with a full invalidation (`src/renderer/atlas/AtlasEngine.cpp:566-568`).
- Width rounding. Slice widths can round past the new buffer edge, so the relayout copy is clamped (`src/buffer/out/ImageSlice.cpp:134-137`).
- Atlas budget. Image slices share the glyph atlas budget of twice the swap chain area, chosen because memory pressure feedback was hard to integrate (`src/renderer/atlas/BackendD3D.cpp:766-770`). A screen of image rows can evict glyphs and force a full atlas rebuild.
- Hot-loop notes: CMOV-friendly unrolled writes because sixel values are random (`src/terminal/adapter/SixelParser.cpp:789-791`), and avoiding `rep stosw` for short repeats (`src/terminal/adapter/SixelParser.cpp:816-818`).
- TODOs near image code: none. `TODO|GH#` in `src/terminal/adapter/SixelParser.cpp`, `src/buffer/out/ImageSlice.cpp` and `src/renderer/atlas/BackendD3D.cpp` has no match that mentions bitmaps, images or sixel.

## Takeaways for alacritree

1. Draw images inside the existing instanced pass, one quad per row slice sampling an atlas keyed by a revision. Windows Terminal adds images to its glyph batch with a passthrough shading type and no extra draw call (section 6). alacritree cannot put them in egui's font atlas, so the fit is a separate image texture (atlas or array) and one extra instanced draw after glyphs, reusing the 12-byte record shape. Do not share a budget with glyphs: Windows Terminal's shared atlas lets images evict glyphs (section 10).
2. Tie image dirty state to rows. A per-row revision that bumps on any image mutation lets the renderer skip unchanged rows and re-upload only damaged ones (sections 4 and 6), which matches alacritree's damaged-row upload. The cost to avoid is Windows Terminal's full-row re-rasterize on any one-cell erase (section 6).
3. For sixel under ConPTY, mirror Windows Terminal's virtual 10x20 cell for row counts and for 14t and 16t replies. Conhost parses the same sixel stream and moves its own cursor by 20-pixel rows (sections 4 and 8), and clients size images from 14t/18t (section 10). A terminal that computes rows from its real cell height disagrees with conhost about where the cursor ends, which surfaces for Win32 clients that use console cursor APIs (section 8, answer 1).
4. Do not copy the pixels-in-rows store for kitty graphics. It makes grid operations correct for free but duplicates pixels per row at 800 bytes per cell with no quota (section 4), and it has sixel semantics: text overwrite erases the image and reflow drops continuation rows (section 5). kitty images need a shared store with id, quota and placements, and a row-anchored placement index that alacritty_terminal's grid ops update.
5. The parser patch must deliver APC with the cursor position captured at dispatch, as `SixelParser` does when the DCS header arrives (section 2). ConPTY forwards APC output unchanged and in any chunking (section 8, answer 1), so the patch needs no Windows special case on output. Replies reach the child only under VT input mode (section 8, answer 2), which covers WSL clients but not Win32 console clients, so prefer quiet transmissions and do not rely on `a=q` probes being answered through ConPTY for native Windows programs.
6. Answer pixel queries in the terminal itself. ConPTY answers neither DA1 nor CSI 14t/16t and cannot carry pixel sizes in its resize signal (section 8, answers 3 and 4), so alacritree's own replies are the only source a WSL program has. Advertise only DA1 attributes alacritree implements, because conhost changes behavior on them (28 switches it to DECCRA, section 8, answer 3).
