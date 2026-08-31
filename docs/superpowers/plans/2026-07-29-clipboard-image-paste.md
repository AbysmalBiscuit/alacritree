# Clipboard Image Paste Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Paste` fall back to the clipboard's non-text formats — copied file paths, or a bitmap written out as a PNG — so a `Win+Shift+S` screenshot reaches a program in the terminal as a path.

**Architecture:** Three probes in priority order (text → `CF_HDROP` file list → bitmap), resolved lazily by a pure function so it is unit-testable. Whatever wins becomes either text (today's path) or a `Vec<PathBuf>` that goes through the *existing* drop payload machinery — `file_drop::shell_payload` for a terminal, literal cursor insertion for the scratchpad. Only the bitmap branch writes anything: a content-addressed PNG under a directory that a count cap keeps bounded.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), `arboard` for the clipboard, the `png` crate for encoding, `egui`/`eframe` for the app shell, `tempfile` for tests.

## Global Constraints

- Design spec: `docs/superpowers/specs/2026-07-29-clipboard-image-paste-design.md`. Read it before starting.
- Work on a branch in a worktree, per `AGENTS.local.md`. Never commit to `master`.
- `docs/superpowers/` is git-ignored — the spec and this plan must never appear in a commit.
- All work lives in `alacritree/`. The `alacritty*` crates are vendored and read-only.
- Comments explain *why*, never *what*. No comment may reference this plan, a task number, a PR, or a TDD phase. See the "Code Comments" rules in `CLAUDE.md`.
- Conventional Commits, imperative subject, ≤72 characters.
- `cargo fmt` is enforced. Run it before every commit.
- Default behavior must be preserved for anyone who sets the new options to `false`.
- New config lives under `[ui.paste]` in `alacritree.toml`, documented in `docs/alacritree.md`.
- Verification commands: `cargo test -p alacritree`, `cargo clippy -p alacritree --all-targets`, `cargo fmt --check`.

---

## File Structure

**Created:**
- `alacritree/src/digest.rs` — the shared FNV-1a helper, moved out of `scratchpad.rs` because a second caller now needs it.
- `alacritree/src/clipboard_image.rs` — encoding a clipboard bitmap to PNG, naming it by content, storing it, and keeping the directory bounded. Pure and filesystem-only; knows nothing about the clipboard or about sessions.

**Modified:**
- `alacritree/Cargo.toml:50` — turn `arboard`'s `image-data` feature back on.
- `alacritree/src/main.rs:3-50` — two new `mod` declarations.
- `alacritree/src/scratchpad.rs:232-242` — drop the local `stable_digest`, import the shared one.
- `alacritree/src/config.rs` — `PathSpelling`, `PasteConfig`, `RawUiPaste`, and their wiring.
- `alacritree/src/file_drop.rs` — `shell_payload` narrows to `&PathSpelling`; the new `paste_payload` routes a pasted path list to its sink.
- `alacritree/src/clipboard.rs` — typed probes that distinguish *format absent* from *read failed*, plus the lazy `resolve` that picks a format.
- `alacritree/src/app.rs:2186-2209` — the two paste arms collapse into one helper.
- `docs/alacritree.md:340` — document `[ui.paste]`.

**Task order:** 1 → 2 → 3 → 4 → 5 → 6 → 7. Each task compiles, tests, and commits on its own. Task 7 consumes all of them.

---

### Task 1: Share the digest helper

`scratchpad.rs` has a private FNV-1a used for file naming. Task 5 needs the same function for content-addressing PNGs. Move it rather than copy it.

**Files:**
- Create: `alacritree/src/digest.rs`
- Modify: `alacritree/src/main.rs` (add `mod digest;`)
- Modify: `alacritree/src/scratchpad.rs:232-242` (delete the local copy, import instead)
- Test: `alacritree/src/digest.rs` (inline `mod tests`, matching this codebase's convention)

**Interfaces:**
- Consumes: nothing.
- Produces: `crate::digest::stable_digest(bytes: &[u8]) -> u64`.

- [ ] **Step 1: Write the failing test**

Create `alacritree/src/digest.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The digest names files on disk, so a change to it silently orphans
    /// every existing scratchpad. Pin the constant.
    #[test]
    fn the_digest_is_stable_across_builds() {
        assert_eq!(stable_digest(b""), 0xcbf29ce484222325);
        assert_eq!(stable_digest(b"a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn different_inputs_give_different_digests() {
        assert_ne!(stable_digest(b"one"), stable_digest(b"two"));
    }
}
```

Add `mod digest;` to `alacritree/src/main.rs`. The list is alphabetical, so it goes after `mod config;` (line 12) and before `mod doppler;` (line 13).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alacritree digest::`
Expected: FAIL to compile, `cannot find function stable_digest in this scope`.

- [ ] **Step 3: Write the implementation**

At the top of `alacritree/src/digest.rs`, above the test module:

```rust
//! A stable content digest, shared by anything that names a file after what is
//! in it.

/// FNV-1a is small, deterministic across Rust versions, and sufficient here:
/// the digest disambiguates file names rather than protecting an adversarial
/// namespace.
pub fn stable_digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alacritree digest::`
Expected: PASS, 2 tests.

- [ ] **Step 5: Point the scratchpad at the shared copy**

In `alacritree/src/scratchpad.rs`, delete the whole `stable_digest` function including its doc comment (lines 232-242, ending at the closing brace after `hash`). Add to the imports near the top, after `use crate::app::WorkspaceKey;`:

```rust
use crate::digest::stable_digest;
```

- [ ] **Step 6: Verify the scratchpad still passes**

Run: `cargo test -p alacritree scratchpad::`
Expected: PASS. The existing scratchpad file-naming tests exercise the moved function; if any fails, the move changed behavior and must be corrected rather than the test.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/digest.rs alacritree/src/main.rs alacritree/src/scratchpad.rs
git commit -m "refactor: extract stable_digest into its own module"
```

---

### Task 2: The `[ui.paste]` config table

**Files:**
- Modify: `alacritree/src/config.rs` (add `PasteConfig`, `RawUiPaste`, wire both)
- Test: `alacritree/src/config.rs` (inline `mod tests`, beside the existing `drop_options_*` tests)

**Interfaces:**
- Consumes: `parse_config_path(raw: &str, key: &str) -> Option<PathBuf>` (`config.rs:1395`), the existing tilde expander.
- Produces:
  - `config::PasteConfig { files: bool, image: bool, image_dir: Option<PathBuf>, image_keep: usize }`
  - `PasteConfig::image_target(&self) -> (PathBuf, bool)` — the directory to write into, and whether alacritree owns it.
  - `config::default_image_dir() -> PathBuf`
  - `UiTheme::paste: PasteConfig`

- [ ] **Step 1: Write the failing tests**

Add to `alacritree/src/config.rs`'s `mod tests`, directly after `drop_options_parse_from_the_ui_drop_table`:

```rust
#[test]
fn paste_options_default_to_on_with_the_owned_image_dir() {
    let ui = ui_from_toml("");
    assert_eq!(
        ui.paste,
        PasteConfig { files: true, image: true, image_dir: None, image_keep: 20 }
    );
    let (dir, owned) = ui.paste.image_target();
    assert_eq!(dir, default_image_dir());
    assert!(owned, "the default directory is alacritree's own");
}

#[test]
fn paste_options_parse_from_the_ui_paste_table() {
    let home = home::home_dir().expect("a home directory");
    let ui = ui_from_toml(
        "[ui.paste]\n\
         files = false\n\
         image = false\n\
         image_dir = \"~/shots\"\n\
         image_keep = 5\n",
    );
    assert_eq!(
        ui.paste,
        PasteConfig {
            files: false,
            image: false,
            image_dir: Some(home.join("shots")),
            image_keep: 5,
        }
    );
}

/// A directory the user chose may hold files alacritree never wrote, so it is
/// never swept — that is what makes pointing this at a pictures folder safe.
#[test]
fn a_configured_image_dir_is_not_owned() {
    let ui = ui_from_toml("[ui.paste]\nimage_dir = \"~/shots\"");
    let (dir, owned) = ui.paste.image_target();
    assert_eq!(dir, home::home_dir().expect("a home directory").join("shots"));
    assert!(!owned);
}

/// A relative path is rejected by `parse_config_path`, which must leave the
/// owned default in place rather than writing somewhere arbitrary.
#[test]
fn an_unusable_image_dir_falls_back_to_the_owned_default() {
    let ui = ui_from_toml("[ui.paste]\nimage_dir = \"relative/path\"");
    assert_eq!(ui.paste.image_dir, None);
    assert!(ui.paste.image_target().1);
}

/// The cap can never reach zero: a paste hands the shell a path, and the shell
/// opens it after the sweep has already run.
#[test]
fn an_image_keep_of_zero_is_raised_to_one() {
    assert_eq!(ui_from_toml("[ui.paste]\nimage_keep = 0").paste.image_keep, 1);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree config::tests::paste`
Expected: FAIL to compile, `cannot find type PasteConfig`.

- [ ] **Step 3: Add the config type**

In `alacritree/src/config.rs`, directly after the `DropConfig` `impl Default` block (ends line 359):

```rust
/// `[ui.paste]`: what Paste does when the clipboard holds no text.  Both
/// fallbacks are independent — one can be off without affecting the other, and
/// both off leaves Paste exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteConfig {
    /// Paste the paths of files and folders copied in a file manager.
    pub files: bool,
    /// Write a clipboard bitmap to a PNG and paste its path.
    pub image: bool,
    /// Where those PNGs go.  `None` is the app-owned default, the only
    /// directory the count cap is ever applied to.
    pub image_dir: Option<PathBuf>,
    /// How many generated PNGs the owned directory keeps.  At least one: the
    /// file a paste just handed to the shell has to still be there when the
    /// shell opens it, so zero is not a reachable state.
    pub image_keep: usize,
}

impl Default for PasteConfig {
    fn default() -> Self {
        Self { files: true, image: true, image_dir: None, image_keep: 20 }
    }
}

impl PasteConfig {
    /// The directory to write into, and whether alacritree owns it.  Ownership
    /// is what licenses deleting anything: a directory the user named may hold
    /// files alacritree never wrote.
    pub fn image_target(&self) -> (PathBuf, bool) {
        match &self.image_dir {
            Some(dir) => (dir.clone(), false),
            None => (default_image_dir(), true),
        }
    }
}

/// Disposable by nature, and reachable from a WSL session through the usual
/// automount once `shell_payload` translates it.
pub fn default_image_dir() -> PathBuf {
    std::env::temp_dir().join("alacritree").join("clipboard")
}
```

- [ ] **Step 4: Add the raw table and wire it**

In `alacritree/src/config.rs`, after the `RawUiDrop` struct (ends line 1276):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawUiPaste {
    files: Option<bool>,
    image: Option<bool>,
    image_dir: Option<String>,
    image_keep: Option<usize>,
}
```

In `struct RawUi`, immediately after the `drop: RawUiDrop,` field (line 1318):

```rust
    paste: RawUiPaste,
```

In `UiTheme`, immediately after the `pub drop: DropConfig,` field (line 621):

```rust
    /// `[ui.paste]`: what Paste does with a clipboard that holds no text.
    pub paste: PasteConfig,
```

In `impl Default for UiTheme`, after `drop: DropConfig::default(),` (line 646):

```rust
            paste: PasteConfig::default(),
```

In `into_config`, immediately after the `drop: DropConfig { … }` block closes (line 1498):

```rust
            paste: PasteConfig {
                files: self.ui.paste.files.unwrap_or(true),
                image: self.ui.paste.image.unwrap_or(true),
                image_dir: self
                    .ui
                    .paste
                    .image_dir
                    .as_deref()
                    .and_then(|raw| parse_config_path(raw, "ui.paste.image_dir")),
                image_keep: self.ui.paste.image_keep.unwrap_or(20).max(1),
            },
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree config::tests::paste && cargo test -p alacritree config::tests::a_configured && cargo test -p alacritree config::tests::an_unusable && cargo test -p alacritree config::tests::an_image_keep`
Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): add the [ui.paste] table"
```

---

### Task 3: Separate path spelling from drop enablement

`shell_payload` takes a whole `&DropConfig` today, so paste would receive
`enabled`, `terminal`, `sidebar` and `scratchpad` — flags about whether *drops*
are accepted, which a paste must never consult. Nothing reads them yet, which
is exactly why this is worth fixing before a second caller arrives: a future
`shell_payload` that starts honoring `DropConfig::enabled` would silently
switch paste off with it.

This is a refactor. No behavior changes, and no existing assertion changes —
the verification is that the whole suite still passes with its expected values
untouched. Only one genuinely new test exists, and it is the point of the task:
spelling a path without a `DropConfig` in sight.

**Files:**
- Modify: `alacritree/src/config.rs:329-359` (extract `PathSpelling`), `:1490-1498` (`into_config`), `:2370-2431` (the drop tests)
- Modify: `alacritree/src/file_drop.rs:92` and `:110` (signatures), `:336-475` and `:568` (test call sites)
- Modify: `alacritree/src/app.rs:1395` (the drop caller)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `config::PathSpelling { quote: Quoting, wsl_translate: bool }`, `Copy` + `Default`
  - `DropConfig::spelling: PathSpelling` replacing the `quote` and `wsl_translate` fields
  - `file_drop::shell_payload(paths: &[PathBuf], distro: Option<&str>, spelling: &PathSpelling) -> String`

- [ ] **Step 1: Write the failing test**

Add to `alacritree/src/file_drop.rs`'s `mod tests`, after
`a_shell_payload_quotes_a_path_containing_spaces`:

```rust
    /// A paste spells paths without ever holding a drop config, which is the
    /// whole reason the spelling is its own type.
    #[test]
    fn a_path_is_spelled_without_a_drop_config() {
        let spelling = PathSpelling { quote: Quoting::Posix, wsl_translate: false };
        let paths = [PathBuf::from("/a/my pic.png")];
        assert_eq!(shell_payload(&paths, None, &spelling), "'/a/my pic.png' ");
    }
```

Add `PathSpelling` to that module's config import (`config.rs`'s items are
imported at `file_drop.rs:247`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alacritree file_drop::tests::a_path_is_spelled`
Expected: FAIL to compile, `cannot find struct PathSpelling`.

- [ ] **Step 3: Extract the type**

In `alacritree/src/config.rs`, directly above `DropConfig` (line 329):

```rust
/// How a path is written for the shell that receives it.  Separate from
/// `DropConfig` because a paste spells paths too, and must not be handed flags
/// about whether drops are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathSpelling {
    pub quote: Quoting,
    /// Rewrite a Windows path to its distro-side spelling before it reaches a
    /// WSL shell, where a `C:\` path resolves to nothing.
    pub wsl_translate: bool,
}

impl Default for PathSpelling {
    fn default() -> Self {
        Self { quote: Quoting::Auto, wsl_translate: true }
    }
}
```

In `DropConfig`, replace the `quote` and `wsl_translate` fields — and the
doc comment that sat on `wsl_translate`, which moved with it — with:

```rust
    pub spelling: PathSpelling,
```

In `impl Default for DropConfig`, replace the `quote:` and `wsl_translate:`
lines with:

```rust
            spelling: PathSpelling::default(),
```

In `into_config` (line 1490), replace the `quote:` and `wsl_translate:` lines
inside the `DropConfig { … }` literal with:

```rust
                spelling: PathSpelling {
                    quote: parse_quoting(self.ui.drop.quote.as_deref()),
                    wsl_translate: self.ui.drop.wsl_translate.unwrap_or(true),
                },
```

`RawUiDrop` and the `[ui.drop]` TOML keys are unchanged — this is an internal
regrouping, not a config change.

- [ ] **Step 4: Update the config tests**

In `alacritree/src/config.rs`'s `mod tests`, in
`drop_options_default_to_on_with_auto_quoting` (line 2371) replace the `quote:`
and `wsl_translate:` lines with:

```rust
                spelling: PathSpelling { quote: Quoting::Auto, wsl_translate: true },
```

In `drop_options_parse_from_the_ui_drop_table` (line 2387), likewise:

```rust
                spelling: PathSpelling { quote: Quoting::Posix, wsl_translate: false },
```

In `every_quoting_name_parses` (line 2424) and
`an_unknown_quoting_name_falls_back_to_auto` (line 2431), `ui.drop.quote`
becomes `ui.drop.spelling.quote`.

- [ ] **Step 5: Narrow the two `file_drop` signatures**

In `alacritree/src/file_drop.rs`, change line 92 and line 110 to take the
spelling, and the two `cfg.` reads inside `shell_word` to match:

```rust
pub fn shell_payload(paths: &[PathBuf], distro: Option<&str>, spelling: &PathSpelling) -> String {
```

```rust
fn shell_word(path: &Path, distro: Option<&str>, spelling: &PathSpelling) -> (String, ShellQuoting) {
    if let Some(distro) = distro.filter(|_| spelling.wsl_translate) {
        if let Some(linux) = distro_path(path, distro) {
            return (linux, ShellQuoting::Posix);
        }
        log::debug!("no path in {distro} for {}, pasting it as-is", path.display());
    }
    (path.to_string_lossy().into_owned(), spelling.quote.resolve(distro.is_some()))
}
```

Update the module's import at line 11 to `use crate::config::{DropConfig, PathSpelling, ShellQuoting};` — `DropConfig` stays, because `Regions::new` and `route` still take one.

In `alacritree/src/app.rs:1395`, the drop caller becomes:

```rust
                    file_drop::shell_payload(&paths, session.wsl_distro(), &self.config.ui.drop.spelling);
```

- [ ] **Step 6: Update the `shell_payload` test call sites**

Only the tests that call `shell_payload` change; the `route` and `Regions`
tests keep their `DropConfig`. Two mechanical forms, at lines 340, 348, 356,
384, 398, 407, 417, 425, 438, 449, 458, 469 and 568:

```rust
// DropConfig { quote: Q, wsl_translate: W, ..DropConfig::default() }
PathSpelling { quote: Q, wsl_translate: W }

// a form naming only one of the two, or DropConfig::default()
// — fill the other from PathSpelling's default (Quoting::Auto, true)
PathSpelling { quote: Q, wsl_translate: true }
PathSpelling::default()
```

The seam test at line 568 (`let cfg = DropConfig { quote, wsl_translate: false, … }`)
becomes `let cfg = PathSpelling { quote, wsl_translate: false };`.

Every assertion's expected value stays exactly as it is. An expected value that
needs changing means a field was filled with the wrong default — fix the
construction, not the assertion.

- [ ] **Step 7: Run the whole suite**

Run: `cargo test -p alacritree && cargo clippy -p alacritree --all-targets`
Expected: PASS with no warnings, and every pre-existing `file_drop` and
`config` assertion unchanged.

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/file_drop.rs alacritree/src/app.rs
git commit -m "refactor(config): split path spelling out of DropConfig"
```

---

### Task 4: Encode a clipboard bitmap as PNG

**Files:**
- Create: `alacritree/src/clipboard_image.rs`
- Modify: `alacritree/Cargo.toml:50` (enable `image-data`)
- Modify: `alacritree/src/main.rs` (add `mod clipboard_image;`)
- Test: `alacritree/src/clipboard_image.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `crate::digest::stable_digest` from Task 1.
- Produces:
  - `clipboard_image::encode_png(image: &arboard::ImageData<'_>) -> Result<Vec<u8>, EncodeError>`
  - `clipboard_image::file_name(png: &[u8]) -> String`
  - `clipboard_image::EncodeError` (implements `Display`)

- [ ] **Step 1: Enable the arboard feature**

In `alacritree/Cargo.toml`, replace line 50:

```toml
arboard = { version = "3", default-features = false, features = ["wayland-data-control", "image-data"] }
```

Run `cargo check -p alacritree` to pull the new dependencies before writing code. Expected: compiles, `image v0.25.x` appears in the build output.

Add `mod clipboard_image;` to `alacritree/src/main.rs` immediately after `mod clipboard;` (line 7).

- [ ] **Step 2: Write the failing tests**

Create `alacritree/src/clipboard_image.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn image(width: usize, height: usize) -> ImageData<'static> {
        let bytes = (0..width * height * 4).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        ImageData { width, height, bytes: Cow::Owned(bytes) }
    }

    #[test]
    fn an_image_survives_the_encode_round_trip() {
        let source = image(7, 5);
        let png = encode_png(&source).expect("encodes");

        let decoder = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = decoder.read_info().expect("valid png");
        let mut out = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).expect("one frame");

        assert_eq!((info.width, info.height), (7, 5));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&out[..info.buffer_size()], source.bytes.as_ref());
    }

    /// A clipboard owner can advertise any dimensions it likes.  Reject before
    /// allocating, because this runs on the UI thread during a keystroke.
    #[test]
    fn an_absurdly_large_image_is_rejected_before_allocating() {
        let huge = ImageData { width: usize::MAX, height: 4, bytes: Cow::Owned(Vec::new()) };
        assert!(matches!(encode_png(&huge), Err(EncodeError::TooLarge { .. })));
    }

    #[test]
    fn a_byte_count_disagreeing_with_the_dimensions_is_rejected() {
        let lying = ImageData { width: 4, height: 4, bytes: Cow::Owned(vec![0; 8]) };
        assert!(matches!(encode_png(&lying), Err(EncodeError::Inconsistent { .. })));
    }

    /// The name is the deduplication key: equal bytes must land on one file.
    #[test]
    fn the_file_name_is_a_function_of_the_content() {
        assert_eq!(file_name(b"same"), file_name(b"same"));
        assert_ne!(file_name(b"one"), file_name(b"two"));
    }

    #[test]
    fn the_file_name_is_sixteen_hex_digits_and_inert() {
        let name = file_name(b"payload");
        let hex = name.strip_prefix("clipboard-").and_then(|r| r.strip_suffix(".png")).expect("shape");
        assert_eq!(hex.len(), 16);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(crate::file_drop::is_terminal_safe(&name));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p alacritree clipboard_image::`
Expected: FAIL to compile, `cannot find function encode_png`.

- [ ] **Step 4: Write the implementation**

At the top of `alacritree/src/clipboard_image.rs`, above the test module:

```rust
//! Turning a clipboard bitmap into a file on disk that something else can open.
//!
//! Nothing here knows about the clipboard or about sessions: it takes pixels,
//! and it returns a path.  That is what keeps it testable without a window.

use std::fmt;

use arboard::ImageData;

/// A clipboard owner can advertise any dimensions it likes, and encoding runs
/// on the UI thread during a keystroke.  64 MP is far past any screenshot.
const MAX_PIXELS: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum EncodeError {
    TooLarge { pixels: usize },
    Inconsistent { expected: usize, actual: usize },
    Encoding(png::EncodingError),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { pixels } => write!(f, "{pixels} pixels is past the {MAX_PIXELS} limit"),
            Self::Inconsistent { expected, actual } => {
                write!(f, "dimensions imply {expected} bytes, got {actual}")
            },
            Self::Encoding(e) => write!(f, "{e}"),
        }
    }
}

/// `Compression::Fast` buys latency on a keypress at the cost of a larger file
/// that nothing keeps.
pub fn encode_png(image: &ImageData<'_>) -> Result<Vec<u8>, EncodeError> {
    let pixels = image.width.saturating_mul(image.height);
    if pixels > MAX_PIXELS {
        return Err(EncodeError::TooLarge { pixels });
    }
    let expected = pixels.saturating_mul(4);
    if image.bytes.len() != expected {
        return Err(EncodeError::Inconsistent { expected, actual: image.bytes.len() });
    }

    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width as u32, image.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(EncodeError::Encoding)?;
    writer.write_image_data(&image.bytes).map_err(EncodeError::Encoding)?;
    writer.finish().map_err(EncodeError::Encoding)?;
    Ok(out)
}

/// The file a set of PNG bytes belongs in.  Content-addressed, so pasting the
/// same screenshot twice reuses one file, and the full 64-bit digest rather
/// than the scratchpad's truncated one, since here a collision would paste the
/// wrong image instead of merely colliding a label.
pub fn file_name(png: &[u8]) -> String {
    format!("clipboard-{:016x}.png", crate::digest::stable_digest(png))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree clipboard_image::`
Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/Cargo.toml Cargo.lock alacritree/src/clipboard_image.rs alacritree/src/main.rs
git commit -m "feat(clipboard): encode a clipboard bitmap as png"
```

---

### Task 5: Store the PNG and bound the directory

**Files:**
- Modify: `alacritree/src/clipboard_image.rs` (add `store` and its helpers)
- Test: `alacritree/src/clipboard_image.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `file_name` from Task 4.
- Produces: `clipboard_image::store(dir: &Path, png: &[u8], cap: Option<usize>) -> io::Result<PathBuf>`. `cap` is `Some(keep)` only for a directory alacritree owns; `None` never deletes anything.

- [ ] **Step 1: Write the failing tests**

Add to `alacritree/src/clipboard_image.rs`'s `mod tests`. Note the explicit `set_modified` calls: filesystem timestamp granularity is coarse enough that files written in a loop can tie, which would make ordering assertions flaky.

```rust
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn age(path: &Path, seconds: u64) {
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - seconds);
        fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    #[test]
    fn store_creates_a_missing_directory_and_writes_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested").join("clipboard");

        let path = store(&dir, b"png bytes", None).unwrap();

        assert_eq!(path.file_name().unwrap(), file_name(b"png bytes").as_str());
        assert_eq!(fs::read(&path).unwrap(), b"png bytes");
    }

    #[test]
    fn storing_the_same_bytes_twice_leaves_one_file() {
        let tmp = tempfile::tempdir().unwrap();

        let first = store(tmp.path(), b"same", None).unwrap();
        let second = store(tmp.path(), b"same", None).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    /// Reuse must refresh the timestamp.  Without it a re-pasted old screenshot
    /// keeps its original mtime, and the next sweep — by which time it is no
    /// longer the returned path and so no longer exempt — deletes a file the
    /// user pasted moments ago.
    ///
    /// The sweep therefore has to run against a *later* store, not the reusing
    /// one: while `old` is the returned path `apply_cap` skips it outright, so
    /// a cap applied there would pass with or without the refresh.
    #[test]
    fn reuse_refreshes_the_timestamp_so_a_later_sweep_spares_it() {
        let tmp = tempfile::tempdir().unwrap();
        let old = store(tmp.path(), b"old", None).unwrap();
        age(&old, 9_000);
        for i in 0..3u8 {
            age(&store(tmp.path(), &[i], None).unwrap(), 1_000);
        }

        store(tmp.path(), b"old", None).unwrap();
        store(tmp.path(), b"newest", Some(2)).unwrap();

        assert!(old.is_file(), "a reused file was swept as though it were stale");
    }

    #[test]
    fn the_cap_keeps_the_newest_and_always_the_returned_path() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..6u8 {
            let path = store(tmp.path(), &[i], None).unwrap();
            age(&path, u64::from(6 - i) * 100);
        }

        let path = store(tmp.path(), b"newest", Some(3)).unwrap();

        assert!(path.is_file());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 3);
    }

    /// A cap smaller than one still has to return a usable path.
    #[test]
    fn a_cap_of_one_keeps_only_the_returned_path() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..4u8 {
            store(tmp.path(), &[i], None).unwrap();
        }

        let path = store(tmp.path(), b"last", Some(1)).unwrap();

        assert!(path.is_file());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    /// The guarantee that makes pointing image_dir at a pictures folder safe.
    #[test]
    fn no_cap_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5u8 {
            store(tmp.path(), &[i], None).unwrap();
        }

        store(tmp.path(), b"another", None).unwrap();

        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 6);
    }

    #[test]
    fn the_cap_never_touches_a_file_it_did_not_name() {
        let tmp = tempfile::tempdir().unwrap();
        let keeper = tmp.path().join("holiday.png");
        fs::write(&keeper, b"a real photo").unwrap();
        age(&keeper, 9_000);
        for i in 0..4u8 {
            store(tmp.path(), &[i], None).unwrap();
        }

        store(tmp.path(), b"newest", Some(1)).unwrap();

        assert!(keeper.is_file(), "a foreign file was deleted");
    }

    #[test]
    fn a_destination_of_the_wrong_length_is_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(file_name(b"payload"));
        fs::write(&path, b"truncated").unwrap();

        let stored = store(tmp.path(), b"payload", None).unwrap();

        assert_eq!(stored, path);
        assert_eq!(fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn a_destination_that_is_a_directory_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join(file_name(b"payload"))).unwrap();

        let stored = store(tmp.path(), b"payload", None).unwrap();

        assert_eq!(fs::read(&stored).unwrap(), b"payload");
    }

    /// The limit of that replacement.  Whatever a populated directory on this
    /// name is, it is not something this module wrote, and its contents are
    /// worth more than one paste succeeding.
    #[test]
    fn a_populated_directory_on_the_name_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let squatter = tmp.path().join(file_name(b"payload"));
        fs::create_dir(&squatter).unwrap();
        fs::write(squatter.join("precious.txt"), b"keep me").unwrap();

        assert!(store(tmp.path(), b"payload", None).is_err());
        assert_eq!(fs::read(squatter.join("precious.txt")).unwrap(), b"keep me");
    }

    #[test]
    fn no_temp_file_survives_a_completed_store() {
        let tmp = tempfile::tempdir().unwrap();

        store(tmp.path(), b"payload", None).unwrap();

        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree clipboard_image::tests::store`
Expected: FAIL to compile, `cannot find function store`.

- [ ] **Step 3: Write the implementation**

Extend the imports at the top of `alacritree/src/clipboard_image.rs`:

```rust
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
```

Add below `file_name`:

```rust
/// Write `png` into `dir` under its content-addressed name and return the path.
///
/// `cap` bounds the directory to that many generated files and is `Some` only
/// for a directory alacritree owns — a directory the user named may hold files
/// alacritree never wrote, and a filename pattern is no proof of ownership.
pub fn store(dir: &Path, png: &[u8], cap: Option<usize>) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(file_name(png));
    if !reusable(&path, png.len() as u64) {
        write_atomically(dir, &path, png)?;
    }
    if let Some(keep) = cap {
        apply_cap(dir, keep, &path);
    }
    Ok(path)
}

/// Whether the destination already holds these bytes *and* its timestamp was
/// refreshed.  Content addressing makes equal names strong evidence of equal
/// bytes, not proof, so the length is checked too; a link, a directory or a
/// timestamp that would not move all mean "write it again".
fn reusable(path: &Path, len: u64) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() != len {
        return false;
    }
    File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()))
        .is_ok()
}

/// Write through a uniquely named temporary in the same directory so a reader
/// never opens a half-written PNG.
fn write_atomically(dir: &Path, path: &Path, png: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        "{}.{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&tmp, png)?;
    clear_directory_at(path);
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            // Another instance writing the same content first is a success, not
            // a collision — but only once the destination is checked the same
            // way step 2 checks it, since a racing writer can equally have left
            // something of the wrong length behind.
            if reusable(path, png.len() as u64) { Ok(()) } else { Err(e) }
        },
    }
}

/// `rename` replaces a file but cannot replace a directory, so a directory
/// squatting on a generated name would fail that image's every paste forever.
///
/// Only an empty one is removed.  A populated directory is something this
/// module did not create, and losing its contents to a name collision is a
/// far worse outcome than the paste failing.
fn clear_directory_at(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    if let Err(e) = fs::remove_dir(path) {
        log::debug!("could not clear the directory at {}: {e}", path.display());
    }
}

/// Keep the `keep` newest generated files, `in_use` always among them.
///
/// Failures are logged and skipped: a file that outlives its turn costs a few
/// hundred kilobytes, while giving up here would abandon the rest of the sweep.
fn apply_cap(dir: &Path, keep: usize, in_use: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::debug!("cannot sweep {}: {e}", dir.display());
            return;
        },
    };
    let mut generated: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log::debug!("skipping an unreadable entry in {}: {e}", dir.display());
                continue;
            },
        };
        if !is_generated_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        if path == in_use {
            continue;
        }
        match fs::metadata(&path).and_then(|meta| meta.modified()) {
            Ok(when) => generated.push((when, path)),
            // Unranked means unswept: a file whose age cannot be read is never
            // the one chosen for deletion.
            Err(e) => log::debug!("cannot age {}: {e}", path.display()),
        }
    }

    let others = keep.saturating_sub(1);
    if generated.len() <= others {
        return;
    }
    generated.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, stale) in generated.into_iter().skip(others) {
        if let Err(e) = fs::remove_file(&stale) {
            log::debug!("could not remove {}: {e}", stale.display());
        }
    }
}

/// Only names this module produces are ever deleted.  The `.tmp` suffix a
/// half-finished write leaves behind fails this too, so a crashed process
/// cannot have its leftovers swept by a later one — a trade for never
/// deleting something a user put here.
fn is_generated_name(name: &str) -> bool {
    name.strip_prefix("clipboard-")
        .and_then(|rest| rest.strip_suffix(".png"))
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree clipboard_image::`
Expected: PASS, 16 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/clipboard_image.rs
git commit -m "feat(clipboard): store pasted images under a count cap"
```

---

### Task 6: Typed clipboard probes and format resolution

`clipboard::read` collapses every failure to `None`. That was fine when `None` meant "paste nothing"; it is wrong now that it means "try the next format", because a text read that *failed* is not evidence that there is no text.

**Files:**
- Modify: `alacritree/src/clipboard.rs`
- Test: `alacritree/src/clipboard.rs` (inline `mod tests` — the file has none today, so create it)

**Interfaces:**
- Consumes: `config::PasteConfig` from Task 2.
- Produces:
  - `clipboard::Probe<T> { Found(T), Absent, Failed }`
  - `clipboard::Payload { Text(String), Paths(Vec<PathBuf>), Image(arboard::ImageData<'static>), Nothing }`
  - `clipboard::read_text(target: Target) -> Probe<String>`
  - `clipboard::read_files() -> Probe<Vec<PathBuf>>`
  - `clipboard::read_image() -> Probe<arboard::ImageData<'static>>`
  - `clipboard::resolve(cfg, text, files, image) -> Payload` where each argument after `cfg` is a `FnOnce` probe
  - `clipboard::read` keeps its current signature and behavior.

- [ ] **Step 1: Write the failing tests**

Add to the bottom of `alacritree/src/clipboard.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::PathBuf;

    use super::*;
    use crate::config::PasteConfig;

    fn absent<T>() -> Probe<T> {
        Probe::Absent
    }

    fn image() -> Probe<arboard::ImageData<'static>> {
        Probe::Found(arboard::ImageData {
            width: 1,
            height: 1,
            bytes: Cow::Owned(vec![0, 0, 0, 255]),
        })
    }

    fn paths() -> Probe<Vec<PathBuf>> {
        Probe::Found(vec![PathBuf::from("/a/one.png")])
    }

    #[test]
    fn text_wins_over_every_other_format() {
        let payload = resolve(
            &PasteConfig::default(),
            || Probe::Found("hello".to_string()),
            paths,
            image,
        );
        assert!(matches!(payload, Payload::Text(t) if t == "hello"));
    }

    #[test]
    fn a_file_list_is_used_when_there_is_no_text() {
        let payload = resolve(&PasteConfig::default(), absent, paths, image);
        assert!(matches!(payload, Payload::Paths(p) if p.len() == 1));
    }

    #[test]
    fn a_bitmap_is_used_when_there_is_neither_text_nor_a_file_list() {
        let payload = resolve(&PasteConfig::default(), absent, absent, image);
        assert!(matches!(payload, Payload::Image(_)));
    }

    /// A failed read is not evidence of an absent format.  Pasting the image
    /// because the *text* read failed would paste something the user never
    /// asked for.
    #[test]
    fn a_failed_read_aborts_instead_of_falling_through() {
        let payload = resolve(&PasteConfig::default(), || Probe::Failed, paths, image);
        assert!(matches!(payload, Payload::Nothing));

        let payload = resolve(&PasteConfig::default(), absent, || Probe::Failed, image);
        assert!(matches!(payload, Payload::Nothing));
    }

    /// A degenerate CF_HDROP would otherwise stop resolution while pasting
    /// nothing at all.
    #[test]
    fn an_empty_file_list_falls_through_to_the_bitmap() {
        let payload = resolve(&PasteConfig::default(), absent, || Probe::Found(Vec::new()), image);
        assert!(matches!(payload, Payload::Image(_)));
    }

    #[test]
    fn each_fallback_can_be_switched_off_on_its_own() {
        let no_files = PasteConfig { files: false, ..PasteConfig::default() };
        let payload = resolve(&no_files, absent, paths, image);
        assert!(matches!(payload, Payload::Image(_)), "files off must skip to the bitmap");

        let no_image = PasteConfig { image: false, ..PasteConfig::default() };
        let payload = resolve(&no_image, absent, absent, image);
        assert!(matches!(payload, Payload::Nothing));
    }

    /// Both off is today's behavior exactly: no text, no paste.
    #[test]
    fn both_fallbacks_off_restores_the_original_behavior() {
        let off = PasteConfig { files: false, image: false, ..PasteConfig::default() };
        let payload = resolve(&off, absent, paths, image);
        assert!(matches!(payload, Payload::Nothing));
    }

    /// A disabled format is never probed, so a clipboard whose image read would
    /// hang or warn costs nothing when the user turned it off.
    #[test]
    fn a_disabled_format_is_never_probed() {
        let off = PasteConfig { files: false, image: false, ..PasteConfig::default() };
        resolve(
            &off,
            absent,
            || panic!("file list probed while disabled"),
            || panic!("image probed while disabled"),
        );
    }

    #[test]
    fn an_absent_format_maps_to_absent_and_other_errors_to_failed() {
        assert!(matches!(classify(Err::<(), _>(arboard::Error::ContentNotAvailable)), Probe::Absent));
        assert!(matches!(classify(Err::<(), _>(arboard::Error::ClipboardOccupied)), Probe::Failed));
        assert!(matches!(classify(Ok::<_, arboard::Error>(7)), Probe::Found(7)));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree clipboard::`
Expected: FAIL to compile, `cannot find type Probe`.

- [ ] **Step 3: Write the implementation**

Add to the imports at the top of `alacritree/src/clipboard.rs`:

```rust
use std::path::PathBuf;

use crate::config::PasteConfig;
```

Add below the existing `read` function:

```rust
/// What one clipboard probe found.  The distinction is load-bearing: `Absent`
/// means "try the next format", while `Failed` must stop the paste, because a
/// read that failed says nothing about whether the format was there.
pub enum Probe<T> {
    Found(T),
    Absent,
    Failed,
}

pub enum Payload {
    Text(String),
    Paths(Vec<PathBuf>),
    Image(arboard::ImageData<'static>),
    Nothing,
}

fn classify<T>(result: Result<T, arboard::Error>) -> Probe<T> {
    match result {
        Ok(value) => Probe::Found(value),
        Err(arboard::Error::ContentNotAvailable) => Probe::Absent,
        Err(e) => {
            log::warn!("clipboard read failed: {e}");
            Probe::Failed
        },
    }
}

fn with_clipboard<T>(read: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Probe<T> {
    match arboard::Clipboard::new() {
        Ok(mut clip) => classify(read(&mut clip)),
        Err(e) => {
            log::warn!("clipboard unavailable: {e}");
            Probe::Failed
        },
    }
}

pub fn read_text(target: Target) -> Probe<String> {
    with_clipboard(|clip| match target {
        Target::Clipboard => clip.get_text(),
        #[cfg(target_os = "linux")]
        Target::Primary => clip.get().clipboard(LinuxClipboardKind::Primary).text(),
        #[cfg(not(target_os = "linux"))]
        Target::Primary => clip.get_text(),
    })
}

/// Paths a file manager put on the clipboard.  Explorer's Cut advertises a move
/// effect alongside the same list; reading the paths neither performs nor
/// completes that move, so Cut and Copy paste identically.
pub fn read_files() -> Probe<Vec<PathBuf>> {
    with_clipboard(|clip| clip.get().file_list())
}

pub fn read_image() -> Probe<arboard::ImageData<'static>> {
    with_clipboard(|clip| clip.get_image())
}

/// Resolve the clipboard in priority order, probing lazily: text outright, then
/// copied paths, then a bitmap.  Each probe runs only once every earlier one
/// came back absent, so an ordinary text paste never opens the image formats,
/// and a format the config switched off is never probed at all.
pub fn resolve(
    cfg: &PasteConfig,
    text: impl FnOnce() -> Probe<String>,
    files: impl FnOnce() -> Probe<Vec<PathBuf>>,
    image: impl FnOnce() -> Probe<arboard::ImageData<'static>>,
) -> Payload {
    match text() {
        Probe::Found(text) => return Payload::Text(text),
        Probe::Failed => return Payload::Nothing,
        Probe::Absent => {},
    }
    if cfg.files {
        match files() {
            // An empty list is a degenerate CF_HDROP, not a decision to paste
            // nothing; fall through rather than stopping here.
            Probe::Found(paths) if !paths.is_empty() => return Payload::Paths(paths),
            Probe::Failed => return Payload::Nothing,
            _ => {},
        }
    }
    if cfg.image {
        match image() {
            Probe::Found(image) => return Payload::Image(image),
            Probe::Failed => return Payload::Nothing,
            Probe::Absent => {},
        }
    }
    Payload::Nothing
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree clipboard::`
Expected: PASS, 9 tests.

- [ ] **Step 5: Fold the old reader into the new one**

`read` now duplicates `read_text`'s clipboard handling. Replace the whole body of the existing `read` function with a delegation, keeping its signature and its `Option` return so existing callers are untouched:

```rust
pub fn read(target: Target) -> Option<String> {
    match read_text(target) {
        Probe::Found(text) => Some(text),
        Probe::Absent | Probe::Failed => None,
    }
}
```

Run: `cargo test -p alacritree`
Expected: PASS. Every existing `clipboard::read` caller behaves as before — a failure and an absent clipboard both still yield `None` for them.

- [ ] **Step 6: Check the whole crate still builds clean**

Run: `cargo clippy -p alacritree --all-targets`
Expected: no warnings. `Payload::Image` is larger than its siblings, so `clippy::large_enum_variant` may fire; boxing the `ImageData` is the fix if it does.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/clipboard.rs
git commit -m "feat(clipboard): probe formats with typed results"
```

---

### Task 7: Wire the fallback into Paste

The two paste arms in `app.rs` duplicate the same scratchpad-or-terminal branch. Collapse them into one helper that resolves the target first — so a paste with nowhere to go touches neither the clipboard nor the filesystem — then resolves the clipboard.

`app.rs` has no unit-test harness, so the decision of *what text a path list
becomes* is pulled out into `file_drop::paste_payload` and tested there —
`file_drop.rs` already does this for the drop path, down to a seam test at line
549. What stays in `app.rs` is only the plumbing: read, store, hand to a sink.
The manual GUI check in Step 10 remains the only thing that exercises a real
clipboard.

**Files:**
- Modify: `alacritree/src/file_drop.rs` (add `paste_payload` beside `document_payload`)
- Modify: `alacritree/src/app.rs:2186-2209`
- Modify: `docs/alacritree.md:340` (document the table)

**Interfaces:**
- Consumes: everything produced by Tasks 2-6, plus the existing `file_drop::shell_payload`, `paste::paste`, and `scratchpad::Editor::insert_at_cursor`.
- Produces: `file_drop::paste_payload(paths: &[PathBuf], scratchpad: bool, distro: Option<&str>, spelling: &PathSpelling) -> String`.

- [ ] **Step 1: Write the failing tests for the routing decision**

Add to `alacritree/src/file_drop.rs`'s `mod tests`, after the
`document_payload` tests (which end at line 503):

```rust
    /// A pasted path reaches a shell exactly as a dropped one does.
    #[test]
    fn a_pasted_path_is_a_shell_word_for_a_terminal() {
        let spelling = PathSpelling { quote: Quoting::Posix, wsl_translate: false };
        let paths = [PathBuf::from("/a/my pic.png")];
        assert_eq!(paste_payload(&paths, false, None, &spelling), "'/a/my pic.png' ");
    }

    /// A scratchpad is a text document: quoting a path into it would put
    /// literal quote characters in the user's notes.
    #[test]
    fn a_pasted_path_is_bare_for_a_scratchpad() {
        let spelling = PathSpelling { quote: Quoting::Posix, wsl_translate: false };
        let paths = [PathBuf::from("/a/my pic.png"), PathBuf::from("/a/two.png")];
        assert_eq!(paste_payload(&paths, true, None, &spelling), "/a/my pic.png\n/a/two.png");
    }

    /// Unlike `document_payload`, which frames a drop as its own block, a paste
    /// lands at the cursor — so it adds no surrounding newline of its own.
    #[test]
    fn a_pasted_path_adds_no_newline_of_its_own() {
        let text = paste_payload(&[PathBuf::from("/a/one.png")], true, None, &PathSpelling::default());
        assert_eq!(text, "/a/one.png");
    }

    #[test]
    fn a_pasted_path_is_translated_for_a_wsl_shell() {
        let paths = [PathBuf::from(r"C:\pics\a.png")];
        assert_eq!(
            paste_payload(&paths, false, Some("Ubuntu"), &PathSpelling::default()),
            "/mnt/c/pics/a.png "
        );
    }

    /// The scratchpad runs on the Windows side whatever the session's shell is,
    /// so a path pasted into it keeps the spelling that opens it there.
    #[test]
    fn a_pasted_path_is_not_translated_for_a_scratchpad() {
        let paths = [PathBuf::from(r"C:\pics\a.png")];
        assert_eq!(
            paste_payload(&paths, true, Some("Ubuntu"), &PathSpelling::default()),
            r"C:\pics\a.png"
        );
    }

    /// The control-character filter is what `shell_payload` is for; routing
    /// through it is what keeps a pasted path from submitting a command.
    #[test]
    fn a_pasted_path_carrying_control_characters_is_filtered_for_a_terminal() {
        let spelling = PathSpelling { quote: Quoting::None, wsl_translate: false };
        let paths = [PathBuf::from("/a/evil\nrm -rf ~"), PathBuf::from("/a/ok.png")];
        assert_eq!(paste_payload(&paths, false, None, &spelling), "/a/ok.png ");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree file_drop::tests::a_pasted`
Expected: FAIL to compile, `cannot find function paste_payload`.

- [ ] **Step 3: Add the routing function**

In `alacritree/src/file_drop.rs`, below `document_payload`:

```rust
/// The text a pasted path list becomes for the sink that is about to receive
/// it.
///
/// The shell form is a drop's, character for character.  The document form is
/// deliberately not `document_payload`'s: a drop frames its paths as their own
/// block, while a paste lands wherever the cursor is and must leave the
/// surrounding line alone.
pub fn paste_payload(
    paths: &[PathBuf],
    scratchpad: bool,
    distro: Option<&str>,
    spelling: &PathSpelling,
) -> String {
    if scratchpad {
        paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n")
    } else {
        shell_payload(paths, distro, spelling)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree file_drop::tests::a_pasted`
Expected: PASS, 6 tests.

- [ ] **Step 5: Replace the two paste arms**

In `alacritree/src/app.rs`, replace lines 2186-2209 — the whole `BindingAction::Named(NamedAction::Paste)` and `BindingAction::Named(NamedAction::PasteSelection)` arms — with:

```rust
            BindingAction::Named(NamedAction::Paste) => {
                self.paste_from_clipboard(ctx, Target::Clipboard);
            },
            BindingAction::Named(NamedAction::PasteSelection) => {
                self.paste_from_clipboard(ctx, Target::Primary);
            },
```

- [ ] **Step 6: Add the helper methods**

Add to the same `impl AlacritreeApp` block, immediately after the method containing the match arms:

```rust
    /// The target is resolved before the clipboard so a paste with nowhere to
    /// go opens nothing.  Only the regular clipboard carries files and images:
    /// PRIMARY is a text selection, so its probes are skipped outright.
    fn paste_from_clipboard(&mut self, ctx: &Context, target: Target) {
        let Some(idx) = self.active_session_index() else {
            return;
        };
        let extras = target == Target::Clipboard;
        let payload = clipboard::resolve(
            &self.config.ui.paste,
            || clipboard::read_text(target),
            || if extras { clipboard::read_files() } else { clipboard::Probe::Absent },
            || if extras { clipboard::read_image() } else { clipboard::Probe::Absent },
        );

        let paths = match payload {
            clipboard::Payload::Text(text) => {
                self.insert_paste(ctx, idx, &text);
                return;
            },
            clipboard::Payload::Paths(paths) => paths,
            clipboard::Payload::Image(image) => match self.store_clipboard_image(&image) {
                Some(path) => vec![path],
                None => return,
            },
            clipboard::Payload::Nothing => return,
        };

        let session = &self.sessions[idx];
        let scratchpad = session.scratchpad.is_some();
        let text = file_drop::paste_payload(
            &paths,
            scratchpad,
            session.wsl_distro(),
            &self.config.ui.drop.spelling,
        );
        // An empty payload means every path was filtered out; pasting it would
        // send a bare space to the shell.
        if !text.is_empty() {
            self.insert_paste(ctx, idx, &text);
        }
    }

    fn insert_paste(&mut self, ctx: &Context, idx: usize, text: &str) {
        let id = self.sessions[idx].id;
        if let Some(editor) = self.sessions[idx].scratchpad.as_mut() {
            editor.insert_at_cursor(ctx, id, text);
        } else {
            paste::paste(&self.sessions[idx], text, true);
        }
    }

    /// The clipboard bitmap as a file something else can open, or `None` with
    /// the reason logged.
    fn store_clipboard_image(&self, image: &arboard::ImageData<'_>) -> Option<PathBuf> {
        let png = match clipboard_image::encode_png(image) {
            Ok(png) => png,
            Err(e) => {
                log::warn!("cannot encode the clipboard image: {e}");
                return None;
            },
        };
        let cfg = &self.config.ui.paste;
        let (dir, owned) = cfg.image_target();
        match clipboard_image::store(&dir, &png, owned.then_some(cfg.image_keep)) {
            Ok(path) => Some(path),
            Err(e) => {
                log::warn!("cannot write the clipboard image to {}: {e}", dir.display());
                None
            },
        }
    }
```

Add `use crate::clipboard_image;` to `app.rs`'s imports, beside the existing `use crate::clipboard;`. If `Target` is not already imported unqualified there, use the same spelling the replaced arms used (`clipboard::Target`) throughout.

- [ ] **Step 7: Verify it builds and the suite is green**

Run: `cargo clippy -p alacritree --all-targets && cargo test -p alacritree`
Expected: no warnings, all tests pass.

- [ ] **Step 8: Document the table**

In `docs/alacritree.md`, insert after the `[ui.drop]` block (which ends with the `highlight` line at line 339) and before `[workspace]`:

```markdown
[ui.paste]                  # what Paste does when the clipboard holds no text
files       = true          # paste the paths of files and folders copied in a
                            # file manager, as Windows Terminal does
image       = true          # write a clipboard bitmap (a Win+Shift+S capture)
                            # to a PNG and paste its path
image_dir   = "~/shots"     # where those PNGs go (default: a temp subdirectory).
                            # A directory you name here is never swept — set it
                            # and you keep every image and clean up yourself
image_keep  = 20            # how many PNGs the default directory keeps.
                            # Minimum 1 — the image a paste just handed to the
                            # shell always survives the sweep
```

Add a paragraph below the config block:

```markdown
Text always wins: a clipboard carrying both text and an image pastes the text.
Only the regular clipboard is checked for files and images — the X11 PRIMARY
selection is text, so middle-click paste is unchanged. Both options `false`
restores the original behavior exactly, where a paste with no text does nothing.
```

- [ ] **Step 9: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs alacritree/src/file_drop.rs docs/alacritree.md
git commit -m "feat(paste): paste a path when the clipboard has no text"
```

- [ ] **Step 10: Verify in the running app**

This is the only step that exercises a real clipboard. Build and run: `cargo run -p alacritree --release`

Check each of these:

1. **Screenshot → terminal.** Take a `Win+Shift+S` capture, focus a terminal session, press `Ctrl+Shift+V`. Expected: a quoted path ending `.png` appears on the command line, followed by a space.
2. **The path resolves.** Press Enter on a `ls` or `file` prefixed to that path. Expected: the file exists and is a PNG.
3. **Claude Code.** Run `claude` in a session, take a capture, press `Ctrl+Shift+V`. Expected: the prompt shows `[Image #1]`, not a raw path.
4. **WSL session.** Repeat 1-3 in a session whose shell runs in a distro. Expected: the pasted path is `/mnt/c/…` and resolves inside the distro.
5. **Deduplication.** Paste the same capture twice without taking a new one. Expected: the same filename both times, and `%TEMP%\alacritree\clipboard\` holds one file for it.
6. **Copied file.** Copy a file in Explorer, press `Ctrl+Shift+V`. Expected: its path is pasted. Repeat with a folder, and with several files selected at once.
7. **Text is unaffected.** Copy ordinary text, paste it. Expected: unchanged behavior, and no file appears in the image directory.
8. **Scratchpad.** Open a scratchpad tab, type a sentence, put the cursor mid-sentence, paste a capture. Expected: the path lands at the cursor with no blank lines added around it. Repeat with text selected. Expected: the selection is replaced.
9. **Off switch.** Set `image = false` and `files = false` in `alacritree.toml`, restart, repeat 1 and 6. Expected: nothing pastes, exactly as before this feature.
10. **The cap.** Set `image_keep = 3`, take and paste four different captures. Expected: the directory holds three files and the most recently pasted path still exists.

Record any deviation before moving on. A failure here means a task above is wrong, not that the check should be relaxed.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| Behavior — three-step resolution order | 6 (`resolve`), 7 (wiring) |
| Behavior — empty file list falls through | 6 |
| Behavior — Cut and Copy paste identically | 6 (documented at `read_files`) |
| Clipboard resolution — typed absent-vs-failed | 6 |
| Configuration — `[ui.paste]`, defaults on | 2, documented in 7 |
| Dependencies — arboard `image-data` | 4 |
| Components — `digest.rs` | 1 |
| Components — `config.rs` grows `PathSpelling` | 3 |
| Components — `clipboard_image.rs` | 4, 5 |
| Components — `app.rs` collapse | 7 |
| Storing a PNG — five-step protocol | 5 |
| Keeping the directory bounded — count cap, three guards | 5 |
| Scratchpad insertion — literal at cursor | 7 (`paste_payload`) |
| Error handling — log and do nothing | 4 (`Display`), 5, 6, 7 |
| WSL paths — reuse `shell_payload` | 7, tested in Step 1, verified in Step 10.4 |
| Testing — every listed assertion | 4, 5, 6 |

Deliberately unimplemented, per the spec's Follow-ups: per-distro automount roots, the `GetClipboardSequenceNumber` guard, per-shell-dialect quoting.

**Type consistency:** `stable_digest` (1) → `file_name` (4) → `store` (5). `PathSpelling` (3) → `shell_payload`/`paste_payload` (3, 7). `PasteConfig`/`image_target` (2) → `resolve` (6) and `store_clipboard_image` (7). `Probe`/`Payload` (6) → `paste_from_clipboard` (7). `EncodeError: Display` (4) is what makes the `log::warn!` in Task 7 compile. `store`'s `cap: Option<usize>` is fed by `owned.then_some(cfg.image_keep)` in Task 7, matching `image_target`'s returned bool, and `image_keep`'s floor of 1 (2) is what keeps that cap from being asked to delete the path it just returned.

**Placeholder scan:** no TBDs; every code step carries complete code; every command has an expected result.
