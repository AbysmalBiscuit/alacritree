# Decoration geometry from font metrics, with adjustment options

Tracked by [#41](https://github.com/AbysmalBiscuit/alacritree/issues/41).
Blocks [#40](https://github.com/AbysmalBiscuit/alacritree/issues/40).

## Problem

Underlines and strikeouts are placed by three constants scaled off cell height,
in `terminal_view.rs` where it builds `decoration_sprites::Geometry`:

```rust
thickness: (cell_h * ppp / 14.0).round().max(1.0),
underline_y: (cell_h - 1.5) * ppp,
strikeout_y: cell_h * 0.5 * ppp,
```

The font's own numbers go unread. The `post` table carries `underlinePosition`
and `underlineThickness`; OS/2 carries `yStrikeoutPosition` and
`yStrikeoutSize`. Upstream alacritty reads all four plus the descent through
`crossfont::Metrics` and places its lines from them in
`alacritty/src/renderer/rects.rs`, so this is a divergence rather than a shared
limitation. There is also no way for a user to correct the result on a face
whose metrics are wrong, which kitty and ghostty both offer.

## Scope

The GL path only. The mesh path keeps its own constants and its single straight
rule, because it is slated for removal once the GL path is stable. SGR 58,
blink and overline are #40 and stay there.

## Reading the face

`ttf-parser` 0.25.1 is already in the lockfile. `Face::underline_metrics()`
reads `post` and `Face::strikeout_metrics()` reads OS/2, and both apply
variable-font metric variations, so a variable face reports metrics for its
instantiated weight rather than its default master.

New in `fonts.rs`:

```rust
/// What a face asks for its decorations, as fractions of the em measured from
/// the baseline with up positive.  That is the sign convention of the `post`
/// and OS/2 tables the numbers come from: an underline position is negative,
/// a strikeout position is positive, and so is the ascender while the
/// descender is negative.
pub struct FaceMetrics {
    pub ascender: f32,
    pub descender: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikeout_position: f32,
    pub strikeout_thickness: f32,
}
```

`install_terminal_fonts` gains a second return value rather than a separate
lookup function, so exactly one place decides which face is primary. That face
is `chain[0]`, pushed unconditionally in `build_font_definitions` ahead of every
fallback. `AlacritreeApp::new` stores the result. It is parsed once at startup;
nothing re-parses per frame.

When `[font.normal]` cannot be resolved at all, `build_font_definitions` returns
`None` and the chain comes back empty. An empty chain yields the same
all-defaults `FaceMetrics` as an unparseable face.

### Fallbacks

A face can ship a table with a zero in it, which kitty and ghostty both guard
against. Same guards here:

| missing or zero | falls back to | precedent |
|---|---|---|
| `underline_thickness` | 0.05 em | |
| `underline_position` | -0.1 em | |
| `strikeout_thickness` | the resolved underline thickness | ghostty's `has_broken_strikethrough` |
| `strikeout_position` | 0.35 x ascender | kitty's `floor(baseline * 0.65)`, restated from the baseline |

A face that will not parse at all yields every default and one `log::warn!`.

## Finding the baseline

`Geometry` needs the baseline's position inside the cell. Deriving it from the
face would drift from where glyphs actually sit, because `FontImpl::new`
quantizes the ascent it keeps: `epaint/src/text/font.rs` divides the scaled
ascent by `pixels_per_point` and calls `round_ui`, which snaps to a multiple of
1/32 of a logical point. So the face's ascent and the one epaint lays out
against are different numbers. `Glyph.font_ascent` is public on a laid-out
galley and is the quantized one, so `show()` lays out a single `M` beside the
existing cell-size computation, which egui already caches, and threads one
`f32` through.

That also yields the em size without depending on epaint's scaling internals:

```
px_per_em = font_ascent_px / metrics.ascender
```

## Geometry

```rust
pub struct Geometry {
    pub cell: [usize; 2],
    pub baseline: f32,
    pub underline_y: f32,
    pub underline_thickness: f32,
    pub strikeout_y: f32,
    pub strikeout_thickness: f32,
    pub descent: f32,
}
```

The single `thickness` splits in two, because the font reports separate weights
for the two lines and nothing justifies picking one of them for both. `baseline`
and `descent` are new. Everything stays in physical pixels: `baseline` and the
two `y` fields measured down from the cell's top edge as now, `descent` a length.

Together those two give the **descent area**, the band from `baseline` to
`baseline + descent`, and it is the anchor for every style that needs vertical
room. Alacritty reaches the same band from the cell's bottom edge, which works
there because its cell height is the font's. Here `cell_h` is a floored row
height plus `font.offset.y`, so the cell bottom and `baseline + descent` are two
different places and only the baseline tracks where glyphs sit.

Rounding before adjusting would quantize the value a `%` knob then scales, so a
50% request against a 1px line would have nothing to halve.

```
baseline   = font_ascent * ppp
px_per_em  = baseline / metrics.ascender

underline_y         = adjust(baseline - metrics.underline_position * px_per_em)
strikeout_y         = adjust(baseline - metrics.strikeout_position * px_per_em)
underline_thickness = adjust(metrics.underline_thickness * px_per_em).round().max(1.0)
strikeout_thickness = adjust(metrics.strikeout_thickness * px_per_em).round().max(1.0)
descent             = -metrics.descender * px_per_em
```

`descent` comes out positive because `descender` is negative, and it carries no
knob: it describes the space the face leaves below the baseline rather than
where a line goes.

### Rasterizer changes

Today every style derives its extent from `thickness`: the double underline
spreads its stems by `t * 1.5`, the curl claims `3.0 * t` of height. Once
thickness comes from the font that coupling breaks, because a face reporting a
fine stroke would get a shallow curl and a double underline whose stems merge.

The two styles with that problem take their vertical room from the descent area
instead, which is what alacritty does:

- double: two stems, centred at `baseline + 0.25 * descent` and
  `baseline + 0.75 * descent`
- curly: a sine wave whose ink fills the band, so centre `baseline + descent / 2`
  and amplitude `(descent - underline_thickness) / 2`
- straight, dashed and dotted: `underline_y` and `underline_thickness`,
  unchanged

Dotted stays with the other two despite alacritty passing it the descent. That
descent is a canvas allocation, not a dot size: alacritty packs every decoration
into one quad and its shader needs a tall one to draw into, but
`draw_dotted_aliased` in `alacritty/res/rect.f.glsl` gives each dot a radius of
`underlineThickness / 2`. Our tiles are already cell-sized, so there is nothing
to allocate and the dots keep the weight the font asked for.

Stroke weight for all five is `underline_thickness`. The strikeout bar is the
one shape that uses `strikeout_thickness`, and it keeps its own position and
weight in all six tiles that carry it, the five paired with an underline and the
strikeout-only one.

The existing `fit()` clamp still pulls a style up when it would overflow the
cell.

### One stale comment

At the construction site in `terminal_view.rs`, the comment above `underline_y`
says the constant matches where `paint_grid` puts the same line, "so a decorated
run comes out in the same place whichever path drew it". With the mesh path
frozen that stops being true, so it gets corrected to record that the two paths
now differ deliberately, rather than extended.

`Geometry`'s own `underline_y` doc in `decoration_sprites.rs` ("Centre of the
underline, measured down from the cell's top edge") stays accurate and stays
where it is. `baseline` and `descent` get doc comments in the same voice, since
their convention is the thing a reader of the rasterizer needs and cannot infer.

## Config

```toml
[ui.decorations]
underline_position = "-2px"
underline_thickness = "150%"
strikeout_position = "2pt"
strikeout_thickness = "200%"
```

Four string fields under `[ui.decorations]` in `alacritree.toml`. `[ui]` is the
alacritree-only namespace and already carries `gpu_grid`, so a grid rendering
option belongs there. `alacritty.toml` is wrong for these because alacritty has
no equivalent setting and would silently ignore them.

### Grammar

| suffix | means |
|---|---|
| `px` | physical pixels, added |
| `pt` or none | points, added after scaling by `pixels_per_point` |
| `%` | multiplies the resolved pixel value |

kitty spells points as a bare number with no explicit form. `pt` is accepted as
well so a config can say what it means; everything kitty accepts still parses.

Positive moves the line down, matching kitty and ghostty, so a value copied from
either behaves the same way.

Each field is additive or multiplicative according to its own suffix, never
both. A knob is one operation applied to one resolved pixel value. For the two
positions that value is the distance from the cell's top edge, not the font's
offset from the baseline, so `"150%"` pushes the line half again as far down the
cell. That is what kitty scales too.

A percentage carries no sign. kitty takes the absolute value of a negative one,
which is a silent surprise; rejecting it and logging is clearer.

### Why zero is the default

`"0"` adds nothing and `"100%"` multiplies by one, so the two units express the
same no-op and both mean draw what the font asked for. An unmodified config
therefore follows the face. The knobs exist for faces with bad metrics, not as a
general placement mechanism.

### Failure

A value that will not parse logs a warning naming the field and the offending
text, then behaves as `"0"`. Nothing here panics, matching how the rest of
`config.rs` treats bad values.

### Schema

Four string fields with pattern
`^(-?[0-9]*\.?[0-9]+(px|pt)?|[0-9]*\.?[0-9]+%)$`, which admits a sign on the
additive forms and not on the percentage. Each carries the grammar in its doc
comment, since those become the published hover text. Regenerate with:

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

## Testing

Everything below runs headless, with no GL context.

- The suffix parser over `"0"`, `"-2"`, `"2px"`, `"2pt"`, `"1.5"`, `"150%"`,
  which all parse, and `""`, `"abc"`, `"2 px"`, `"-150%"`, which all fail.
- `FaceMetrics` parsed from the bundled `alacritree-symbols.ttf`, asserting each
  value is normalized to the em: ascender in `(0.5, 1.5)`, descender in
  `(-0.6, 0.0)`, both positions under 1 em in magnitude, both thicknesses in
  `(0.0, 0.5]`. Raw font units are in the hundreds, so these bounds are what
  catch a read that forgot to divide by `units_per_em`.
- Each fallback in the table above, from a synthetic `FaceMetrics` with the one
  field zeroed.
- Geometry resolution with zero adjustments: `underline_y > baseline`,
  `strikeout_y < baseline`, and `descent > 0.0`, the last because the two
  descent-anchored styles divide by it.
- Adjustments: `"0"` and `"100%"` are both identity, `"2px"` moves the line by
  exactly two physical pixels, `"2pt"` by `2 * pixels_per_point`.
- Rasterized output: on a `Geometry` whose descent area sits clear of the cell
  edges, so `fit()` cannot fire, the curly tile's ink stays inside
  `[baseline, baseline + descent]` and the double tile's two stems straddle its
  midpoint. This is the assertion that catches an anchor read off the cell
  bottom instead of the baseline.

Then a visual pass with the demo sheet at two font sizes with `[ui] gpu_grid`
on, checking that every style still reads as itself. The sheet is untracked and
lives only in the main checkout, at
`C:\Users\Lev\Git\github\alacritree\terminal-decorations-demo.local.nu`. Run it
by absolute path against the binary built from the worktree.

## Open questions

None.
