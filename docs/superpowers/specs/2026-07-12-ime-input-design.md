# fix/ime-input — design

Branch: `fix/ime-input` off `fix/input-encoding` (stacked; that PR merges
first). Upstream PR target: mathix420/alacritree.
Local-only spec (git-excluded); the PR description carries the context.

## Problem

Composed input (CJK IMEs, and any dead-key/compose flow routed through the
OS input method) is completely dead:

- egui-winit only calls winit's `set_ime_allowed(true)` when the app sets
  `PlatformOutput.ime` during a frame (`egui-winit/src/lib.rs:868`).
  alacritree never sets it, so the OS IME is hard-disabled and composition
  never even starts.
- `egui::Event::Ime` events are ignored everywhere, so even if the IME were
  enabled, `Commit` text would never reach the PTY.
- No preedit is rendered and no IME cursor area is reported, so the
  candidate window would appear at a default position instead of the caret.

Reference behavior is upstream alacritty (`alacritty/src/event.rs:2018`
`WindowEvent::Ime` handling, `alacritty/src/display/mod.rs::draw_ime_preview`,
`alacritty/src/input/keyboard.rs:24` preedit guard); this fork mirrors
alacritty per its conventions.

## egui constraints (divergences forced by the platform layer)

- egui-winit drops winit's preedit cursor offset
  (`Ime::Preedit(text, Some(_cursor))` → `ImeEvent::Preedit(text)`), so
  upstream's `cursor_byte_offset` / mid-preedit beam-vs-hollow-block cursor
  cannot be represented. The preedit caret is always rendered at the end.
- `Ime::Preedit(_, None)` and `Ime::Commit` are both followed by
  `ImeEvent::Disabled` from egui-winit's debouncing; `Enabled` is
  synthesized before the first `Preedit`.
- `set_ime_allowed` is output-driven (frame-by-frame), not a persistent
  window property: we express "IME allowed" by setting `PlatformOutput.ime`
  every frame the terminal view has focus.

## Design

### 1. IME state — new `ime.rs`

```rust
pub struct Ime { preedit: Option<String> }
```

Mirrors upstream `display/mod.rs::Ime` minus what egui makes unreachable:
no `enabled` flag (enablement is output-driven, and preedit presence gates
drawing), no cursor offsets (dropped by egui-winit). Owned by
`AlacritreeApp` — composition is a property of the window/IME conversation,
not of a PTY, and upstream likewise keeps it per window — and passed
`&mut` into `terminal_view::show`.

Cleared on: `Commit`, `Disabled`, empty `Preedit`, terminal focus loss,
and active-session/workspace switch. The focus-loss clear matters because
the `Disabled` event arrives after the view stops draining input, so
without it a painted preedit would stick (egui's `TextEdit` has the same
guard on `lost_focus`).

### 2. Event handling — `terminal_view.rs`

Add `Event::Ime(ime_event)` to the consumed events:

- `Commit(text)`: `paste::paste(session, &text, text.chars().count() > 1)`
  — upstream's "don't use bracketed paste for single char input"
  (`event.rs:2021`). Clear preedit. Ignore commits of exactly `"\n"` or
  `"\r"` (mirrors egui `TextEdit`'s guard for the platform quirk where
  confirming a composition emits a bare newline commit).
- `Preedit(text)`: set the preedit; empty text clears it. Nothing is
  written to the PTY.
- `Enabled` / `Disabled`: clear the preedit.

Key suppression while composing: while `preedit.is_some()`, drop
`Key`/`Text`/`Copy`/`Cut`/`Paste` events from the byte-translation path;
only `Ime` events are processed. This mirrors alacritty's early return for
all key input during preedit (`keyboard.rs:24`) — candidate-window
navigation keys (Space/Enter/arrows/Backspace/Escape) must not leak bytes
to the PTY. The same guard applies in `app.rs` to the app-level shortcuts
(Ctrl+B/G/Tab/T/Q/Shift+O), since upstream's early return sits above
binding dispatch. Because suppression engages only while a preedit exists,
IMEs sitting in direct/Latin mode pass keys through normally.

`paste::on_terminal_input_start` (selection clear + scroll snap) fires on
commit, not on preedit updates.

### 3. Preedit rendering — `terminal_view.rs`

Mirrors `draw_ime_preview`, adapted to the per-cell painter:

- While a preedit is active, hide the normal terminal cursor (upstream
  `content.rs:55`).
- Draw the preedit on the cursor's line in the default palette foreground
  on background, an underline across the preedit width, and a beam cursor
  at the end cell.
- Width-aware layout: advance 2 columns for double-width chars
  (`unicode-width`, already in-tree via `alacritty_terminal`) — CJK preedit
  text is predominantly wide. Necessary divergence from upstream's
  `draw_string`, which our painter has no equivalent of.
- Overflow keeps upstream's rule — the end of the preedit stays visible:
  `end = min(cursor_col + width, cols)`, `start = end - width`, truncating
  whole chars from the left when wider than the grid. Plain truncation;
  upstream's `…` `StrShortener` is cosmetic and skipped.
- If the cursor line is scrolled out of the viewport, skip drawing.
- Layout math lives in a pure function
  (`preedit_layout(text, cursor_col, cols) -> (start_col, visible)`) so it
  is unit-testable.

### 4. Candidate-window positioning + enablement — `terminal_view.rs`

Every frame the terminal view has focus:

```rust
ui.ctx().output_mut(|o| o.ime = Some(IMEOutput { rect: caret, cursor_rect: caret }));
```

`caret` = the preedit-end cell rect while composing, else the terminal
cursor cell rect. egui-winit drives `set_ime_cursor_area` from `ime.rect`
(not `cursor_rect`), so the candidate window lands at the caret — matching
alacritty's `update_ime_position`. (`TextEdit` passes its whole widget rect
there; for a fullscreen terminal that would pin the popup to the window
corner, so this deliberately diverges from `TextEdit` to stay faithful to
alacritty.) When unfocused, `o.ime` stays `None` and egui-winit's debounced
`set_ime_allowed(false)` disables the IME — upstream's
`ImeInhibitor::FOCUS` for free.

Once at startup, send `ViewportCommand::IMEPurpose(IMEPurpose::Terminal)`
(mirrors upstream `set_ime_purpose(ImePurpose::Terminal)`,
`window.rs:194`).

### 5. What doesn't change

`input.rs` is untouched (no conflict surface beyond sharing the branch
base). `paste.rs`, `session.rs`, `bindings.rs`, both config files: no
changes. No new config options.

## Testing

Unit (TDD, in-crate):

- `preedit_layout`: truncation, right-alignment of overflow, wide-char
  width accounting, zero-column/degenerate grids.
- Event→state reducer: commit newline guard, empty preedit clears,
  `Enabled`/`Disabled` transitions, commit clears preedit.
- Suppression predicate: `Key`/`Text`/`Copy`/`Cut`/`Paste` dropped while a
  preedit is active, `Ime` always processed, everything passes when idle.
- Commit bracketing decision: single char verbatim, multi-char bracketed.

Manual (Windows, user-run, release build, CJK IME):

- Candidate window appears at the caret, not the window corner.
- Preedit shows underlined at the cursor while typing; terminal cursor
  hidden during composition.
- Commit inserts the text exactly once — no doubled input, no stray
  Space/Enter bytes from candidate navigation.
- Escape cancels composition cleanly (preedit disappears, nothing sent).
- Latin typing and app shortcuts unaffected when not composing.
- Clicking the sidebar mid-composition does not leave a stuck preedit.
