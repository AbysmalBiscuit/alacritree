# Font Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Glyph fallback on Windows (user-configured `[font] fallback` list on all platforms + an automatic coverage-trimmed chain on Windows), per the approved spec `docs/superpowers/specs/2026-07-11-font-fallback-design.md`.

**Architecture:** `config.rs` gains a `[font] fallback` list. `fonts.rs` gains (a) a lazily-loaded `SystemFonts` wrapper so one `fontdb::Database` serves a whole `install_terminal_fonts` call, (b) a platform-neutral `coverage` module (Unicode coverage sets, candidate ordering, FcFontSort-style greedy trim — pure, unit-tested), (c) a real `gather_fallback_faces` for `cfg(not(unix))` that scans system font cmaps once and trims per variant, and (d) `register_user_fallbacks`, which resolves the user list per variant and registers it between the primary faces and the automatic chain. Unix fontconfig resolution is untouched.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui 0.31, fontdb 0.23, ttf-parser 0.25 (new explicit dep; already in Cargo.lock at 0.25.1 via fontdb).

## Global Constraints

- Only touch the `alacritree/` crate (`alacritree/src/*.rs`, `alacritree/Cargo.toml`).
- All cargo commands use `-p alacritree`; run them from the worktree root.
- `cargo fmt` before every commit (rustfmt is enforced).
- Conventional Commits; subject imperative, lowercase after colon, ≤50 chars.
- `MAX_FALLBACK_FACES` stays 32 and is the trim limit.
- Unix fontconfig resolution and `FcFontSort` chain must not change behavior.
- Font problems never fail startup: unreadable/unparseable files → `log::debug!` and skip; unresolvable user entry → one `log::warn!` per entry (not per entry×variant).
- Comments explain *why*, timeless, no task/PR references.
- New config field is documented on the `RawFont` struct; recommended home is `alacritree.toml` (upstream alacritty warns on unknown keys in the shared `alacritty.toml`).

## Setup (before Task 1)

Work happens in a dedicated worktree (worktree-per-feature convention):

```powershell
# from C:\Users\Lev\Git\github\alacritree
git worktree add ../alacritree-worktrees/feat/font-fallback -b feat/font-fallback master
```

All subsequent commands run in `C:\Users\Lev\Git\github\alacritree-worktrees\feat\font-fallback`. The spec and this plan are git-excluded and exist only in the main checkout — reference them by absolute path (`C:\Users\Lev\Git\github\alacritree\docs\superpowers\...`).

The crate currently has zero tests; `#[cfg(test)] mod tests` modules added below are the first ones.

---

### Task 1: `[font] fallback` config field

**Files:**
- Modify: `alacritree/src/config.rs` (struct `FontConfig` ~line 33, `impl Default for FontConfig` ~line 198, struct `RawFont` ~line 446, `RawConfig::into_config` font section ~line 683; tests module appended at end of file)

**Interfaces:**
- Consumes: existing `merge()` / `RawConfig` / `into_config()` in `config.rs`.
- Produces: `pub fallback: Vec<String>` on `FontConfig` (default empty). Later tasks read `font.fallback`.

- [ ] **Step 1: Write the failing tests**

Append at the end of `alacritree/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Config {
        let value: toml::Value = toml::from_str(s).unwrap();
        let raw: RawConfig = value.try_into().unwrap();
        raw.into_config()
    }

    #[test]
    fn font_fallback_list_parses() {
        let config = parse(
            r#"
            [font]
            fallback = ["JetBrainsMono Nerd Font", "C:\\Fonts\\custom.ttf"]
            "#,
        );
        assert_eq!(config.font.fallback, ["JetBrainsMono Nerd Font", "C:\\Fonts\\custom.ttf"]);
    }

    #[test]
    fn font_fallback_defaults_empty() {
        assert!(parse("").font.fallback.is_empty());
    }

    #[test]
    fn font_fallback_arrays_concatenate_across_files() {
        // alacritty merge semantics: an array in alacritree.toml appends to
        // the same array from alacritty.toml rather than replacing it.
        let base: toml::Value = toml::from_str("[font]\nfallback = [\"A\"]").unwrap();
        let over: toml::Value = toml::from_str("[font]\nfallback = [\"B\"]").unwrap();
        let merged = merge(base, over);
        let raw: RawConfig = merged.try_into().unwrap();
        assert_eq!(raw.into_config().font.fallback, ["A", "B"]);
    }
}
```

(Note: inside the raw string, `"C:\\Fonts\\custom.ttf"` is TOML-escaped; the parsed value is `C:\Fonts\custom.ttf`, and the Rust expected literal `"C:\\Fonts\\custom.ttf"` is the same string.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree`
Expected: compilation FAILURE — `no field 'fallback' on type FontConfig` (RED via compile error; the field doesn't exist yet).

- [ ] **Step 3: Implement**

In `FontConfig` (after `builtin_box_drawing`):

```rust
    /// Ordered fallback families or font file paths, consulted after the four
    /// primary faces and before the automatic system fallback chain.
    pub fallback: Vec<String>,
```

In `impl Default for FontConfig`, add to the struct literal:

```rust
            fallback: Vec::new(),
```

In `RawFont` (after `builtin_box_drawing`):

```rust
    /// Ordered list of fallback font families or font file paths, tried in
    /// order after the four primary faces and before the automatic system
    /// chain.  Recommended home is `alacritree.toml`: upstream alacritty
    /// warns about unknown keys, so putting it in the shared `alacritty.toml`
    /// would make the real alacritty noisy.
    fallback: Option<Vec<String>>,
```

In `RawConfig::into_config`, font section (after the `builtin_box_drawing` block):

```rust
        font.fallback = self.font.fallback.clone().unwrap_or_default();
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: PASS — 3 tests, 0 failures.

- [ ] **Step 5: Format and commit**

```powershell
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): add [font] fallback list"
```

---

### Task 2: Shared fontdb database (`SystemFonts`) + `SharedFile` fix

Pure refactor plus one latent-bug fix. No new tests (behavior is system-font-dependent); the gate is `cargo check` clean and Task 1's tests still green.

**Files:**
- Modify: `alacritree/src/fonts.rs` (`install_terminal_fonts`, `load_variant`, `resolve_face` both cfgs, `resolve_via_fontdb`, `register_fallback_faces`, both `gather_fallback_faces` cfgs)

**Interfaces:**
- Produces (used by Tasks 3 and 6):
  - `struct SystemFonts { db: OnceCell<fontdb::Database> }` with `fn db(&self) -> &fontdb::Database`
  - `fn variant_query(variant: Variant) -> (fontdb::Weight, fontdb::Style)`
  - `resolve_face(family_or_path: &str, style: Option<&str>, variant: Variant, fonts: &SystemFonts) -> Option<ResolvedFace>` (both cfgs gain the `fonts` param)
  - `load_variant(family: &str, style: Option<&str>, variant: Variant, normal_path: &Path, fonts: &SystemFonts) -> Option<Vec<u8>>`
  - `register_fallback_faces(..., fonts: &SystemFonts, loaded_paths: &mut HashSet<PathBuf>)` and `gather_fallback_faces(..., fonts: &SystemFonts)` (unix impl ignores it as `_fonts`)

- [ ] **Step 1: Add `SystemFonts` and `variant_query`**

At the top of `fonts.rs`, extend imports:

```rust
use std::cell::OnceCell;
```

Below the `DEFAULT_FAMILY` const, add:

```rust
/// Lazily-loaded system font database shared by every resolution within one
/// `install_terminal_fonts` call.  Loading is deferred so Unix systems where
/// fontconfig answers everything never pay for a fontdb scan.
#[derive(Default)]
struct SystemFonts {
    db: OnceCell<fontdb::Database>,
}

impl SystemFonts {
    fn db(&self) -> &fontdb::Database {
        self.db.get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            db
        })
    }
}

fn variant_query(variant: Variant) -> (fontdb::Weight, fontdb::Style) {
    match variant {
        Variant::Normal => (fontdb::Weight::NORMAL, fontdb::Style::Normal),
        Variant::Bold => (fontdb::Weight::BOLD, fontdb::Style::Normal),
        Variant::Italic => (fontdb::Weight::NORMAL, fontdb::Style::Italic),
        Variant::BoldItalic => (fontdb::Weight::BOLD, fontdb::Style::Italic),
    }
}
```

- [ ] **Step 2: Rewrite `resolve_via_fontdb` to use the shared db and accept `SharedFile`**

Replace the whole function:

```rust
fn resolve_via_fontdb(family: &str, variant: Variant, fonts: &SystemFonts) -> Option<ResolvedFace> {
    let (weight, style) = variant_query(variant);
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight,
        stretch: fontdb::Stretch::Normal,
        style,
    };
    let db = fonts.db();
    let face_id = db.query(&query)?;
    let face_info = db.face(face_id)?;
    match &face_info.source {
        // A memory-mapped `SharedFile` still names a real file on disk.
        fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
            Some(ResolvedFace { path: path.clone() })
        },
        // Embedded faces aren't path-addressable; we'd have to re-architect
        // the loader to support them and they're rare.
        fontdb::Source::Binary(_) => None,
    }
}
```

- [ ] **Step 3: Thread `fonts: &SystemFonts` through**

- Both `resolve_face` cfgs gain a trailing `fonts: &SystemFonts` parameter; their `resolve_via_fontdb(family_or_path, variant)` calls become `resolve_via_fontdb(family_or_path, variant, fonts)`.
- `load_variant` gains a trailing `fonts: &SystemFonts` parameter, passed to its `resolve_face` call.
- `register_fallback_faces` gains a trailing `fonts: &SystemFonts` parameter, passed to `gather_fallback_faces`.
- Both `gather_fallback_faces` cfgs gain a trailing `fonts` parameter (`_fonts: &SystemFonts` in both bodies for now — unix never uses it, and the Windows implementation lands in Task 6).
- In `install_terminal_fonts`, immediately after the `family` binding, add:

```rust
    let fonts = SystemFonts::default();
```

and pass `&fonts` to the `resolve_face` call for `normal_match`, all three `load_variant` calls, and the `register_fallback_faces` call in the seeds loop.

- [ ] **Step 4: Verify**

Run: `cargo check -p alacritree` — expected: clean (no warnings about unused params; unix-only code paths aren't compiled here).
Run: `cargo test -p alacritree` — expected: PASS (Task 1's 3 tests).

- [ ] **Step 5: Format and commit**

```powershell
cargo fmt
git add alacritree/src/fonts.rs
git commit -m "refactor(fonts): reuse one fontdb database"
```

Body for the commit message:

```
Every fontdb resolution used to build and scan its own database.  Share a
lazily-initialized one per install_terminal_fonts call so the upcoming
Windows fallback chain and the variant resolutions pay for one scan.

Also accept fontdb's SharedFile source: it is memory-mapped but still
names a real on-disk file, so dropping it lost valid matches.
```

---

### Task 3: User fallback list registration (all platforms)

**Files:**
- Modify: `alacritree/src/fonts.rs` (`install_terminal_fonts` signature + seeds loop, `register_fallback_faces` signature, new `FallbackBook` + `register_user_fallbacks`, new tests module)
- Modify: `alacritree/src/app.rs:182-188` (call site)

**Interfaces:**
- Consumes: `FontConfig.fallback: Vec<String>` (Task 1), `SystemFonts` + `resolve_face(.., fonts)` (Task 2).
- Produces:
  - `pub fn install_terminal_fonts(ctx: &Context, font: &FontConfig)` — **signature change**, call site updated.
  - `struct FallbackBook { loaded_paths: HashSet<PathBuf>, ids_by_path: HashMap<PathBuf, String>, warned_entries: HashSet<String> }`
  - `fn register_user_fallbacks(defs: &mut FontDefinitions, entries: &[String], variant: Variant, targets: &[FontFamily], fonts: &SystemFonts, book: &mut FallbackBook)`
  - `register_fallback_faces` now takes `book: &mut FallbackBook` instead of `loaded_paths: &mut HashSet<PathBuf>`.

- [ ] **Step 1: Write the failing tests**

Append at the end of `fonts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_fallback_path_registers_for_every_variant() {
        // A file-path entry resolves to the same file for all four variants;
        // the bytes must be loaded once and the same egui font id appended to
        // each variant's family list (a plain HashSet dedup would starve
        // every variant after the first).
        let path = std::env::temp_dir().join("alacritree_test_user_fallback.ttf");
        std::fs::write(&path, b"egui parses this later; registration only reads bytes").unwrap();

        let mut defs = FontDefinitions::default();
        let fonts = SystemFonts::default();
        let mut book = FallbackBook::default();
        let entries = vec![path.to_string_lossy().into_owned()];

        let normal_targets = [FontFamily::Monospace];
        register_user_fallbacks(&mut defs, &entries, Variant::Normal, &normal_targets, &fonts, &mut book);
        let bold_targets = [FontFamily::Name(BOLD_FAMILY.into())];
        register_user_fallbacks(&mut defs, &entries, Variant::Bold, &bold_targets, &fonts, &mut book);

        assert_eq!(book.ids_by_path.len(), 1);
        let id = book.ids_by_path.values().next().unwrap();
        assert!(defs.families[&FontFamily::Monospace].contains(id));
        assert!(defs.families[&FontFamily::Name(BOLD_FAMILY.into())].contains(id));

        std::fs::remove_file(&path).ok();
    }

    // Unix-excluded: fontconfig substitutes *some* font for any family name,
    // so an unresolvable entry only exists where fontdb answers the query.
    #[cfg(not(unix))]
    #[test]
    fn unresolved_user_fallback_warns_once_and_adds_nothing() {
        let mut defs = FontDefinitions::default();
        let fonts = SystemFonts::default();
        let mut book = FallbackBook::default();
        let entries = vec![String::from("alacritree-no-such-family-6c1e")];
        let before = defs.families[&FontFamily::Monospace].len();

        let targets = [FontFamily::Monospace];
        register_user_fallbacks(&mut defs, &entries, Variant::Normal, &targets, &fonts, &mut book);
        register_user_fallbacks(&mut defs, &entries, Variant::Bold, &targets, &fonts, &mut book);

        assert_eq!(defs.families[&FontFamily::Monospace].len(), before);
        assert_eq!(book.warned_entries.len(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree`
Expected: compilation FAILURE — `cannot find struct FallbackBook` / `cannot find function register_user_fallbacks`.

- [ ] **Step 3: Implement**

Extend the `use` block at the top of `fonts.rs`:

```rust
use std::collections::{HashMap, HashSet};
```

(replacing the existing `use std::collections::HashSet;`), and add `FontConfig` to the config import:

```rust
use crate::config::{FontConfig, FontFace};
```

Add below `SystemFonts`:

```rust
/// Bookkeeping shared by all fallback registration within one install: which
/// files already back an egui font, which font id serves each file (so one
/// file can join several variants' family lists without duplicate data), and
/// which user entries have already produced a warning.
#[derive(Default)]
struct FallbackBook {
    loaded_paths: HashSet<PathBuf>,
    ids_by_path: HashMap<PathBuf, String>,
    warned_entries: HashSet<String>,
}

/// Register the user-configured `[font] fallback` entries for one variant.
/// They slot between the primary face and the automatic system chain, in
/// list order.  Entries are family names or font file paths, resolved with
/// the variant's weight/slant so bold cells cascade through bold fallbacks.
fn register_user_fallbacks(
    defs: &mut FontDefinitions,
    entries: &[String],
    variant: Variant,
    targets: &[FontFamily],
    fonts: &SystemFonts,
    book: &mut FallbackBook,
) {
    for entry in entries {
        let Some(resolved) = resolve_face(entry, None, variant, fonts) else {
            if book.warned_entries.insert(entry.clone()) {
                log::warn!("font.fallback entry '{entry}' did not resolve to any font");
            }
            continue;
        };
        if let Some(id) = book.ids_by_path.get(&resolved.path) {
            for family in targets {
                defs.families.entry(family.clone()).or_default().push(id.clone());
            }
            continue;
        }
        if book.loaded_paths.contains(&resolved.path) {
            // Already registered as a primary face, which sits ahead of every
            // fallback in the family lists; appending it again is pointless.
            continue;
        }
        let bytes = match std::fs::read(&resolved.path) {
            Ok(b) => b,
            Err(e) => {
                log::debug!("skipping fallback font {}: {e}", resolved.path.display());
                continue;
            },
        };
        let id = format!("alacritree_fallback_{}", defs.font_data.len());
        defs.font_data.insert(id.clone(), Arc::new(FontData::from_owned(bytes)));
        for family in targets {
            defs.families.entry(family.clone()).or_default().push(id.clone());
        }
        book.loaded_paths.insert(resolved.path.clone());
        book.ids_by_path.insert(resolved.path, id);
    }
}
```

Change `install_terminal_fonts` to take the whole font config:

```rust
pub fn install_terminal_fonts(ctx: &Context, font: &FontConfig) {
    let (normal, bold, italic, bold_italic) =
        (&font.normal, &font.bold, &font.italic, &font.bold_italic);
```

(the rest of the body stays; where it created `loaded_paths` it now creates the book):

```rust
    let mut book = FallbackBook::default();
    book.loaded_paths.insert(normal_match.path.clone());
```

and the seeds loop becomes:

```rust
    for (family, style, variant, targets) in seeds {
        register_user_fallbacks(&mut defs, &font.fallback, variant, targets, &fonts, &mut book);
        register_fallback_faces(&mut defs, family, style, variant, targets, &fonts, &mut book);
    }
```

`register_fallback_faces` swaps its `loaded_paths: &mut HashSet<PathBuf>` parameter for `book: &mut FallbackBook`; inside, `loaded_paths` becomes `book.loaded_paths` (the `gather_fallback_faces` call passes `&book.loaded_paths`, and the registration loop inserts into `book.loaded_paths`).

Update `alacritree/src/app.rs:182-188` to:

```rust
        crate::fonts::install_terminal_fonts(&cc.egui_ctx, &config.font);
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: PASS — 5 tests total (3 config + 2 fonts), 0 failures. (The unresolved-entry test loads the system font db once; a couple hundred ms is normal.)

- [ ] **Step 5: Format and commit**

```powershell
cargo fmt
git add alacritree/src/fonts.rs alacritree/src/app.rs
git commit -m "feat(fonts): honor user [font] fallback list"
```

Body:

```
Resolve each entry per variant (family names via the platform matcher,
file paths directly) and register the hits between the primary faces and
the automatic chain, so users can put a Nerd Font or emoji font ahead of
whatever the system picks.  A path shared by several variants is loaded
once and its font id appended to each variant's families.
```

> **Execution amendment (2026-07-12, task-3 review):** the Step 3 code above
> had a gap — `register_fallback_faces` inserted into `book.loaded_paths` but
> never `book.ids_by_path`, so a user entry resolving to a file already loaded
> by an *earlier variant's automatic chain* silently vanished from that
> variant. Fixed in-place: `register_fallback_faces` now records
> `ids_by_path` for every face it loads. The integration assertion lands with
> Task 6 (the Windows chain), since the automatic chain loads nothing on
> Windows before then.

---

### Task 4: Coverage sets + greedy trim (pure logic)

**Files:**
- Modify: `alacritree/src/fonts.rs` (new `mod coverage` with its own tests)

**Interfaces:**
- Produces (consumed by Tasks 5 and 6):
  - `coverage::Coverage` — `fn from_codepoints(Vec<u32>) -> Coverage`, `fn merge(&mut self, other: &Coverage)`, `fn has_novel_codepoint(&self, other: &Coverage) -> bool`, plus `Default + Clone + Debug + PartialEq`
  - `coverage::Candidate { path: PathBuf, face_index: u32, family: String, weight: u16, italic: bool, monospaced: bool }` (`Clone`)
  - `coverage::trim_by_coverage(candidates: Vec<(Candidate, Coverage)>, seed_coverage: &Coverage, limit: usize) -> Vec<Candidate>`

- [ ] **Step 1: Write the failing tests**

Add to `fonts.rs` (module body comes in Step 3; write the module skeleton with tests first):

```rust
// Pure candidate-selection logic for the automatic fallback chain: Unicode
// coverage sets and FcFontSort-style greedy trimming.  Platform-neutral so
// the unit tests run on every platform, even though only the Windows chain
// consumes it at runtime.
#[cfg_attr(unix, allow(dead_code))]
mod coverage {
    use std::path::PathBuf;

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Coverage {
        ranges: Vec<(u32, u32)>,
    }

    #[derive(Clone, Debug)]
    pub struct Candidate {
        pub path: PathBuf,
        pub face_index: u32,
        pub family: String,
        pub weight: u16,
        pub italic: bool,
        pub monospaced: bool,
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        pub(super) fn cand(family: &str) -> Candidate {
            Candidate {
                path: PathBuf::from(family),
                face_index: 0,
                family: family.into(),
                weight: 400,
                italic: false,
                monospaced: true,
            }
        }

        #[test]
        fn from_codepoints_sorts_dedups_and_merges_adjacent() {
            let c = Coverage::from_codepoints(vec![3, 1, 2, 2, 10]);
            assert_eq!(c, Coverage { ranges: vec![(1, 3), (10, 10)] });
        }

        #[test]
        fn merge_coalesces_overlapping_and_adjacent_ranges() {
            let mut a = Coverage::from_codepoints(vec![1, 2, 10]);
            a.merge(&Coverage::from_codepoints(vec![3, 4, 9]));
            assert_eq!(a, Coverage { ranges: vec![(1, 4), (9, 10)] });
        }

        #[test]
        fn novel_codepoint_detection() {
            let seed = Coverage::from_codepoints(vec![1, 2, 3, 4, 5]);
            assert!(!Coverage::from_codepoints(vec![2, 4]).has_novel_codepoint(&seed));
            assert!(Coverage::from_codepoints(vec![5, 6]).has_novel_codepoint(&seed));
            assert!(Coverage::from_codepoints(vec![100]).has_novel_codepoint(&seed));
            assert!(!Coverage::default().has_novel_codepoint(&seed));
            assert!(seed.has_novel_codepoint(&Coverage::default()));
        }

        #[test]
        fn trim_drops_subsumed_keeps_novel_respects_limit_in_order() {
            let seed = Coverage::from_codepoints((0x20u32..0x7f).collect());
            let candidates = vec![
                (cand("subsumed"), Coverage::from_codepoints(vec![0x41, 0x42])),
                (cand("nerd"), Coverage::from_codepoints(vec![0xE0A0, 0xE0B0])),
                (cand("nerd-dup"), Coverage::from_codepoints(vec![0xE0A0])),
                (cand("emoji"), Coverage::from_codepoints(vec![0x1F600])),
                (cand("cjk"), Coverage::from_codepoints(vec![0x4E00])),
            ];
            let kept = trim_by_coverage(candidates, &seed, 2);
            let names: Vec<_> = kept.iter().map(|c| c.family.as_str()).collect();
            assert_eq!(names, ["nerd", "emoji"]);
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree`
Expected: compilation FAILURE — `no function or associated item named from_codepoints`, `trim_by_coverage` not found.

- [ ] **Step 3: Implement inside `mod coverage`**

```rust
    impl Coverage {
        /// Build from an arbitrary codepoint list: sorted, deduped, and
        /// collapsed into inclusive, disjoint ranges.
        pub fn from_codepoints(mut codepoints: Vec<u32>) -> Self {
            codepoints.sort_unstable();
            codepoints.dedup();
            let mut ranges: Vec<(u32, u32)> = Vec::new();
            for cp in codepoints {
                match ranges.last_mut() {
                    Some((_, end)) if *end + 1 == cp => *end = cp,
                    _ => ranges.push((cp, cp)),
                }
            }
            Self { ranges }
        }

        pub fn merge(&mut self, other: &Coverage) {
            let mut merged: Vec<(u32, u32)> =
                Vec::with_capacity(self.ranges.len() + other.ranges.len());
            let push = |merged: &mut Vec<(u32, u32)>, range: (u32, u32)| match merged.last_mut() {
                Some((_, end)) if *end >= range.0.saturating_sub(1) => *end = (*end).max(range.1),
                _ => merged.push(range),
            };
            let (mut a, mut b) =
                (self.ranges.iter().copied().peekable(), other.ranges.iter().copied().peekable());
            while let (Some(&ra), Some(&rb)) = (a.peek(), b.peek()) {
                if ra.0 <= rb.0 {
                    push(&mut merged, ra);
                    a.next();
                } else {
                    push(&mut merged, rb);
                    b.next();
                }
            }
            for range in a {
                push(&mut merged, range);
            }
            for range in b {
                push(&mut merged, range);
            }
            self.ranges = merged;
        }

        /// True if `self` covers at least one codepoint that `other` doesn't —
        /// the FcFontSort(trim) keep-test.
        pub fn has_novel_codepoint(&self, other: &Coverage) -> bool {
            let mut i = 0;
            for &(start, end) in &self.ranges {
                let mut cp = start;
                loop {
                    while i < other.ranges.len() && other.ranges[i].1 < cp {
                        i += 1;
                    }
                    match other.ranges.get(i) {
                        Some(&(other_start, other_end)) if other_start <= cp => {
                            // Covered through other_end; resume past it.
                            if other_end >= end {
                                break;
                            }
                            cp = other_end + 1;
                        },
                        _ => return true,
                    }
                }
            }
            false
        }
    }

    /// Greedy trim mirroring FcFontSort(trim=true): walk candidates in order,
    /// keeping only faces that cover at least one codepoint the seed face and
    /// the already-kept faces don't.
    pub fn trim_by_coverage(
        candidates: Vec<(Candidate, Coverage)>,
        seed_coverage: &Coverage,
        limit: usize,
    ) -> Vec<Candidate> {
        let mut covered = seed_coverage.clone();
        let mut kept = Vec::new();
        for (candidate, coverage) in candidates {
            if kept.len() >= limit {
                break;
            }
            if coverage.has_novel_codepoint(&covered) {
                covered.merge(&coverage);
                kept.push(candidate);
            }
        }
        kept
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: PASS — 9 tests total, 0 failures.

- [ ] **Step 5: Format and commit**

```powershell
cargo fmt
git add alacritree/src/fonts.rs
git commit -m "feat(fonts): add coverage-trim selection logic"
```

---

### Task 5: Candidate ordering (pure logic)

**Files:**
- Modify: `alacritree/src/fonts.rs` (`mod coverage`: new `order_candidates` + test)

**Interfaces:**
- Consumes: `coverage::Candidate`, `coverage::Coverage` (Task 4).
- Produces: `coverage::order_candidates(candidates: &mut [(Candidate, Coverage)], family: &str, weight: u16, italic: bool)`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` inside `mod coverage` (a `cand2` helper that extends `cand`):

```rust
        fn cand2(family: &str, weight: u16, italic: bool, monospaced: bool) -> Candidate {
            Candidate { weight, italic, monospaced, ..cand(family) }
        }

        #[test]
        fn orders_family_then_style_then_monospace_then_name() {
            let mut candidates = vec![
                (cand2("Zeta", 400, false, false), Coverage::default()),
                (cand2("Beta", 400, false, true), Coverage::default()),
                (cand2("Alpha", 700, true, false), Coverage::default()),
                (cand2("Seed Family", 400, false, false), Coverage::default()),
                (cand2("Beta", 700, false, true), Coverage::default()),
            ];
            order_candidates(&mut candidates, "seed family", 700, false);
            let order: Vec<_> =
                candidates.iter().map(|(c, _)| (c.family.as_str(), c.weight)).collect();
            assert_eq!(order, [
                ("Seed Family", 400), // same family wins even without a style match
                ("Beta", 700),        // style match + monospace
                ("Beta", 400),        // monospace
                ("Alpha", 700),       // italic mismatches the variant; name order
                ("Zeta", 400),
            ]);
        }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alacritree`
Expected: compilation FAILURE — `cannot find function order_candidates`.

- [ ] **Step 3: Implement in `mod coverage`**

```rust
    /// Order candidates by fontconfig-like affinity to the seed face:
    /// same-family siblings, then weight/slant matches, then monospace, then
    /// everything else; ties break on family name, path, and face index so
    /// the resulting chain is deterministic across runs.
    pub fn order_candidates(
        candidates: &mut [(Candidate, Coverage)],
        family: &str,
        weight: u16,
        italic: bool,
    ) {
        candidates.sort_by(|(a, _), (b, _)| {
            let affinity = |c: &Candidate| {
                (
                    !c.family.eq_ignore_ascii_case(family),
                    !(c.weight == weight && c.italic == italic),
                    !c.monospaced,
                )
            };
            affinity(a)
                .cmp(&affinity(b))
                .then_with(|| a.family.cmp(&b.family))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.face_index.cmp(&b.face_index))
        });
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: PASS — 10 tests total, 0 failures.

- [ ] **Step 5: Format and commit**

```powershell
cargo fmt
git add alacritree/src/fonts.rs
git commit -m "feat(fonts): order fallback candidates"
```

---

### Task 6: Windows automatic fallback chain

**Files:**
- Modify: `alacritree/Cargo.toml` (add `ttf-parser`)
- Modify: `alacritree/src/fonts.rs` (`SystemFonts` gains the coverage cache; real `cfg(not(unix))` `gather_fallback_faces`; cmap helpers; smoke test)

**Interfaces:**
- Consumes: `SystemFonts`, `variant_query` (Task 2); `coverage::{Coverage, Candidate, trim_by_coverage, order_candidates}` (Tasks 4-5); existing `FallbackFace`, `MAX_FALLBACK_FACES`, `resolve_face`.
- Produces: working `gather_fallback_faces` on Windows — same signature as the unix version: `fn gather_fallback_faces(family: &str, style: Option<&str>, variant: Variant, skip_paths: &HashSet<PathBuf>, limit: usize, fonts: &SystemFonts) -> Vec<FallbackFace>`.

- [ ] **Step 1: Add the dependency**

In `alacritree/Cargo.toml` `[dependencies]`, after the `fontdb` line:

```toml
# Reads cmap tables to build coverage sets for the Windows fallback chain.
# Same major as the ttf-parser fontdb already pulls in, so no duplicate build.
ttf-parser = "0.25"
```

Run: `cargo check -p alacritree` — expected: clean; confirm `Cargo.lock` still lists a single `ttf-parser 0.25.x`.

- [ ] **Step 2: Write the failing smoke test**

The chain's selection logic is already unit-tested (Tasks 4-5); this test pins the integration invariants and passes even on a machine with no fonts (empty result is valid degradation).

Add to the `#[cfg(test)] mod tests` at the bottom of `fonts.rs`:

```rust
    #[cfg(not(unix))]
    #[test]
    fn windows_chain_respects_limit_skip_set_and_uniqueness() {
        let fonts = SystemFonts::default();
        let skip = HashSet::new();
        let faces = gather_fallback_faces("Consolas", None, Variant::Normal, &skip, 8, &fonts);
        assert!(faces.len() <= 8);
        let mut seen = HashSet::new();
        for face in &faces {
            assert!(!skip.contains(&face.path));
            assert!(seen.insert((face.path.clone(), face.face_index)));
        }
        // On any machine with system fonts the chain must not be empty —
        // that emptiness is the Windows-tofu bug this feature fixes.
        if fonts.db().faces().next().is_some() {
            assert!(!faces.is_empty());
        }
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alacritree windows_chain`
Expected: FAIL — `assert!(!faces.is_empty())` panics, because the `cfg(not(unix))` `gather_fallback_faces` still returns `Vec::new()` (RED for the actual bug: no fallback chain on Windows).

- [ ] **Step 4: Implement**

Extend `SystemFonts`:

```rust
#[derive(Default)]
struct SystemFonts {
    db: OnceCell<fontdb::Database>,
    #[cfg(not(unix))]
    coverage: OnceCell<Vec<(coverage::Candidate, coverage::Coverage)>>,
}
```

Add to `impl SystemFonts`:

```rust
    /// Scan every system face's cmap once per install; all four variant
    /// chains reorder and trim this shared list.
    #[cfg(not(unix))]
    fn scanned_coverage(&self) -> &[(coverage::Candidate, coverage::Coverage)] {
        self.coverage.get_or_init(|| {
            let started = std::time::Instant::now();
            let db = self.db();
            let mut scanned = Vec::new();
            for face in db.faces() {
                let (path, face_index) = match &face.source {
                    fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => {
                        (p.clone(), face.index)
                    },
                    // Embedded faces aren't path-addressable by our loader.
                    fontdb::Source::Binary(_) => continue,
                };
                let Some(cov) = db
                    .with_face_data(face.id, |data, index| {
                        let parsed = ttf_parser::Face::parse(data, index).ok()?;
                        cmap_coverage(&parsed)
                    })
                    .flatten()
                else {
                    log::debug!("skipping unparseable font {}", path.display());
                    continue;
                };
                let family =
                    face.families.first().map(|(name, _)| name.clone()).unwrap_or_default();
                scanned.push((
                    coverage::Candidate {
                        path,
                        face_index,
                        family,
                        weight: face.weight.0,
                        italic: face.style != fontdb::Style::Normal,
                        monospaced: face.monospaced,
                    },
                    cov,
                ));
            }
            log::info!(
                "scanned {} font faces for fallback coverage in {} ms",
                scanned.len(),
                started.elapsed().as_millis()
            );
            scanned
        })
    }
```

Add the cmap helpers near `gather_fallback_faces`:

```rust
#[cfg(not(unix))]
fn cmap_coverage(face: &ttf_parser::Face) -> Option<coverage::Coverage> {
    let cmap = face.tables().cmap?;
    let mut codepoints = Vec::new();
    for subtable in cmap.subtables {
        if !subtable.is_unicode() {
            continue;
        }
        subtable.codepoints(|cp| codepoints.push(cp));
    }
    Some(coverage::Coverage::from_codepoints(codepoints))
}

/// Coverage of an already-resolved primary face.  Reads index 0, matching
/// how the primary bytes are handed to egui.
#[cfg(not(unix))]
fn face_coverage_from_path(path: &Path) -> Option<coverage::Coverage> {
    let data = std::fs::read(path).ok()?;
    let parsed = ttf_parser::Face::parse(&data, 0).ok()?;
    cmap_coverage(&parsed)
}
```

Replace the `cfg(not(unix))` `gather_fallback_faces` stub:

```rust
/// The fontdb equivalent of fontconfig's coverage-trimmed FcFontSort: order
/// every system face by affinity to the seed, then keep only faces that add
/// codepoints the seed and earlier picks don't cover.
#[cfg(not(unix))]
fn gather_fallback_faces(
    family: &str,
    style: Option<&str>,
    variant: Variant,
    skip_paths: &HashSet<PathBuf>,
    limit: usize,
    fonts: &SystemFonts,
) -> Vec<FallbackFace> {
    let seed_coverage = resolve_face(family, style, variant, fonts)
        .and_then(|face| face_coverage_from_path(&face.path))
        .unwrap_or_default();

    let mut candidates: Vec<_> = fonts
        .scanned_coverage()
        .iter()
        .filter(|(candidate, _)| !skip_paths.contains(&candidate.path))
        .cloned()
        .collect();
    let (weight, db_style) = variant_query(variant);
    coverage::order_candidates(&mut candidates, family, weight.0, db_style != fontdb::Style::Normal);

    coverage::trim_by_coverage(candidates, &seed_coverage, limit)
        .into_iter()
        .map(|candidate| FallbackFace { path: candidate.path, face_index: candidate.face_index })
        .collect()
}
```

(Keep the `_fonts` parameter name change: it's now used, so rename to `fonts`. The unix version keeps `_fonts`.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: PASS — 11 tests, 0 failures. The smoke test exercises the full scan; a few seconds on a machine with hundreds of fonts is expected. Note the scan-duration figure isn't printed here (log capture); the timing check happens in Task 7.

- [ ] **Step 6: Format and commit**

```powershell
cargo fmt
git add alacritree/Cargo.toml Cargo.lock alacritree/src/fonts.rs
git commit -m "feat(fonts): add windows automatic fallback chain"
```

Body:

```
gather_fallback_faces on non-unix returned an empty list, so any glyph
missing from the primary face rendered as tofu (Nerd Font symbols, emoji,
CJK).  Enumerate the system fonts once via the shared fontdb database,
record each face's cmap coverage, order by affinity to the seed face, and
greedily keep faces that add uncovered codepoints — the same shape as
fontconfig's coverage-trimmed FcFontSort that Unix already relies on.
```

---

### Task 7: Verification (fmt, tests, manual GUI check, scan timing)

**Files:** none (verification only; a fix here belongs to the task that caused it).

- [ ] **Step 1: Full check**

```powershell
cargo fmt
git diff --exit-code        # expected: no output (fmt was already clean)
cargo test -p alacritree    # expected: 11 passed, 0 failed
cargo build -p alacritree --release
```

- [ ] **Step 2: Scan timing**

Run the release binary from a console with info logging and read the scan line:

```powershell
$env:RUST_LOG = "info"; & target\release\alacritree.exe
```

Expected log: `scanned N font faces for fallback coverage in X ms`. If X > ~100 ms, note it in the session summary — the spec defers a disk cache (keyed by path+mtime+size) to a follow-up commit on this branch; do not build it preemptively.

- [ ] **Step 3: Manual GUI verification (user-gated)**

This needs Lev's eyes; report readiness and ask him to check:

1. **Automatic chain (empty user list):** in the alacritree terminal run `echo "🚀 ✔ 你好 ▶ │"` — emoji and CJK render, no tofu boxes.
2. **User list:** add to `%APPDATA%\alacritty\alacritree.toml`:
   ```toml
   [font]
   fallback = ["Segoe UI Emoji"]
   ```
   (or a Nerd Font family if installed) — restart, confirm a starship prompt renders Nerd Font glyphs.
3. **Bad entry:** add `"no-such-font-xyz"` to the list — startup still works, one warning in the log.

- [ ] **Step 4: Linux regression check (user-gated)**

The unix code path only changed signatures (`fonts`/`book` threading). Lev runs `cargo check -p alacritree` in WSL himself once the branch is pushed to his fork — flag it as pending in the wrap-up, don't attempt it from this session.

- [ ] **Step 5: Wrap up**

Implementation done. Use superpowers:finishing-a-development-branch to decide merge/PR (per project convention: PR upstream to mathix420/alacritree; the PR description carries the spec context since specs are git-excluded). Update `docs/specs/planned_features.md` and the `alacritree-feature-workflow` memory with a status line (append, never rewrite others' entries).

---

### Task 8 (added during execution, 2026-07-12): normalize fallback glyph scale

Lev's GUI check found fallback glyphs render at the wrong visual size
(starship's rounded powerline caps overshoot the cell). egui instantiates
every family-list font at the same point size, but `(ascender − descender)
/ units_per_em` varies per font. Fix: register every fallback face with a
`FontTweak::scale` equal to the primary face's height ratio divided by its
own, clamped to `[0.5, 2.0]`; primaries stay untweaked as the reference.
Platform-neutral — deliberately also changes Unix fallback *rendering*
(same latent bug); Unix *resolution* untouched. Commit
`fix(fonts): normalize fallback glyph scale`.

### Measured during Task 7 (2026-07-12)

Coverage scan: `scanned 1282 font faces for fallback coverage in ~1490 ms`
(two consistent runs, release build). This exceeds the spec's ~100 ms bar,
so the spec-sanctioned disk cache (keyed by path+mtime+size) is now
justified as a follow-up commit on this branch — awaiting Lev's go-ahead.
Also observed: ~3.8 GB working set after startup (likely fontdb memmaps
touched during the cmap scan; would shrink with the disk cache) — worth a
look alongside the cache follow-up.

## Resolved questions (Lev, 2026-07-12)

1. **egui built-in font ordering:** keep user fallbacks after egui's built-ins in the `Proportional`/`Monospace` lists — matches today's automatic-chain behavior and the spec's "after the four primary faces".
2. **Per-entry style:** none; entries are resolved with each variant's weight/slant (YAGNI).
3. **Linux verification:** Lev runs the WSL `cargo check` himself after the branch is pushed to his fork.
