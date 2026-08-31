# Sidebar Appearance Customization — Design

**Date:** 2026-07-16
**Status:** Draft (pending user review)
**Note:** This spec stays untracked. It must never be committed or reach an upstream PR.

## Goal

Four appearance features, landing as one branch:

1. Sidebar font and scale, independent from the terminal font (defaults to reusing it).
2. PR status shown as a colored icon on worktree rows, toggleable in config.
3. Customizable glyphs for the sidebar's status icons.
4. A focus outline around the focused panel, per-panel toggleable, with color and thickness options.

## Branch structure

- `feat/sidebar-appearance` — based on upstream master. Carries all three features (they touch the same config/theme/painting code; separate branches would conflict constantly).
- Merges into `integration/all-features` when done. Upstream PR only when the user asks.
- Independent of the pending stacked PRs #101/#105/#106.

## Feature 1: Independent sidebar font & scale

### Config (alacritree.toml)

```toml
[ui.font]
family = "Inter"   # optional; default: the terminal font
size = 12.0        # optional, points (same unit as [font] size);
                   # default: terminal size × UI_NORMAL_RATIO
```

New `RawUiFont { family: Option<String>, size: Option<f32> }` nested in `RawUi`, surfaced as `UiFont` on `Config`. `size` is clamped to ≥ 1.0 like `[font] size`.

### Mechanics

- **Family:** today `install_terminal_fonts` registers the terminal font as both `Monospace` and `Proportional` (fonts.rs). When `ui.font.family` is set, resolve it through the existing `resolve_face` machinery (`Variant::Normal`; file paths work like they do for `[font]`) and insert it at the **head** of egui's `Proportional` family, followed by its own fallback chain (same `register_user_fallbacks`/`register_fallback_faces` flow, seeded from the UI family). `Monospace` — and therefore the terminal grid — is untouched. If the family fails to resolve, warn and keep today's behavior.
- **Scope note:** all egui chrome shares the `Proportional` family, so the UI font applies to both sidebars, modals, and the shortcuts window alike — "sidebar font" really means "everything that isn't the terminal grid". This is the coherent interpretation; per-panel fonts are rejected as needless complexity.
- **Size:** when `ui.font.size` is set: `font_normal = px(size)` (same point→logical-pixel conversion as `FontConfig::egui_size`), `font_heading = font_normal × (UI_HEADING_RATIO / UI_NORMAL_RATIO)`, and `ui_scale = font_normal / 11.25` — so icons, paddings, and modal widths scale with the sidebar font exactly as they do today with the terminal font. Unset: current derived values, bit-for-bit.
- One knob covers "scaling customization": chrome scale follows the UI font size. A separate scale multiplier is rejected as a second knob fighting over the same derived values.
- **Restart required** (fonts are installed once at startup) — same caveat as window transparency.

### Trade-off (accepted)

A proportional (non-mono) UI family changes label truncation widths; egui handles that per-label (`.truncate()`), no layout code changes expected.

## Feature 2: PR status icon

### Data

`pr_status.rs` already polls `gh pr view <branch> --json number,baseRefName,url` per worktree (TTL 300 s, background thread, never blocks). Extend the field list with `state,isDraft` and add to `PrInfo`:

```rust
pub enum PrState { Open, Draft, Merged, Closed }
```

Mapping: `state == "OPEN" && isDraft` → `Draft`; otherwise `OPEN`/`MERGED`/`CLOSED` map 1:1. Unknown strings → treat as `Open` (paint something rather than nothing; gh's enum is stable).

### Display

- Worktree rows whose branch has a known PR paint a small glyph right-aligned in the row's trailing icon cluster (the same slot pattern as the project rows' attention dot — the name label truncates into all remaining width, so nothing can sit "just after" it), colored by state, theme-mapped to GitHub conventions:
  - Open → ANSI green (`palette.normal[2]`)
  - Draft → `text_muted`
  - Merged → ANSI magenta (`palette.normal[5]`)
  - Closed → ANSI red (`palette.normal[1]`)
- Hover tooltip: `PR #106 — draft`. No click action (scoped out; the URL is in `PrInfo` if that changes later).
- Glyphs come from the `[ui.icons]` table (feature 3): `pr_open`/`pr_merged`/`pr_closed` default `⬤`, `pr_draft` default `◯`.

### Polling scope

- The left sidebar polls `PrCache` only for worktrees of **expanded** projects; collapsed projects cost no `gh` processes. The existing TTL bounds refresh volume; first expand of a project spawns at most one short-lived `gh` per worktree per 5 minutes.
- The existing diff-base polling for the current workspace is unchanged.

### Config

```toml
[ui]
pr_status = false  # default off (opt-in); true enables the paint and the sidebar polling
```

(Default flipped to off at final review: an unmodified config must not spawn new `gh`
processes, per the fork's opt-in rule.)

Best-effort like the existing lookup: no `gh`, not authenticated, non-GitHub remote → silently no icon.

## Feature 3: Customizable icons

### Config (alacritree.toml)

```toml
[ui.icons]
worktree_main     = "●"
worktree          = "○"
session           = "▪"
home              = "⌂"
project_expanded  = "▾"
project_collapsed = "▸"
pr_open           = "⬤"
pr_draft          = "◯"
pr_merged         = "⬤"
pr_closed         = "⬤"
```

New `Icons` struct on `Config`, every field independently overridable, defaults exactly today's glyphs. Values are trimmed; an empty/whitespace override falls back to the default (a row marker must never be blank). Action buttons (`×`, `+`, `↻`, `⇅`) are controls, not status — they stay fixed.

### Mechanics

- `paint_row_status_icon` centers the glyph as painter-drawn text in a fixed slot (`row_status_icon_size`) precisely because glyph metrics vary across fallback fonts — default and overridden glyphs go through the same path, so an override is just a different string and alignment is unchanged. (Only the attention dot is a painted circle, and it is not customizable — it must stay visually identical to itself.) A custom glyph (e.g. a nerd-font icon) pairs naturally with `ui.font.family`.
- `project_expanded`/`project_collapsed` are already text (`icon_button`), so those overrides just swap the string.

## Feature 4: Focus outline

### Config (alacritree.toml)

```toml
[ui.focus_outline]
sidebar  = false    # outline the projects sidebar while it owns keyboard focus
terminal = false    # outline the terminal panel while it owns keyboard focus
color = "#89b4fa"   # optional; default: the theme accent (sidebar_accent / ANSI blue)
thickness = 1.0     # logical px, default 1.0
```

Per-panel booleans; color and thickness are shared across panels (per-panel styling is
rejected as config surface without a use case). Both toggles default **off** — unmodified
config keeps today's look, like every other feature in this spec. `thickness` is clamped
to ≥ 0.5 and is an absolute logical-pixel value (not `ui_scale`-multiplied, so the user
gets exactly what they set).

### Mechanics

- Only the two keyboard-focusable panes exist today (`PaneFocus::{Terminal,
  ProjectsSidebar}`); the git sidebar never takes focus and so gets no outline. If it
  becomes focusable later, it joins as a third boolean.
- Painting: when the pane owns `self.focus` and its toggle is on, stroke an inset rect
  around the panel's rect on the `Middle` layer — the same layer trick as
  `paint_panel_border`, so the outline sits above panel content but below modals,
  popups, and tooltips. Inset by half the stroke width so the line isn't clipped at the
  panel edge.
- **Hidden while a modal is open** (`is_modal_open()`): the modal owns the keyboard, so a
  focus outline underneath it would be a lie.
- New `FocusOutline` config struct on `UiTheme`/`Config`, resolved into `Theme` alongside
  the other colors (default color = `accent`).

## Testing

Unit tests (no egui harness; painting stays untested, consistent with the codebase):

- config: `[ui.font]`, `[ui] pr_status`, `[ui.icons]`, `[ui.focus_outline]` parse; defaults when absent; empty icon falls back; size and thickness clamps.
- theme: derived `font_normal`/`font_heading`/`ui_scale` with and without `ui.font.size`.
- pr_status: `parse_gh_output` with `state`/`isDraft` combinations (open, draft, merged, closed, unknown state).
- icons: trim/fallback normalization.

Manual GUI verification in the isolated lab: sidebar font family/size change (restart), PR icon on a branch with an open/draft PR, `pr_status = false` kills it, custom icons render in the slot, focus outline follows Tab/Escape focus switches and hides while a modal is open.

## Out of scope

- Clicking the PR icon to open the URL.
- CI-check status (`statusCheckRollup`) — a later extension of `PrInfo` if wanted.
- Customizing action-button glyphs or colors beyond the existing `[ui]` color options.
- Live font reload.
