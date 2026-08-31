# Sidebar Appearance Customization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Four opt-in appearance features for the alacritree GUI: an independent UI font family/size, PR status icons on worktree rows, configurable status glyphs, and a focus outline on the focused panel.

**Architecture:** All work lands in the `alacritree/` crate on one branch (`feat/sidebar-appearance` off `master`). Config parsing extends `config.rs` (new `[ui.font]`, `[ui.icons]`, `[ui.focus_outline]` tables and `[ui] pr_status`), theme derivation extends `Theme` in `app.rs`, font installation extends `fonts.rs`, and PR data extends `pr_status.rs`. Every default reproduces today's behavior exactly.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui 0.31, serde/toml, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-16-sidebar-appearance-design.md` (untracked — never commit it).

## Global Constraints

- Branch: `feat/sidebar-appearance`, created from `master`. Never commit on `master` directly.
- Only touch files under `alacritree/` (and this plan's checkboxes). The `alacritty*/` crates are vendored upstream — read-only.
- **Never commit anything under `docs/superpowers/`** — specs and plans stay untracked. Stage files explicitly (`git add <paths>`); never `git add -A` or `git add .`.
- Defaults must keep today's behavior bit-for-bit: unset `[ui.font]` → current derived sizes; unset icons → today's glyphs; `pr_status` defaults `true` but paints nothing new until data exists; focus outline defaults off.
- Commit messages: Conventional Commits, imperative mood, subject ≤ 72 chars, lowercase after the colon. End every commit message with the trailer line `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Run `cargo fmt` before every commit (rustfmt is enforced).
- Test command: `cargo test -p alacritree`. Fast check loop: `cargo check -p alacritree`.
- Do NOT run the built GUI, and never kill any running `alacritree.exe` — the user has live sessions.
- Comments explain *why*, never *what*; no task/PR references in comments.
- Search with `rg` / `fd` only (never `grep`/`find`; never `rg -r` — `-r` is `--replace`, not recursion).

## File Structure

- `alacritree/src/config.rs` — new `UiFont`, `Icons`, `FocusOutline` structs + raw parse structs; `UiTheme` gains `pr_status`, `icons`, `focus_outline`; `Config` gains `ui_font`. All unit-tested here.
- `alacritree/src/app.rs` — `Theme` derivation (`ui_text_px`, PR colors, focus-outline theme), sidebar wiring (icons params, PR precompute + badge, outline painting).
- `alacritree/src/fonts.rs` — `install_ui_font` (UI family at the head of egui's `Proportional`), `install_terminal_fonts` signature gains the UI family.
- `alacritree/src/pr_status.rs` — `PrState` enum, extended `gh` query and parser.

---

### Task 1: `[ui.font]` config + UI text-size derivation

**Files:**
- Modify: `alacritree/src/config.rs` (structs near `UiTheme` at line ~234, `RawUi` at ~787, `into_config` at ~873, tests at end)
- Modify: `alacritree/src/app.rs` (`Theme::from_config` at ~67, new pure fn + tests)

**Interfaces:**
- Produces: `config::UiFont { pub family: Option<String>, pub size: Option<f32> }`; `Config.ui_font: UiFont`; `app::ui_text_px(&FontConfig, &UiFont) -> (f32, f32)` returning `(font_normal, font_heading)` in logical px. Task 2 consumes `config.ui_font.family`.

- [ ] **Step 1: Create the branch**

```bash
git checkout -b feat/sidebar-appearance master
```

(If executing in a worktree created by the worktree skill, the branch may already exist — verify with `git branch --show-current`.)

- [ ] **Step 2: Write the failing config tests**

Append to the existing `mod tests` in `alacritree/src/config.rs` (it already has a `parse(s: &str) -> Config` helper):

```rust
#[test]
fn ui_font_defaults_to_none() {
    let config = parse("");
    assert_eq!(config.ui_font, UiFont::default());
}

#[test]
fn ui_font_parses_family_and_size() {
    let config = parse("[ui.font]\nfamily = \"Inter\"\nsize = 12.5");
    assert_eq!(config.ui_font.family.as_deref(), Some("Inter"));
    assert_eq!(config.ui_font.size, Some(12.5));
}

#[test]
fn ui_font_size_clamps_to_one() {
    let config = parse("[ui.font]\nsize = 0.1");
    assert_eq!(config.ui_font.size, Some(1.0));
}

#[test]
fn blank_ui_font_family_is_ignored() {
    let config = parse("[ui.font]\nfamily = \"  \"");
    assert_eq!(config.ui_font.family, None);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alacritree ui_font`
Expected: compile error — `UiFont` not defined.

- [ ] **Step 4: Implement the config side**

In `alacritree/src/config.rs`:

Add near `UiTheme` (after the `ConfirmSessionClose` block):

```rust
/// alacritree-only `[ui.font]`: font family/size for the chrome (sidebars,
/// modals — everything that isn't the terminal grid).  Both fields default
/// to deriving from `[font]`, so an absent table changes nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiFont {
    pub family: Option<String>,
    /// Typographic points, same unit as `[font] size`; clamped to ≥ 1.0.
    pub size: Option<f32>,
}
```

Add `pub ui_font: UiFont,` to `struct Config` and `ui_font: UiFont::default(),` to `impl Default for Config`.

Add to `struct RawUi`: `font: RawUiFont,` and define:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawUiFont {
    family: Option<String>,
    size: Option<f32>,
}
```

In `RawConfig::into_config`, before the final `Config { ... }` literal (e.g. next to the `// ---- Font ----` section), add:

```rust
let ui_font = UiFont {
    family: self.ui.font.family.clone().filter(|f| !f.trim().is_empty()),
    size: self.ui.font.size.map(|s| s.max(1.0)),
};
```

and add `ui_font,` to the returned `Config { ... }` literal.

Note: `self.ui` fields are consumed piecemeal in `into_config` — build `ui_font` before/after the `UiTheme` literal, whichever avoids partial-move errors (the `UiTheme` literal only moves color/notification fields, so ordering is flexible; use `.clone()` as shown to sidestep it entirely).

- [ ] **Step 5: Run config tests**

Run: `cargo test -p alacritree ui_font`
Expected: 4 passed.

- [ ] **Step 6: Write the failing theme-derivation tests**

`alacritree/src/app.rs` has a `#[cfg(test)] mod tests` at the end of the file (it tests `move_target` and friends). If it exists, append there; otherwise create one with `use super::*;`. Add:

```rust
#[test]
fn ui_text_px_defaults_to_terminal_derivation() {
    let font = crate::config::FontConfig::default();
    let (normal, heading) = ui_text_px(&font, &crate::config::UiFont::default());
    assert_eq!(normal, font.ui_normal_px());
    assert_eq!(heading, font.ui_heading_px());
}

#[test]
fn ui_text_px_overrides_from_ui_font_size() {
    let font = crate::config::FontConfig::default();
    let ui = crate::config::UiFont { family: None, size: Some(12.0) };
    let (normal, heading) = ui_text_px(&font, &ui);
    assert_eq!(normal, 16.0); // 12 pt × 96/72
    assert_eq!(
        heading,
        16.0 * (crate::config::FontConfig::UI_HEADING_RATIO
            / crate::config::FontConfig::UI_NORMAL_RATIO)
    );
}
```

- [ ] **Step 7: Run to verify failure**

Run: `cargo test -p alacritree ui_text_px`
Expected: compile error — `ui_text_px` not defined.

- [ ] **Step 8: Implement `ui_text_px` and wire it into `Theme::from_config`**

In `alacritree/src/app.rs`, add near `Theme` (above `impl Theme`):

```rust
/// Logical-pixel (normal, heading) sizes for UI text.  `[ui.font] size`
/// overrides the normal size directly (same pt→px conversion as
/// `FontConfig::egui_size`); the heading keeps its existing ratio to normal
/// text.  Unset, both fall back to the `[font]`-derived values unchanged.
fn ui_text_px(font: &FontConfig, ui_font: &UiFont) -> (f32, f32) {
    match ui_font.size {
        Some(pt) => {
            let normal = pt * 96.0 / 72.0;
            let heading =
                normal * (FontConfig::UI_HEADING_RATIO / FontConfig::UI_NORMAL_RATIO);
            (normal, heading)
        },
        None => (font.ui_normal_px(), font.ui_heading_px()),
    }
}
```

Import `UiFont` in app.rs's existing `use crate::config::{...}` list (it currently imports `Config`, `FontConfig`, etc. — check with `rg -n "use crate::config" alacritree/src/app.rs`).

In `Theme::from_config` (app.rs:67), replace the three derived fields:

```rust
// before the Self { ... } literal:
let (font_normal, font_heading) = ui_text_px(&config.font, &config.ui_font);
```

and in the literal replace

```rust
font_heading: config.font.ui_heading_px(),
font_normal: config.font.ui_normal_px(),
ui_scale: config.font.ui_normal_px() / 11.25,
```

with

```rust
font_heading,
font_normal,
ui_scale: font_normal / 11.25,
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all pass (both new tests and the existing suite).

- [ ] **Step 10: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/app.rs
git commit -m "feat(ui): add [ui.font] size for an independent chrome scale

The sidebar/modal text size and every derived chrome dimension
(ui_scale) previously followed [font] size only.  [ui.font] size sets
the normal UI text size directly (points, like [font] size); heading
size and ui_scale derive from it with the existing ratios.  Unset, the
old derivation is used unchanged.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: UI font family at the head of egui's `Proportional`

**Files:**
- Modify: `alacritree/src/fonts.rs` (`install_terminal_fonts` at ~580, new `install_ui_font`, tests at end)
- Modify: `alacritree/src/app.rs` (call site at ~283)

**Interfaces:**
- Consumes: `config.ui_font.family: Option<String>` from Task 1.
- Produces: `fonts::install_terminal_fonts(ctx: &Context, font: &FontConfig, ui_family: Option<&str>) -> Vec<ChainFace>` (signature change); private `install_ui_font(defs: &mut FontDefinitions, family_or_path: &str, fonts: &SystemFonts) -> bool`.

Background you need: `install_terminal_fonts` registers the terminal face at the head of both `FontFamily::Monospace` and `FontFamily::Proportional` (fonts.rs:629-630), then appends fallback faces to both. The UI font must end up at the *head* of `Proportional` (followed by its own fallback chain, then the terminal font and its fallbacks), while `Monospace` — the grid — is untouched. The terminal's returned `book.chain` feeds the color-glyph renderer and must NOT include UI faces, so the UI font gets its own throwaway `FallbackBook`.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `alacritree/src/fonts.rs` (model: the existing `user_fallback_path_registers_for_every_variant` test — registration only reads bytes, so a fake TTF file works):

```rust
#[test]
fn ui_font_heads_the_proportional_family() {
    let path = std::env::temp_dir().join("alacritree_test_ui_font.ttf");
    std::fs::write(&path, b"registration only maps bytes").unwrap();

    let mut defs = FontDefinitions::default();
    let fonts = SystemFonts::with_cache_dir(None);
    let mono_before = defs.families[&FontFamily::Monospace].clone();

    assert!(install_ui_font(&mut defs, path.to_string_lossy().as_ref(), &fonts));

    let prop = &defs.families[&FontFamily::Proportional];
    assert_eq!(prop.first().map(String::as_str), Some(UI_FONT_ID));
    // The grid's family is untouched.
    assert_eq!(defs.families[&FontFamily::Monospace], mono_before);
    // The temporary splice family does not leak into the definitions.
    assert!(!defs.families.contains_key(&FontFamily::Name(UI_FAMILY.into())));

    std::fs::remove_file(&path).ok();
}

#[test]
fn unresolvable_ui_font_changes_nothing() {
    let mut defs = FontDefinitions::default();
    let fonts = SystemFonts::with_cache_dir(None);
    let before = defs.families[&FontFamily::Proportional].clone();

    assert!(!install_ui_font(&mut defs, "alacritree-no-such-ui-family-9f3a", &fonts));

    assert_eq!(defs.families[&FontFamily::Proportional], before);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree ui_font_heads`
Expected: compile error — `install_ui_font` / `UI_FONT_ID` / `UI_FAMILY` not defined.

- [ ] **Step 3: Implement `install_ui_font`**

In `alacritree/src/fonts.rs`, add next to the other `const *_FONT_ID` items (~line 34):

```rust
const UI_FONT_ID: &str = "alacritree_ui_normal";
/// Temporary family the UI chain is assembled under before being spliced to
/// the head of `Proportional`; removed again so it never leaks to egui.
const UI_FAMILY: &str = "alacritree_ui";
```

Add the function (near `install_terminal_fonts`):

```rust
/// Put the `[ui.font]` family — and its own fallback chain — ahead of the
/// terminal font in egui's `Proportional` family, so all chrome text prefers
/// it.  `Monospace` (the grid) is untouched.  Returns `false` and leaves the
/// definitions unchanged when the family cannot be resolved or read, in which
/// case the chrome keeps using the terminal font.
fn install_ui_font(defs: &mut FontDefinitions, family_or_path: &str, fonts: &SystemFonts) -> bool {
    let Some(resolved) = resolve_face(family_or_path, None, Variant::Normal, fonts) else {
        log::warn!("could not resolve ui font '{family_or_path}'; keeping the terminal font");
        return false;
    };
    let bytes = match map_font_file(&resolved.path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("could not read ui font file {}: {e}", resolved.path.display());
            return false;
        },
    };
    insert_face(defs, UI_FONT_ID, bytes);
    let ui_family = FontFamily::Name(UI_FAMILY.into());
    defs.families.insert(ui_family.clone(), vec![UI_FONT_ID.to_string()]);

    // Its own book: the UI chain must not leak into the terminal's normal
    // chain (which feeds the colour-glyph renderer), and fallback height
    // normalization must anchor to the UI face, not the terminal face.
    let mut book = FallbackBook::default();
    book.loaded_paths.insert(resolved.path.clone());
    book.primary_height_ratio = face_height_ratio(bytes, resolved.face_index);
    let targets = [ui_family.clone()];
    register_fallback_faces(defs, family_or_path, None, Variant::Normal, &targets, fonts, &mut book);

    // Splice the assembled UI chain ahead of everything already in
    // `Proportional` (terminal font + its fallbacks).
    let ui_ids = defs.families.remove(&ui_family).unwrap_or_default();
    let prop = defs.families.entry(FontFamily::Proportional).or_default();
    for id in ui_ids.into_iter().rev() {
        prop.insert(0, id);
    }
    true
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p alacritree -- ui_font_heads unresolvable_ui_font`
Expected: 2 passed.

- [ ] **Step 5: Wire it into `install_terminal_fonts`**

Change the signature (fonts.rs:580):

```rust
pub fn install_terminal_fonts(
    ctx: &Context,
    font: &FontConfig,
    ui_family: Option<&str>,
) -> Vec<ChainFace> {
```

Immediately before `ctx.set_fonts(defs);` (after the fallback-seeding `for` loop):

```rust
if let Some(ui_family) = ui_family {
    install_ui_font(&mut defs, ui_family, &fonts);
}
```

(The two early `return Vec::new()` paths — unresolvable/unreadable terminal font — skip the UI font too; that degenerate case already falls back to egui's bundled fonts and a warning.)

Find all callers: `rg -n "install_terminal_fonts" alacritree/src/`. The only call site is `alacritree/src/app.rs:283`; change it to:

```rust
let font_chain = crate::fonts::install_terminal_fonts(
    &cc.egui_ctx,
    &config.font,
    config.ui_font.family.as_deref(),
);
```

- [ ] **Step 6: Full test run**

Run: `cargo test -p alacritree`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/fonts.rs alacritree/src/app.rs
git commit -m "feat(fonts): let [ui.font] family override the chrome font

Resolve the configured family (or file path) through the existing
face-resolution machinery and splice it, with its own height-normalized
fallback chain, ahead of the terminal font in egui's Proportional
family.  Monospace — the grid — is untouched, and an unresolvable
family degrades to a warning plus today's behavior.  Fonts are
installed once at startup, so changes require a restart (same caveat
as window transparency).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `[ui.icons]` — configurable status glyphs

**Files:**
- Modify: `alacritree/src/config.rs` (new `Icons` struct, `RawIcons`, `UiTheme.icons`, tests)
- Modify: `alacritree/src/app.rs` (glyph call sites: `worktree_row` ~2709, `session_row` ~2841, `home_row` ~2566, `creating_row` ~2689, project arrows ~1456, plus their call sites)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `config::Icons` with `pub` `String` fields `worktree_main, worktree, session, home, project_expanded, project_collapsed, pr_open, pr_draft, pr_merged, pr_closed`; `UiTheme.icons: Icons`. Task 5 consumes the four `pr_*` fields. `worktree_row`, `session_row`, `home_row`, `creating_row` each gain an `icons: &Icons` parameter (inserted immediately before their `theme: &Theme` parameter).

- [ ] **Step 1: Write the failing config tests**

Append to `mod tests` in `config.rs` (the `ui_from_toml` helper already exists):

```rust
#[test]
fn icons_default_to_todays_glyphs() {
    let ui = ui_from_toml("");
    assert_eq!(ui.icons, Icons::default());
    assert_eq!(ui.icons.worktree_main, "●");
    assert_eq!(ui.icons.worktree, "○");
    assert_eq!(ui.icons.session, "▪");
    assert_eq!(ui.icons.home, "⌂");
    assert_eq!(ui.icons.project_expanded, "▾");
    assert_eq!(ui.icons.project_collapsed, "▸");
}

#[test]
fn icon_overrides_apply_and_trim() {
    let ui = ui_from_toml("[ui.icons]\nworktree = \" W \"\nhome = \"H\"");
    assert_eq!(ui.icons.worktree, "W");
    assert_eq!(ui.icons.home, "H");
    assert_eq!(ui.icons.worktree_main, "●", "untouched fields keep defaults");
}

#[test]
fn blank_icon_override_falls_back() {
    let ui = ui_from_toml("[ui.icons]\nworktree_main = \"   \"\nsession = \"\"");
    assert_eq!(ui.icons.worktree_main, "●");
    assert_eq!(ui.icons.session, "▪");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree icons`
Expected: compile error — `Icons` not defined.

- [ ] **Step 3: Implement the config side**

In `config.rs`, near `UiTheme`:

```rust
/// Sidebar status glyphs, each independently overridable from `[ui.icons]`.
/// Overrides are trimmed and a blank value falls back to the default, so a
/// row marker can never be rendered empty.  Action buttons (×, +, ↻, ⇅) are
/// controls, not status, and stay fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icons {
    pub worktree_main: String,
    pub worktree: String,
    pub session: String,
    pub home: String,
    pub project_expanded: String,
    pub project_collapsed: String,
    pub pr_open: String,
    pub pr_draft: String,
    pub pr_merged: String,
    pub pr_closed: String,
}

impl Default for Icons {
    fn default() -> Self {
        Self {
            worktree_main: "●".into(),
            worktree: "○".into(),
            session: "▪".into(),
            home: "⌂".into(),
            project_expanded: "▾".into(),
            project_collapsed: "▸".into(),
            pr_open: "⬤".into(),
            pr_draft: "◯".into(),
            pr_merged: "⬤".into(),
            pr_closed: "⬤".into(),
        }
    }
}
```

Add `pub icons: Icons,` to `UiTheme` and `icons: Icons::default(),` to its `Default` impl.

Raw side (next to `RawUi`):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawIcons {
    worktree_main: Option<String>,
    worktree: Option<String>,
    session: Option<String>,
    home: Option<String>,
    project_expanded: Option<String>,
    project_collapsed: Option<String>,
    pr_open: Option<String>,
    pr_draft: Option<String>,
    pr_merged: Option<String>,
    pr_closed: Option<String>,
}

/// A trimmed, non-blank override — or the default.
fn icon_or(raw: Option<String>, default: &str) -> String {
    raw.map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn build_icons(raw: RawIcons) -> Icons {
    let d = Icons::default();
    Icons {
        worktree_main: icon_or(raw.worktree_main, &d.worktree_main),
        worktree: icon_or(raw.worktree, &d.worktree),
        session: icon_or(raw.session, &d.session),
        home: icon_or(raw.home, &d.home),
        project_expanded: icon_or(raw.project_expanded, &d.project_expanded),
        project_collapsed: icon_or(raw.project_collapsed, &d.project_collapsed),
        pr_open: icon_or(raw.pr_open, &d.pr_open),
        pr_draft: icon_or(raw.pr_draft, &d.pr_draft),
        pr_merged: icon_or(raw.pr_merged, &d.pr_merged),
        pr_closed: icon_or(raw.pr_closed, &d.pr_closed),
    }
}
```

Add `icons: RawIcons,` to `struct RawUi`, and `icons: build_icons(self.ui.icons),` to the `UiTheme { ... }` literal in `into_config`.

- [ ] **Step 4: Run config tests**

Run: `cargo test -p alacritree icons`
Expected: 3 passed. (Also run the full config module: `cargo test -p alacritree config` — the `UiTheme` literal change must not break other tests.)

- [ ] **Step 5: Wire the glyph call sites in app.rs**

Import `Icons` in app.rs's `use crate::config::{...}` list. All edits below are mechanical parameter-threading; the glyph strings are the only behavior change.

1. In `show_project_sidebar`, next to the other pre-closure locals (~line 1362, near `let distros = wsl::distros();`), add:

```rust
let icons = self.config.ui.icons.clone();
```

2. Project arrow (~line 1456):

```rust
let arrow =
    if project.expanded { icons.project_expanded.as_str() } else { icons.project_collapsed.as_str() };
```

3. `worktree_row` (~2709): add parameter `icons: &Icons,` immediately before `theme: &Theme,`. Inside, replace

```rust
let default_icon = if wt.is_main { "●" } else { "○" };
```

with

```rust
let default_icon = if wt.is_main { &icons.worktree_main } else { &icons.worktree };
```

Update its call site (~1639) to pass `&icons,` before `&theme,`.

4. `session_row` (~2841): add `icons: &Icons,` before `theme: &Theme`. Replace the `"▪"` argument in its `paint_row_status_icon` call with `&icons.session`. Update both call sites (~1414 home-session loop and ~1683 worktree-session loop) to pass `&icons`.

5. `home_row` (~2566): add `icons: &Icons,` before `theme: &Theme,`. Replace `"⌂"` in its `paint_row_status_icon` call with `&icons.home`. Update the call site (~1399).

6. `creating_row` (~2689): add `icons: &Icons,` before `theme: &Theme`. Replace `RichText::new("○")` with `RichText::new(&icons.worktree)`. Update the call site (~1695).

Note: `paint_row_status_icon` takes `default_glyph: &str` and centers it as painter text in a fixed slot — passing a configured `String` needs no other change.

- [ ] **Step 6: Check and test**

Run: `cargo check -p alacritree` (fix any missed call sites the compiler reports), then `cargo test -p alacritree`.
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/app.rs
git commit -m "feat(sidebar): make status glyphs configurable via [ui.icons]

Every sidebar status marker (worktree, session, home, project arrows,
and the PR badges the next commits add) gets an independently
overridable glyph.  Overrides are trimmed, blank values fall back to
the default, and unmodified config renders exactly today's glyphs
through the existing fixed-slot painter path.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `PrState` — parse PR state and draft flag from `gh`

**Files:**
- Modify: `alacritree/src/pr_status.rs` (`PrInfo` at ~28, `query_gh` args at ~141, `parse_gh_output` at ~153, tests)

**Interfaces:**
- Produces: `pr_status::PrState { Open, Draft, Merged, Closed }` (derives `Debug, Clone, Copy, PartialEq, Eq`); `PrInfo.state: PrState`. Task 5 consumes both.

- [ ] **Step 1: Write the failing tests**

Extend `mod tests` in `pr_status.rs`:

```rust
#[test]
fn parses_pr_states() {
    for (json_state, is_draft, expected) in [
        ("OPEN", false, PrState::Open),
        ("OPEN", true, PrState::Draft),
        ("MERGED", false, PrState::Merged),
        ("CLOSED", false, PrState::Closed),
        ("SOMETHING_NEW", false, PrState::Open),
    ] {
        let stdout = format!(
            r#"{{"baseRefName":"main","number":1,"url":"https://github.com/o/r/pull/1","state":"{json_state}","isDraft":{is_draft}}}"#
        );
        let info = parse_gh_output(stdout.as_bytes()).unwrap();
        assert_eq!(info.state, expected, "state={json_state} draft={is_draft}");
    }
}

#[test]
fn missing_state_fields_default_to_open() {
    // Old gh versions may omit fields we didn't ask for; degrade, don't drop.
    let stdout = br#"{"baseRefName":"main","number":42,"url":"https://github.com/o/r/pull/42"}"#;
    assert_eq!(parse_gh_output(stdout).unwrap().state, PrState::Open);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree pr_status`
Expected: compile error — `PrState` not defined.

- [ ] **Step 3: Implement**

In `pr_status.rs`:

```rust
/// GitHub's PR lifecycle, folded to what the sidebar paints.  `gh` reports
/// draftness as a separate boolean, so OPEN splits into Open/Draft here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

fn pr_state(state: &str, is_draft: bool) -> PrState {
    match state {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        "OPEN" if is_draft => PrState::Draft,
        // Unknown states paint as open rather than vanishing; gh's enum is
        // stable, so this is a forward-compatibility hedge, not a real case.
        _ => PrState::Open,
    }
}
```

Add `pub state: PrState,` to `PrInfo`.

In `query_gh`, extend the field list:

```rust
.args(["pr", "view", branch, "--json", "number,baseRefName,url,state,isDraft"])
```

In `parse_gh_output`, before the final `Some(...)`:

```rust
let state = value.get("state").and_then(|v| v.as_str()).unwrap_or("OPEN");
let is_draft = value.get("isDraft").and_then(|v| v.as_bool()).unwrap_or(false);
```

and return `Some(PrInfo { number, base_branch: base, url, state: pr_state(state, is_draft) })`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alacritree pr_status`
Expected: all pass (including the pre-existing `parses_gh_json` / `rejects_empty_output`).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/pr_status.rs
git commit -m "feat(pr): parse PR state and draft flag from gh

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: PR status badge on worktree rows + `[ui] pr_status` toggle

**Files:**
- Modify: `alacritree/src/config.rs` (`UiTheme.pr_status`, `RawUi`, tests)
- Modify: `alacritree/src/app.rs` (`Theme` PR colors ~39/67, PR precompute in `show_project_sidebar` ~1360, `worktree_row` badge ~2709, `pr_badge` helper)

**Interfaces:**
- Consumes: `PrState`/`PrInfo.state` (Task 4), `Icons.pr_*` (Task 3), `PrCache::poll(&Path, Option<&str>, &egui::Context) -> Option<PrInfo>` (existing, field `self.pr_cache`).
- Produces: `UiTheme.pr_status: bool` (default `true`); `Theme` fields `pr_open, pr_draft, pr_merged, pr_closed: Color32`; `worktree_row` gains a `pr: Option<&PrInfo>` parameter (inserted immediately after `wt: &Worktree`).

- [ ] **Step 1: Write the failing config test**

Append to config.rs tests:

```rust
#[test]
fn pr_status_defaults_on_and_parses_off() {
    assert!(ui_from_toml("").pr_status);
    assert!(!ui_from_toml("[ui]\npr_status = false").pr_status);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree pr_status_defaults`
Expected: compile error — no `pr_status` field.

- [ ] **Step 3: Implement the config flag**

In `UiTheme`: add

```rust
/// Paint PR-status badges on worktree rows (and poll `gh` for expanded
/// projects' worktrees).  Best-effort like the diff-base lookup: no `gh`,
/// no auth, or no PR silently paints nothing.
pub pr_status: bool,
```

with `pr_status: true,` in its `Default`. In `RawUi`: `pr_status: Option<bool>,`. In the `UiTheme { ... }` literal in `into_config`: `pr_status: self.ui.pr_status.unwrap_or(true),`.

Run: `cargo test -p alacritree pr_status_defaults` → PASS.

- [ ] **Step 4: Add PR colors to `Theme`**

In `struct Theme` (app.rs:39), after `attention`:

```rust
/// PR badge colors, mapped to GitHub's conventions from the ANSI palette.
pr_open: Color32,
pr_draft: Color32,
pr_merged: Color32,
pr_closed: Color32,
```

In `Theme::from_config`, hoist `text_muted` into a local so `pr_draft` can share it:

```rust
let text_muted = blend_toward(text, sidebar_bg, 0.55);
```

then in the `Self { ... }` literal use `text_muted,` for the existing field and add:

```rust
pr_open: rgb_to_color32(config.palette.normal[2]),   // green
pr_draft: text_muted,
pr_merged: rgb_to_color32(config.palette.normal[5]), // magenta
pr_closed: rgb_to_color32(config.palette.normal[1]), // red
```

- [ ] **Step 5: Add the badge to `worktree_row`**

Add a helper near `worktree_row`:

```rust
/// Badge glyph, color, and tooltip word for a PR state.
fn pr_badge<'a>(
    icons: &'a Icons,
    theme: &Theme,
    state: PrState,
) -> (&'a str, Color32, &'static str) {
    match state {
        PrState::Open => (&icons.pr_open, theme.pr_open, "open"),
        PrState::Draft => (&icons.pr_draft, theme.pr_draft, "draft"),
        PrState::Merged => (&icons.pr_merged, theme.pr_merged, "merged"),
        PrState::Closed => (&icons.pr_closed, theme.pr_closed, "closed"),
    }
}
```

Import `PrState` alongside the existing `PrCache` import (`rg -n "PrCache" alacritree/src/app.rs` shows the `use` line; it becomes `use crate::pr_status::{PrCache, PrInfo, PrState};`).

Change `worktree_row`'s signature — add `pr: Option<&PrInfo>,` immediately after `wt: &Worktree,`.

In the trailing closure, after the `+` (spawn) button block and before the closure's end (so it renders leftmost in the right-to-left cluster, the same slot pattern as the project rows' attention dot), add:

```rust
if let Some(info) = pr {
    let (glyph, color, word) = pr_badge(icons, theme, info.state);
    let (rect, resp) =
        ui.allocate_exact_size(row_status_icon_size(theme), egui::Sense::hover());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(10.0 * theme.ui_scale),
        color,
    );
    resp.on_hover_text(format!("PR #{} — {word}", info.number));
}
```

(The trailing closure's early `return` while `deleting` naturally suppresses the badge during deletion — the spinner replaces the whole cluster.)

- [ ] **Step 6: Precompute PR info and pass it at the call site**

In `show_project_sidebar`, next to the other pre-closure locals (~1360), add (plain `for` loops — `self.projects` immutable + `self.pr_cache` mutable are disjoint field borrows):

```rust
// Polled up front, expanded projects only: collapsed projects cost no gh
// processes, and the panel closure borrows `projects` mutably so the cache
// cannot be polled from inside it.
let pr_enabled = self.config.ui.pr_status;
let mut pr_infos: Vec<Vec<Option<PrInfo>>> = Vec::with_capacity(self.projects.len());
for project in &self.projects {
    let mut rows = Vec::with_capacity(project.worktrees.len());
    for wt in &project.worktrees {
        let info = if pr_enabled && project.expanded {
            self.pr_cache.poll(&wt.path, wt.branch.as_deref(), ctx)
        } else {
            None
        };
        rows.push(info);
    }
    pr_infos.push(rows);
}
```

At the `worktree_row` call (~1639), pass after `wt`:

```rust
pr_infos.get(idx).and_then(|v| v.get(wt_idx)).and_then(Option::as_ref),
```

- [ ] **Step 7: Check and test**

Run: `cargo check -p alacritree`, then `cargo test -p alacritree`.
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/app.rs
git commit -m "feat(sidebar): show PR status badges on worktree rows

Worktree rows whose branch has a PR paint a state-colored glyph in the
trailing icon cluster (green open, muted draft, magenta merged, red
closed) with a `PR #n — state` tooltip.  Lookups reuse PrCache's
throttled background gh queries and only run for expanded projects;
[ui] pr_status = false disables both the paint and the polling.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: `[ui.focus_outline]` — outline the focused panel

**Files:**
- Modify: `alacritree/src/config.rs` (`FocusOutline` struct, `RawUi`, tests)
- Modify: `alacritree/src/app.rs` (`Theme` field, `paint_focus_outline`, wiring in `update` ~3963-3975)

**Interfaces:**
- Consumes: `PaneFocus::{Terminal, ProjectsSidebar}` (existing, app.rs:125), `is_modal_open()` (existing, app.rs:839).
- Produces: `config::FocusOutline { pub sidebar: bool, pub terminal: bool, pub color: Option<Color32>, pub thickness: f32 }`; `UiTheme.focus_outline: FocusOutline`; app-side `FocusOutlineTheme { sidebar, terminal, color: Color32, thickness }` on `Theme`.

- [ ] **Step 1: Write the failing config tests**

```rust
#[test]
fn focus_outline_defaults_off() {
    let fo = ui_from_toml("").focus_outline;
    assert!(!fo.sidebar);
    assert!(!fo.terminal);
    assert_eq!(fo.color, None);
    assert_eq!(fo.thickness, 1.0);
}

#[test]
fn focus_outline_parses_all_fields() {
    let fo = ui_from_toml(
        "[ui.focus_outline]\nsidebar = true\nterminal = true\ncolor = \"#89b4fa\"\nthickness = 2.5",
    )
    .focus_outline;
    assert!(fo.sidebar);
    assert!(fo.terminal);
    assert_eq!(fo.color, Some(Color32::from_rgb(0x89, 0xb4, 0xfa)));
    assert_eq!(fo.thickness, 2.5);
}

#[test]
fn focus_outline_thickness_clamps() {
    let fo = ui_from_toml("[ui.focus_outline]\nthickness = 0.1").focus_outline;
    assert_eq!(fo.thickness, 0.5);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree focus_outline`
Expected: compile error — no `focus_outline` field.

- [ ] **Step 3: Implement the config side**

In `config.rs`:

```rust
/// `[ui.focus_outline]`: stroke a border around a panel while it owns
/// keyboard focus.  Per-panel toggles, shared color/thickness; both toggles
/// default off so unmodified config keeps today's look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusOutline {
    pub sidebar: bool,
    pub terminal: bool,
    /// `None` falls back to the theme accent at resolution time.
    pub color: Option<Color32>,
    /// Absolute logical pixels (deliberately not ui_scale-multiplied);
    /// clamped to ≥ 0.5.
    pub thickness: f32,
}

impl Default for FocusOutline {
    fn default() -> Self {
        Self { sidebar: false, terminal: false, color: None, thickness: 1.0 }
    }
}
```

Add `pub focus_outline: FocusOutline,` to `UiTheme` (+ `focus_outline: FocusOutline::default(),` in its `Default`).

Raw side:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawFocusOutline {
    sidebar: Option<bool>,
    terminal: Option<bool>,
    color: Option<RgbStr>,
    thickness: Option<f32>,
}
```

Add `focus_outline: RawFocusOutline,` to `RawUi`, and to the `UiTheme { ... }` literal:

```rust
focus_outline: FocusOutline {
    sidebar: self.ui.focus_outline.sidebar.unwrap_or(false),
    terminal: self.ui.focus_outline.terminal.unwrap_or(false),
    color: self.ui.focus_outline.color.map(|v| rgb_to_color32(v.0)),
    thickness: self.ui.focus_outline.thickness.map_or(1.0, |t| t.max(0.5)),
},
```

Run: `cargo test -p alacritree focus_outline` → 3 passed.

- [ ] **Step 4: Resolve into `Theme` and paint**

In app.rs, define next to `Theme`:

```rust
#[derive(Clone, Copy)]
struct FocusOutlineTheme {
    sidebar: bool,
    terminal: bool,
    color: Color32,
    thickness: f32,
}
```

Add `focus_outline: FocusOutlineTheme,` to `struct Theme`, and in `from_config` (after `accent` exists):

```rust
focus_outline: FocusOutlineTheme {
    sidebar: config.ui.focus_outline.sidebar,
    terminal: config.ui.focus_outline.terminal,
    color: config.ui.focus_outline.color.unwrap_or(accent),
    thickness: config.ui.focus_outline.thickness,
},
```

Add the painter next to `paint_panel_border` (same `Middle`-layer trick — above panel content, below modals and tooltips; `StrokeKind::Inside` keeps the stroke inside the rect so it isn't clipped at the panel edge):

```rust
fn paint_focus_outline(ctx: &Context, rect: egui::Rect, theme: &Theme) {
    let fo = theme.focus_outline;
    let layer = egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::new(("focus_outline", rect.min.x.to_bits())),
    );
    ctx.layer_painter(layer).rect_stroke(
        rect,
        0.0,
        Stroke::new(fo.thickness, fo.color),
        egui::StrokeKind::Inside,
    );
}
```

- [ ] **Step 5: Wire it in `update`**

In `eframe::App::update` (app.rs ~3963), extend the two panel blocks and capture the central panel's response:

```rust
if self.show_left_sidebar {
    let r = self.show_project_sidebar(ctx, panel_frame.clone());
    paint_panel_border(ctx, r.right(), r.y_range(), theme.sidebar_border);
    if theme.focus_outline.sidebar
        && !modal_open
        && self.focus == PaneFocus::ProjectsSidebar
    {
        paint_focus_outline(ctx, r, &theme);
    }
}
```

(The git sidebar block is unchanged — it never takes keyboard focus, so it gets no outline.)

Change the central panel call to bind its response:

```rust
let central = egui::CentralPanel::default()
    .frame(Frame::default().fill(central_fill).inner_margin(Margin::same(0)))
    .show(ctx, |ui| {
        // ... existing body unchanged ...
    });
if theme.focus_outline.terminal && !modal_open && self.focus == PaneFocus::Terminal {
    paint_focus_outline(ctx, central.response.rect, &theme);
}
```

Note: `self.focus` is read *after* the closures run, so a click that moves focus this frame paints the outline on the new owner immediately — the desired behavior.

- [ ] **Step 6: Check and test**

Run: `cargo check -p alacritree`, then `cargo test -p alacritree`.
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/app.rs
git commit -m "feat(ui): outline the focused panel via [ui.focus_outline]

Per-panel toggles (projects sidebar, terminal) with a shared color
(default: theme accent) and thickness.  Painted as an inside stroke on
the Middle layer so it sits above panel content but below modals, and
suppressed entirely while a modal owns the keyboard.  The git sidebar
never takes keyboard focus and so has no outline.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Final verification

**Files:** none new.

- [ ] **Step 1: Full suite and format check**

Run: `cargo fmt --check && cargo test -p alacritree`
Expected: no fmt diffs, all tests pass.

- [ ] **Step 2: Default-behavior audit**

Verify with `rg` that no default changed:
- `rg -n '"●"|"○"|"▪"|"⌂"|"▾"|"▸"' alacritree/src/app.rs` — remaining literals should only be in `Icons::default()`-independent places (e.g. `paint_cursor_outline` has none; the `mark` bullet `•` in the shell picker is not a status icon and stays).
- `rg -n "ui_normal_px|ui_heading_px" alacritree/src/app.rs` — only `ui_text_px` should call them.

- [ ] **Step 3: Report**

Report completion; manual GUI verification (isolated lab only — never the user's running instance) is a separate follow-up: UI font family/size after restart, PR badges on a branch with an open/draft PR, `pr_status = false` kills badge + polling, custom icons render in the slot, focus outline follows Tab/Escape focus switches and hides while a modal is open.

## Self-review notes

- Spec's "painted circle path" claim was corrected against master before planning: `paint_row_status_icon` already renders all default glyphs as centered painter text in a fixed slot, so icon overrides reuse the exact same path (spec updated).
- Spec's session default glyph corrected to `▪` (master's actual glyph).
- PR badge placement: trailing cluster (spec updated) — the name label truncates into all remaining width, so nothing can render "just after" it.
- `Theme` stays `Copy`: all new fields (`Color32`, `f32`, `bool`, nested `FocusOutlineTheme`) are `Copy`.
