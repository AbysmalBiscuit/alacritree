//! Laid-out single-character galleys, reused across frames.
//!
//! The grid paints one glyph per cell so every character lands on the cursor's
//! `col * cell_w` boundary — egui's own run layout drifts off it.  Going
//! through `Painter::text` for each one costs a `String`, a `LayoutJob`, and a
//! galley-cache probe per glyph *per frame*, which at a maximized window is
//! tens of thousands of allocations to redraw glyphs that mostly did not
//! change.  A galley is immutable and its colour can be replaced at paint time
//! with `TextShape::override_text_color`, so one galley per character and
//! style serves every cell that ever shows it.

use std::collections::HashMap;
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, Context, FontFamily, FontId, Galley};

use crate::fonts::{BOLD_FAMILY, BOLD_ITALIC_FAMILY, ITALIC_FAMILY};

/// Which of the four terminal faces a glyph is drawn with.  Cheaper to hash
/// than a `FontId`, whose `f32` size is not `Hash` anyway.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    Normal,
    Bold,
    Italic,
    BoldItalic,
}

impl Face {
    pub fn new(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (true, true) => Self::BoldItalic,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (false, false) => Self::Normal,
        }
    }

    fn font_id(self, size: f32) -> FontId {
        match self {
            Self::Normal => FontId::monospace(size),
            Self::Bold => FontId::new(size, FontFamily::Name(BOLD_FAMILY.into())),
            Self::Italic => FontId::new(size, FontFamily::Name(ITALIC_FAMILY.into())),
            Self::BoldItalic => FontId::new(size, FontFamily::Name(BOLD_ITALIC_FAMILY.into())),
        }
    }
}

/// `layout_job` rather than `layout_no_wrap` so the character is laid out with
/// `PLACEHOLDER`, which `override_text_color` is defined against; a concrete
/// colour here would be the one egui reuses if the override is ever dropped.
fn glyph_job(ch: char, face: Face, size: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        ch.to_string(),
        egui::TextFormat::simple(face.font_id(size), Color32::PLACEHOLDER),
    );
    job.wrap.max_width = f32::INFINITY;
    job
}

/// One terminal cell, in points.  Passed when `[font] cell_fitting` is on, and
/// `None` otherwise.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    pub width: f32,
    pub height: f32,
}

/// A laid-out glyph and where in its cell it goes.
#[derive(Clone)]
pub struct Glyph {
    pub galley: Arc<Galley>,
    /// Points to add to the top of the cell.  Non-zero only for a glyph laid
    /// out below the configured size, whose shorter line would otherwise leave
    /// all the slack beneath it.
    pub dy: f32,
}

/// How small fitting may take a glyph.  An overrun past this is a face with
/// broken metrics rather than a wide one, and a smudge reads worse than a
/// glyph crossing into its neighbour.
const MIN_FIT_SCALE: f32 = 0.25;

/// epaint rounds a font's raster size to whole pixels, so scaling by the
/// measured overrun can still land a hair over the cell.  A second pass
/// measures what the rounding left and settles it.
const FIT_PASSES: usize = 2;

/// Lay `ch` out, shrinking it to the cells it occupies when `cell` is given.
/// Fallback faces are drawn at the primary's point size, so an icon face with
/// a wider em than the terminal font spills into the next cell untouched.
fn lay_out(ctx: &Context, ch: char, face: Face, size: f32, cell: Option<Cell>) -> Glyph {
    let mut galley = ctx.fonts(|f| f.layout_job(glyph_job(ch, face, size)));
    let Some(cell) = cell else {
        return Glyph { galley, dy: 0.0 };
    };

    let span = cell.width * crate::ime::char_cells(ch) as f32;
    let mut scale = 1.0_f32;
    for _ in 0..FIT_PASSES {
        let step = span / galley.size().x;
        if !step.is_finite() || step >= 1.0 {
            break;
        }
        scale = (scale * step).max(MIN_FIT_SCALE);
        galley = ctx.fonts(|f| f.layout_job(glyph_job(ch, face, size * scale)));
    }

    let dy = if scale < 1.0 { ((cell.height - galley.size().y) * 0.5).max(0.0) } else { 0.0 };
    Glyph { galley, dy }
}

/// The font atlas a set of galleys was laid out against.  A galley's mesh
/// stores atlas positions, so it only means anything while that atlas is the
/// one being sampled.
#[derive(Clone, Copy, PartialEq)]
struct AtlasState {
    pixels_per_point: f32,
    max_texture_side: usize,
    image_size: [usize; 2],
    fill_ratio: f32,
}

impl AtlasState {
    fn read(ctx: &Context) -> Self {
        ctx.fonts(|f| Self {
            pixels_per_point: f.pixels_per_point(),
            max_texture_side: f.max_texture_side(),
            image_size: f.font_image_size(),
            fill_ratio: f.font_atlas_fill_ratio(),
        })
    }

    /// Whether galleys laid out against `self` can still be painted now that
    /// the atlas looks like `now`.  Repacking is the case that matters, but
    /// growth moves nothing and is folded in anyway: it costs one relayout of
    /// the visible glyphs on a frame the atlas changed shape, and keeping the
    /// rule to "anything moved" leaves no repack unnoticed.
    fn outlived_by(self, now: Self) -> bool {
        self.pixels_per_point != now.pixels_per_point
            || self.max_texture_side != now.max_texture_side
            || self.image_size != now.image_size
            || now.fill_ratio < self.fill_ratio
    }
}

#[derive(Default)]
pub struct GlyphCache {
    /// Point size the cached galleys were laid out at.  A font-size change
    /// (zoom, config reload) invalidates every one of them.
    size: f32,
    /// The cell the entries were fitted to, so a metrics change that leaves
    /// the point size alone still discards them.
    cell: Option<Cell>,
    /// The atlas the entries were laid out against, once a frame has observed
    /// one.  `None` before the first `begin_frame`, when there is nothing
    /// cached to invalidate.
    atlas: Option<AtlasState>,
    entries: HashMap<(char, Face), Glyph>,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every galley egui's font atlas has outlived.  Call once per frame
    /// before any `get`.
    ///
    /// `epaint::Fonts::begin_pass` replaces the whole atlas — and drops egui's
    /// own galley cache with it — when the scale changes, the texture limit
    /// changes, or the atlas passes 80% full.  Glyphs are repacked into
    /// different positions, so a galley held across that boundary addresses
    /// whatever landed in its old slot and paints some other character.
    pub fn begin_frame(&mut self, ctx: &Context) {
        let now = AtlasState::read(ctx);
        if self.atlas.is_some_and(|prev| prev.outlived_by(now)) {
            self.entries.clear();
        }
        self.atlas = Some(now);
    }

    /// The glyph for `ch` in `face`, laid out once and reused.  Colour is not
    /// baked in: callers override it per cell.
    pub fn get(
        &mut self,
        ctx: &Context,
        ch: char,
        face: Face,
        size: f32,
        cell: Option<Cell>,
    ) -> Glyph {
        if self.size != size || self.cell != cell {
            self.entries.clear();
            self.size = size;
            self.cell = cell;
        }
        if let Some(glyph) = self.entries.get(&(ch, face)) {
            return glyph.clone();
        }
        let glyph = lay_out(ctx, ch, face, size, cell);
        self.entries.insert((ch, face), glyph.clone());
        glyph
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context with fonts available and the three named terminal families
    /// bound, as `fonts::install_terminal_fonts` leaves it in the app.  egui
    /// has no fonts at all until a frame has run, and panics on a family it
    /// was never given.
    fn ctx() -> Context {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let mono = fonts.families[&FontFamily::Monospace].clone();
        for name in [BOLD_FAMILY, ITALIC_FAMILY, BOLD_ITALIC_FAMILY] {
            fonts.families.insert(FontFamily::Name(name.into()), mono.clone());
        }
        ctx.set_fonts(fonts);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    /// A context whose only monospace face is a deliberately oversized one,
    /// standing in for a fallback whose em is wider than the terminal font's.
    /// The cell every test pairs it with comes from `ctx()`, the unscaled
    /// face, so each glyph here overruns exactly the way a fallback does.
    fn oversized_ctx() -> Context {
        const OVERSIZED: &str = "oversized";
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let mut data = (*fonts.font_data["Hack"]).clone();
        data.tweak.scale = 4.0;
        fonts.font_data.insert(OVERSIZED.into(), Arc::new(data));
        for family in [
            FontFamily::Monospace,
            FontFamily::Name(BOLD_FAMILY.into()),
            FontFamily::Name(ITALIC_FAMILY.into()),
            FontFamily::Name(BOLD_ITALIC_FAMILY.into()),
        ] {
            fonts.families.insert(family, vec![OVERSIZED.into()]);
        }
        ctx.set_fonts(fonts);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    /// The cell the terminal derives from its primary face: one `'a'` wide.
    fn terminal_cell(size: f32) -> Cell {
        let galley = lay_out(&ctx(), 'a', Face::Normal, size, None).galley;
        Cell { width: galley.size().x, height: galley.size().y }
    }

    /// A fallback face is drawn at the primary's point size, not fitted to its
    /// cell, so a wider em spills into the neighbouring column.
    #[test]
    fn an_oversized_glyph_is_shrunk_into_its_cell() {
        let ctx = oversized_ctx();
        let cell = terminal_cell(14.0);
        let mut cache = GlyphCache::new();

        let loose = cache.get(&ctx, 'a', Face::Normal, 14.0, None).galley.size().x;
        assert!(loose > cell.width, "the face is not overrunning, so this cannot detect fitting");

        let mut cache = GlyphCache::new();
        let fitted = cache.get(&ctx, 'a', Face::Normal, 14.0, Some(cell)).galley.size().x;

        assert!(fitted <= cell.width, "{fitted} wide in a {} cell", cell.width);
    }

    /// Fitting shrinks the whole glyph, so a fitted line is shorter than the
    /// cell.  Left at the cell's top it would hang with all the slack beneath
    /// it, out of line with the text around it.
    #[test]
    fn a_shrunken_glyph_is_centred_in_the_line() {
        let ctx = oversized_ctx();

        assert!(
            GlyphCache::new().get(&ctx, 'a', Face::Normal, 14.0, Some(terminal_cell(14.0))).dy
                > 0.0
        );
    }

    /// Only glyphs that overrun are touched: shrinking one that already fits
    /// would resize ordinary text.
    #[test]
    fn a_glyph_that_already_fits_is_laid_out_untouched() {
        let ctx = oversized_ctx();
        let roomy = Cell { width: 500.0, height: 500.0 };
        let mut cache = GlyphCache::new();

        let glyph = cache.get(&ctx, 'a', Face::Normal, 14.0, Some(roomy));

        assert_eq!(glyph.galley.size(), lay_out(&ctx, 'a', Face::Normal, 14.0, None).galley.size());
        assert_eq!(glyph.dy, 0.0);
    }

    /// A character claiming two columns is fitted to both of them rather than
    /// squeezed into the first.
    #[test]
    fn a_double_width_character_is_fitted_to_both_its_cells() {
        let ctx = oversized_ctx();
        let cell = terminal_cell(14.0);
        let mut cache = GlyphCache::new();

        let fitted = cache.get(&ctx, '😀', Face::Normal, 14.0, Some(cell)).galley.size().x;

        assert!(fitted <= cell.width * 2.0, "{fitted} wide in two {} cells", cell.width);
        assert!(fitted > cell.width, "{fitted} was squeezed into one {} cell", cell.width);
    }

    /// Cell size follows the font's metrics, which a family change moves
    /// without moving the point size.  Galleys fitted to the old cell would
    /// keep their old scale until the cache happened to miss.
    #[test]
    fn a_cell_size_change_discards_the_cached_glyphs() {
        let ctx = oversized_ctx();
        let cell = terminal_cell(14.0);
        let mut cache = GlyphCache::new();
        cache.get(&ctx, 'a', Face::Normal, 14.0, Some(cell));

        cache.get(&ctx, 'a', Face::Normal, 14.0, Some(Cell { width: cell.width * 2.0, ..cell }));

        assert_eq!(cache.len(), 1, "galleys fitted to the old cell survived the change");
    }

    /// The whole point: painting the same character again must not lay it out
    /// again, however many cells show it.
    #[test]
    fn a_repeated_character_is_laid_out_once() {
        let ctx = ctx();
        let mut cache = GlyphCache::new();

        for _ in 0..100 {
            cache.get(&ctx, 'a', Face::Normal, 14.0, None);
        }

        assert_eq!(cache.len(), 1);
    }

    /// Bold and italic are separate faces, so they cannot share a galley —
    /// reusing one would paint every bold cell in the regular face.
    #[test]
    fn each_face_gets_its_own_galley() {
        let ctx = ctx();
        let mut cache = GlyphCache::new();

        for face in [Face::Normal, Face::Bold, Face::Italic, Face::BoldItalic] {
            cache.get(&ctx, 'a', face, 14.0, None);
        }

        assert_eq!(cache.len(), 4);
    }

    /// Galleys carry their laid-out size, so a zoom step has to discard them —
    /// keeping them would paint the old size until the cache happened to miss.
    #[test]
    fn a_font_size_change_discards_the_cached_galleys() {
        let ctx = ctx();
        let mut cache = GlyphCache::new();
        cache.get(&ctx, 'a', Face::Normal, 14.0, None);

        cache.get(&ctx, 'b', Face::Normal, 20.0, None);

        assert_eq!(cache.len(), 1, "galleys laid out at the old size survived a size change");
    }

    /// The atlas position of the first vertex of a galley's mesh — where in
    /// the font texture painting this galley actually samples from.
    fn atlas_pos(galley: &Galley) -> egui::Pos2 {
        galley.rows[0].visuals.mesh.vertices[0].uv
    }

    /// egui repacks the whole font atlas when the scale changes, and drops its
    /// own galley cache doing it.  A galley kept across that boundary still
    /// addresses the slot it had in the discarded atlas, so it paints whatever
    /// character was repacked into that slot instead of its own.
    #[test]
    fn a_repacked_atlas_discards_the_cached_galleys() {
        let ctx = ctx();
        let mut cache = GlyphCache::new();
        cache.begin_frame(&ctx);
        let before = atlas_pos(&cache.get(&ctx, 'a', Face::Normal, 14.0, None).galley);

        ctx.set_pixels_per_point(2.0);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        cache.begin_frame(&ctx);
        let served = atlas_pos(&cache.get(&ctx, 'a', Face::Normal, 14.0, None).galley);

        let repacked = ctx.fonts(|f| f.layout_job(glyph_job('a', Face::Normal, 14.0)));
        assert_ne!(
            before,
            atlas_pos(&repacked),
            "the scale change did not move 'a' in the atlas, so this cannot detect a stale galley"
        );
        assert_eq!(
            served,
            atlas_pos(&repacked),
            "cache served a galley addressing the discarded atlas"
        );
    }

    #[test]
    fn face_maps_bold_and_italic_flags() {
        assert_eq!(Face::new(false, false), Face::Normal);
        assert_eq!(Face::new(true, false), Face::Bold);
        assert_eq!(Face::new(false, true), Face::Italic);
        assert_eq!(Face::new(true, true), Face::BoldItalic);
    }
}
