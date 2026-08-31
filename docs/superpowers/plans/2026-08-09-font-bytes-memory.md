# Stop Copying Font Files Into Memory — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `alacritree` from holding ~960 MB of redundant, never-evicted copies of font files it has already memory-mapped, and stop re-parsing fallback seed faces once per variant at startup.

**Architecture:** Three independent defects in the font path, fixed in one PR. (1) `color_glyph::load` reads whole font files into `Arc<Vec<u8>>` and never evicts them — it will borrow the `&'static [u8]` mapping `fonts::map_font_file` already holds instead. (2) `gather_fallback_faces` (Windows) resolves seed coverage through `face_coverage`, a whole-file `std::fs::read`, once per variant — it will consult a per-install memo, then the existing disk-cached coverage scan, and only then a mapped parse. (3) `ColorGlyphCache` never memoizes a character no face claims, and never uses the chain index it already records — both are fixed by restructuring one `match`.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), `swash` 0.2.9, `ttf_parser`, `memmap2`, `egui`/`epaint` 0.31.1, `fontdb`. Tests are in-module under `#[cfg(test)]`. Verification is a PowerShell + C# `VirtualQueryEx` region walk on Windows.

**Source spec:** `docs/superpowers/specs/2026-08-09-font-bytes-memory-design.md`
**Review log:** `docs/superpowers/specs/2026-08-09-font-bytes-memory-review-log.md` (5 rounds, Codex adversarial)

## Global Constraints

- Baseline commit: `upstream/master` at `3617fe53`. Branch off that, not `master` of any fork.
- Branch: `fix-font-bytes`, worktree at `../alacritree-worktrees/fix-font-bytes`.
- PR marker is `[11]`. Title: `fix(fonts): map font files instead of copying them [11]`.
- All edits live in `alacritree/`. `alacritty*/` and `egui-winit/` are read-only vendored crates.
- Every commit carries the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Conventional Commits, imperative subject, ≤72 chars.
- No config gate. Rendering and fallback order are unchanged; only memory and startup time move.
- Tests must pass at the **default** thread count, not only `--test-threads=1`. CI runs `cargo test -p alacritree --locked` on `ubuntu-latest` and `windows-2022`.
- Local release builds on this machine need `-j 1`; parallel rustc dies with `STATUS_STACK_BUFFER_OVERRUN` on cold builds of this workspace.
- Comments explain *why*, never *what*. No PR/issue/task references in comments.
- Nothing under `docs/superpowers/` is ever committed — it is in `.git/info/exclude`.
- Do **not** push or open a PR without an explicit instruction from the user.

## File Structure

| File | Change | Responsibility after the change |
|---|---|---|
| `alacritree/src/fonts.rs` | Modify | `map_font_file` becomes `pub(crate)`; `SystemFonts` gains a seed-coverage memo; `face_coverage` maps instead of reads; a `#[cfg(test)]` parse counter and `is_mapped` are added |
| `alacritree/src/color_glyph.rs` | Modify | `files` holds borrowed mappings; `get` consults the memo before walking the chain and records unclaimed characters; a `#[cfg(test)]` chain-walk counter is added |

No new files. No new dependencies.

## Deviations From The Spec

Two, both narrower than the spec, both deliberate:

1. `is_mapped` is a private `fn` in `fonts.rs` gated `#[cfg(all(test, not(unix)))]`, not `#[cfg(test)] pub(crate)`. Its only consumer is a Windows-gated test in `fonts.rs`'s own test module, so `pub(crate)` would widen visibility for nothing and an ungated `#[cfg(test)]` would be dead code on Linux.
2. Tests 1 and 4 write the fixture through `include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/alacritree-symbols.ttf"))`; tests 2b and 3 reuse the existing `write_parseable_font` helper in `fonts.rs`'s test module, which already produces a unique temp path from egui's bundled font. Both give a path no other test touches, which is the property `FONT_MAPS` being global and never cleared demands.

---

### Task 1: Worktree, branch, and the visibility change

Behaviour-free. It exists so the RED runs in Tasks 2 and 4 compile: `color_glyph.rs` cannot call `fonts::map_font_file` while it is private.

**Files:**
- Modify: `alacritree/src/fonts.rs` (the `map_font_file` signature, near line 1146)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn map_font_file(path: &Path) -> std::io::Result<&'static [u8]>` — callable from any module in the `alacritree` crate.

- [ ] **Step 1: Confirm the base has not moved, then create the worktree**

Every line anchor in this plan and in the spec was derived against `3617fe53`, so the worktree is cut from that commit rather than from a ref that may have advanced since.

```sh
git -C C:/Users/Lev/Git/github/alacritree fetch upstream
git -C C:/Users/Lev/Git/github/alacritree rev-parse --short upstream/master
```

If that prints anything other than `3617fe53`, **stop**: upstream has advanced, and both the branch base and every anchor in this plan need re-deriving before it is executable. If it prints `3617fe53`:

```sh
git -C C:/Users/Lev/Git/github/alacritree worktree add \
    ../alacritree-worktrees/fix-font-bytes -b fix-font-bytes 3617fe53
```

All remaining steps run in `C:/Users/Lev/Git/github/alacritree-worktrees/fix-font-bytes`.

- [ ] **Step 2: Widen `map_font_file`**

In `alacritree/src/fonts.rs`, change the signature only. The body and the doc comment above `FONT_MAPS` are untouched.

```rust
pub(crate) fn map_font_file(path: &Path) -> std::io::Result<&'static [u8]> {
```

- [ ] **Step 3: Verify the crate still builds and tests still pass**

Run: `cargo test -p alacritree`
Expected: PASS, same test count as before the change.

- [ ] **Step 4: Commit**

```sh
git add alacritree/src/fonts.rs
git commit -m "refactor(fonts): widen map_font_file visibility

The colour glyph cache reads each chain face's file into its own buffer
because it cannot reach the mapping fonts already holds.  Exposing the
mapper crate-wide is the prerequisite for borrowing it instead.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Borrow the mapping instead of copying the file

Fixes defect (a) — the 960 MB. `ColorGlyphCache::files` currently holds `Arc<Vec<u8>>` copies of every chain face it has ever consulted, and nothing evicts them: the cell-size reset clears `entries`/`used`/`bytes` only, and `evict_to_budget` touches `entries`/`used` only. The bytes are already resident as mappings.

**Files:**
- Modify: `alacritree/src/color_glyph.rs` (imports near line 20; the `files` field near line 51; `claiming_index` near line 144; `render` near line 173; `load` near line 251)
- Test: `alacritree/src/color_glyph.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::fonts::map_font_file(&Path) -> std::io::Result<&'static [u8]>` from Task 1.
- Produces: `fn load(files: &mut HashMap<PathBuf, Option<&'static [u8]>>, path: &Path) -> Option<&'static [u8]>` — a private free function in `color_glyph.rs`, used by Task 3's tests only indirectly.

- [ ] **Step 1: Add the shared test fixture constant**

At the top of `mod tests` in `alacritree/src/color_glyph.rs`, after `use crate::config::{FontConfig, UiFont};`:

```rust
    /// The crate's own baked face, written to a unique path per test.
    /// `fonts::FONT_MAPS` is global and never cleared, so a test that asserts
    /// something *about* mapping has to own the path it asserts on.
    const FIXTURE: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/alacritree-symbols.ttf"));
```

- [ ] **Step 2: Write the failing test**

Append to `mod tests` in `alacritree/src/color_glyph.rs`:

```rust
    /// Every chain face is already memory-mapped by `fonts::map_font_file`.
    /// Reading it again cost 960 MB of private memory on a chain whose primary
    /// is a 792 MB collection, so pointer identity — not equal contents — is
    /// what this has to assert.
    #[test]
    fn load_returns_the_mapping_rather_than_a_copy() {
        let path = std::env::temp_dir().join("alacritree_test_color_glyph_load.ttf");
        std::fs::write(&path, FIXTURE).unwrap();

        let mapped = crate::fonts::map_font_file(&path).expect("the fixture maps");
        let mut files = HashMap::new();
        let loaded = load(&mut files, &path).expect("the fixture loads");

        assert!(
            std::ptr::eq(mapped.as_ptr(), loaded.as_ptr()),
            "load copied the file instead of borrowing the mapping"
        );
    }
```

This compiles against both the old and the new type: `Arc<Vec<u8>>` derefs to `Vec<u8>`, which also has `as_ptr()`.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p alacritree color_glyph::tests::load_returns_the_mapping_rather_than_a_copy`
Expected: FAIL — `load copied the file instead of borrowing the mapping`. The `Arc<Vec<u8>>` is a fresh heap buffer at an unrelated address.

- [ ] **Step 4: Change the field type and drop the now-unused import**

In `alacritree/src/color_glyph.rs`, delete line 20 (`use std::sync::Arc;`) — `Arc` has no other use in this file — and change the `files` field and its doc comment:

```rust
    /// Font files behind the chain, borrowed from the mappings `fonts` already
    /// holds.  A `None` marks a file that would not map, so a broken font is
    /// not retried on every cache miss.
    files: HashMap<PathBuf, Option<&'static [u8]>>,
```

- [ ] **Step 5: Rewrite `load`**

Replace the whole function and its doc comment (near line 248):

```rust
/// Borrow the mapping `fonts::map_font_file` already holds for the face rather
/// than reading the file again: the chain's faces are handed to egui as
/// mappings, and a second owned copy of a 792 MB collection is 792 MB of
/// private memory that nothing evicts.
fn load(
    files: &mut HashMap<PathBuf, Option<&'static [u8]>>,
    path: &Path,
) -> Option<&'static [u8]> {
    *files.entry(path.to_path_buf()).or_insert_with(|| match crate::fonts::map_font_file(path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            // Every face in the chain mapped during install and `FONT_MAPS`
            // never evicts, so arriving here means the two have diverged.
            log::warn!("colour font {} is in the chain but will not map: {e}", path.display());
            None
        },
    })
}
```

- [ ] **Step 6: Update the two call sites to pass the slice directly**

The returned reference is `'static`, so the mutable borrow of `self.files` ends at the call and `self.chain` / `self.scale` stay usable. Passing `data` rather than `&data` avoids a pointless `&&[u8]`.

In `claiming_index` (near line 147):

```rust
            let claims = FontRef::from_index(data, face.face_index as usize)
                .is_some_and(|font| font.charmap().map(c) != 0);
```

In `render` (near line 174):

```rust
        let font = FontRef::from_index(data, face.face_index as usize)?;
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p alacritree color_glyph::`
Expected: PASS — seven tests, the six pre-existing ones plus the new one.

- [ ] **Step 8: Commit**

```sh
git add alacritree/src/color_glyph.rs
git commit -m "fix(color_glyph): borrow mapped font bytes

The colour glyph cache read every chain face it consulted into an
Arc<Vec<u8>> and kept it forever: neither the cell-size reset nor
evict_to_budget touches that map, and the glyph budget accounts only for
rasterized textures.  With a 792 MB primary and five 33 MB CJK fallbacks
that is 960 MB of private memory before the first keystroke, duplicating
mappings the process already holds.

The first painted ASCII character triggers it — the cache is consulted for
every non-space, non-box glyph, and a miss walks the chain from index 0.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Make the memo answer

Fixes defect (c). Two separate misses in one code path:

- `let index = self.claiming_index(c)?;` propagates `None` and returns *before* `self.source.insert(c, None)`, so a character no face claims is never recorded and re-walks the whole chain on every frame it is on screen.
- The `Some(index)` recorded in `source` is never read. After `evict_to_budget` drops a glyph, the next lookup falls through to `claiming_index` and rediscovers the same face by re-parsing every earlier face's cmap.

**Files:**
- Modify: `alacritree/src/color_glyph.rs` (the `source` doc comment near line 52; a new field near line 63; `new()` near line 67; `get` lines 108–121; `claiming_index` near line 141)
- Test: `alacritree/src/color_glyph.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `FIXTURE`, `metrics()`, and `chain_with_color_fonts(&Context) -> Option<Vec<ChainFace>>` from the existing test module; `ChainFace { path, face_index, color_only }` from `crate::fonts`.
- Produces: `ColorGlyphCache::chain_walks: usize` under `#[cfg(test)]` — read by Task 3's own test only.

- [ ] **Step 1: Add the test-only chain-walk counter (scaffolding)**

The counter cannot exist at `3617fe53`, so it goes in before the RED run. The RED is about the memo, not about the counter.

Add the field to `ColorGlyphCache`, after `scale: ScaleContext` (near line 63):

```rust
    /// Chain walks performed, so a test can prove a post-eviction re-render
    /// answers from `source` instead of re-parsing every earlier face's cmap.
    /// `usize` rather than an atomic because `claiming_index` takes `&mut self`.
    #[cfg(test)]
    chain_walks: usize,
```

Add the initializer to `new()`, after `scale: ScaleContext::new(),`:

```rust
            #[cfg(test)]
            chain_walks: 0,
```

Increment it at the top of `claiming_index`:

```rust
    fn claiming_index(&mut self, c: char) -> Option<usize> {
        #[cfg(test)]
        {
            self.chain_walks += 1;
        }
        for i in 0..self.chain.len() {
```

- [ ] **Step 2: Write the two failing tests**

Append to `mod tests` in `alacritree/src/color_glyph.rs`:

```rust
    /// A character no face in the chain claims must be recorded as egui's, or
    /// the whole chain is re-walked for it on every frame it is on screen.
    #[test]
    fn an_unclaimed_character_is_memoized() {
        let ctx = Context::default();
        let path = std::env::temp_dir().join("alacritree_test_unclaimed_memo.ttf");
        std::fs::write(&path, FIXTURE).unwrap();
        let chain = vec![ChainFace { path, face_index: 0, color_only: false }];
        let mut cache = ColorGlyphCache::new(chain, 10);

        // The baked symbols face carries box drawing and chrome glyphs only.
        let unclaimed = '\u{4E00}';
        assert!(
            cache.resolve_claiming_face(unclaimed).is_none(),
            "the fixture claims U+4E00; this test would prove nothing"
        );

        assert!(cache.get(&ctx, unclaimed, &metrics(), 1).is_none());
        assert_eq!(
            cache.source.get(&unclaimed),
            Some(&None),
            "an unclaimed character was not recorded, so every frame re-walks the chain"
        );
    }

    /// After an eviction the glyph must be re-rasterized from the chain index
    /// already recorded for it, not rediscovered by re-parsing the chain.
    #[test]
    fn a_post_eviction_rerender_skips_the_chain_walk() {
        let ctx = Context::default();
        let Some(chain) = chain_with_color_fonts(&ctx) else {
            log::warn!("no colour emoji font installed; nothing to assert");
            return;
        };

        // `chain_with_color_fonts` proves renderability for U+1F600 alone, so
        // the second glyph is found rather than assumed.  A throwaway cache
        // keeps the probe out of the cache under test.
        let mut probe = ColorGlyphCache::new(chain.clone(), 10);
        let renderable: Vec<char> = ['\u{1F600}', '\u{1F601}', '\u{2764}', '\u{1F44D}']
            .into_iter()
            .filter(|c| probe.get(&ctx, *c, &metrics(), 2).is_some())
            .collect();
        if renderable.len() < 2 {
            log::warn!("fewer than two renderable colour glyphs here; nothing to assert");
            return;
        }
        let (first, second) = (renderable[0], renderable[1]);

        // One byte of budget: each insert evicts everything but itself.
        let mut cache = ColorGlyphCache { budget: 1, ..ColorGlyphCache::new(chain, 0) };
        assert!(cache.get(&ctx, first, &metrics(), 2).is_some(), "first glyph did not rasterize");
        assert!(cache.get(&ctx, second, &metrics(), 2).is_some(), "second glyph did not rasterize");
        assert!(!cache.entries.contains_key(&first), "the first glyph was not evicted");

        let walks = cache.chain_walks;
        assert!(cache.get(&ctx, first, &metrics(), 2).is_some(), "re-render after eviction failed");
        assert_eq!(cache.chain_walks, walks, "the chain was re-walked after an eviction");
    }
```

- [ ] **Step 3: Run both tests to verify they fail**

`cargo test` takes a single positional filter — a second one is rejected as an unexpected argument — so run them one at a time:

```sh
cargo test -p alacritree color_glyph::tests::an_unclaimed_character_is_memoized
cargo test -p alacritree color_glyph::tests::a_post_eviction_rerender_skips_the_chain_walk
```

Expected:
- `an_unclaimed_character_is_memoized` FAILS on the `assert_eq!` — `source.get(&unclaimed)` is `None`, because the `?` on `claiming_index` returns before the insert.
- `a_post_eviction_rerender_skips_the_chain_walk` FAILS on the final `assert_eq!` — the re-request falls through to `claiming_index`, so the counter increased. On a machine with no colour emoji font this test reports PASS by early return; that is stated in the spec and is why it is not the only test for this task.

- [ ] **Step 4: Restructure the lookup in `get`**

Step 1 inserted lines above this point, so match on source text rather than line numbers. In `alacritree/src/color_glyph.rs`, find this exact block inside `get` — it sits between the `entries.contains_key` early return and `let face = self.chain[index].clone();`:

```rust
        // A character already known to have no colour artwork costs one lookup;
        // the whole grid takes this path on every frame.
        if self.source.get(&c) == Some(&None) {
            return None;
        }

        // Only the claiming face is considered.  Looking further down the chain
        // would rasterize from a font egui had already passed over, so the two
        // renderers would disagree about which face owns the character.
        let index = self.claiming_index(c)?;
```

and replace all of it with:

```rust
        // Only the claiming face is considered.  Looking further down the chain
        // would rasterize from a font egui had already passed over, so the two
        // renderers would disagree about which face owns the character.
        let index = match self.source.get(&c) {
            // Known monochrome: egui's own glyph pipeline draws it.  The whole
            // grid takes this path on every frame, so it costs one lookup.
            Some(None) => return None,
            // Re-render after a budget eviction.  The chain is fixed at
            // construction, so the recorded index still names the same face.
            Some(Some(i)) => *i,
            None => match self.claiming_index(c) {
                Some(i) => i,
                None => {
                    self.source.insert(c, None);
                    return None;
                },
            },
        };
```

- [ ] **Step 5: Update the `source` doc comment**

Near line 52, so the recorded index reads as load-bearing:

```rust
    /// Which chain entry, if any, draws this character in colour.  `None` means
    /// egui's own glyph pipeline owns it.  The index is what a re-render after
    /// a budget eviction uses instead of walking the chain again.
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree color_glyph::`
Expected: PASS, including `the_cache_evicts_down_to_its_budget`, `resizing_the_cell_clears_the_cache` and `plain_text_is_left_to_egui` — the restructure must not change `entries`, `used`, byte budgeting, eviction or the cell-size reset.

- [ ] **Step 7: Commit**

```sh
git add alacritree/src/color_glyph.rs
git commit -m "fix(color_glyph): memoize the chain lookup

A character no chain face claims returned through the ? on claiming_index,
before the line that records it, so it re-walked and re-parsed the whole
chain on every frame it was on screen.

The chain index recorded for a claimed character was written and never
read, so a glyph dropped by evict_to_budget was rediscovered the same
expensive way.  Consulting the memo first turns both into one lookup.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Stop re-reading the fallback seed

Fixes defect (b). Windows only — the `#[cfg(unix)]` `gather_fallback_faces` resolves coverage through fontconfig and never calls `face_coverage`. `install_terminal_fonts` runs four variant seeds and then `install_ui_font` reaches `gather_fallback_faces` again, so up to five whole-file reads of the primary (792 MB each on this machine) happen before the window opens.

Seed coverage resolves in three steps, cheapest first: a per-install memo on `SystemFonts`, then the disk-cached `scanned_coverage`, then a mapped parse.

**Files:**
- Modify: `alacritree/src/fonts.rs` (imports near line 16; `SystemFonts` near line 106; `impl SystemFonts` near line 114; `face_coverage` near line 1087; `gather_fallback_faces` near line 1105; `FONT_MAPS` near line 1144)
- Test: `alacritree/src/fonts.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `ResolvedFace { path: PathBuf, face_index: u32 }`; `coverage::Coverage` (`Clone + Default + PartialEq`); `coverage::Candidate { path, face_index, family, weight, italic, monospaced, bytes }`; `SystemFonts::scanned_coverage(&self) -> &[(coverage::Candidate, coverage::Coverage)]`; `SystemFonts::with_cache_dir(Option<PathBuf>) -> Self`; `write_parseable_font(&str) -> PathBuf` from the test module; `map_font_file` from Task 1.
- Produces: `SystemFonts::seed_coverage(&self, face: &ResolvedFace) -> Option<coverage::Coverage>` and `fn scanned_seed_coverage(fonts: &SystemFonts, face: &ResolvedFace) -> Option<coverage::Coverage>`, both `#[cfg(not(unix))]`.

- [ ] **Step 1: Add the test-only observability (scaffolding)**

Neither the counter nor `is_mapped` exists at `3617fe53`, so both go in before the RED run.

In `alacritree/src/fonts.rs`, immediately above `fn face_coverage` (near line 1084):

```rust
#[cfg(all(not(unix), test))]
thread_local! {
    /// Per-thread because the Windows fallback tests call
    /// `gather_fallback_faces` concurrently at the default thread count, and a
    /// process-wide count would fold their parses into whichever test asserts.
    static FACE_COVERAGE_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(all(not(unix), test))]
fn reset_face_coverage_parses() {
    FACE_COVERAGE_PARSES.with(|n| n.set(0));
}

#[cfg(all(not(unix), test))]
fn face_coverage_parses() -> usize {
    FACE_COVERAGE_PARSES.with(|n| n.get())
}
```

Add the increment as the first statement of `face_coverage` (near line 1088):

```rust
fn face_coverage(path: &Path, face_index: u32) -> Option<coverage::Coverage> {
    #[cfg(test)]
    FACE_COVERAGE_PARSES.with(|n| n.set(n.get() + 1));
    let data = std::fs::read(path).ok()?;
```

And immediately after `fn map_font_file` (near line 1164):

```rust
/// Whether `path` already has a mapping.  Test-only: nothing in the app needs
/// to ask, and a release build should not carry the lookup.  Gated on
/// `not(unix)` because its only caller is Windows-gated; otherwise this
/// test-only helper is dead code on Linux.
#[cfg(all(test, not(unix)))]
fn is_mapped(path: &Path) -> bool {
    FONT_MAPS.get().is_some_and(|maps| {
        maps.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(path)
    })
}
```

- [ ] **Step 2: Write the three failing tests**

Append to `mod tests` in `alacritree/src/fonts.rs`:

```rust
    /// A seed the coverage scan already carries must not be parsed again — the
    /// scan is disk-cached across launches and the parse is a whole-file read.
    #[cfg(not(unix))]
    #[test]
    fn a_seed_present_in_the_scan_is_not_parsed_again() {
        let fonts = SystemFonts::with_cache_dir(None);
        let Some(seed) = resolve_face("Consolas", None, Variant::Normal, &fonts) else {
            log::warn!("Consolas is not installed; nothing to assert");
            return;
        };
        // A seed missing from the scan would pass this by falling through, so
        // its presence is the precondition, not an assumption.
        assert!(
            fonts.scanned_coverage().iter().any(|(candidate, _)| candidate.path == seed.path
                && candidate.face_index == seed.face_index),
            "the seed is absent from the scan; this test would prove nothing"
        );

        reset_face_coverage_parses();
        let skip = HashSet::new();
        gather_fallback_faces("Consolas", None, Variant::Normal, &skip, 8, &fonts);

        assert_eq!(face_coverage_parses(), 0, "a seed already in the scan was parsed anyway");
    }

    /// A seed the scan cannot answer for is parsed at most once per install,
    /// however many variant chains ask for it.
    #[cfg(not(unix))]
    #[test]
    fn a_seed_outside_the_scan_is_parsed_once_per_install() {
        let fonts = SystemFonts::with_cache_dir(None);
        let path = write_parseable_font("alacritree_test_seed_memo.ttf");
        let family = path.to_str().expect("the temp path is utf-8");
        let seed = resolve_face(family, None, Variant::Normal, &fonts).expect("a path resolves");
        // An explicit path is not automatically outside the scan: a system
        // font's own path resolves the same way, and the scan contains it.
        assert!(
            !fonts.scanned_coverage().iter().any(|(candidate, _)| candidate.path == seed.path
                && candidate.face_index == seed.face_index),
            "the fixture is in the scan; this test would prove nothing"
        );

        reset_face_coverage_parses();
        let skip = HashSet::new();
        gather_fallback_faces(family, None, Variant::Normal, &skip, 8, &fonts);
        gather_fallback_faces(family, None, Variant::Normal, &skip, 8, &fonts);

        assert_eq!(face_coverage_parses(), 1, "the seed was re-parsed for the second chain");
    }

    /// The fallback parse borrows the mapping like every other font read in
    /// this module, rather than pulling a whole collection onto the heap.
    #[cfg(not(unix))]
    #[test]
    fn face_coverage_maps_the_file_instead_of_reading_it() {
        let path = write_parseable_font("alacritree_test_face_coverage_maps.ttf");
        assert!(!is_mapped(&path), "the fixture path must be untouched by other tests");

        let _ = face_coverage(&path, 0);

        assert!(is_mapped(&path), "face_coverage read the file instead of mapping it");
    }
```

- [ ] **Step 3: Run the three tests to verify they fail**

One positional filter per invocation — `cargo test` rejects a second as an unexpected argument:

```sh
cargo test -p alacritree fonts::tests::a_seed_present_in_the_scan_is_not_parsed_again
cargo test -p alacritree fonts::tests::a_seed_outside_the_scan_is_parsed_once_per_install
cargo test -p alacritree fonts::tests::face_coverage_maps_the_file_instead_of_reading_it
```

Expected:
- `a_seed_present_in_the_scan_is_not_parsed_again` FAILS: `1` parses, not `0`.
- `a_seed_outside_the_scan_is_parsed_once_per_install` FAILS: `2` parses, not `1`.
- `face_coverage_maps_the_file_instead_of_reading_it` FAILS on the second assertion: the baseline `face_coverage` never touches `FONT_MAPS`.

On Linux all three are compiled out; that is expected and is why the CI matrix runs both.

- [ ] **Step 4: Add the memo field**

In `alacritree/src/fonts.rs`, add a gated import below the existing `use std::cell::OnceCell;` at line 16. It must carry the gate: the only use of `RefCell` is the `#[cfg(not(unix))]` field, so an unconditional import is an unused import on Linux.

```rust
#[cfg(not(unix))]
use std::cell::RefCell;
```

Then add the field to `SystemFonts` (near line 111):

```rust
    /// `RefCell` rather than `OnceCell` because the map is keyed and grows;
    /// `&self` access matches `db` and `coverage`.
    #[cfg(not(unix))]
    seed_coverage: RefCell<HashMap<(PathBuf, u32), Option<coverage::Coverage>>>,
```

`SystemFonts` derives `Default` and `with_cache_dir` builds with `..Self::default()`, so neither needs a change.

- [ ] **Step 5: Add the memo method**

Inside `impl SystemFonts`, after `scanned_coverage` (near line 148):

```rust
    /// Coverage of a resolved seed face, computed at most once per install.
    /// The four variant seeds and the UI family commonly resolve to the same
    /// one or two files, and a miss is cached too so an unresolvable seed is
    /// not retried once per variant.
    #[cfg(not(unix))]
    fn seed_coverage(&self, face: &ResolvedFace) -> Option<coverage::Coverage> {
        let key = (face.path.clone(), face.face_index);
        if let Some(hit) = self.seed_coverage.borrow().get(&key) {
            return hit.clone();
        }
        // The borrow above is released here, so the fallback parse cannot
        // panic against the borrow_mut below.
        let computed = scanned_seed_coverage(self, face)
            .or_else(|| face_coverage(&face.path, face.face_index));
        self.seed_coverage.borrow_mut().insert(key, computed.clone());
        computed
    }
```

- [ ] **Step 6: Add the scan lookup**

In `alacritree/src/fonts.rs`, immediately above `fn face_coverage`. It needs the same `#[cfg(not(unix))]` as `scanned_coverage`, or the Unix build breaks:

```rust
/// The scan already carries every system face and is disk-cached across
/// launches, so a seed found here costs no parse at all.  `Candidate` carries
/// both path and face index, so the match is exact — face 0 of a collection
/// file can be an unrelated family.
#[cfg(not(unix))]
fn scanned_seed_coverage(
    fonts: &SystemFonts,
    face: &ResolvedFace,
) -> Option<coverage::Coverage> {
    fonts
        .scanned_coverage()
        .iter()
        .find(|(candidate, _)| {
            candidate.path == face.path && candidate.face_index == face.face_index
        })
        .map(|(_, coverage)| coverage.clone())
}
```

- [ ] **Step 7: Switch `face_coverage` to the mapping**

Step 1 already made the counter the first statement of this function, so match on source text. Replace exactly these two lines inside `face_coverage`, leaving the counter above them and `cmap_coverage(&parsed)` below them untouched:

```rust
    let data = std::fs::read(path).ok()?;
    let parsed = ttf_parser::Face::parse(&data, face_index).ok()?;
```

with:

```rust
    let data = map_font_file(path).ok()?;
    let parsed = ttf_parser::Face::parse(data, face_index).ok()?;
```

The doc comment above the function is unchanged; add nothing that restates the code.

- [ ] **Step 8: Route the seed through the memo**

In the `#[cfg(not(unix))]` `gather_fallback_faces` (near line 1105):

```rust
    let seed_coverage = resolve_face(family, style, variant, fonts)
        .and_then(|face| fonts.seed_coverage(&face))
        .unwrap_or_default();
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p alacritree fonts::`
Expected: PASS, including `windows_chain_respects_limit_skip_set_and_uniqueness`, `later_variants_reuse_faces_loaded_by_an_earlier_chain` and the disk-cache tests — the memo must not change which faces the chain produces.

- [ ] **Step 10: Commit**

```sh
git add alacritree/src/fonts.rs
git commit -m "perf(fonts): stop re-reading the fallback seed

The Windows fallback chain resolved its seed face's coverage with a
whole-file read, once for each of the four variant seeds and again for the
UI family.  With a 792 MB primary that is most of the time before the
window opens, and it duplicates a mapping the process already holds.

Seed coverage now resolves cheapest-first: a per-install memo, then the
disk-cached coverage scan, then a mapped parse.  Misses are memoized too,
so an unresolvable seed is not retried once per variant.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Verification Appendix — full check and acceptance measurement

Not a task: it changes no source and produces no commit. The unit tests prove the mechanism; only the region walk proves the megabytes. This appendix produces the numbers the PR description carries, and it runs once, after Task 4's commit.

**Files:**
- No source changes. Every artefact is written outside the worktree, so `git status` stays clean.

**Interfaces:**
- Consumes: the `fix-font-bytes` worktree at HEAD, and the unmodified checkout at `C:\Users\Lev\Git\github\alacritree` (`3617fe53`) as the baseline.
- Produces: `baseline-dupes.txt`, `baseline-mapped.txt`, one `*-allocs.csv` per memory sample, and a summary of six memory and twelve startup samples.

- [ ] **Step 1: Run the full local gate**

```sh
cargo fmt --check
cargo clippy -p alacritree --all-targets --locked --no-deps
cargo test -p alacritree
```

Expected: all three clean, `cargo test` at the default thread count.

The clippy invocation matches `.github/workflows/ci.yml:41` exactly. `--no-deps` is not optional: the vendored `alacritty_terminal` is `#![deny(clippy::all)]`, so linting dependencies fails on any toolchain that has added a lint since it was vendored — a failure that says nothing about this change.

- [ ] **Step 2: Build both release binaries and pin the paths**

```sh
cargo build -p alacritree --release -j 1   # run in the worktree      -> revised
cargo build -p alacritree --release -j 1   # run in the main checkout -> baseline
```

`-j 1` is not optional: parallel rustc dies with `STATUS_STACK_BUFFER_OVERRUN` on cold builds of this workspace. The two checkouts have separate `target/` directories, so neither binary overwrites the other.

```powershell
$revised  = "C:\Users\Lev\Git\github\alacritree-worktrees\fix-font-bytes\target\release\alacritree.exe"
$baseline = "C:\Users\Lev\Git\github\alacritree\target\release\alacritree.exe"
$scratch  = Join-Path $env:TEMP 'alacritree-font-bytes-measurement'
New-Item -ItemType Directory -Force $scratch | Out-Null
```

Every artefact goes under `$scratch`. A relative `Set-Content` would drop untracked files into whichever checkout is the current directory.

- [ ] **Step 3: Load the region-walk type and the shared helpers**

Run once per PowerShell session.

```powershell
$src = @'
using System; using System.Runtime.InteropServices; using System.Collections.Generic;
public class VM {
  [StructLayout(LayoutKind.Sequential)]
  public struct MBI { public IntPtr BaseAddress; public IntPtr AllocationBase; public uint AllocationProtect;
    public IntPtr RegionSize; public uint State; public uint Protect; public uint Type; }
  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint a, bool i, int pid);
  [DllImport("kernel32.dll")] public static extern IntPtr VirtualQueryEx(IntPtr h, IntPtr a, out MBI m, IntPtr l);
  [DllImport("psapi.dll", CharSet=CharSet.Unicode)] public static extern uint GetMappedFileNameW(IntPtr h, IntPtr a, System.Text.StringBuilder n, uint sz);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern uint QueryDosDeviceW(string dev, System.Text.StringBuilder target, uint max);
  public static string Dump(int pid) {
    IntPtr h = OpenProcess(0x0400|0x0010, false, pid);
    if (h == IntPtr.Zero) return "open failed";
    var sb = new System.Text.StringBuilder(); var byAlloc = new Dictionary<long, long[]>();
    long priv=0, mapped=0; IntPtr addr = IntPtr.Zero; MBI m; int sz = Marshal.SizeOf(typeof(MBI));
    while (VirtualQueryEx(h, addr, out m, (IntPtr)sz) != IntPtr.Zero) {
      long size = (long)m.RegionSize;
      if (m.State == 0x1000) {
        long ab = (long)m.AllocationBase;
        if (!byAlloc.ContainsKey(ab)) byAlloc[ab] = new long[2];
        if (m.Type == 0x20000) { byAlloc[ab][0] += size; priv += size; }
        else if (m.Type == 0x40000) { byAlloc[ab][1] += size; mapped += size; }
      }
      long next = (long)m.BaseAddress + size;
      if (next <= (long)addr) break;
      addr = (IntPtr)next; if (next > 0x7FFFFFFF0000L) break;
    }
    sb.AppendLine(String.Format("TOTALS,{0},{1}", priv, mapped));
    foreach (var kv in byAlloc) {
      string name = "";
      if (kv.Value[1] > 0) { var nb = new System.Text.StringBuilder(1024);
        if (GetMappedFileNameW(h,(IntPtr)kv.Key,nb,1024) > 0) name = nb.ToString(); }
      sb.AppendLine(String.Format("ALLOC,0x{0:X},{1},{2},{3}", kv.Key, kv.Value[0], kv.Value[1], name));
    }
    return sb.ToString();
  }
}
'@
Add-Type -TypeDefinition $src -Language CSharp
```

Every allocation is emitted with exact byte counts and no size floor, because the criteria are stated in bytes and one lost 33 MB mapping must stay visible.

`GetMappedFileNameW` returns `\Device\HarddiskVolume4\Windows\Fonts\…`, so paths are canonicalized before any comparison — a basename match would let a same-named font under another directory produce a false pass.

```powershell
$dosByDevice = @{}
foreach ($d in (Get-CimInstance Win32_Volume | Where-Object DriveLetter)) {
    $target = New-Object Text.StringBuilder 260
    [void][VM]::QueryDosDeviceW($d.DriveLetter, $target, 260)
    $dosByDevice[$target.ToString()] = $d.DriveLetter
}
function Canonical($devicePath) {
    if (-not $devicePath) { return "" }
    foreach ($k in $dosByDevice.Keys) {
        if ($devicePath.StartsWith($k, 'OrdinalIgnoreCase')) {
            return ($dosByDevice[$k] + $devicePath.Substring($k.Length)).ToLowerInvariant()
        }
    }
    return $devicePath.ToLowerInvariant()
}

$FontExt = '\.(ttf|ttc|otf|otc)$'

function Median($values) { $s = @($values | Sort-Object); $s[[int](($s.Count - 1) / 2)] }
function Spread($values) { $s = @($values | Sort-Object); "$($s[0])-$($s[-1])" }
```

One readiness gate serves both harnesses. It gates on the **exit code**, never on stdout being non-empty: in JSON mode a failure prints `{"error": …}` to stdout with exit 1 (`cli/mod.rs:250`), so a truthiness test would treat an unavailable socket as readiness and report a startup time near zero. It is bounded and checks liveness, so a crashed or hung instance fails the sample instead of spinning forever.

```powershell
function Wait-Ready($exe, $proc, $timeoutMs = 120000) {
    $sock = "\\.\pipe\alacritree-$($proc.Id).sock"
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        if ($proc.HasExited) { throw "alacritree exited before answering IPC (code $($proc.ExitCode))" }
        if ($clock.ElapsedMilliseconds -gt $timeoutMs) { throw "no IPC answer within $timeoutMs ms" }
        & $exe --socket $sock --json session list 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { return $sock }
        Start-Sleep -Milliseconds 50
    }
}
```

Addressing the socket by the launched pid, rather than letting the client discover an instance, stops a second alacritree from answering for the one being measured. `ipc_socket` must stay enabled for every run — the poll is the stop signal.

- [ ] **Step 4: Define the two sample harnesses**

```powershell
function Measure-Memory($exe, $label, $scratch) {
    $p = Start-Process $exe -PassThru
    try {
        $sock = Wait-Ready $exe $p
        # The window opens with one session; the stated condition is two.
        & $exe --socket $sock session create | Out-Null
        $n = (& $exe --socket $sock --json session list | ConvertFrom-Json).sessions.Count
        if ($n -ne 2) { throw "expected 2 sessions, found $n" }
        Start-Sleep -Seconds 60
        # `@()` is load-bearing: a one-line result is a scalar string, so
        # `$rows[0]` would index into the string and yield a character, and an
        # empty result would be `$null`, which cannot be indexed at all.
        $rows = @([VM]::Dump($p.Id) -split "`r?`n" | Where-Object { $_ })
    } finally {
        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    }

    # The allocation base is kept: it is the identity that makes duplicate
    # pairing one-to-one in Step 6.
    $allocs = @($rows | Where-Object { $_ -like "ALLOC,*" } | ForEach-Object {
        $f = $_ -split ',', 5
        [pscustomobject]@{ Base = $f[1]; Priv = [long]$f[2]; Mapped = [long]$f[3]; Path = Canonical $f[4] }
    })

    # `VM::Dump` returns the string "open failed" when OpenProcess is refused.
    # Treated as data that produces zero totals, one refused sample would hide
    # inside the median of the other two.
    $totalRows = @($rows | Where-Object { $_ -like "TOTALS,*" })
    if ($totalRows.Count -ne 1) { throw "$label: VM::Dump produced no TOTALS row (got '$($rows[0])')" }
    if ($allocs.Count -lt 1)    { throw "$label: VM::Dump produced no ALLOC rows" }
    $totals = $totalRows[0] -split ','
    if ([long]$totals[1] -le 0 -or [long]$totals[2] -le 0) {
        throw "$label: committed totals were private=$($totals[1]) mapped=$($totals[2]); the walk failed"
    }

    $allocs | Export-Csv (Join-Path $scratch "$label-allocs.csv") -NoTypeInformation
    [pscustomobject]@{
        Label = $label; PrivateTotal = [long]$totals[1]; MappedTotal = [long]$totals[2]; Allocs = $allocs
    }
}

$CachePath = "$env:LOCALAPPDATA\alacritree\coverage-cache.v1.bin"

# A suppressed removal error would leave the previous binary's cache in place
# and relabel a warm launch as cold, so every deletion asserts absence.
function Clear-CoverageCache {
    Remove-Item $CachePath -Force -ErrorAction SilentlyContinue
    if (Test-Path $CachePath) { throw "the coverage cache at $CachePath could not be deleted" }
}

function Measure-Startup($exe, $cold) {
    if ($cold) {
        Clear-CoverageCache
    } elseif (-not (Test-Path $CachePath)) {
        throw "no coverage cache present; prime the warm series with this binary first"
    }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p  = Start-Process $exe -PassThru
    try { Wait-Ready $exe $p | Out-Null; $sw.Stop() }
    finally { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
    $sw.ElapsedMilliseconds
}

# One discarded launch that writes the cache, so a warm series is warm on a
# cache the binary under test produced rather than on whatever was left behind.
function Initialize-WarmCache($exe) {
    Clear-CoverageCache
    $p = Start-Process $exe -PassThru
    try { Wait-Ready $exe $p | Out-Null }
    finally { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
    if (-not (Test-Path $CachePath)) { throw "the priming launch wrote no coverage cache" }
}
```

Conditions fixed across every sample: same machine, same `release` profile, same toolchain; `~/.config/alacritty/alacritty.toml` and `alacritree.toml` unchanged between runs, including the 20-entry fallback chain and `Sarasa Fixed K` as primary; exactly two sessions; no scrollback; window not resized; 60 s idle settling; exactly one instance alive per sample.

- [ ] **Step 5: Take the six memory samples**

```powershell
$baseMem = 1..3 | ForEach-Object { Measure-Memory $baseline "baseline-mem-$_" $scratch }
$revMem  = 1..3 | ForEach-Object { Measure-Memory $revised  "revised-mem-$_"  $scratch }
```

- [ ] **Step 6: Record the baseline artefacts**

A font's private copy and its file mapping are two *independent* allocations with different `AllocationBase` values — the heap buffer from `std::fs::read` and the mmap are unrelated regions. They are correlated by **size**, not by base address, so filtering for a single allocation carrying both private and mapped bytes would yield an empty set and make criterion 1 vacuously true.

Both artefacts are derived from **all three** baseline samples rather than the first, and each handles instability differently because the two properties differ:

- **Mappings** — the *intersection* defines the stable baseline. A font mapped in only some baseline runs is dropped rather than demanded, because criterion 3 must not require of the revised binary something the baseline itself did not do reproducibly. The `-lt 6` floor is what stops the intersection from collapsing to a set too small to prove anything, and it counts **distinct files**: a single font mapped six times is one font, and a row count would let it clear a floor of six on its own.
- **Duplicate sizes** — the three multisets must *match*. A count alone would let three different size sets pass, and criterion 1 checks sizes, not counts; comparing the sorted sizes within the same 64 KiB tolerance is what makes the written file representative of every sample.

```powershell
# Real font mappings only: unnamed and non-font MEM_MAPPED regions are not
# what criterion 3 claims to cover.  Kept as rows, one per mapping, because
# that is what Get-TwinSizes pairs against.
$baseFontSets = foreach ($s in $baseMem) {
    ,@($s.Allocs | Where-Object { $_.Mapped -gt 0 -and $_.Path -match $FontExt })
}

# Distinct files, because the floor is a claim about fonts: one file mapped
# six times is one font, and counting rows would let it satisfy a floor of six.
$baseFontPaths = foreach ($set in $baseFontSets) {
    ,@($set | Select-Object -ExpandProperty Path | Sort-Object -Unique)
}
foreach ($i in 0..2) {
    $n = @($baseFontPaths[$i]).Count
    if ($n -lt 6) {
        throw "baseline sample $($baseMem[$i].Label) mapped $n distinct font files, expected at least 6"
    }
}

# Only paths mapped in EVERY baseline sample: criterion 3 must not demand a
# mapping the baseline itself produced only sometimes.
$mappedAlways = @($baseFontPaths[0] | Where-Object {
    $p = $_
    ($baseFontPaths[1] -contains $p) -and ($baseFontPaths[2] -contains $p)
})
if ($mappedAlways.Count -lt 6) {
    throw "only $($mappedAlways.Count) distinct font files were mapped in all three baseline samples"
}
$mappedAlways | Set-Content (Join-Path $scratch 'baseline-mapped.txt')

# For each mapped font, the private allocation of the same size — its copy.
# 64 KiB is the Windows allocation granularity, so sizes match within that.
# Pairing is one-to-one: the five CJK fallbacks are the same size, so without
# consuming each private allocation the same one would answer for all five and
# five "pairs" would be reported where one copy exists.
function Get-TwinSizes($sample, $fonts) {
    $used = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($f in $fonts) {
        $twin = $sample.Allocs | Where-Object {
            $_.Priv -gt 0 -and $_.Mapped -eq 0 -and
            [math]::Abs($_.Priv - $f.Mapped) -le 64KB -and -not $used.Contains($_.Base)
        } | Select-Object -First 1
        if ($twin) { [void]$used.Add($twin.Base); $twin.Priv }
    }
}
$twinSets = foreach ($i in 0..2) { ,@(Get-TwinSizes $baseMem[$i] $baseFontSets[$i] | Sort-Object) }

# Checked before the multiset comparison, so a baseline with no duplicates at
# all reports that rather than "the samples differ" — they agree, at zero.
if ($twinSets[0].Count -lt 6) {
    throw "baseline duplicate pairs: $($twinSets[0].Count); expected at least 6"
}
foreach ($i in 1..2) {
    $a = $twinSets[0]; $b = $twinSets[$i]
    $same = $a.Count -eq $b.Count
    if ($same) {
        foreach ($j in 0..($a.Count - 1)) {
            if ([math]::Abs($a[$j] - $b[$j]) -gt 64KB) { $same = $false; break }
        }
    }
    if (-not $same) {
        throw "baseline duplicate sizes differ between $($baseMem[0].Label) and $($baseMem[$i].Label): $($a -join ', ') vs $($b -join ', ')"
    }
}
$twinSets[0] | Set-Content (Join-Path $scratch 'baseline-dupes.txt')
```

Expected on this machine: six entries — one near 831,328,256 and five near 35,336,192 — and a `baseline-mapped.txt` carrying `Sarasa-SuperTTC.ttc` and the five `NotoSansMonoCJK*-VF.ttf` files among others. The `throw`s are the point: if the baseline does not exhibit the defect, or does not exhibit it reproducibly, the comparison proves nothing and must not be reported as a pass.

- [ ] **Step 7: Take the twelve startup samples**

```powershell
Initialize-WarmCache $baseline
$startup = [ordered]@{ 'baseline-warm' = 1..3 | ForEach-Object { Measure-Startup $baseline $false } }
$startup['baseline-cold'] = 1..3 | ForEach-Object { Measure-Startup $baseline $true }

Initialize-WarmCache $revised
$startup['revised-warm'] = 1..3 | ForEach-Object { Measure-Startup $revised $false }
$startup['revised-cold'] = 1..3 | ForEach-Object { Measure-Startup $revised $true }
```

Each warm series is preceded by a discarded priming launch of the binary under test, so "warm" means a cache that binary wrote rather than whatever the previous series left behind. Each cold launch deletes `%LOCALAPPDATA%\alacritree\coverage-cache.v1.bin` and then asserts it is gone — a suppressed deletion failure would relabel a warm launch as cold and report it as an improvement. `Measure-Startup` likewise refuses a warm run when no cache is present.

This bounds font installation from above rather than isolating it, which is the point: the claim is about time the user waits.

- [ ] **Step 8: Evaluate every criterion**

Each criterion throws on failure, so a silently skipped check cannot be reported as a pass.

Criteria 1 and 3 are structural — a duplicate is present or it is not, a mapping is present or it is not — so they are checked against **every** revised sample. A median is meaningless for a yes/no property, and evaluating one sample would let an intermittently lost mapping through. Criteria 2 and 4 are magnitudes, so they use medians.

```powershell
$basePriv = Median ($baseMem | ForEach-Object { $_.PrivateTotal })
$baseMap  = Median ($baseMem | ForEach-Object { $_.MappedTotal })
$revPriv  = Median ($revMem  | ForEach-Object { $_.PrivateTotal })
$revMap   = Median ($revMem  | ForEach-Object { $_.MappedTotal })

$dupes  = Get-Content (Join-Path $scratch 'baseline-dupes.txt')  | ForEach-Object { [long]$_ }
$wanted = Get-Content (Join-Path $scratch 'baseline-mapped.txt')

foreach ($s in $revMem) {
    # 1 — none of the baseline's duplicate sizes survives as a private allocation.
    $survivors = foreach ($size in $dupes) {
        $s.Allocs | Where-Object { $_.Priv -gt 0 -and [math]::Abs($_.Priv - $size) -le 64KB }
    }
    if (@($survivors).Count -gt 0) {
        throw "criterion 1: $(@($survivors).Count) duplicate allocation(s) survived in $($s.Label)"
    }

    # 3 — every font path mapped in the baseline is still mapped, full path.
    $mappedNow = $s.Allocs | Where-Object { $_.Mapped -gt 0 } | Select-Object -ExpandProperty Path
    $missing = $wanted | Where-Object { $mappedNow -notcontains $_ }
    if (@($missing).Count -gt 0) {
        throw "criterion 3: $($s.Label) lost mappings: $($missing -join ', ')"
    }
}

# 2 — private committed.
if ($revPriv -gt 250MB) { throw "criterion 2: private median $revPriv > 250 MB" }

# 4 — mapped committed, aggregate.
if ([math]::Abs($revMap - $baseMap) / $baseMap -gt 0.05) {
    throw "criterion 4: mapped median moved from $baseMap to $revMap, outside +/-5%"
}

# 5 and 6 — startup, one-sided.
foreach ($series in 'warm', 'cold') {
    $b = Median $startup["baseline-$series"]
    $r = Median $startup["revised-$series"]
    if ($r -gt $b + 100) { throw "criterion $series startup: $r ms vs baseline $b ms, over +100 ms" }
}
"all six criteria passed; private $basePriv -> $revPriv, mapped $baseMap -> $revMap"
```

| Criterion | Threshold | Fails if |
|---|---|---|
| The baseline's duplicate allocations are gone | in **every** revised sample, no private allocation within 64 KiB of any size in `baseline-dupes.txt` | any survives in any sample |
| Private committed | median ≤ 250 MB (baseline median ~1,113 MB; arithmetic predicts ~153 MB, and the headroom absorbs allocator and GPU-driver variance) | > 250 MB |
| Mapped retention, per path | in **every** revised sample, every path in `baseline-mapped.txt` still mapped, full canonicalized comparison | any is absent in any sample |
| Mapped committed, aggregate | within ±5% of the baseline median | outside that band |
| Startup, warm cache | revised median ≤ baseline median + 100 ms | above that |
| Startup, cold cache | revised median ≤ baseline median + 100 ms | above that |

The mapped column staying flat matters as much as the private column falling: the bytes must stay reachable and stay evictable by the OS. The aggregate ±5% band is too coarse to catch a single lost 33 MB Noto mapping against a ~1.1 GB total, which is why the per-path check is the binding one. The startup criteria are one-sided by design — the seed memo should improve them, but the change is not justified by startup time, so a flat result passes.

- [ ] **Step 9: Report and stop**

```powershell
foreach ($set in ([ordered]@{
        'private-baseline' = ($baseMem | ForEach-Object { $_.PrivateTotal })
        'private-revised'  = ($revMem  | ForEach-Object { $_.PrivateTotal })
        'mapped-baseline'  = ($baseMem | ForEach-Object { $_.MappedTotal })
        'mapped-revised'   = ($revMem  | ForEach-Object { $_.MappedTotal })
    }).GetEnumerator()) {
    "{0}: median {1}, spread {2}" -f $set.Key, (Median $set.Value), (Spread $set.Value)
}
$startup.Keys | ForEach-Object { "{0}: median {1} ms, spread {2}" -f $_, (Median $startup[$_]), (Spread $startup[$_]) }
```

Report every median and spread, plus each criterion's pass/fail. Do **not** push the branch or open a PR without an explicit instruction.

When that instruction comes: push `fix-font-bytes` to `origin` (`AbysmalBiscuit/alacritree`), open the PR against `master` on `mathix420/alacritree` with the title `fix(fonts): map font files instead of copying them [11]`, and put the measurement table in the description — the spec is never committed, so the PR body carries the context. Then merge into `all-features` and run `install.local.ps1`.

---

## Risks Carried Into Implementation

**The mmap access window widens, for colour-only faces only.** A font file truncated or replaced in place while mapped faults the process — the bet already documented at `map_font_file`. Task 2 creates no new mapping (`map_font_file` runs at `fonts.rs:548` and `:1020` *before* the `is_color_only` check, because that check needs the bytes), but it changes *when* mappings are dereferenced. Every chain face except colour-only ones is already handed to epaint as borrowed `FontData`, so those mappings are dereferenced every frame today. Colour-only faces are the exception: skipped before `insert_face`, their mapping is touched only during startup classification today, while rendering goes through an owned `Arc<Vec<u8>>`. After Task 2, paint-time rasterization dereferences their mapping too. Accepted, not mitigated — retaining owned snapshots would not close the window, because `claiming_index` already dereferences every chain face's cmap at first sight of a new character.

**A deliberate consistency choice.** Task 4's Step 6 makes seed coverage trust the same size-plus-mtime identity that candidate coverage already trusts. A font replaced by a same-size file within the same millisecond would yield stale seed coverage where today it yields fresh. Accepted: that same stale cache already determines every candidate's coverage and therefore the whole chain, so a freshly parsed seed only produces an *inconsistent* chain, not a correct one.

**Widened visibility.** `map_font_file` goes from private to `pub(crate)`. `is_mapped`, `chain_walks`, and the parse counter are `#[cfg(test)]`. Crate-internal only.

**Behaviour.** None intended. Identical glyphs, identical fallback order. Task 3 changes *when* the chain is walked, never which face wins, because the recorded index is the one `claiming_index` would have returned.

## Out Of Scope

- Trimming the 20-entry fallback chain or the Sarasa SuperTTC bundle. Both are configuration.
- A config option to disable colour glyphs as a memory workaround. `font.color_glyphs = false` already exists and short-circuits at `terminal_view.rs:1097`; this PR makes it unnecessary rather than promoting it.
- Strengthening the coverage cache's size-plus-mtime identity. Real but pre-existing, and a change that would invalidate every user's cache for unrelated reasons.
- Reporting font memory from `alacritree doctor`. Plausible follow-up.
- Any change to scrollback sizing. `[scrolling] history` is already configurable.
- Replacing `ColorGlyphCache::files` with a `Vec<Option<&'static [u8]>>` preloaded in `new()` and indexed by chain position. See the open question below.

## Open Questions

1. Codex argued across the review that `files` is redundant: every chain face is already mapped, so the `None` arm added in Task 2 Step 5 is unreachable, and a `Vec<Option<&'static [u8]>>` indexed by chain position would drop a hash and a `PathBuf` clone per lookup. This plan keeps `files` per the design-review decision. Adopting the `Vec` instead would change Task 2 Steps 4–6 and leave Tasks 1, 3, 4 and the Verification Appendix untouched.
2. The two round-5 fixes to the acceptance scripts — the exit-code readiness gate and the `QueryDosDeviceW` declaration — were applied to the spec *after* Codex's final verdict, so they carry no review. Both carry into the Verification Appendix, Step 3.
