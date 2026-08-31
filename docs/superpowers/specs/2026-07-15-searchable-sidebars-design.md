# Searchable & Filterable Sidebars — Design

Status: approved design, pre-implementation.
Note: this spec stays untracked — never commit it or let it reach an upstream PR.

## Goal

Both sidebars become keyboard-searchable and keyboard-filterable:

1. **Search**: LazyVim/fzf-style `/` filter-as-you-type in whichever sidebar has
   keyboard focus. Typing narrows visible rows via fuzzy matching; Esc clears.
2. **Filters**: single-key toggles (neo-tree style) that persist until cleared,
   shown as chips, stacking with the search query (AND).
3. **Right-panel focus**: the git sidebar gains a keyboard focus mode of its own
   (cursor, Enter-to-open-diff), entered via a new named action
   `FocusGitSidebar`, configurable in the settings file like every other
   binding.

## Focus model & bindings

- `PaneFocus` gains a third variant: `Terminal | ProjectsSidebar | GitSidebar`.
- New `NamedAction::FocusGitSidebar`, parseable from
  `[[keyboard.bindings]]` as `action = "FocusGitSidebar"`. Default binding:
  **Ctrl+Shift+G** (mirrors `Ctrl+Shift+B` → `ToggleSidebarFocus`).
  Behavior: if the git sidebar is not focused, focus it (auto-showing the panel
  if hidden, with the same `sidebar_auto_shown`-style round-trip the left panel
  uses: leaving focus re-hides a panel that was auto-shown); if already
  focused, return focus to the terminal.
- `ToggleSidebarFocus` (Ctrl+Shift+B) keeps its current terminal↔left meaning.
  No three-way cycle.
- `Escape` in either sidebar ultimately returns focus to the terminal (see the
  Esc ladder below).
- Focusing the git sidebar seeds its cursor on the row for the currently open
  diff if one is visible, else the first file row, else no cursor (empty
  status).

## Input modes (per focused sidebar)

Each sidebar, while focused, is in one of two modes. Mode and filter state are
per-panel and independent (left and right keep separate queries/toggles).

### Browsing (default; current left-sidebar behavior)

- Arrows move the cursor; Enter activates (left: existing semantics; right:
  open the diff for the cursor row via the same `DiffRequest` path as a click).
- `/` enters **search** mode.
- Single unmodified toggle keys flip filters (see Filters below).
- **Esc ladder**: if any filter toggles are active, first Esc clears them all;
  otherwise Esc returns focus to the terminal.

### Search

- A search prompt appears in the panel header next to the title; all printable
  input goes to the query (toggle keys are inert in this mode). The prompt is
  custom-drawn (`/query` + caret glyph), not an egui `TextEdit`: the app
  consumes `Event::Text`/Backspace itself, avoiding an egui-native-focus fight
  with the terminal view (which egui fake-clicks on Space/Enter when natively
  focused).
- The list narrows live as the query changes.
- Up/Down move the cursor through the *filtered* rows while the box keeps text
  focus.
- **Enter**: activates the cursor row with the same rules as browsing mode,
  then clears the query and returns to browsing. Search is a transient jump,
  not a persistent view. (Activating a worktree/Home or opening a diff also
  moves focus per that action's normal behavior, e.g. worktree activation
  focuses the terminal.)
- **Esc**: clears the query and returns to browsing mode (panel keeps focus).

## Fuzzy matching

- Crate: **`nucleo-matcher` 0.3** (Helix's matcher). Chosen over `frizbee`
  (API had six breaking changes across its last two releases, July 2026) and
  `fuzzy-matcher` (superseded; skim itself moved off it). Performance is
  irrelevant at sidebar scale; nucleo wins on API stability, correct
  Unicode/grapheme handling, and built-in smart-case.
- Pattern: `Pattern::parse(query, CaseMatching::Smart, Normalization::Smart)`
  — subsequence fuzzy matching, case-insensitive unless the query contains an
  uppercase letter.
- Matched text per row: left panel — project display name, worktree name
  (branch display string as rendered); right panel — the file path string.
- Rows are *filtered*, not re-ranked: relative render order is preserved
  (tree order left, section order right). No match-character highlighting in
  v1.

## Filter toggles

Active toggles render as small chips next to the panel title (e.g. `[s] [a]`).
Toggles and query stack: a row must pass both. Filter state is transient —
never persisted to `state.toml`; it resets when the app restarts (but survives
focus round-trips within a run).

### Left panel (projects/worktrees)

- `s` — **sessions**: show only workspaces (Home/worktrees) with at least one
  running session.
- `a` — **attention**: show only workspaces whose attention flag is set (the
  same predicate as the attention dot).

### Right panel (git status)

Kind toggles form a union: when at least one is active, a file row is shown
only if its `ChangeKind` is in the union of active sets.

- `m` — Modified + Renamed
- `d` — Deleted
- `u` — Untracked + Added
- Conflicted files are always shown regardless of active kind toggles —
  conflicts must never be filterable out of sight.
- The "Changes vs <base>" section's rows are `DiffStat`s with no
  `ChangeKind`, so kind toggles do not apply there — that section is
  filtered by the query only.

## Row visibility rules

### Left panel

- A worktree row is visible iff it passes the query and toggles.
- A project header is visible iff its own name matches the query (with no
  toggles pinning it out) **or** it has at least one visible worktree.
- While any query or toggle is active, visible projects render expanded
  regardless of their stored `expanded` flag (display-only; the persisted flag
  is untouched). A filter that hides its own results is useless.
- The Home row obeys the same predicates (query matched against its rendered
  label "Home"; `s`/`a` evaluated for the home workspace).
- Session rows under a workspace are shown iff their parent row is visible;
  the query does not match against individual session titles in v1.
- Cursor: when the filtered row set changes, the cursor stays put if its row
  is still visible, otherwise it moves to the first visible row (or clears
  when nothing is visible).

### Right panel

- Row model: section headers (Staged / Unstaged / Changes vs <base>) plus file
  rows, in current render order. The cursor rests on file rows only.
- File rows are filtered by query (fuzzy on path) and kind toggles.
- Section headers stay visible while filtering and show filtered counts:
  `Unstaged (2 of 14)`; unfiltered sections keep the current plain count. A
  section with zero visible rows collapses to its header.
- Cursor fallback mirrors the left panel.

## Components

- **`panel_filter.rs` (new, pure)**: `PanelFilter` — query string, toggle set,
  input mode; a key-event reducer returning what changed (query edited, toggle
  flipped, exit requests); fuzzy predicate wrapping `nucleo-matcher`
  (`Matcher` reused across calls, `Pattern` rebuilt on query change). No egui
  types beyond `egui::Key`/text. Two instances live in `AlacritreeApp`, one
  per sidebar.
- **`sidebar_nav.rs` (extend)**: `visible_rows` gains a filter argument (or a
  filtered sibling) implementing the left-panel visibility rules; `step`,
  `left_target`, `seed` operate on the filtered rows unchanged.
- **`git_nav.rs` (new, pure)**: right-panel row model and cursor. Rows keyed
  stably by `(section, path)` — the status lists refresh underneath the cursor
  every 1.5 s, so indices would silently retarget (same rationale as
  `sidebar_nav`). Provides `visible_rows(status, filter)`, `step`, `seed`.
- **`bindings.rs`**: `NamedAction::FocusGitSidebar`, parse-table entry,
  Ctrl+Shift+G default.
- **`app.rs`**: `PaneFocus::GitSidebar`; route sidebar-nav key handling by
  focused panel; render search box + chips in both panel headers; plumb
  filtered rows and cursor painting into `show_git_sidebar` (cursor outline +
  scroll-to-row reuse the left panel's `paint_cursor_outline` approach);
  dispatch `FocusGitSidebar`.
- **`Cargo.toml`**: add `nucleo-matcher = "0.3"`.

## Error handling

No new failure modes: filtering empty statuses yields empty sections; a stale
cursor falls back per the cursor rules; an unparseable query cannot occur
(any string is a valid fuzzy pattern). Git-status errors keep their current
rendering and short-circuit filtering (nothing to filter).

## Testing

- `panel_filter.rs`: unit tests for the mode reducer (`/` enters search, Esc
  ladder, Enter clears query, toggle keys inert in search mode) and matching
  (smart-case, fuzzy subsequence, empty query passes all).
- `sidebar_nav.rs`: extend existing tests — filtered visibility (project kept
  by matching worktree, forced expansion, Home matching), cursor fallback on
  filter change.
- `git_nav.rs`: visibility under kind toggles (union semantics, Conflicted
  always visible), filtered counts, cursor step/seed/fallback.
- `bindings.rs`: add `FocusGitSidebar` to the existing action round-trip test.
- No egui-driven UI tests; all logic under test is in the pure modules,
  matching the crate's existing pattern.

## Out of scope (v1)

- Match-character highlighting in rows.
- Persisting filters/queries to `state.toml`.
- Making toggle keys user-configurable.
- Searching session titles or terminal scrollback.
- Typo-tolerant matching / a global fuzzy palette (revisit `frizbee` if that
  ever lands).
