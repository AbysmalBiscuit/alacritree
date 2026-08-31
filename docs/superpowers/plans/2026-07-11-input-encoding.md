# Input Encoding Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `alacritree/src/input.rs` emit correct terminal byte sequences for Ctrl+punctuation, modifier-held navigation keys, Alt+printables, and Shift+Tab, matching upstream alacritty/xterm.

**Architecture:** All changes stay inside `alacritree/src/input.rs`, a pure `egui::Event -> Option<Vec<u8>>` translator with no state. Three layers: named-key encoding (CSI/SS3/tilde with xterm modifier parameters), character-derived control bytes, and ESC-prefixed Alt+printables. An explicit invariant protects AltGr (reported as Ctrl+Alt on Windows): printable keys with both ctrl and alt produce no bytes, because the composed character arrives via `Event::Text`.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui 0.31 (`egui::Key`, `egui::Modifiers`), `cargo test -p alacritree` (this adds the crate's first tests).

## Global Constraints

- Branch: `fix/input-encoding` off `master`, in its own worktree (created at execution start via superpowers:using-git-worktrees). PR target: mathix420/alacritree.
- Only `alacritree/src/input.rs` may change. No new dependencies.
- Do NOT commit any file under `docs/specs/`, `docs/superpowers/`, or `docs/plans/` (they are git-excluded; never force-add them).
- Commits: Conventional Commits, imperative, ≤50-char subject, lowercase after colon.
- `cargo fmt` before every commit (rustfmt is enforced).
- Comments explain *why*, never narrate the change; no task/PR references in code.
- Existing behavior that must not regress: plain arrows/F-keys/Enter/Tab/Backspace/Escape sequences, `Event::Text` passthrough, `Event::Copy`/`Event::Cut` → `0x03`/`0x18` on non-macOS, Alt+named-simple-key ESC prefix.

---

### Task 1: Character-derived control bytes

Fixes the headline bug: `Ctrl+/` sends nothing (tmux/psmux keybinds dead). Replaces the hand-rolled A–Z `ctrl_byte` table with a derivation from the character the key produces, covering the full xterm set.

**Files:**
- Modify: `alacritree/src/input.rs` (replace `ctrl_byte`, add `key_char`; add `#[cfg(test)] mod tests` at end of file)

**Interfaces:**
- Produces: `fn key_char(key: Key, shift: bool) -> Option<char>` — the character a printable key produces (letters honor `shift` for case; shifted punctuation arrives as distinct egui keys). Task 3 consumes this for Alt+printables.
- Produces: `fn control_byte(key: Key, mods: Modifiers) -> Option<u8>` — replaces `ctrl_byte(key: Key) -> Option<u8>` (deleted).
- `key_to_bytes(key, mods)` keeps its signature; its ctrl branch now calls `control_byte(key, mods)`.

- [ ] **Step 1: Write the failing tests**

Append to `alacritree/src/input.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    use Modifiers as M;

    // egui ships `Modifiers::NONE/CTRL/SHIFT/ALT` constants; combos are
    // built with struct-update syntax inside test bodies.
    fn ctrl_shift() -> Modifiers {
        Modifiers { ctrl: true, shift: true, ..M::NONE }
    }

    #[test]
    fn ctrl_slash_sends_unit_separator() {
        assert_eq!(key_to_bytes(Key::Slash, M::CTRL), Some(vec![0x1f]));
    }

    #[test]
    fn ctrl_letters_send_c0_bytes() {
        assert_eq!(key_to_bytes(Key::A, M::CTRL), Some(vec![0x01]));
        assert_eq!(key_to_bytes(Key::C, M::CTRL), Some(vec![0x03]));
        assert_eq!(key_to_bytes(Key::Z, M::CTRL), Some(vec![0x1a]));
        // Shift+Ctrl+letter sends the same byte as Ctrl+letter.
        assert_eq!(key_to_bytes(Key::C, ctrl_shift()), Some(vec![0x03]));
    }

    #[test]
    fn ctrl_punctuation_matches_xterm() {
        assert_eq!(key_to_bytes(Key::Space, M::CTRL), Some(vec![0x00]));
        assert_eq!(key_to_bytes(Key::Num2, M::CTRL), Some(vec![0x00]));
        assert_eq!(key_to_bytes(Key::OpenBracket, M::CTRL), Some(vec![0x1b]));
        assert_eq!(key_to_bytes(Key::Backslash, M::CTRL), Some(vec![0x1c]));
        assert_eq!(key_to_bytes(Key::CloseBracket, M::CTRL), Some(vec![0x1d]));
        assert_eq!(key_to_bytes(Key::Num6, M::CTRL), Some(vec![0x1e]));
        assert_eq!(key_to_bytes(Key::Minus, M::CTRL), Some(vec![0x1f]));
        assert_eq!(key_to_bytes(Key::Questionmark, M::CTRL), Some(vec![0x7f]));
    }

    #[test]
    fn ctrl_unmapped_key_sends_nothing() {
        assert_eq!(key_to_bytes(Key::Quote, M::CTRL), None);
    }

    #[test]
    fn plain_named_keys_unchanged() {
        assert_eq!(key_to_bytes(Key::ArrowUp, M::NONE), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_to_bytes(Key::Enter, M::NONE), Some(b"\r".to_vec()));
        assert_eq!(key_to_bytes(Key::Tab, M::NONE), Some(b"\t".to_vec()));
        assert_eq!(key_to_bytes(Key::Backspace, M::NONE), Some(b"\x7f".to_vec()));
        assert_eq!(key_to_bytes(Key::F1, M::NONE), Some(b"\x1bOP".to_vec()));
        assert_eq!(key_to_bytes(Key::F5, M::NONE), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn text_event_passes_through() {
        let ev = Event::Text("é".to_string());
        assert_eq!(event_to_bytes(&ev), Some("é".as_bytes().to_vec()));
    }
}
```

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `cargo test -p alacritree`
Expected: compile error — `Modifiers::NONE` is fine, but `ctrl_slash_sends_unit_separator`, `ctrl_punctuation_matches_xterm` FAIL (current `ctrl_byte` has no Slash/Num2/Num6/Minus/Questionmark arms; Ctrl+Slash returns `None`). `plain_named_keys_unchanged` and `text_event_passes_through` PASS (regression guards).

- [ ] **Step 3: Implement `key_char` and `control_byte`, delete `ctrl_byte`**

Replace the entire `ctrl_byte` function with:

```rust
/// Character a printable key produces, as far as byte encoding is concerned.
/// Letters honor `shift` for case; shifted punctuation already arrives as its
/// own logical key in egui (`?`, `{`, `|`, …), so those map one-to-one.
fn key_char(key: Key, shift: bool) -> Option<char> {
    let c = match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        Key::Space => ' ',
        Key::Minus => '-',
        Key::Plus => '+',
        Key::Equals => '=',
        Key::Slash => '/',
        Key::Questionmark => '?',
        Key::Backslash => '\\',
        Key::Pipe => '|',
        Key::OpenBracket => '[',
        Key::CloseBracket => ']',
        Key::OpenCurlyBracket => '{',
        Key::CloseCurlyBracket => '}',
        Key::Semicolon => ';',
        Key::Colon => ':',
        Key::Comma => ',',
        Key::Period => '.',
        Key::Backtick => '`',
        Key::Quote => '\'',
        Key::Exclamationmark => '!',
        _ => return None,
    };
    Some(if shift && c.is_ascii_alphabetic() { c.to_ascii_uppercase() } else { c })
}

/// xterm's legacy Ctrl encoding, derived from the key's character the way
/// alacritty does it, so `Ctrl+/` and friends work without enumerating keys.
fn control_byte(key: Key, mods: Modifiers) -> Option<u8> {
    let c = key_char(key, false)?;
    let byte = match c {
        ' ' | '2' => 0x00,
        'a'..='z' => c as u8 & 0x1f,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '6' => 0x1e,
        '-' | '/' => 0x1f,
        '?' => 0x7f,
        _ => return None,
    };
    let _ = mods;
    Some(byte)
}
```

In `key_to_bytes`, change the ctrl branch to call it:

```rust
    if mods.ctrl && !mods.alt {
        if let Some(b) = control_byte(key, mods) {
            return Some(vec![b]);
        }
    }
```

(`control_byte` takes `mods` because Task 3's restructuring passes modifiers through; until then the parameter is unused — the `let _ = mods;` keeps clippy quiet and disappears in Task 3.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all tests PASS.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/input.rs
git commit -m "fix(input): derive control bytes from key characters"
```

---

### Task 2: Modifier-encoded CSI sequences and Shift+Tab

Arrows/Home/End/Insert/Delete/PgUp/PgDn/F1–F12 held with Shift/Alt/Ctrl emit xterm modified sequences; Shift+Tab emits backtab. Today, modifiers on these keys either do nothing or (Alt) produce a malformed `ESC ESC [ A`; Shift+Tab silently sends plain `\t`.

**Files:**
- Modify: `alacritree/src/input.rs` (replace the byte-table `match` in `key_to_bytes` with `named_key_bytes`; extend `mod tests`)

**Interfaces:**
- Consumes: nothing from Task 1 (independent of `key_char`/`control_byte`).
- Produces: `fn named_key_bytes(key: Key, mods: Modifiers) -> Option<Vec<u8>>` — full encoding for non-printable keys, all modifier combinations. Task 3's restructured `key_to_bytes` calls this first.
- Produces: `fn csi_modifier(mods: Modifiers) -> Option<u8>` — xterm modifier digit `b'2'..=b'8'` (`1 + shift·1 + alt·2 + ctrl·4`), `None` when no modifier held.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`:

```rust
    #[test]
    fn modified_arrows_and_nav_keys_use_csi_modifiers() {
        assert_eq!(key_to_bytes(Key::ArrowRight, M::CTRL), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(key_to_bytes(Key::ArrowLeft, M::ALT), Some(b"\x1b[1;3D".to_vec()));
        assert_eq!(key_to_bytes(Key::ArrowUp, M::SHIFT), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(
            key_to_bytes(Key::Home, Modifiers { ctrl: true, shift: true, ..Modifiers::NONE }),
            Some(b"\x1b[1;6H".to_vec())
        );
        assert_eq!(key_to_bytes(Key::Delete, M::SHIFT), Some(b"\x1b[3;2~".to_vec()));
        assert_eq!(key_to_bytes(Key::PageUp, M::CTRL), Some(b"\x1b[5;5~".to_vec()));
    }

    #[test]
    fn modified_function_keys() {
        // Modified F1-F4 switch from SS3 to CSI form.
        assert_eq!(key_to_bytes(Key::F1, M::SHIFT), Some(b"\x1b[1;2P".to_vec()));
        assert_eq!(key_to_bytes(Key::F5, M::CTRL), Some(b"\x1b[15;5~".to_vec()));
    }

    #[test]
    fn shift_tab_sends_backtab() {
        assert_eq!(key_to_bytes(Key::Tab, M::SHIFT), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn alt_on_simple_named_keys_prefixes_esc() {
        assert_eq!(key_to_bytes(Key::Enter, M::ALT), Some(b"\x1b\r".to_vec()));
        assert_eq!(key_to_bytes(Key::Backspace, M::ALT), Some(b"\x1b\x7f".to_vec()));
    }

    #[test]
    fn ctrl_alt_on_named_keys_is_encoded_not_suppressed() {
        // AltGr suppression applies to printables only; arrows never compose
        // text, so Ctrl+Alt encodes as modifier 7.
        let ctrl_alt = Modifiers { ctrl: true, alt: true, ..Modifiers::NONE };
        assert_eq!(key_to_bytes(Key::ArrowRight, ctrl_alt), Some(b"\x1b[1;7C".to_vec()));
    }
```

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `cargo test -p alacritree`
Expected: the five new tests FAIL (`Ctrl+Right` currently returns plain `\x1b[C`; `Shift+Tab` returns `\t`; `Alt+Left` returns `\x1b\x1b[D`). Task 1 tests still PASS.

- [ ] **Step 3: Implement `csi_modifier` and `named_key_bytes`**

Replace the entire byte-table `match key { ... }` block and the trailing `if mods.alt { ... }` in `key_to_bytes` with a call, so `key_to_bytes` becomes:

```rust
fn key_to_bytes(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
    if mods.ctrl && !mods.alt {
        if let Some(b) = control_byte(key, mods) {
            return Some(vec![b]);
        }
    }

    named_key_bytes(key, mods)
}
```

Add below it:

```rust
/// xterm modifier parameter: `1 + (Shift=1 | Alt=2 | Ctrl=4)`, as an ASCII
/// digit.  `None` when no encodable modifier is held, so callers can emit
/// the shorter unmodified sequence.
fn csi_modifier(mods: Modifiers) -> Option<u8> {
    let m = (mods.shift as u8) | ((mods.alt as u8) << 1) | ((mods.ctrl as u8) << 2);
    (m != 0).then_some(b'1' + m)
}

/// Encoding for keys that never produce composed text.  Because no
/// `Event::Text` follows these, every modifier combination is safe to encode
/// here — including Ctrl+Alt, which on printables must stay silent (AltGr).
fn named_key_bytes(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
    // Arrows/Home/End: `ESC [ <final>`, or `ESC [ 1 ; <m> <final>` modified.
    let csi = |final_byte: u8| match csi_modifier(mods) {
        Some(m) => vec![0x1b, b'[', b'1', b';', m, final_byte],
        None => vec![0x1b, b'[', final_byte],
    };
    // F1-F4 are SS3 (`ESC O <f>`) unmodified but switch to CSI when modified.
    let ss3 = |final_byte: u8| match csi_modifier(mods) {
        Some(m) => vec![0x1b, b'[', b'1', b';', m, final_byte],
        None => vec![0x1b, b'O', final_byte],
    };
    // Editing/function keys: `ESC [ <n> ~`, or `ESC [ <n> ; <m> ~` modified.
    let tilde = |num: &[u8]| {
        let mut v = vec![0x1b, b'['];
        v.extend_from_slice(num);
        if let Some(m) = csi_modifier(mods) {
            v.push(b';');
            v.push(m);
        }
        v.push(b'~');
        v
    };

    let bytes = match key {
        Key::ArrowUp => csi(b'A'),
        Key::ArrowDown => csi(b'B'),
        Key::ArrowRight => csi(b'C'),
        Key::ArrowLeft => csi(b'D'),
        Key::Home => csi(b'H'),
        Key::End => csi(b'F'),
        Key::Insert => tilde(b"2"),
        Key::Delete => tilde(b"3"),
        Key::PageUp => tilde(b"5"),
        Key::PageDown => tilde(b"6"),
        Key::F1 => ss3(b'P'),
        Key::F2 => ss3(b'Q'),
        Key::F3 => ss3(b'R'),
        Key::F4 => ss3(b'S'),
        Key::F5 => tilde(b"15"),
        Key::F6 => tilde(b"17"),
        Key::F7 => tilde(b"18"),
        Key::F8 => tilde(b"19"),
        Key::F9 => tilde(b"20"),
        Key::F10 => tilde(b"21"),
        Key::F11 => tilde(b"23"),
        Key::F12 => tilde(b"24"),
        Key::Tab if mods.shift => vec![0x1b, b'[', b'Z'],
        Key::Enter | Key::Tab | Key::Backspace | Key::Escape => {
            let base: &[u8] = match key {
                Key::Enter => b"\r",
                Key::Tab => b"\t",
                Key::Backspace => b"\x7f",
                Key::Escape => b"\x1b",
                _ => unreachable!(),
            };
            if mods.alt {
                // Long-standing meta convention: Alt+key sends ESC + key.
                let mut out = Vec::with_capacity(base.len() + 1);
                out.push(0x1b);
                out.extend_from_slice(base);
                out
            } else {
                base.to_vec()
            }
        },
        _ => return None,
    };
    Some(bytes)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all tests PASS (including Task 1's).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/input.rs
git commit -m "feat(input): encode modified keys as xterm sequences"
```

---

### Task 3: Alt+printables and the explicit AltGr invariant

`Alt+b`/`Alt+f`/`Alt+.` (readline word motions) send `ESC` + character. The AltGr protection — printable keys with ctrl+alt produce nothing, because winit reports AltGr as Ctrl+Alt and the composed character arrives via `Event::Text` — becomes an explicit, commented, tested rule instead of an accident of the old guard.

**Files:**
- Modify: `alacritree/src/input.rs` (restructure `key_to_bytes`; extend `mod tests`)

**Interfaces:**
- Consumes: `key_char(key, shift)` from Task 1, `named_key_bytes(key, mods)` from Task 2.
- Produces: final `key_to_bytes` shape — named keys first, then the ctrl+alt printable guard, then ctrl, then alt. No signature changes.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`:

```rust
    #[test]
    fn alt_printables_send_esc_prefixed_char() {
        assert_eq!(key_to_bytes(Key::B, M::ALT), Some(b"\x1bb".to_vec()));
        assert_eq!(
            key_to_bytes(Key::B, Modifiers { alt: true, shift: true, ..Modifiers::NONE }),
            Some(b"\x1bB".to_vec())
        );
        assert_eq!(key_to_bytes(Key::Period, M::ALT), Some(b"\x1b.".to_vec()));
        assert_eq!(key_to_bytes(Key::Num1, M::ALT), Some(b"\x1b1".to_vec()));
    }

    #[test]
    fn ctrl_alt_printables_stay_silent_for_altgr() {
        // winit reports AltGr as Ctrl+Alt; the composed character arrives via
        // Event::Text, so emitting bytes here would double the input.
        let ctrl_alt = Modifiers { ctrl: true, alt: true, ..Modifiers::NONE };
        assert_eq!(key_to_bytes(Key::Q, ctrl_alt), None);
        assert_eq!(key_to_bytes(Key::Num2, ctrl_alt), None);
        assert_eq!(key_to_bytes(Key::OpenBracket, ctrl_alt), None);
    }
```

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `cargo test -p alacritree`
Expected: `alt_printables_send_esc_prefixed_char` FAILS (Alt+B currently returns `None`). `ctrl_alt_printables_stay_silent_for_altgr` PASSES already (documents the invariant; keep it as the regression guard).

- [ ] **Step 3: Restructure `key_to_bytes`**

Replace the whole function:

```rust
fn key_to_bytes(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
    // Named keys never produce composed text, so every modifier combination
    // is safe to encode.
    if let Some(bytes) = named_key_bytes(key, mods) {
        return Some(bytes);
    }

    // winit reports AltGr as Ctrl+Alt.  A printable key carrying both must
    // stay silent: the composed character arrives via `Event::Text`, and
    // emitting bytes here too would double the input.
    if mods.ctrl && mods.alt {
        return None;
    }

    if mods.ctrl {
        return control_byte(key, mods).map(|b| vec![b]);
    }

    if mods.alt {
        // Long-standing meta convention: Alt+char sends ESC + char.
        let c = key_char(key, mods.shift)?;
        let mut out = vec![0x1b];
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        return Some(out);
    }

    None
}
```

Also remove the now-dead `let _ = mods;` line in `control_byte` and use the parameter for the shift-aware lookup it was reserved for:

```rust
fn control_byte(key: Key, mods: Modifiers) -> Option<u8> {
    let c = key_char(key, mods.shift)?;
    let byte = match c {
        ' ' | '2' => 0x00,
        'a'..='z' => c as u8 & 0x1f,
        'A'..='Z' => c.to_ascii_lowercase() as u8 & 0x1f,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '6' => 0x1e,
        '-' | '/' => 0x1f,
        '?' => 0x7f,
        _ => return None,
    };
    Some(byte)
}
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p alacritree`
Expected: all tests PASS. Also run `cargo check -p alacritree` — no warnings about unused functions (the old `ctrl_byte` must be gone).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/input.rs
git commit -m "feat(input): send esc-prefixed bytes for alt+printables"
```

---

### Task 4: Build verification and manual test round

No code. Confirms the branch is shippable and hands the binary to the user for the symptom-driven checks from the spec.

**Files:** none modified.

**Interfaces:** none.

- [ ] **Step 1: Full check**

Run: `cargo fmt --check && cargo test -p alacritree && cargo check -p alacritree`
Expected: clean.

- [ ] **Step 2: Release build**

Run: `cargo build -p alacritree --release`
Expected: success; binary at `target/release/alacritree.exe` (inside the worktree).

- [ ] **Step 3: Hand off for manual verification**

Ask the user to verify, per the spec's manual list:
- `Ctrl+/` in psmux and in tmux-under-WSL triggers the bound action
- `Alt+B` / `Alt+F` word motion in WSL bash; `Alt+.` inserts last argument
- `Ctrl+Arrow` word-jump in PSReadLine
- `Shift+Tab` in pwsh `MenuComplete` moves backwards through candidates
- On an EU layout: AltGr characters produce exactly one character
- Regression: plain typing, arrows, F-keys, Enter/Backspace, Ctrl+C

Report results; any failure becomes a fix inside this branch before the PR.
