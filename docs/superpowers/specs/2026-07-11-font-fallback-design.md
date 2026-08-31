# feat/font-fallback — design

Branch: `feat/font-fallback` off `master`, own worktree. Upstream PR target:
mathix420/alacritree. Local-only spec (git-excluded); the PR description
carries the context.

## Problem

On Windows, glyph fallback does not exist: `gather_fallback_faces` is
`#[cfg(not(unix))] Vec::new()` (`fonts.rs:205-214`), so any glyph missing
from the primary face renders as tofu — starship/powerline Nerd Font
symbols, emoji, CJK, box-drawing when `builtin_box_drawing = false`. On
Unix, fallback works (FcFontSort chain) but the user has no way to steer
which fonts are preferred.

Two deliverables (approach C, approved 2026-07-11):

1. A user-configured, ordered fallback list in config, honored on all
   platforms, sitting in front of the automatic chain.
2. An automatic fallback chain on Windows equivalent in spirit to
   fontconfig's coverage-trimmed sort. Unix fontconfig behavior stays
   untouched.

## Config

New field on the existing `[font]` table:

```toml
[font]
fallback = ["JetBrainsMono Nerd Font", "Segoe UI Emoji", "C:\\Fonts\\custom.ttf"]
```

- Each entry is a family name or a font file path (same duality
  `resolve_face` already supports for `[font.normal] family`).
- Order is priority order; entries go after the four primary faces and
  before the automatic chain.
- Empty/absent list = today's behavior plus the new Windows automatic
  chain.
- Plumbing: `RawFont` gains `fallback: Option<Vec<String>>`; `FontConfig`
  gains `pub fallback: Vec<String>` (default empty). Documented in the
  `RawFont` struct per project convention.
- Recommended home is `alacritree.toml` (upstream alacritty warns on
  unknown config keys, so putting it in the shared `alacritty.toml` would
  make real alacritty noisy). The merged-parse accepts it from either
  file; docs say alacritree.toml.

## Resolution pipeline (fonts.rs)

`install_terminal_fonts` gains one step between the primary faces and
`register_fallback_faces`: for each of the four variant seeds, resolve
every user fallback entry via the existing per-platform `resolve_face`
(paths hit `resolve_via_path`; families hit fontconfig on Unix / fontdb on
Windows, with the variant's weight/slant so bold cells cascade through
bold fallbacks). Register each hit on that variant's target families,
dedup through the existing `loaded_paths` set. Unresolvable entries log
one `warn!` naming the entry.

## Windows automatic chain

`gather_fallback_faces` for `cfg(not(unix))` becomes real:

1. **Enumerate once.** Build one `fontdb::Database` with
   `load_system_fonts()` per `install_terminal_fonts` call, shared by all
   four variant seeds (today's code builds a throwaway db per
   `resolve_via_fontdb` call — fold that into the shared one).
2. **Coverage scan.** For each candidate face, read its cmap via
   `fontdb::Database::with_face_data` + `ttf-parser` (already in the tree
   as fontdb's parser; added as an explicit dependency) and record the set
   of covered codepoints as sorted ranges.
3. **Order candidates:** same-family siblings first, then faces whose
   style matches the variant (weight/italic from `fontdb::FaceInfo`),
   monospace faces before proportional, then the rest in stable
   (family-name) order for determinism.
4. **Greedy trim, mirroring FcFontSort(trim=true):** walk the ordered
   list; keep a face only if it covers at least one codepoint not covered
   by the primary face + already-kept faces. Stop at
   `MAX_FALLBACK_FACES` (32, unchanged).
5. Skip faces already in `loaded_paths`; skip `Source::Binary` (not
   path-addressable); use the path from `Source::SharedFile` (it carries
   one — today's `resolve_via_fontdb` wrongly drops it).

The trim logic is a pure function (`fn trim_by_coverage(candidates:
Vec<(FaceCandidate, CoverageRanges)>, seed_coverage: &CoverageRanges,
limit: usize) -> Vec<FaceCandidate>` or equivalent) so it is unit-testable
without system fonts.

## Caching

- Coverage sets live in memory for the duration of the scan; the scan
  runs once per `install_terminal_fonts` (startup + config reload), not
  per variant.
- The scan logs its wall-clock duration at `info` level. Disk persistence
  (keyed by font path + mtime + size) is explicitly deferred: only added
  if the measured scan exceeds ~100 ms on real hardware during manual
  verification, as a follow-up commit on the same branch.

## Unix

No change to the fontconfig resolution or FcFontSort chain other than the
user-list insertion happening before `register_fallback_faces`. The user
list uses the same `resolve_face` path that exists today.

## Error handling

- Unreadable/unparseable font files: skip with `debug!`, never fail
  startup (matches the existing `register_fallback_faces` pattern).
- User fallback entry that resolves to nothing: one `warn!` per entry.
- fontdb returning zero system fonts: automatic chain is empty, primary
  faces still load — same degradation as today.

## Testing

- Unit tests (no system fonts needed): coverage-trim function (subsumed
  face dropped, novel face kept, limit respected, deterministic order);
  `RawFont.fallback` parsing + merge semantics (alacritree.toml overrides
  concatenate arrays — verify a list in alacritree.toml lands in
  `FontConfig.fallback`).
- Manual verification on this machine (Windows): starship prompt renders
  Nerd Font glyphs with a Nerd Font in `fallback`; emoji and CJK sample
  text render without tofu with an empty user list (automatic chain);
  startup scan duration logged; Linux regression check that fontconfig
  behavior is unchanged (build + visual check if available, else CI
  compile check for the unix cfg).
