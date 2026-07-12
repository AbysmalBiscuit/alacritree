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

use egui::{Context, FontData, FontDefinitions, FontFamily};

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

fn variant_query(variant: Variant) -> (fontdb::Weight, fontdb::Style) {
    match variant {
        Variant::Normal => (fontdb::Weight::NORMAL, fontdb::Style::Normal),
        Variant::Bold => (fontdb::Weight::BOLD, fontdb::Style::Normal),
        Variant::Italic => (fontdb::Weight::NORMAL, fontdb::Style::Italic),
        Variant::BoldItalic => (fontdb::Weight::BOLD, fontdb::Style::Italic),
    }
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
    let fallbacks = gather_fallback_faces(
        family,
        style,
        variant,
        &book.loaded_paths,
        MAX_FALLBACK_FACES,
        fonts,
    );
    if fallbacks.is_empty() {
        return;
    }

    for face in fallbacks {
        let bytes = match std::fs::read(&face.path) {
            Ok(b) => b,
            Err(e) => {
                log::debug!("skipping fallback font {}: {e}", face.path.display());
                continue;
            },
        };
        let id = format!("alacritree_fallback_{}", defs.font_data.len());
        let data = FontData { index: face.face_index, ..FontData::from_owned(bytes) };
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
fn gather_fallback_faces(
    _family: &str,
    _style: Option<&str>,
    _variant: Variant,
    _skip_paths: &HashSet<PathBuf>,
    _limit: usize,
    _fonts: &SystemFonts,
) -> Vec<FallbackFace> {
    Vec::new()
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
}
