# fix/input-encoding — design

Branch: `fix/input-encoding` off `master`. Upstream PR target: mathix420/alacritree.
Local-only spec (git-excluded); the PR description carries the context.

## Problem

`input.rs` hand-rolls a partial subset of terminal key encoding, so several
key combinations send nothing to the PTY:

- `Ctrl+/` (psmux/tmux keybind) — `ctrl_byte()` maps only A–Z, `[ \ ]`,
  and Space; xterm sends `0x1F` for `Ctrl+/`.
- `Ctrl/Alt/Shift + arrows, Home, End, Delete, PgUp/PgDn` — no
  modifier-encoded CSI sequences, so PSReadLine/readline word motions and
  selections are dead.
- `Alt+<letter>` — letters fall through `key_to_bytes` and return `None`,
  so readline `Alt+B`/`Alt+F`/`Alt+.` do nothing.
- `Shift+Tab` — no `ESC [ Z` (backtab). Some flows appear to work because
  applications tolerate plain Tab; emit the correct sequence.

Reference behavior is upstream alacritty's built-in bindings and its
character-derived control encoding (`alacritty/src/input/keyboard.rs` and
`config/bindings.rs`); this fork mirrors alacritty per its conventions.

## Design

All changes stay inside `alacritree/src/input.rs` (pure
`egui::Event -> bytes` translation; no caller changes).

### 1. Character-derived control bytes

Replace the `ctrl_byte` match table with a derivation from the key's
character, mirroring xterm/alacritty:

- `Ctrl+Space`, `Ctrl+@` → `0x00`
- `Ctrl+A`..`Ctrl+Z` → `0x01`..`0x1A` (char uppercased `& 0x1F`)
- `Ctrl+[` → `0x1B`, `Ctrl+\` → `0x1C`, `Ctrl+]` → `0x1D`
- `Ctrl+^` / `Ctrl+6` → `0x1E`
- `Ctrl+_` / `Ctrl+-` / `Ctrl+/` → `0x1F`
- `Ctrl+?` → `0x7F`

egui gives a `Key`, not a character, so a `key_char(Key) -> Option<char>`
helper maps the printable keys; the control byte is computed from that
char. Unknown keys keep returning `None`.

### 2. Modifier-encoded CSI

For arrows/Home/End/Insert/Delete/PgUp/PgDn/F1–F12 with any of
Shift/Alt/Ctrl held, emit the xterm modified form:

- modifier parameter = `1 + (shift·1 + alt·2 + ctrl·4)`
- arrows/Home/End/F1–F4: `ESC [ 1 ; <m> <final>` (modified F1–F4 switch
  from SS3 `ESC O P..S` to CSI `ESC [ 1 ; <m> P..S`)
- tilde keys (Ins/Del/PgUp/PgDn/F5–F12): `ESC [ <num> ; <m> ~`

Unmodified sequences are unchanged.

### 3. Alt+printable

`Alt+<key>` where the key has a derivable character sends `ESC` +
character (lowercase; `Shift+Alt+letter` sends the uppercase form). This
is the long-standing meta convention, and matches what the code already
does for named keys.

### 4. Shift+Tab

`Shift+Tab` → `ESC [ Z`.

### AltGr invariant (Windows)

winit reports AltGr as Ctrl+Alt. Key events carrying *both* ctrl and alt
must keep producing no bytes for printable keys — the composed character
arrives via `Event::Text` and handling the key event too would double the
input. The current `ctrl && !alt` guard preserves this accidentally;
after this change it is an explicit, commented, tested invariant:
printable keys with `ctrl && alt` → `None`.

(`Ctrl+Alt` on named keys — arrows etc. — is safe to encode via CSI
modifiers, since those never produce composed text.)

## Error handling

None to speak of: the translator stays a total function returning
`Option<Vec<u8>>`; unknown combinations return `None` and egui's default
handling proceeds.

## Testing

`input.rs` is pure, so unit tests cover the whole matrix in-crate (the
alacritree crate currently has zero tests; this starts the suite):

- every control byte in §1, including `Ctrl+/` → `0x1F`
- modified CSI for a representative set (Ctrl+Right, Alt+Left,
  Shift+Delete, Ctrl+Shift+Home, Ctrl+F5)
- `Alt+b` → `ESC b`, `Shift+Alt+b` → `ESC B`
- `Shift+Tab` → `ESC [ Z`
- AltGr guard: `ctrl+alt` + printable → `None`
- regressions: plain arrows/F-keys unchanged, `Event::Text` passthrough

Manual verification on this machine: `Ctrl+/` in psmux and tmux-in-WSL,
`Alt+B`/`Alt+F` word motion in WSL bash, `Ctrl+Arrow` in PSReadLine,
`Shift+Tab` in pwsh MenuComplete, AltGr chars on an EU layout produce
exactly one character.
