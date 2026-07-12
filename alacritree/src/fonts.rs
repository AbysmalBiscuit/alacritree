//! Resolve font faces and register them as egui font families.
//!
//! Four faces are loaded so ANSI bold/italic cells use real Bold/Italic
//! glyphs.  On Unix we go through libfontconfig directly (same pattern flow
//! as `crossfont::ft::FreeTypeRasterizer::get_face`) — `fc-match` on the CLI
//! mishandles `family:weight=bold` patterns when the family is an `<alias>`,
//! so building the pattern programmatically is what makes weight/slant pick
//! the real variant for aliased families.
//!
//! Beyond the four explicit faces we ask fontconfig for a `FcFontSort`
//! Unicode-coverage-trimmed list and register every entry as a fallback.
//! egui resolves glyphs by walking each family's font list in order, so
//! this is what mirrors alacritty/crossfont's per-glyph fallback for
//! symbols and box-drawing characters that aren't in the primary face.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily, FontTweak};

use crate::config::FontConfig;

/// Hard cap on fallback faces.  fontconfig's trimmed sort tops out at a few
/// dozen on a typical system; this just bounds startup memory and parse cost
/// when someone has hundreds of fonts installed.
const MAX_FALLBACK_FACES: usize = 32;

pub const BOLD_FAMILY: &str = "alacritree_bold";
pub const ITALIC_FAMILY: &str = "alacritree_italic";
pub const BOLD_ITALIC_FAMILY: &str = "alacritree_bold_italic";

const NORMAL_FONT_ID: &str = "alacritree_terminal_normal";
const BOLD_FONT_ID: &str = "alacritree_terminal_bold";
const ITALIC_FONT_ID: &str = "alacritree_terminal_italic";
const BOLD_ITALIC_FONT_ID: &str = "alacritree_terminal_bold_italic";

#[derive(Clone, Copy)]
enum Variant {
    Normal,
    Bold,
    Italic,
    BoldItalic,
}

impl Variant {
    fn label(self) -> &'static str {
        match self {
            Variant::Normal => "regular",
            Variant::Bold => "bold",
            Variant::Italic => "italic",
            Variant::BoldItalic => "bold italic",
        }
    }
}

/// Platform default that mirrors `crossfont::FontDescription::default`.  Used
/// when the user hasn't set `[font.normal] family`, so alacritree picks the
/// same face alacritty would pick from the same (empty) config.
const DEFAULT_FAMILY: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(windows) {
    "Consolas"
} else {
    "monospace"
};

/// Lazily-loaded system font database shared by every resolution within one
/// `install_terminal_fonts` call.  Loading is deferred so Unix systems where
/// fontconfig answers everything never pay for a fontdb scan.
#[derive(Default)]
struct SystemFonts {
    db: OnceCell<fontdb::Database>,
    #[cfg(not(unix))]
    coverage: OnceCell<Vec<(coverage::Candidate, coverage::Coverage)>>,
}

impl SystemFonts {
    fn db(&self) -> &fontdb::Database {
        self.db.get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            db
        })
    }

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
}

/// Bookkeeping shared by all fallback registration within one install: which
/// files already back an egui font, which font id serves each file (so one
/// file can join several variants' family lists without duplicate data), and
/// which user entries have already produced a warning.
#[derive(Default)]
struct FallbackBook {
    loaded_paths: HashSet<PathBuf>,
    ids_by_path: HashMap<PathBuf, String>,
    warned_entries: HashSet<String>,
    /// Height ratio of the primary normal face, used to normalize fallback
    /// faces to the same visual size at a given point size.
    primary_height_ratio: Option<f32>,
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
        let tweak = fallback_tweak(book.primary_height_ratio, &bytes, 0);
        let data = FontData { tweak, ..FontData::from_owned(bytes) };
        defs.font_data.insert(id.clone(), Arc::new(data));
        for family in targets {
            defs.families.entry(family.clone()).or_default().push(id.clone());
        }
        book.loaded_paths.insert(resolved.path.clone());
        book.ids_by_path.insert(resolved.path, id);
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

/// A font's visual height for a given point size is
/// `(ascender - descender) / units_per_em`, which varies between fonts.
fn face_height_ratio(data: &[u8], index: u32) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let units = f32::from(face.units_per_em());
    let height = f32::from(face.ascender()) - f32::from(face.descender());
    (units > 0.0 && height > 0.0).then(|| height / units)
}

/// Scale a fallback face so one point of it is as tall as one point of the
/// primary face; without this, powerline caps, emoji, and CJK glyphs from
/// fallback fonts overshoot or undershoot the cell.  Clamped so a face with
/// broken metrics cannot render unreadably small or huge.
fn fallback_tweak(primary_ratio: Option<f32>, data: &[u8], index: u32) -> FontTweak {
    let scale = match (primary_ratio, face_height_ratio(data, index)) {
        (Some(primary), Some(own)) => (primary / own).clamp(0.5, 2.0),
        _ => 1.0,
    };
    FontTweak { scale, ..FontTweak::default() }
}

pub fn install_terminal_fonts(ctx: &Context, font: &FontConfig) {
    let (normal, bold, italic, bold_italic) =
        (&font.normal, &font.bold, &font.italic, &font.bold_italic);
    let family = normal.family.as_deref().unwrap_or(DEFAULT_FAMILY);
    let fonts = SystemFonts::default();

    // The variant lookups compare their resolved path against this one to
    // detect when fontconfig substituted the regular face for a missing variant.
    let normal_match = match resolve_face(family, normal.style.as_deref(), Variant::Normal, &fonts)
    {
        Some(m) => m,
        None => {
            log::warn!("could not resolve font '{family}'; using bundled monospace");
            return;
        },
    };
    let normal_bytes = match std::fs::read(&normal_match.path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("could not read font file {}: {e}", normal_match.path.display());
            return;
        },
    };

    // Bold/italic/bold-italic inherit the normal family unless overridden.
    let bold_family = bold.family.as_deref().unwrap_or(family);
    let italic_family = italic.family.as_deref().unwrap_or(family);
    let bold_italic_family = bold_italic.family.as_deref().unwrap_or(family);

    let bold_bytes =
        load_variant(bold_family, bold.style.as_deref(), Variant::Bold, &normal_match.path, &fonts);
    let italic_bytes = load_variant(
        italic_family,
        italic.style.as_deref(),
        Variant::Italic,
        &normal_match.path,
        &fonts,
    );
    let bold_italic_bytes = load_variant(
        bold_italic_family,
        bold_italic.style.as_deref(),
        Variant::BoldItalic,
        &normal_match.path,
        &fonts,
    );

    let mut defs = FontDefinitions::default();

    insert_face(&mut defs, NORMAL_FONT_ID, normal_bytes.clone());
    register_default_family(&mut defs, FontFamily::Monospace, NORMAL_FONT_ID);
    register_default_family(&mut defs, FontFamily::Proportional, NORMAL_FONT_ID);

    register_variant(&mut defs, BOLD_FONT_ID, BOLD_FAMILY, bold_bytes, &normal_bytes);
    register_variant(&mut defs, ITALIC_FONT_ID, ITALIC_FAMILY, italic_bytes, &normal_bytes);
    register_variant(
        &mut defs,
        BOLD_ITALIC_FONT_ID,
        BOLD_ITALIC_FAMILY,
        bold_italic_bytes,
        &normal_bytes,
    );

    let mut book = FallbackBook::default();
    book.loaded_paths.insert(normal_match.path.clone());
    book.primary_height_ratio = face_height_ratio(&normal_bytes, 0);

    // Each variant gets its own fallback chain seeded from that variant's
    // configured family — same as crossfont's per-FontDesc fallback search,
    // so bold cells cascade through bold's chain and so on.
    let normal_targets = [FontFamily::Monospace, FontFamily::Proportional];
    let variant_targets =
        [BOLD_FAMILY, ITALIC_FAMILY, BOLD_ITALIC_FAMILY].map(|n| [FontFamily::Name(n.into())]);
    let seeds: [(&str, Option<&str>, Variant, &[FontFamily]); 4] = [
        (family, normal.style.as_deref(), Variant::Normal, &normal_targets),
        (bold_family, bold.style.as_deref(), Variant::Bold, &variant_targets[0]),
        (italic_family, italic.style.as_deref(), Variant::Italic, &variant_targets[1]),
        (
            bold_italic_family,
            bold_italic.style.as_deref(),
            Variant::BoldItalic,
            &variant_targets[2],
        ),
    ];
    for (family, style, variant, targets) in seeds {
        register_user_fallbacks(&mut defs, &font.fallback, variant, targets, &fonts, &mut book);
        register_fallback_faces(&mut defs, family, style, variant, targets, &fonts, &mut book);
    }

    ctx.set_fonts(defs);
}

/// Append every font from fontconfig's trimmed sort to `target_families` so
/// that glyphs missing from the primary face (symbols, box drawing, emoji)
/// fall through to a system font that has them.  Mirrors what crossfont does
/// per-glyph in upstream alacritty.
fn register_fallback_faces(
    defs: &mut FontDefinitions,
    family: &str,
    style: Option<&str>,
    variant: Variant,
    target_families: &[FontFamily],
    fonts: &SystemFonts,
    book: &mut FallbackBook,
) {
    // Only primaries lack an id to reuse; everything else the chain finds
    // can join this variant's family list without reloading.
    let primaries: HashSet<PathBuf> = book
        .loaded_paths
        .iter()
        .filter(|path| !book.ids_by_path.contains_key(*path))
        .cloned()
        .collect();
    let fallbacks =
        gather_fallback_faces(family, style, variant, &primaries, MAX_FALLBACK_FACES, fonts);
    if fallbacks.is_empty() {
        return;
    }

    for face in fallbacks {
        if let Some(id) = book.ids_by_path.get(&face.path) {
            for family in target_families {
                defs.families.entry(family.clone()).or_default().push(id.clone());
            }
            continue;
        }
        let bytes = match std::fs::read(&face.path) {
            Ok(b) => b,
            Err(e) => {
                log::debug!("skipping fallback font {}: {e}", face.path.display());
                continue;
            },
        };
        let id = format!("alacritree_fallback_{}", defs.font_data.len());
        let tweak = fallback_tweak(book.primary_height_ratio, &bytes, face.face_index);
        let data = FontData { index: face.face_index, tweak, ..FontData::from_owned(bytes) };
        defs.font_data.insert(id.clone(), Arc::new(data));

        for family in target_families {
            defs.families.entry(family.clone()).or_default().push(id.clone());
        }
        book.loaded_paths.insert(face.path.clone());
        book.ids_by_path.insert(face.path, id);
    }
}

struct FallbackFace {
    path: PathBuf,
    face_index: u32,
}

#[cfg(unix)]
fn gather_fallback_faces(
    family: &str,
    style: Option<&str>,
    variant: Variant,
    skip_paths: &HashSet<PathBuf>,
    limit: usize,
    _fonts: &SystemFonts,
) -> Vec<FallbackFace> {
    fontconfig_resolve::sorted_fallbacks(family, style, variant, skip_paths, limit)
}

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
    coverage::order_candidates(
        &mut candidates,
        family,
        weight.0,
        db_style != fontdb::Style::Normal,
    );

    coverage::trim_by_coverage(candidates, &seed_coverage, limit)
        .into_iter()
        .map(|candidate| FallbackFace { path: candidate.path, face_index: candidate.face_index })
        .collect()
}

fn insert_face(defs: &mut FontDefinitions, id: &str, bytes: Vec<u8>) {
    defs.font_data.insert(id.to_string(), Arc::new(FontData::from_owned(bytes)));
}

fn register_default_family(defs: &mut FontDefinitions, family: FontFamily, id: &str) {
    defs.families.entry(family).or_default().insert(0, id.to_string());
}

fn register_variant(
    defs: &mut FontDefinitions,
    font_id: &str,
    family_name: &str,
    bytes: Option<Vec<u8>>,
    fallback: &[u8],
) {
    let bytes = bytes.unwrap_or_else(|| fallback.to_vec());
    insert_face(defs, font_id, bytes);
    defs.families.insert(FontFamily::Name(family_name.into()), vec![font_id.to_string()]);
}

/// Returns the bytes of the variant face if a *real* variant exists, or
/// `None` if the matcher fell back to the normal file.  The caller registers
/// the normal face as a fallback under the variant's family name.
fn load_variant(
    family: &str,
    style: Option<&str>,
    variant: Variant,
    normal_path: &Path,
    fonts: &SystemFonts,
) -> Option<Vec<u8>> {
    let resolved = resolve_face(family, style, variant, fonts)?;
    if resolved.path == normal_path {
        log::debug!(
            "no real {} face for '{family}'; cells with that style will use the regular face",
            variant.label()
        );
        return None;
    }
    match std::fs::read(&resolved.path) {
        Ok(b) => Some(b),
        Err(e) => {
            log::warn!(
                "could not read {} font file {}: {e}",
                variant.label(),
                resolved.path.display()
            );
            None
        },
    }
}

struct ResolvedFace {
    path: PathBuf,
}

#[cfg(unix)]
fn resolve_face(
    family_or_path: &str,
    style: Option<&str>,
    variant: Variant,
    fonts: &SystemFonts,
) -> Option<ResolvedFace> {
    if let Some(face) = resolve_via_path(family_or_path) {
        return Some(face);
    }
    if let Some(face) = fontconfig_resolve::resolve(family_or_path, style, variant) {
        return Some(face);
    }
    // fontdb fallback for the case where libfontconfig isn't available; it
    // doesn't expand <alias> rules, so it's strictly second-best on Unix.
    resolve_via_fontdb(family_or_path, variant, fonts)
}

#[cfg(not(unix))]
fn resolve_face(
    family_or_path: &str,
    _style: Option<&str>,
    variant: Variant,
    fonts: &SystemFonts,
) -> Option<ResolvedFace> {
    if let Some(face) = resolve_via_path(family_or_path) {
        return Some(face);
    }
    resolve_via_fontdb(family_or_path, variant, fonts)
}

fn resolve_via_path(family_or_path: &str) -> Option<ResolvedFace> {
    let path = Path::new(family_or_path);
    if path.is_file() {
        return Some(ResolvedFace { path: path.to_path_buf() });
    }
    None
}

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

#[cfg(unix)]
mod fontconfig_resolve {
    //! Mirrors `crossfont::ft::FreeTypeRasterizer::get_face`: build a pattern
    //! with family + weight + slant and let `font_match` run substitution.
    //! Doing this in code (vs `fc-match` CLI) is what makes `<alias>` rules
    //! plus weight/slant pick the right variant.

    use std::collections::HashSet;
    use std::ffi::CString;
    use std::path::PathBuf;

    use fontconfig::{
        FC_FAMILY, FC_SLANT, FC_SLANT_ITALIC, FC_SLANT_ROMAN, FC_STYLE, FC_WEIGHT, FC_WEIGHT_BOLD,
        FC_WEIGHT_REGULAR, Fontconfig, Pattern, sort_fonts,
    };

    use super::{FallbackFace, ResolvedFace, Variant};

    pub fn resolve(family: &str, style: Option<&str>, variant: Variant) -> Option<ResolvedFace> {
        let fc = Fontconfig::new()?;
        let mut pattern = Pattern::new(&fc);

        let family_c = CString::new(family).ok()?;
        pattern.add_string(FC_FAMILY, &family_c);

        if let Some(style) = style {
            if let Ok(style_c) = CString::new(style) {
                pattern.add_string(FC_STYLE, &style_c);
            }
        }

        let (weight, slant) = match variant {
            Variant::Normal => (FC_WEIGHT_REGULAR, FC_SLANT_ROMAN),
            Variant::Bold => (FC_WEIGHT_BOLD, FC_SLANT_ROMAN),
            Variant::Italic => (FC_WEIGHT_REGULAR, FC_SLANT_ITALIC),
            Variant::BoldItalic => (FC_WEIGHT_BOLD, FC_SLANT_ITALIC),
        };
        pattern.add_integer(FC_WEIGHT, weight);
        pattern.add_integer(FC_SLANT, slant);

        let matched = pattern.font_match();
        let path = matched.filename()?;
        Some(ResolvedFace { path: PathBuf::from(path) })
    }

    /// `FcFontSort` with `trim=true` returns fonts in match order, dropping
    /// any whose Unicode coverage is fully covered by an earlier entry.  This
    /// is the same chain `FcFontMatch` walks per glyph when crossfont misses,
    /// so registering it up front in egui gives equivalent coverage.
    pub fn sorted_fallbacks(
        family: &str,
        style: Option<&str>,
        variant: Variant,
        skip_paths: &HashSet<PathBuf>,
        limit: usize,
    ) -> Vec<FallbackFace> {
        let Some(fc) = Fontconfig::new() else {
            return Vec::new();
        };
        let mut pattern = Pattern::new(&fc);

        if let Ok(family_c) = CString::new(family) {
            pattern.add_string(FC_FAMILY, &family_c);
        }
        if let Some(style) = style {
            if let Ok(style_c) = CString::new(style) {
                pattern.add_string(FC_STYLE, &style_c);
            }
        }
        let (weight, slant) = match variant {
            Variant::Normal => (FC_WEIGHT_REGULAR, FC_SLANT_ROMAN),
            Variant::Bold => (FC_WEIGHT_BOLD, FC_SLANT_ROMAN),
            Variant::Italic => (FC_WEIGHT_REGULAR, FC_SLANT_ITALIC),
            Variant::BoldItalic => (FC_WEIGHT_BOLD, FC_SLANT_ITALIC),
        };
        pattern.add_integer(FC_WEIGHT, weight);
        pattern.add_integer(FC_SLANT, slant);

        // FcFontSort requires FcConfigSubstitute + FcDefaultSubstitute to have
        // been applied to the input pattern; otherwise <alias> rules never
        // expand and the result list misses the fonts the user actually has.
        // The 0.8 fontconfig wrapper keeps those private but applies them as
        // a side effect inside `font_match`, so we run it for the side effect
        // and discard the matched pattern.
        let _ = pattern.font_match();

        let sorted = sort_fonts(&pattern, true);
        let mut out = Vec::with_capacity(limit.min(16));
        for matched in sorted.iter() {
            if out.len() >= limit {
                break;
            }
            let Some(path_str) = matched.filename() else {
                continue;
            };
            let path = PathBuf::from(path_str);
            if skip_paths.contains(&path) {
                continue;
            }
            let face_index = matched.face_index().unwrap_or(0).max(0) as u32;
            out.push(FallbackFace { path, face_index });
        }
        out
    }
}

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
        register_user_fallbacks(
            &mut defs,
            &entries,
            Variant::Normal,
            &normal_targets,
            &fonts,
            &mut book,
        );
        let bold_targets = [FontFamily::Name(BOLD_FAMILY.into())];
        register_user_fallbacks(
            &mut defs,
            &entries,
            Variant::Bold,
            &bold_targets,
            &fonts,
            &mut book,
        );

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

    #[cfg(not(unix))]
    #[test]
    fn later_variants_reuse_faces_loaded_by_an_earlier_chain() {
        let fonts = SystemFonts::default();
        let mut defs = FontDefinitions::default();
        let mut book = FallbackBook::default();

        let normal_targets = [FontFamily::Monospace];
        register_fallback_faces(
            &mut defs,
            "Consolas",
            None,
            Variant::Normal,
            &normal_targets,
            &fonts,
            &mut book,
        );
        let normal_ids: HashSet<String> = book.ids_by_path.values().cloned().collect();

        let bold_family = FontFamily::Name(BOLD_FAMILY.into());
        let bold_targets = [bold_family.clone()];
        register_fallback_faces(
            &mut defs,
            "Consolas",
            Some("Bold"),
            Variant::Bold,
            &bold_targets,
            &fonts,
            &mut book,
        );

        if fonts.db().faces().next().is_some() && !normal_ids.is_empty() {
            // The bold chain must be able to reach faces the normal chain already
            // loaded; on any real system at least one top coverage-adder overlaps.
            assert!(defs.families[&bold_family].iter().any(|id| normal_ids.contains(id)));
        }
    }

    #[cfg(not(unix))]
    #[test]
    fn automatic_chain_records_every_loaded_path_in_ids_by_path() {
        let mut defs = FontDefinitions::default();
        let fonts = SystemFonts::default();
        let mut book = FallbackBook::default();

        let targets = [FontFamily::Monospace];
        register_fallback_faces(
            &mut defs,
            "Consolas",
            None,
            Variant::Normal,
            &targets,
            &fonts,
            &mut book,
        );

        if fonts.db().faces().next().is_some() {
            let ids_keys: HashSet<_> = book.ids_by_path.keys().cloned().collect();
            assert_eq!(ids_keys, book.loaded_paths);
        }
    }

    #[test]
    fn fallback_tweak_defaults_to_unscaled_for_unparseable_data() {
        let tweak = fallback_tweak(Some(1.2), b"not a font", 0);
        assert_eq!(tweak.scale, 1.0);
    }

    #[test]
    fn user_fallbacks_precede_the_automatic_chain() {
        // User-configured fallbacks slot between the primary face and the
        // automatic system chain, so their font id must land ahead of every
        // id the automatic chain appends afterward in the family list.
        let path = std::env::temp_dir().join("alacritree_test_user_precedes.ttf");
        std::fs::write(&path, b"egui parses this later; registration only reads bytes").unwrap();

        let mut defs = FontDefinitions::default();
        let fonts = SystemFonts::default();
        let mut book = FallbackBook::default();
        let entries = vec![path.to_string_lossy().into_owned()];
        let targets = [FontFamily::Monospace];

        register_user_fallbacks(&mut defs, &entries, Variant::Normal, &targets, &fonts, &mut book);
        let user_id = book.ids_by_path.get(&path).cloned().unwrap();
        let user_index =
            defs.families[&FontFamily::Monospace].iter().position(|id| *id == user_id).unwrap();
        let before_len = defs.families[&FontFamily::Monospace].len();

        register_fallback_faces(
            &mut defs,
            DEFAULT_FAMILY,
            None,
            Variant::Normal,
            &targets,
            &fonts,
            &mut book,
        );

        let family_list = &defs.families[&FontFamily::Monospace];
        if family_list.len() > before_len {
            for id in &family_list[before_len..] {
                let index = family_list.iter().position(|x| x == id).unwrap();
                assert!(index > user_index);
            }
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn primary_faces_are_never_tweaked() {
        // Fallback faces get a tweak that rescales them to the primary
        // face's height; the primary itself is the scale reference and must
        // never carry a tweak, or scaling would be relative to a moving target.
        let mut defs = FontDefinitions::default();
        insert_face(
            &mut defs,
            "test_primary",
            b"egui parses this later; registration only reads bytes".to_vec(),
        );
        assert_eq!(defs.font_data["test_primary"].tweak, FontTweak::default());
    }

    #[cfg(not(unix))]
    #[test]
    fn fallback_tweak_normalizes_height_to_the_primary_face() {
        // Any real font gives a positive height ratio; scaling it against a
        // primary of half / double its ratio must move scale in that direction.
        let data = std::fs::read("C:/Windows/Fonts/arial.ttf").unwrap();
        let own = face_height_ratio(&data, 0).unwrap();
        assert!(own > 0.0);
        assert_eq!(fallback_tweak(Some(own), &data, 0).scale, 1.0);
        assert!(fallback_tweak(Some(own * 1.5), &data, 0).scale > 1.0);
        assert!(fallback_tweak(Some(own * 0.5), &data, 0).scale < 1.0);
    }
}

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
            assert_eq!(
                order,
                [
                    ("Seed Family", 400), // same family wins even without a style match
                    ("Beta", 700),        // style match + monospace
                    ("Beta", 400),        // monospace
                    ("Alpha", 700),       // italic mismatches the variant; name order
                    ("Zeta", 400),
                ]
            );
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
