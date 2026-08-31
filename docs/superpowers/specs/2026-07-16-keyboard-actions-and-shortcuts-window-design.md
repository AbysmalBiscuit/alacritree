# Keyboard Actions & Shortcuts Window — Design

**Date:** 2026-07-16
**Status:** Approved (user, 2026-07-16)
**Note:** This spec stays untracked. It must never be committed or reach an upstream PR.

## Goal

Three keyboard features for alacritree, landing as two stacked branches:

1. Restore the rebindable `CloseSession` action (Ctrl+Shift+W) from archive commit 530b4c07.
2. Rebindable sidebar-navigation actions: Home/End to top/bottom, PgUp/PgDn to jump between projects.
3. A searchable shortcuts window: rebindable action, default F1, `/` to fuzzy-search.

## Branch structure

- `feat/keyboard-actions` — based on `feat/searchable-sidebars` (7d601695, PR #101's branch). Carries features 1 and 2.
- `feat/shortcuts-window` — based on `feat/keyboard-actions`. Carries feature 3 (so the window can describe the new actions).
- Both merge into `integration/all-features` when done. Upstream PRs are stacked behind PR #101 and are opened only when the user asks.
- Worktrees under `C:/Users/Lev/Git/github/alacritree-worktrees/` (existing convention).

## Feature 1: CloseSession (restore)

Cherry-pick archive commit `530b4c07` onto `feat/keyboard-actions`. It was written against a tree with `SidebarRow::Session`, which `feat/searchable-sidebars` has, so it should apply with at most minor conflict resolution. Semantics (unchanged from the archive):

- New `NamedAction::CloseSession`, config name `"CloseSession"`, default binding Ctrl+Shift+W.
- Dispatch: if the sidebar owns focus and the cursor is on a session row, close that session; otherwise close the active session in the current workspace. Either path goes through `request_close_session`, so `confirm_session_close` is honored (may open the confirmation dialog).
- `docs/keyboard-shortcuts.md`: default-bindings table row + action-list entry (both in the archive commit).
- Tests (in the archive commit): default-binding match test + config-name parse test.

## Feature 2: Sidebar-navigation actions

Four new `NamedAction` variants with **unmodified** default keys:

| Action | Default key | Behavior |
|---|---|---|
| `SidebarTop` | Home | Cursor to the first row (the Home row) |
| `SidebarBottom` | End | Cursor to the last visible row |
| `SidebarNextProject` | PageDown | Cursor to the nearest Project row below; no-op if none below |
| `SidebarPreviousProject` | PageUp | Cursor to the nearest Project row above; no-op if none above |

Design points:

- **Focus-scoped, non-consuming when unfocused.** These actions act only while `focus == PaneFocus::ProjectsSidebar`. When the terminal owns focus, a matched sidebar-scoped binding must NOT consume the key event — plain Home/End/PgUp/PgDn must still reach the terminal as CSI sequences. Implementation: a `NamedAction::is_sidebar_scoped()` predicate; in `handle_shortcuts`' retain closure, if every matched action is sidebar-scoped and the sidebar is not focused, treat the event as unmatched (do not consume, do not dispatch). The four nav actions and nothing else are sidebar-scoped. (`CloseSession` is NOT sidebar-scoped — it works from the terminal too.)
- **Row set:** movement operates on the same rows the arrow keys use — `rows_for_nav()` on the searchable-sidebars branch, i.e. the filtered set while a `/` filter or toggle is active, the full visible set otherwise.
- **Stale-cursor handling:** same as `apply_sidebar_nav` — if the current cursor is not in the row set, reseat on `SidebarRow::Home` and stop.
- **Non-wrapping:** project jumps stop at the extremes; they do not wrap.
- New pure functions in `sidebar_nav.rs`:
  - `next_project(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow>` — first `Project` row strictly after the cursor's position.
  - `previous_project(rows: &[SidebarRow], cursor: &SidebarRow) -> Option<SidebarRow>` — last `Project` row strictly before the cursor's position.
  - Top/bottom need no new helpers (`rows.first()` / `rows.last()`).
- Config names: `"SidebarTop"`, `"SidebarBottom"`, `"SidebarNextProject"`, `"SidebarPreviousProject"` in `parse_action`.
- `docs/keyboard-shortcuts.md`: four default-table rows + four action-list entries, with a note that they act only while the sidebar has keyboard focus and pass through to the terminal otherwise.

## Feature 3: Shortcuts window

### Action

- New `NamedAction::ShowShortcuts`, config name `"ShowShortcuts"`, default binding F1 (no modifiers).
- Dispatch toggles the window: open if closed, close if open.
- **Known trade-off (accepted by user):** bindings are matched globally, so default F1 never reaches terminal TUIs (htop help, mc). Rebindable, so users can move it.
- NOT sidebar-scoped — F1 works from any focus.

### Window UI

- Centered `egui::Window` overlay following the existing dialog styling in `app.rs`. Fixed reasonable max size, vertical scroll for the list.
- State on `AlacritreeApp`: `shortcuts_window_open: bool` and `shortcuts_query: String`. Not persisted.
- Content, two sections:
  1. **App shortcuts** — the *effective* Named-action bindings with descriptions. "Effective" = shadowing applied: bindings are checked user-first (same precedence as dispatch), so for each (key, mods) only the first-matching Named action is listed. One row per surviving binding: key combo (e.g. `Ctrl+Shift+W`), action name, description. `ReceiveChar`/`NoOp` entries are omitted (they unbind, not bind).
  2. **Sidebar navigation** — static entries for the hardcoded nav keys: Up/Down (move cursor), Right/Left (expand/collapse or jump to parent), Enter (activate/toggle), Space (consumed), Escape (back to terminal), plus PR #101's filter keys (`/` to filter, the toggle keys, Enter/Escape filter behavior) as they exist on the base branch.
- **Descriptions:** new `NamedAction::description(&self) -> &'static str` in `bindings.rs` returning a short human phrase per variant (e.g. `CloseSession` → "Close the cursored or active session"). Rendering falls back to the action's config name if a description is empty, so future actions never render blank.
- **Key handling while open:** Esc clears the query if non-empty, closes the window otherwise (lazyvim feel). `/` focuses the search field. F1 (through normal binding dispatch → `ShowShortcuts`) closes it. The window does not block binding dispatch and is not added to `is_modal_open()` — it is an informational overlay, not a modal (font-size shortcuts etc. keep working while it is open). Input routing while open (discovered during planning — typed text must reach only the search box): the sidebar/git nav drains are skipped (`PanelFilter` must not intercept text), the terminal view's active flag gains `&& !shortcuts_window_open` (no double-typing into the PTY), and the window hides while a real modal is up so modals keep key priority. One extra transient field `shortcuts_focus_search: bool` (one-shot) gives the search box focus on open and on `/`.

### Search

- Text field at the top of the window; typing filters rows live.
- Match: case-insensitive **subsequence** match of the query against the concatenation of key combo + action name + description. Empty query shows everything. Section headers hide when a section has no surviving rows.
- Implemented as a small standalone function (e.g. `fn fuzzy_match(query: &str, haystack: &str) -> bool` in the new window module) — deliberately NOT reusing `panel_filter.rs`, whose toggle/outcome machinery is sidebar-specific.

### Module layout

New file `alacritree/src/shortcuts_window.rs`: the pure parts (effective-binding computation, row model, fuzzy matcher, static sidebar-nav entries) plus the paint function, keeping `app.rs` growth to the state fields + dispatch arm + one paint call.

## Testing

Unit tests (no egui harness exists in the crate; painting stays untested, consistent with the codebase):

- `sidebar_nav.rs`: `next_project` / `previous_project` — middle, at-extremes (no-op), from session/worktree rows, empty/filtered row sets.
- `bindings.rs`: default-binding match + parse tests for all six new action names (CloseSession's come with the cherry-pick); a test that every action name accepted by `parse_action` has a nonempty `description()` (excluding `ReceiveChar`/`NoOp` if they are given none).
- `app.rs` or `shortcuts_window.rs`: effective-binding shadowing (a user override of Ctrl+Shift+W yields the user's action, not two rows); sidebar-scoped pass-through predicate (pure part).
- `shortcuts_window.rs`: fuzzy matcher — empty query, case-insensitive, subsequence vs. substring, no match.
- TDD per repo convention: RED for the right reason before GREEN (compile-error RED acceptable for new API).

Manual GUI verification in the isolated lab (existing `target/` lab setup) before merging into `integration/all-features`: F1 open/toggle/Esc/search, Home/End/PgUp/PgDn in and out of sidebar focus (including that the terminal still receives them when focused), Ctrl+Shift+W both from sidebar session row and terminal focus.

## Out of scope

- Persisting window state.
- Listing raw `Chars` bindings in the window (user chose to omit).
- Wrapping project jumps.
- Upstream PR creation (user asks separately; PRs are stacked behind #101).
