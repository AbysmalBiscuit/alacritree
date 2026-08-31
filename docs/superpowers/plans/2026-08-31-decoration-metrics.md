# Decoration metrics implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Place underlines and strikeouts from the font's own `post` and OS/2 metrics instead of three constants scaled off cell height, and give the user four kitty-style knobs to correct a face whose metrics are wrong.

**Architecture:** `fonts.rs` parses the primary face's decoration tables once at startup into a `FaceMetrics` of em fractions. `config.rs` parses four `[ui.decorations]` strings into an `Adjust` each. `decoration_sprites::Geometry::resolve` combines them with the baseline egui actually lays out against, producing physical pixels; the rasterizer then hangs the double and curly styles off the descent area rather than off stroke weight. Everything except the final wiring is a pure function testable with no GL context and no window.

**Tech Stack:** Rust 2024, `ttf-parser` 0.25.1 (already in the lockfile), `egui`/`epaint` 0.31.1, `schemars` for the published config schema.

## Global constraints

- Work in the worktree `C:\Users\Lev\Git\github\alacritree-worktrees\feat\decoration-metrics`, on branch `feat/decoration-metrics`. It is based on `perf/instanced-grid`, not `master`, because `decoration_sprites.rs` exists only there.
- Design doc: `docs/superpowers/specs/2026-08-31-decoration-metrics-design.md` in the main checkout. It is not present in the worktree.
- Workspace MSRV 1.85, edition 2024.
- `cargo fmt` is enforced. Run it before every commit.
- Scope is the GL path only. Do not touch `paint_grid`'s own underline and strikeout drawing in `terminal_view.rs`. The mesh path keeps its constants deliberately.
- Comments explain a non-obvious *why*, never restate the *what*. No issue numbers, no "previously", no change narration. Match the voice already in `decoration_sprites.rs` and `config.rs`.
- Conventional Commits, imperative subject under 72 characters, and every commit carries the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Only `alacritree/` may be edited. The vendored alacritty crates are read-only.

---

### Task 1: Read decoration metrics from the primary face

**Files:**
- Modify: `alacritree/src/fonts.rs` (add `FaceMetrics` near `face_height_ratio` at line 618; change `install_terminal_fonts` at line 859)
- Modify: `alacritree/src/app.rs:726-727` (call site), `alacritree/src/app.rs:889` (struct literal), `alacritree/src/app.rs:547-553` (field declarations)
- Test: `alacritree/src/fonts.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub struct FaceMetrics` with public `f32` fields `ascender`, `descender`, `underline_position`, `underline_thickness`, `strikeout_position`, `strikeout_thickness`; `impl Default for FaceMetrics`; `pub fn FaceMetrics::from_face(data: &[u8], index: u32) -> FaceMetrics`. `install_terminal_fonts` changes return type from `Vec<ChainFace>` to `(Vec<ChainFace>, FaceMetrics)`. `AlacritreeApp` gains a `face_metrics: crate::fonts::FaceMetrics` field.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `alacritree/src/fonts.rs`:

```rust
/// Raw font units are in the hundreds; em fractions are not.  A face read
/// without dividing by `units_per_em` passes every other test in this file
/// and puts the underline several cells below the glyph.
#[test]
fn the_bundled_face_reports_em_fractions() {
    let m = FaceMetrics::from_face(SYMBOLS_FONT, 0);
    assert!((0.5..1.5).contains(&m.ascender), "ascender {}", m.ascender);
    assert!((-0.6..0.0).contains(&m.descender), "descender {}", m.descender);
    assert!(m.underline_position.abs() < 1.0, "underline {}", m.underline_position);
    assert!(m.strikeout_position.abs() < 1.0, "strikeout {}", m.strikeout_position);
    assert!(
        m.underline_thickness > 0.0 && m.underline_thickness <= 0.5,
        "underline thickness {}",
        m.underline_thickness
    );
    assert!(
        m.strikeout_thickness > 0.0 && m.strikeout_thickness <= 0.5,
        "strikeout thickness {}",
        m.strikeout_thickness
    );
}

/// Bytes that are not a font at all, which is what a truncated or swapped
/// file looks like by the time it reaches here.
#[test]
fn an_unreadable_face_yields_defaults() {
    assert_eq!(FaceMetrics::from_face(b"not a font", 0), FaceMetrics::default());
}

/// ghostty guards the same way in `has_broken_strikethrough`: a zero in OS/2
/// would otherwise draw a bar with no height at all.
#[test]
fn a_zero_strikeout_thickness_borrows_the_underline_weight() {
    let broken = FaceMetrics { strikeout_thickness: 0.0, ..FaceMetrics::default() };
    let fixed = resolve_fallbacks(broken);
    assert_eq!(fixed.strikeout_thickness, fixed.underline_thickness);
}

/// kitty puts the bar at `floor(baseline * 0.65)` from the cell top, which is
/// 0.35 of the ascender above the baseline.
#[test]
fn a_zero_strikeout_position_follows_the_ascender() {
    let broken = FaceMetrics { strikeout_position: 0.0, ascender: 0.9, ..FaceMetrics::default() };
    let fixed = resolve_fallbacks(broken);
    assert!((fixed.strikeout_position - 0.315).abs() < 1e-6, "{}", fixed.strikeout_position);
}

#[test]
fn a_zero_underline_pair_falls_back_to_the_defaults() {
    let broken = FaceMetrics {
        underline_position: 0.0,
        underline_thickness: 0.0,
        ..FaceMetrics::default()
    };
    let fixed = resolve_fallbacks(broken);
    assert_eq!(fixed.underline_position, FaceMetrics::default().underline_position);
    assert_eq!(fixed.underline_thickness, FaceMetrics::default().underline_thickness);
}

/// `[font.normal]` unresolvable means `build_font_definitions` returned
/// `None` and there is no face to read, which is not the same case as a face
/// that failed to parse but reaches the same place.
#[test]
fn an_empty_chain_yields_defaults() {
    assert_eq!(primary_face_metrics(&[]), FaceMetrics::default());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree --lib fonts::tests`

Expected: FAIL to compile, with `cannot find type FaceMetrics in this scope` and `cannot find function resolve_fallbacks in this scope`.

- [ ] **Step 3: Add `FaceMetrics` and its fallbacks**

Insert after `face_height_ratio` (currently ending at line 623) in `alacritree/src/fonts.rs`:

```rust
/// Em fractions used where a face reports nothing usable.  A zero in a metric
/// table means "not supplied" rather than "at the baseline", so every field is
/// checked against these rather than used as read.
const DEFAULT_ASCENDER: f32 = 0.8;
const DEFAULT_DESCENDER: f32 = -0.2;
const DEFAULT_UNDERLINE_POSITION: f32 = -0.1;
const DEFAULT_UNDERLINE_THICKNESS: f32 = 0.05;

/// Where a strikeout goes above the baseline when OS/2 does not say, as a
/// fraction of the ascender.  kitty spells the same rule as
/// `floor(baseline * 0.65)` measured down from the cell top.
const STRIKEOUT_ASCENDER_RATIO: f32 = 0.35;

/// What a face asks for its decorations, as fractions of the em measured from
/// the baseline with up positive.  That is the sign convention of the `post`
/// and OS/2 tables the numbers come from: an underline position is negative,
/// a strikeout position is positive, and so is the ascender while the
/// descender is negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetrics {
    pub ascender: f32,
    pub descender: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikeout_position: f32,
    pub strikeout_thickness: f32,
}

impl Default for FaceMetrics {
    fn default() -> Self {
        Self {
            ascender: DEFAULT_ASCENDER,
            descender: DEFAULT_DESCENDER,
            underline_position: DEFAULT_UNDERLINE_POSITION,
            underline_thickness: DEFAULT_UNDERLINE_THICKNESS,
            strikeout_position: STRIKEOUT_ASCENDER_RATIO * DEFAULT_ASCENDER,
            strikeout_thickness: DEFAULT_UNDERLINE_THICKNESS,
        }
    }
}

impl FaceMetrics {
    /// Read face `index` of `data`.  Anything the face leaves at zero, omits,
    /// or cannot express is filled in by `resolve_fallbacks`.
    pub fn from_face(data: &[u8], index: u32) -> Self {
        let Ok(face) = ttf_parser::Face::parse(data, index) else {
            log::warn!("could not parse the terminal face; using default decoration metrics");
            return Self::default();
        };
        let units = f32::from(face.units_per_em());
        if units <= 0.0 {
            log::warn!("the terminal face reports no em size; using default decoration metrics");
            return Self::default();
        }
        let em = |v: i16| f32::from(v) / units;
        let underline = face.underline_metrics();
        let strikeout = face.strikeout_metrics();

        resolve_fallbacks(Self {
            ascender: em(face.ascender()),
            descender: em(face.descender()),
            underline_position: underline.map_or(0.0, |m| em(m.position)),
            underline_thickness: underline.map_or(0.0, |m| em(m.thickness)),
            strikeout_position: strikeout.map_or(0.0, |m| em(m.position)),
            strikeout_thickness: strikeout.map_or(0.0, |m| em(m.thickness)),
        })
    }
}

/// Substitute for every field a face left at zero.  Split out from
/// `from_face` so each substitution is reachable from a test without a font
/// file engineered to be broken in exactly one way.
fn resolve_fallbacks(raw: FaceMetrics) -> FaceMetrics {
    let defaults = FaceMetrics::default();
    let ascender = nonzero(raw.ascender).unwrap_or(defaults.ascender);
    let underline_thickness =
        nonzero(raw.underline_thickness).unwrap_or(defaults.underline_thickness);
    FaceMetrics {
        ascender,
        descender: nonzero(raw.descender).unwrap_or(defaults.descender),
        underline_position: nonzero(raw.underline_position)
            .unwrap_or(defaults.underline_position),
        underline_thickness,
        strikeout_position: nonzero(raw.strikeout_position)
            .unwrap_or(STRIKEOUT_ASCENDER_RATIO * ascender),
        strikeout_thickness: nonzero(raw.strikeout_thickness).unwrap_or(underline_thickness),
    }
}

fn nonzero(value: f32) -> Option<f32> {
    (value != 0.0 && value.is_finite()).then_some(value)
}
```

- [ ] **Step 4: Return the metrics from `install_terminal_fonts`**

Replace the body of `install_terminal_fonts` (line 859) in `alacritree/src/fonts.rs`:

```rust
/// Register the terminal faces with egui and return the normal-variant
/// fallback chain, in the order egui consults it, for the colour glyph
/// renderer to resolve against, together with the decoration metrics of the
/// face at its head.
pub fn install_terminal_fonts(
    ctx: &Context,
    font: &FontConfig,
    ui: &UiFont,
) -> (Vec<ChainFace>, FaceMetrics) {
    let fonts = SystemFonts::default();
    match build_font_definitions(font, ui, &fonts) {
        Some((defs, chain)) => {
            ctx.set_fonts(defs);
            let metrics = primary_face_metrics(&chain);
            (chain, metrics)
        },
        None => {
            ctx.set_fonts(unresolvable_font_definitions(ui));
            (Vec::new(), FaceMetrics::default())
        },
    }
}

/// The chain's head is the `[font.normal]` face, pushed ahead of every
/// fallback, so its metrics are the ones the grid is laid out against.  An
/// empty chain means the family could not be resolved at all.
fn primary_face_metrics(chain: &[ChainFace]) -> FaceMetrics {
    let Some(primary) = chain.first() else {
        return FaceMetrics::default();
    };
    match map_font_file(&primary.path) {
        Ok(data) => FaceMetrics::from_face(data, primary.face_index),
        Err(err) => {
            log::warn!("could not read {} for decoration metrics: {err}", primary.path.display());
            FaceMetrics::default()
        },
    }
}
```

- [ ] **Step 5: Store the metrics on the app**

In `alacritree/src/app.rs`, change lines 726-727 to:

```rust
        let (font_chain, face_metrics) =
            crate::fonts::install_terminal_fonts(&cc.egui_ctx, &config.font, &config.ui_font);
```

Add the field to the struct literal, immediately after the `color_glyphs:` entry that ends at line 891:

```rust
            face_metrics,
```

Add the declaration to the `AlacritreeApp` struct, immediately after `glyph_cache` at line 550:

```rust
    /// The `[font.normal]` face's own decoration metrics, parsed once when the
    /// fonts were installed.  Nothing re-reads the file per frame.
    face_metrics: crate::fonts::FaceMetrics,
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree --lib fonts::tests`

Expected: PASS, six new tests included.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/fonts.rs alacritree/src/app.rs
git commit -m "feat(fonts): read decoration metrics from the terminal face

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Parse the four adjustment knobs

**Files:**
- Modify: `alacritree/src/config.rs` (add `Adjust`/`Decorations` near the other resolved types around line 916; add `RawDecorations` near `RawUiFont` at line 1878; add the field to `RawUi` near `gpu_grid` at line 2011; add the `resolve` arm near line 2245)
- Modify: `schema/alacritree-config.json` (regenerated, not hand-edited)
- Test: `alacritree/src/config.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub enum Adjust { Pixels(f32), Points(f32), Scale(f32) }` with `Adjust::NONE`, `Adjust::parse(&str) -> Option<Adjust>` and `Adjust::apply(self, value: f32, pixels_per_point: f32) -> f32`; `pub struct Decorations` with four `Adjust` fields named `underline_position`, `underline_thickness`, `strikeout_position`, `strikeout_thickness`; `Ui` gains `pub decorations: Decorations`.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `alacritree/src/config.rs`:

```rust
#[test]
fn every_accepted_adjustment_spelling_parses() {
    assert_eq!(Adjust::parse("0"), Some(Adjust::Points(0.0)));
    assert_eq!(Adjust::parse("-2"), Some(Adjust::Points(-2.0)));
    assert_eq!(Adjust::parse("1.5"), Some(Adjust::Points(1.5)));
    assert_eq!(Adjust::parse("2pt"), Some(Adjust::Points(2.0)));
    assert_eq!(Adjust::parse("2px"), Some(Adjust::Pixels(2.0)));
    assert_eq!(Adjust::parse("-2px"), Some(Adjust::Pixels(-2.0)));
    assert_eq!(Adjust::parse("150%"), Some(Adjust::Scale(1.5)));
}

/// A percentage is a magnitude.  kitty silently takes the absolute value of a
/// negative one, which gives back a line the user did not ask for and no way
/// to tell that happened.
#[test]
fn unusable_adjustment_spellings_are_rejected() {
    for text in ["", "abc", "2 px", "-150%", "px", "%", "nan", "inf"] {
        assert_eq!(Adjust::parse(text), None, "{text:?} should not parse");
    }
}

/// The two spellings of "leave it alone" have to agree, since one is the
/// default and the other is what a user writes to say the same thing.
#[test]
fn a_zero_adjustment_is_the_identity_in_both_units() {
    assert_eq!(Adjust::parse("0").unwrap().apply(7.0, 2.0), 7.0);
    assert_eq!(Adjust::parse("100%").unwrap().apply(7.0, 2.0), 7.0);
    assert_eq!(Adjust::NONE.apply(7.0, 2.0), 7.0);
}

/// Pixels are physical and points are not, which is the whole reason both
/// spellings exist.
#[test]
fn pixels_are_absolute_and_points_scale_with_the_display() {
    assert_eq!(Adjust::parse("2px").unwrap().apply(10.0, 2.0), 12.0);
    assert_eq!(Adjust::parse("2pt").unwrap().apply(10.0, 2.0), 14.0);
    assert_eq!(Adjust::parse("150%").unwrap().apply(10.0, 2.0), 15.0);
}

/// A malformed knob must not fail the whole config load, and must not leave
/// the line somewhere the user cannot predict.
#[test]
fn a_malformed_adjustment_behaves_as_zero() {
    assert_eq!(parse_adjust("underline_position", Some("2 px")), Adjust::NONE);
    assert_eq!(parse_adjust("underline_position", None), Adjust::NONE);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree --lib config::tests`

Expected: FAIL to compile, with `cannot find type Adjust in this scope`.

- [ ] **Step 3: Add `Adjust` and `Decorations`**

Insert into `alacritree/src/config.rs`, above the `Ui` struct that declares `gpu_grid` at line 916:

```rust
/// One correction to a decoration the font placed: a shift in physical pixels,
/// a shift in points, or a multiplier.  kitty's grammar, so a value copied from
/// a kitty config behaves the same way here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Adjust {
    Pixels(f32),
    Points(f32),
    Scale(f32),
}

impl Adjust {
    /// Draw what the font asked for.
    pub const NONE: Self = Self::Pixels(0.0);

    /// `"2px"`, `"2pt"`, a bare `"2"` (points, which is how kitty spells it),
    /// or `"150%"`.  `None` for anything else, a signed percentage included.
    pub fn parse(raw: &str) -> Option<Self> {
        if let Some(number) = raw.strip_suffix('%') {
            let percent = finite(number)?;
            return (percent >= 0.0).then_some(Self::Scale(percent / 100.0));
        }
        if let Some(number) = raw.strip_suffix("px") {
            return finite(number).map(Self::Pixels);
        }
        finite(raw.strip_suffix("pt").unwrap_or(raw)).map(Self::Points)
    }

    /// `value` is already in physical pixels, so a point shift scales by
    /// `pixels_per_point` and a percentage multiplies what the font resolved
    /// to rather than the em fraction it was read from.
    pub fn apply(self, value: f32, pixels_per_point: f32) -> f32 {
        match self {
            Self::Pixels(px) => value + px,
            Self::Points(pt) => value + pt * pixels_per_point,
            Self::Scale(factor) => value * factor,
        }
    }
}

/// `"inf"` and `"nan"` parse as `f32` and would put a line nowhere at all.
fn finite(raw: &str) -> Option<f32> {
    raw.parse::<f32>().ok().filter(|value| value.is_finite())
}

/// `[ui.decorations]`: corrections to what the font reports for its underline
/// and strikeout, for a face whose tables are wrong.  Every knob is a no-op by
/// default, so an unmodified config draws what the face asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decorations {
    pub underline_position: Adjust,
    pub underline_thickness: Adjust,
    pub strikeout_position: Adjust,
    pub strikeout_thickness: Adjust,
}

impl Default for Decorations {
    fn default() -> Self {
        Self {
            underline_position: Adjust::NONE,
            underline_thickness: Adjust::NONE,
            strikeout_position: Adjust::NONE,
            strikeout_thickness: Adjust::NONE,
        }
    }
}

/// A knob that will not parse logs and behaves as `"0"`, the way the rest of
/// this file treats a value it does not recognize.
fn parse_adjust(field: &str, raw: Option<&str>) -> Adjust {
    let Some(text) = raw else {
        return Adjust::NONE;
    };
    Adjust::parse(text).unwrap_or_else(|| {
        log::warn!("unusable ui.decorations.{field} value {text:?}, using \"0\"");
        Adjust::NONE
    })
}
```

Add to the `Ui` struct, immediately after the `pub gpu_grid: bool` field at line 916:

```rust
    /// Corrections applied to the underline and strikeout the font placed
    /// ([`Decorations`]).  Only the GPU grid reads these; the mesh path draws
    /// a straight rule at a fixed offset either way.
    pub decorations: Decorations,
```

- [ ] **Step 4: Add the raw config type**

Insert into `alacritree/src/config.rs`, above `RawUiFont` at line 1876:

```rust
/// Corrections applied to what the font reports for its underline and
/// strikeout.  Each value is `"2px"` (physical pixels, added), `"2pt"` or a
/// bare `"2"` (points, added), or `"150%"` (a multiplier).  Positive moves a
/// line down, matching kitty and ghostty.  A percentage takes no sign.
/// Default `"0"`, which draws what the font asked for.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(default)]
struct RawDecorations {
    /// Shift or scale of how far the underline sits from the top of the cell.
    #[schemars(extend("pattern" = r"^(-?[0-9]*\.?[0-9]+(px|pt)?|[0-9]*\.?[0-9]+%)$"))]
    underline_position: Option<String>,
    /// Shift or scale of the underline's stroke weight.
    #[schemars(extend("pattern" = r"^(-?[0-9]*\.?[0-9]+(px|pt)?|[0-9]*\.?[0-9]+%)$"))]
    underline_thickness: Option<String>,
    /// Shift or scale of how far the strikeout sits from the top of the cell.
    #[schemars(extend("pattern" = r"^(-?[0-9]*\.?[0-9]+(px|pt)?|[0-9]*\.?[0-9]+%)$"))]
    strikeout_position: Option<String>,
    /// Shift or scale of the strikeout bar's weight.
    #[schemars(extend("pattern" = r"^(-?[0-9]*\.?[0-9]+(px|pt)?|[0-9]*\.?[0-9]+%)$"))]
    strikeout_thickness: Option<String>,
}
```

Add to `RawUi`, immediately after `gpu_grid: Option<bool>,` at line 2011:

```rust
    /// Corrections to the underline and strikeout the font placed
    /// ([`RawDecorations`]).
    decorations: RawDecorations,
```

- [ ] **Step 5: Resolve it**

Add to the `Ui { .. }` literal in `resolve`, immediately after `gpu_grid: self.ui.gpu_grid.unwrap_or(false),` at line 2245:

```rust
            decorations: Decorations {
                underline_position: parse_adjust(
                    "underline_position",
                    self.ui.decorations.underline_position.as_deref(),
                ),
                underline_thickness: parse_adjust(
                    "underline_thickness",
                    self.ui.decorations.underline_thickness.as_deref(),
                ),
                strikeout_position: parse_adjust(
                    "strikeout_position",
                    self.ui.decorations.strikeout_position.as_deref(),
                ),
                strikeout_thickness: parse_adjust(
                    "strikeout_thickness",
                    self.ui.decorations.strikeout_thickness.as_deref(),
                ),
            },
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree --lib config::tests`

Expected: PASS, five new tests included.

- [ ] **Step 7: Regenerate the schema and check where the pattern landed**

```bash
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

Then confirm the constraint reached the four properties:

```bash
grep -n -A 4 '"underline_position"' schema/alacritree-config.json
```

Expected: each of the four properties carries both its description and a `"pattern"` key.

If `schemars` put the pattern somewhere a validator will not apply it, which can happen when `extend` lands beside an `Option`'s `["string","null"]` type union rather than inside it, replace the four `Option<String>` fields with a newtype carrying a hand-written `JsonSchema`, following `RgbStr` at `alacritree/src/config.rs:2117-2136`. Then rerun the regeneration command above.

- [ ] **Step 8: Run the full test suite, format and commit**

```bash
cargo test -p alacritree
cargo fmt
git add alacritree/src/config.rs schema/alacritree-config.json
git commit -m "feat(config): add [ui.decorations] adjustment knobs

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Reshape `Geometry` and hang the styles off the descent

**Files:**
- Modify: `alacritree/src/decoration_sprites.rs:38-45` (the struct), `:82-113` (`rasterize`), `:118-151` (`draw_underline`), `:180-215` (`curl`), `:218-326` (the tests)
- Test: `alacritree/src/decoration_sprites.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::fonts::FaceMetrics` from Task 1, `crate::config::Decorations` from Task 2.
- Produces: `Geometry` loses `thickness` and gains `baseline: f32`, `descent: f32`, `underline_thickness: f32`, `strikeout_thickness: f32`, keeping `cell`, `underline_y` and `strikeout_y`; `pub fn Geometry::resolve(cell: [usize; 2], font_ascent_pt: f32, pixels_per_point: f32, metrics: &FaceMetrics, knobs: &Decorations) -> Geometry`.

- [ ] **Step 1: Write the failing tests**

In `alacritree/src/decoration_sprites.rs`, replace the `geometry()` helper at the top of `mod tests` with:

```rust
    /// A cell roomy enough that no style hits the `fit` clamp, so a test that
    /// fails is describing the arithmetic rather than the clamp.
    fn geometry() -> Geometry {
        Geometry {
            cell: [10, 24],
            baseline: 14.0,
            descent: 8.0,
            underline_y: 17.0,
            underline_thickness: 2.0,
            strikeout_y: 10.0,
            strikeout_thickness: 2.0,
        }
    }
```

Then append these tests to the same module:

```rust
    /// The descent area hangs from the baseline, not from the cell's bottom
    /// edge.  `cell_h` is a floored row height plus `font.offset.y`, so an
    /// anchor read off the bottom drifts by the line gap and by the offset,
    /// and this is the assertion that catches it.
    #[test]
    fn the_curl_stays_inside_the_descent_area() {
        let g = geometry();
        let image = rasterize(g);
        let band = (g.baseline as usize)..=((g.baseline + g.descent) as usize);
        for y in 0..g.cell[1] {
            if band.contains(&y) {
                continue;
            }
            for x in 0..g.cell[0] {
                assert_eq!(alpha(&image, CURLY, x, y), 0, "curl ink at row {y}, outside {band:?}");
            }
        }
    }

    /// One stem in each half of the descent area.  Both above or both below
    /// its midpoint would mean the stems were placed from a single position
    /// rather than from the band.
    #[test]
    fn the_double_stems_straddle_the_descent_midpoint() {
        let g = geometry();
        let image = rasterize(g);
        let inked: Vec<usize> =
            (0..g.cell[1]).filter(|&y| alpha(&image, DOUBLE, 5, y) > 128).collect();
        let midpoint = (g.baseline + g.descent / 2.0) as usize;
        assert!(inked.iter().any(|&y| y < midpoint), "nothing above {midpoint}: {inked:?}");
        assert!(inked.iter().any(|&y| y > midpoint), "nothing below {midpoint}: {inked:?}");
    }

    /// The two lines carry separate weights because the font reports them
    /// separately, and a tile has to honour both at once.
    #[test]
    fn the_strikeout_keeps_its_own_weight() {
        let g = Geometry { strikeout_thickness: 4.0, ..geometry() };
        let image = rasterize(g);
        let index = tile(STRAIGHT, true);
        let bar = (0..g.cell[1]).filter(|&y| alpha(&image, index, 5, y) > 128).count();
        assert!(bar >= 4 + 2, "strikeout and underline together cover {bar} rows");
    }

    /// Zero adjustments must reproduce the face: the underline below the
    /// baseline, the strikeout above it, and a descent the multi-line styles
    /// can divide.
    #[test]
    fn an_unadjusted_geometry_follows_the_face() {
        let metrics = crate::fonts::FaceMetrics::default();
        let g = Geometry::resolve([10, 24], 16.0, 1.0, &metrics, &Default::default());
        assert!(g.underline_y > g.baseline, "underline {} at {}", g.underline_y, g.baseline);
        assert!(g.strikeout_y < g.baseline, "strikeout {} at {}", g.strikeout_y, g.baseline);
        assert!(g.descent > 0.0, "descent {}", g.descent);
        assert!(g.underline_thickness >= 1.0);
        assert!(g.strikeout_thickness >= 1.0);
    }

    /// A knob shifts by exactly what it says, and a point shift is the one
    /// that grows with the display.
    #[test]
    fn a_position_knob_moves_the_line_by_what_it_asked_for() {
        use crate::config::{Adjust, Decorations};
        let metrics = crate::fonts::FaceMetrics::default();
        let plain = Geometry::resolve([10, 24], 16.0, 2.0, &metrics, &Decorations::default());
        let shifted = Geometry::resolve(
            [10, 24],
            16.0,
            2.0,
            &metrics,
            &Decorations { underline_position: Adjust::Pixels(2.0), ..Decorations::default() },
        );
        assert_eq!(shifted.underline_y - plain.underline_y, 2.0);

        let in_points = Geometry::resolve(
            [10, 24],
            16.0,
            2.0,
            &metrics,
            &Decorations { underline_position: Adjust::Points(2.0), ..Decorations::default() },
        );
        assert_eq!(in_points.underline_y - plain.underline_y, 4.0);
    }

    /// Rounding the font's thickness before a percentage scales it would
    /// quantize a 1px line to nothing a knob could halve.
    #[test]
    fn a_thickness_percentage_scales_before_rounding() {
        use crate::config::{Adjust, Decorations};
        let metrics = crate::fonts::FaceMetrics::default();
        let doubled = Geometry::resolve(
            [10, 24],
            16.0,
            1.0,
            &metrics,
            &Decorations { underline_thickness: Adjust::Scale(2.0), ..Decorations::default() },
        );
        let plain = Geometry::resolve([10, 24], 16.0, 1.0, &metrics, &Decorations::default());
        assert!(
            doubled.underline_thickness > plain.underline_thickness,
            "{} is not thicker than {}",
            doubled.underline_thickness,
            plain.underline_thickness
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree --lib decoration_sprites`

Expected: FAIL to compile, with `struct Geometry has no field named baseline` and `no function or associated item named resolve`.

- [ ] **Step 3: Reshape the struct and add `resolve`**

Replace the `Geometry` declaration at `alacritree/src/decoration_sprites.rs:38-45`:

```rust
/// Where the lines sit and how thick they are, in physical pixels.  The `y`
/// values and `baseline` are measured down from the cell's top edge;
/// `descent` is a length.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Geometry {
    pub cell: [usize; 2],
    /// Where epaint puts the glyph baseline inside the cell.  The descent area
    /// hangs from here rather than from the cell's bottom edge: `cell_h` is a
    /// floored row height plus `font.offset.y`, so the two are different places
    /// and only this one tracks where glyphs actually sit.
    pub baseline: f32,
    /// Height of the descent area below the baseline.  It is the vertical room
    /// the double and curly styles divide up, which is what keeps them legible
    /// on a face whose strokes are fine.
    pub descent: f32,
    /// Centre of the underline, measured down from the cell's top edge.
    pub underline_y: f32,
    pub underline_thickness: f32,
    pub strikeout_y: f32,
    pub strikeout_thickness: f32,
}

impl Geometry {
    /// Turn a face's em fractions into pixels for one cell.
    ///
    /// The face resolves to pixels before a knob touches it, and thickness
    /// rounds after: rounding first would quantize the value a percentage then
    /// scales, leaving a 50% request against a one-pixel line nothing to halve.
    pub fn resolve(
        cell: [usize; 2],
        font_ascent_pt: f32,
        pixels_per_point: f32,
        metrics: &FaceMetrics,
        knobs: &Decorations,
    ) -> Self {
        let ppp = pixels_per_point;
        let baseline = font_ascent_pt * ppp;
        let px_per_em = baseline / metrics.ascender;
        // Table positions are measured up from the baseline; the cell is
        // measured down from its top edge.
        let down_from_top = |em: f32| baseline - em * px_per_em;

        Self {
            cell,
            baseline,
            descent: -metrics.descender * px_per_em,
            underline_y: knobs
                .underline_position
                .apply(down_from_top(metrics.underline_position), ppp),
            underline_thickness: knobs
                .underline_thickness
                .apply(metrics.underline_thickness * px_per_em, ppp)
                .round()
                .max(1.0),
            strikeout_y: knobs
                .strikeout_position
                .apply(down_from_top(metrics.strikeout_position), ppp),
            strikeout_thickness: knobs
                .strikeout_thickness
                .apply(metrics.strikeout_thickness * px_per_em, ppp)
                .round()
                .max(1.0),
        }
    }
}
```

Add to the imports at the top of the file, beside the existing `use egui::{...}` line:

```rust
use crate::config::Decorations;
use crate::fonts::FaceMetrics;
```

- [ ] **Step 4: Give the strikeout its own weight in `rasterize`**

In `rasterize`, replace the strikeout bar inside the style loop:

```rust
            if strikeout {
                let t = geometry.strikeout_thickness;
                rect(&mut coverage, width, x0, w, geometry.strikeout_y - t / 2.0, t);
            }
```

and the standalone strikeout tile below the loop:

```rust
    // A struck cell with no underline is the one tile the loop above cannot
    // reach: it has no underline style to iterate over.
    let x0 = tile(NONE, true) as usize * w;
    let t = geometry.strikeout_thickness;
    rect(&mut coverage, width, x0, w, geometry.strikeout_y - t / 2.0, t);
```

- [ ] **Step 5: Anchor double and curly on the descent area**

In `draw_underline`, change `let t = geometry.thickness;` to `let t = geometry.underline_thickness;` and replace the `DOUBLE` arm:

```rust
        // One stem in each half of the descent area.  Deriving the gap from
        // the stroke instead would merge the pair on a face with fine strokes,
        // leaving the style indistinguishable from a straight rule.  Both move
        // together when the lower one would fall off the cell, so the pair
        // survives rather than the spacing.
        DOUBLE => {
            let lower = (geometry.baseline + 0.75 * geometry.descent).min(h as f32 - t / 2.0);
            let upper = lower - 0.5 * geometry.descent;
            rect(buf, stride, x0, w, upper - t / 2.0, t);
            rect(buf, stride, x0, w, lower - t / 2.0, t);
        },
```

Leave `STRAIGHT`, `DOTTED` and `DASHED` as they are. They keep `underline_y` and the font's stroke weight. Alacritty passes its dotted style the descent, but that is a canvas allocation rather than a dot size: it packs every decoration into one quad and its shader needs a tall one, while `draw_dotted_aliased` in `alacritty/res/rect.f.glsl` gives each dot a radius of `underlineThickness / 2`. Our tiles are already cell-sized, so there is nothing to allocate.

In `curl`, replace the four lines that compute the wave's extent:

```rust
    let t = geometry.underline_thickness;
    // The wave's ink fills the descent area, so the amplitude comes from the
    // room the band leaves rather than from the stroke: a face with fine
    // strokes would otherwise get a curl too shallow to read as one.  Pulled
    // up whole when the band runs past the cell, so the shape survives instead
    // of losing its lower lobe to the edge.
    let bottom = (geometry.baseline + geometry.descent).min(h as f32);
    let top = (bottom - geometry.descent).max(0.0);
    let centre = (top + bottom) / 2.0;
    let amplitude = ((bottom - top - t) / 2.0).max(0.5);
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree --lib decoration_sprites`

Expected: PASS. The six pre-existing tests still pass against the new fixture, and six new ones join them.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/decoration_sprites.rs
git commit -m "feat(grid): place decorations from the face's own metrics

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Wire the baseline through and verify on screen

**Files:**
- Modify: `alacritree/src/terminal_view.rs:31-41` (the `show` signature), `:44-48` (the cell-size layout), `:1298-1310` (the `Geometry` construction)
- Modify: `alacritree/src/app.rs:8496-8506` (the `show` call)
- Test: manual, plus the whole existing suite

**Interfaces:**
- Consumes: `FaceMetrics` from Task 1 via `AlacritreeApp::face_metrics`, `Decorations` from Task 2 via `config.ui.decorations`, `Geometry::resolve` from Task 3.
- Produces: nothing later tasks depend on. This is the last task.

- [ ] **Step 1: Take the metrics as an argument**

In `alacritree/src/terminal_view.rs`, add a parameter to `show` after `config`:

```rust
    face_metrics: &crate::fonts::FaceMetrics,
```

In `alacritree/src/app.rs`, add the matching argument to the call at line 8496, after `&self.config,`:

```rust
                        &self.face_metrics,
```

- [ ] **Step 2: Read the baseline from a laid-out glyph**

In `show`, replace the `ui.ctx().fonts(..)` block that computes `cell_w_pt` and `cell_h_pt`:

```rust
    // `Fonts` exposes no ascent, and deriving one from the face would miss
    // the quantization `FontImpl::new` applies when it stores its own.
    // `font_ascent` on a laid-out glyph is the number epaint draws at.  egui
    // caches this layout, so it costs one hash after the first frame.
    let (cell_w_pt, cell_h_pt, font_ascent_pt) = ui.ctx().fonts(|f| {
        let w = f.glyph_width(&font_id, 'M');
        let h = f.row_height(&font_id);
        let mut job = egui::text::LayoutJob::single_section(
            "M".to_owned(),
            egui::TextFormat::simple(font_id.clone(), Color32::PLACEHOLDER),
        );
        job.wrap.max_width = f32::INFINITY;
        let galley = f.layout_job(job);
        let ascent = galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first())
            .map_or(h, |glyph| glyph.font_ascent);
        (w, h, ascent)
    });
```

- [ ] **Step 3: Build the geometry from the metrics**

Replace the `Geometry` literal at `alacritree/src/terminal_view.rs:1300-1309`:

```rust
        let geometry = decoration_sprites::Geometry::resolve(
            [(cell_w * ppp) as usize, (cell_h * ppp) as usize],
            font_ascent_pt,
            ppp,
            face_metrics,
            &config.ui.decorations,
        );
```

The comment above it, which says the constants match where `paint_grid` puts the same two lines "so a decorated run comes out in the same place whichever path drew it", goes away with the constants. Put this above the call in its place:

```rust
        // The mesh path still draws a straight rule at a fixed offset, so the
        // two paths deliberately disagree; it is on its way out rather than
        // waiting to be brought along.
```

- [ ] **Step 4: Build and run the whole suite**

```bash
cargo fmt --check
cargo clippy -p alacritree --all-targets -- -D warnings
cargo test -p alacritree
```

Expected: no warnings, and every test passing.

- [ ] **Step 5: Look at it**

The demo sheet is untracked and lives only in the main checkout, at `C:\Users\Lev\Git\github\alacritree\terminal-decorations-demo.local.nu`. Build from the worktree, enable the GPU grid, and run the sheet by absolute path at two font sizes:

```toml
[ui]
gpu_grid = true
```

```bash
cargo run -p alacritree --release
```

Check that each of the five underline styles still reads as itself, that the double underline keeps a visible gap, that the curl keeps its shape, and that underlines and strikeouts sit where the glyphs suggest they should rather than where the cell edge is. Then set `[ui.decorations] underline_position = "2px"` and confirm the underline moves down by two pixels and nothing else does.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/terminal_view.rs alacritree/src/app.rs
git commit -m "feat(grid): draw decorations at the laid-out baseline

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Notes for the reviewer

- The mesh path is untouched on purpose. An unmodified config sees nothing from this work until `[ui] gpu_grid` is on.
- `Geometry` derives `PartialEq` and `DecorationAtlas` compares it to decide whether to re-rasterize. Adding fields keeps that correct: a change in any of them is a change worth redrawing for.
- SGR 58 underline colour, blink and overline stay out. They are tracked separately and the one-tile-one-colour problem in the sprite strip has to be solved before the first of them can land.
