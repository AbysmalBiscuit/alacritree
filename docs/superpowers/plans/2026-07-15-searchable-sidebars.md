# Searchable & Filterable Sidebars Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** LazyVim-style `/` fuzzy filter and single-key filter toggles in both sidebars, plus a keyboard focus mode for the right (git) sidebar entered via a new configurable `FocusGitSidebar` binding.

**Architecture:** A pure `panel_filter` module owns the search/toggle state machine and nucleo-based fuzzy matching (one instance per sidebar). The left panel extends `sidebar_nav` with filtered row visibility; the right panel gets a new pure `git_nav` cursor/row model. `app.rs` grows a `PaneFocus::GitSidebar` variant and wires input routing plus header rendering (custom-drawn search prompt + toggle chips — no egui `TextEdit`).

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, `nucleo-matcher = "0.3"` (new dependency).

**Spec:** `docs/superpowers/specs/2026-07-15-searchable-sidebars-design.md` (same worktree). The spec is the authority on semantics; this plan is the authority on decomposition.

## Global Constraints

- All changes live in `alacritree/`; `alacritty*/` crates are vendored upstream and read-only.
- `cargo fmt` before every commit; `cargo test -p alacritree` must pass.
- Conventional Commits, imperative, ≤72-char subject, lowercase after colon, no trailing period. Every commit ends with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Never `git add` anything under `docs/` — spec and plan stay untracked.
- Comments explain *why*, never *what*; timeless (no task/PR references).
- Toggle keys: left panel `s` (has running sessions), `a` (needs attention); right panel `m` (Modified+Renamed), `d` (Deleted), `u` (Untracked+Added); Conflicted rows always visible; branch-diff section ignores kind toggles (its rows have no `ChangeKind`).
- Esc ladder: search mode → clear query, back to browsing; browsing with active toggles → clear toggles; browsing otherwise → focus terminal.
- Enter in search mode activates the cursor row and clears the query (transient jump).
- Filter state is never persisted to `state.toml`.
- Default binding for `FocusGitSidebar` is Ctrl+Shift+G.

---

### Task 1: `FocusGitSidebar` named action

**Files:**
- Modify: `alacritree/src/bindings.rs`

**Interfaces:**
- Produces: `NamedAction::FocusGitSidebar` (dispatched in Task 5), parseable from TOML as `action = "FocusGitSidebar"`, default binding Ctrl+Shift+G.

- [ ] **Step 1: Write the failing test.** In `bindings.rs`, the existing round-trip test (~line 698) maps action-name strings to variants. Add `("FocusGitSidebar", NamedAction::FocusGitSidebar)` to its list, and add `(Key::G, ctrl_shift, FocusGitSidebar)` to the default-bindings test (~line 768, next to the `(Key::B, ctrl_shift, ToggleSidebarFocus)` entry).

- [ ] **Step 2: Verify RED.** Run: `cargo test -p alacritree bindings` — expect compile failure (`FocusGitSidebar` not found), which is the correct RED for a new enum variant.

- [ ] **Step 3: Implement.**
  - Add `FocusGitSidebar,` to `NamedAction` directly after `FocusProjectsSidebar` (~line 54).
  - In `parse_action` (~line 499), after the `"ToggleSidebarFocus"` arm's neighbors, add: `"FocusGitSidebar" => BindingAction::Named(FocusGitSidebar),`.
  - In `default_bindings()` app-level block (~line 190), after the `ToggleSidebarFocus` entry, add:
    ```rust
    KeyBinding { key: Key::G, mods: ctrl_shift, action: BindingAction::Named(FocusGitSidebar) },
    ```

- [ ] **Step 4: Verify GREEN.** `cargo test -p alacritree bindings` passes; `cargo check -p alacritree` clean (the `BindingAction::Named(other) => dispatch_scroll_or_other(other)` catch-all in `app.rs` absorbs the new variant until Task 5).

- [ ] **Step 5: Commit.** `feat(bindings): add configurable FocusGitSidebar action`

---

### Task 2: `panel_filter` module

**Files:**
- Create: `alacritree/src/panel_filter.rs`
- Modify: `alacritree/src/main.rs` (add `mod panel_filter;` in the alphabetical mod list), `alacritree/Cargo.toml` (add `nucleo-matcher = "0.3"` to `[dependencies]`)

**Interfaces:**
- Produces (consumed by Tasks 5–7):
  ```rust
  pub enum Mode { Browsing, Search }
  pub enum Outcome { FilterChanged, MoveCursor(i32), Activate, LeavePanel, Consumed }
  pub struct PanelFilter { /* private */ }
  impl PanelFilter {
      pub fn new(allowed_toggles: &'static [char]) -> Self;
      pub fn mode(&self) -> Mode;
      pub fn query(&self) -> &str;
      pub fn is_toggled(&self, key: char) -> bool;
      pub fn active_toggles(&self) -> Vec<char>;          // render order = allowed order
      pub fn is_filtering(&self) -> bool;                 // query non-empty || any toggle
      pub fn on_key(&mut self, key: egui::Key) -> Option<Outcome>;
      pub fn on_text(&mut self, text: &str) -> Option<Outcome>;
      pub fn matches(&mut self, haystack: &str) -> bool;  // empty query => true
  }
  ```

**Semantics (implement exactly):**
- `on_text`, Browsing: `"/"` → `mode = Search`, return `Some(Consumed)`. A single char in `allowed_toggles` → flip it, `Some(FilterChanged)`. Anything else → `None` (not consumed).
- `on_text`, Search: append the text to `query`, rebuild pattern, `Some(FilterChanged)`.
- `on_key`, Browsing: `Escape` with ≥1 active toggle → clear all toggles, `Some(FilterChanged)`; `Escape` otherwise → `Some(LeavePanel)`. All other keys → `None` (arrows/Enter stay with the caller's existing nav).
- `on_key`, Search: `Backspace` → pop one char (grapheme-naive `pop()` is fine — query is user-typed ASCII in practice, and a split surrogate can't occur in a Rust `String` of `char` pops), `Some(FilterChanged)`; `ArrowUp` → `Some(MoveCursor(-1))`; `ArrowDown` → `Some(MoveCursor(1))`; `Enter` → clear query, `mode = Browsing`, `Some(Activate)`; `Escape` → clear query, `mode = Browsing`, `Some(FilterChanged)`. Others → `None`.
- `matches`: with empty query always true; else `Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart)` (rebuilt only when the query changes, cached in the struct) scored via a reused `nucleo_matcher::Matcher` and `Utf32Str::new(haystack, &mut self.buf)`; a `Some` score is a match.
- Toggles are stored in a `BTreeSet<char>`; `active_toggles` returns them in `allowed_toggles` order.

- [ ] **Step 1: Add the dependency and module.** `nucleo-matcher = "0.3"` in `alacritree/Cargo.toml`; `mod panel_filter;` in `main.rs`. Create `panel_filter.rs` with a module doc comment explaining why the prompt is hand-rolled (egui-native-focus fight with the terminal view — see spec) and the type skeleton with `todo!()` bodies.

- [ ] **Step 2: Write the failing tests** in `#[cfg(test)] mod tests` inside `panel_filter.rs`:
  ```rust
  #[test] fn slash_enters_search_mode_and_is_consumed()
  #[test] fn typing_in_search_builds_the_query_and_reports_filter_change()
  #[test] fn backspace_pops_and_esc_clears_back_to_browsing()
  #[test] fn enter_in_search_activates_and_clears_the_query()
  #[test] fn arrows_in_search_move_the_cursor()
  #[test] fn toggle_keys_flip_in_browsing_and_are_inert_in_search()   // 's' toggles, then in search 's' extends the query instead
  #[test] fn esc_in_browsing_clears_toggles_before_leaving_the_panel() // two Escapes: FilterChanged then LeavePanel
  #[test] fn unknown_keys_and_text_are_not_consumed_in_browsing()
  #[test] fn fuzzy_match_is_subsequence_and_smart_case()
  // "fdps" matches "fix/diff-pane-scroll"; "readme" matches "README.md";
  // "Read" does NOT match "readme"; empty query matches everything.
  ```

- [ ] **Step 3: Verify RED.** `cargo test -p alacritree panel_filter` — tests fail on `todo!()`.

- [ ] **Step 4: Implement** the semantics above. Keep the module free of egui types except `egui::Key`.

- [ ] **Step 5: Verify GREEN**, run the full suite: `cargo test -p alacritree`.

- [ ] **Step 6: Commit.** `feat(sidebar): add panel_filter search and toggle state model`

---

### Task 3: filtered rows in `sidebar_nav`

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs`

**Interfaces:**
- Consumes: existing `SidebarRow`, `visible_rows`.
- Produces (consumed by Task 6):
  ```rust
  pub struct RowPredicates<'a> {
      pub home: bool,
      pub project_self: &'a dyn Fn(&Project) -> bool,
      pub worktree: &'a mut dyn FnMut(&Project, &Worktree) -> bool,
  }
  /// Render-order rows under an active filter.  Projects are force-expanded
  /// (a filter that hides its own results is useless); a header survives when
  /// it matches itself or keeps at least one visible worktree.
  pub fn filtered_rows(projects: &[Project], preds: RowPredicates<'_>) -> Vec<SidebarRow>;
  /// Cursor fallback: unchanged when still visible, else the first row.
  pub fn ensure_cursor(rows: &[SidebarRow], cursor: Option<&SidebarRow>) -> Option<SidebarRow>;
  ```
  (`worktree` is `FnMut` because the fuzzy matcher needs `&mut self`.)
- Callers use `filtered_rows` only while `is_filtering()`; otherwise `visible_rows` (stored `expanded` flags stay authoritative).

- [ ] **Step 1: Write the failing tests** (reuse the existing `project()` helper):
  ```rust
  #[test] fn filtered_rows_keeps_projects_with_matching_worktrees_and_forces_expansion()
  // collapsed project whose worktree passes => header + that worktree, others dropped
  #[test] fn filtered_rows_keeps_a_self_matching_header_without_its_worktrees()
  #[test] fn filtered_rows_drops_home_when_it_fails_the_predicate()
  #[test] fn ensure_cursor_keeps_a_visible_row_and_falls_back_to_first()
  // also: empty rows => None
  ```

- [ ] **Step 2: Verify RED**, **Step 3: Implement**, **Step 4: GREEN** (`cargo test -p alacritree sidebar_nav`), full suite.

- [ ] **Step 5: Commit.** `feat(sidebar): add filtered row visibility to sidebar_nav`

---

### Task 4: `git_nav` module

**Files:**
- Create: `alacritree/src/git_nav.rs`
- Modify: `alacritree/src/main.rs` (add `mod git_nav;`)

**Interfaces:**
- Consumes: `crate::git_status::{FileChange, ChangeKind, DiffStat}`.
- Produces (consumed by Tasks 5 and 7):
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum GitSection { Staged, Unstaged, Branch }
  /// A file row the git-panel cursor can rest on.  Identity is
  /// (section, path): kind can change between 1.5 s status refreshes and
  /// must not retarget the cursor.
  #[derive(Debug, Clone)]
  pub struct GitRow {
      pub section: GitSection,
      pub path: String,
      /// None for branch-diff rows (DiffStat carries no ChangeKind).
      pub kind: Option<ChangeKind>,
  }
  impl PartialEq for GitRow { /* section + path only */ }
  pub struct SectionCount { pub visible: usize, pub total: usize }
  pub struct GitRows { pub rows: Vec<GitRow>, pub staged: SectionCount, pub unstaged: SectionCount, pub branch: SectionCount }
  pub fn visible_rows(
      staged: &[FileChange], unstaged: &[FileChange], branch: &[DiffStat],
      kind_pass: &dyn Fn(ChangeKind) -> bool,
      query_pass: &mut dyn FnMut(&str) -> bool,
  ) -> GitRows;
  pub fn step(rows: &[GitRow], cursor: &GitRow, delta: i32) -> Option<GitRow>; // clamped; None only when rows is empty
  pub fn ensure_cursor(rows: &[GitRow], cursor: Option<&GitRow>) -> Option<GitRow>;
  ```
- Rules: staged/unstaged rows pass `kind_pass(kind) && query_pass(path)`; Conflicted rows bypass `kind_pass`; branch rows pass `query_pass(path)` only. Counts are per-section visible/total. Render order is preserved (Staged, Unstaged, Branch).

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn rows_preserve_section_order_and_counts()
  #[test] fn kind_filter_applies_to_staged_and_unstaged_but_not_branch()
  #[test] fn conflicted_rows_bypass_the_kind_filter()
  #[test] fn query_filters_all_sections()
  #[test] fn step_clamps_and_ensure_cursor_falls_back_to_first()
  #[test] fn cursor_identity_ignores_kind_changes() // same (section,path), different kind => equal
  ```

- [ ] **Step 2: RED**, **Step 3: Implement**, **Step 4: GREEN** (`cargo test -p alacritree git_nav`), full suite.

- [ ] **Step 5: Commit.** `feat(sidebar): add git_nav row and cursor model for the git panel`

---

### Task 5: git-sidebar keyboard focus mode

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: Task 1's `NamedAction::FocusGitSidebar`, Task 4's `git_nav`.
- Produces: `PaneFocus::GitSidebar`; fields `git_cursor: Option<git_nav::GitRow>`, `git_cursor_moved: bool`, `git_rows: Vec<git_nav::GitRow>`, `git_sidebar_auto_shown: bool`; methods `focus_git_sidebar()`, `handle_git_sidebar_nav(ctx)`. Task 7 builds on all of these.

**Implementation notes (follow the left panel's existing idioms throughout):**
- `PaneFocus` gains `GitSidebar`. Fix the two exhaustive matches: `ToggleSidebarFocus` dispatch (`GitSidebar => self.focus_sidebar()` — Ctrl+Shift+B keeps meaning "left↔terminal", and from the right panel it hops left) and anything else the compiler flags.
- `focus_git_sidebar()`: mirror `focus_sidebar()` — if `!show_right_sidebar`, show it and set `git_sidebar_auto_shown = true` + `persist_sidebars()`; set focus; set `git_cursor_moved = true`. Do **not** seed here (rows come from the render pass): leave `git_cursor` as-is; the render pass repairs it.
- `focus_terminal()`: additionally, if `git_sidebar_auto_shown`, hide the right sidebar and clear the flag (same round-trip as the left panel).
- `ToggleRightSidebar` dispatch: clearing `git_sidebar_auto_shown` and dropping focus back to terminal when hiding a focused panel — copy the `ToggleLeftSidebar` arm's pattern.
- Dispatch `FocusGitSidebar`: `if self.focus != PaneFocus::GitSidebar { self.focus_git_sidebar() } else { self.focus_terminal() }`.
- `handle_git_sidebar_nav(ctx)`: same event-drain shape as `handle_sidebar_nav` (retain-loop over unmodified `is_sidebar_nav_key` keys). Semantics: Up/Down step the cursor over `self.git_rows` (via `git_nav::step`, setting `git_cursor_moved`); Enter opens the diff for the cursor row; Escape → `focus_terminal()`; Left/Right/Space consumed no-ops.
- Enter → `DiffRequest`: `GitSection::Staged` → `DiffSource::Staged`; `GitSection::Unstaged` → `DiffSource::Untracked` when `kind == Some(ChangeKind::Untracked)` else `DiffSource::Worktree`; `GitSection::Branch` → `DiffSource::Branch { base }` where base = the same resolved default the render pass uses — store it alongside the rows in a `git_branch_base: Option<String>` field refreshed by the render pass (skip opening when `None`, matching the render pass's unclickable base-less rows). Call `self.open_diff(ctx, req)`; keep panel focus (the user may want the next file).
- `update()`: route `PaneFocus::GitSidebar => self.handle_git_sidebar_nav(ctx)` next to the existing ProjectsSidebar branch.
- `show_git_sidebar` render pass: build `git_nav::visible_rows(&status.staged, &status.unstaged, &status.branch_diff, &|_| true, &mut |_| true)` (real filters arrive in Task 7), store `self.git_rows` + `self.git_branch_base`, repair the cursor with `git_nav::ensure_cursor` — but only seed/repair while `self.focus == PaneFocus::GitSidebar`; prefer seeding on the row whose rebuilt `diff_key` equals `active_diff_key` when the cursor is `None`. Paint the cursor row: reuse `paint_cursor_outline` + `scroll_to_rect` gated on `std::mem::take(&mut git_cursor_moved)`, wrapping each `file_row`/`branch_diff_row` response rect exactly the way the project rows do (full-width rect from `ui.max_rect().x_range()` + the response's `y_range()`).
- Borrow note: `show_git_sidebar` currently takes `&mut self` and the closure uses `self.git_status.entry(...)`. Cursor repair inside the closure mutates more of `self` — that is fine (single `&mut self` borrow), but the status snapshot from `cache.poll` may need cloning into locals (`staged`/`unstaged`/`branch_diff` vectors) before further `self` mutation, depending on what `poll` returns. Prefer cloning the three lists into locals; they are small.

- [ ] **Step 1:** Add the enum variant + fields + `focus_git_sidebar` + dispatch arms; `cargo check -p alacritree` until exhaustive matches are clean.
- [ ] **Step 2:** Implement `handle_git_sidebar_nav` + update() routing.
- [ ] **Step 3:** Implement the render-pass row building, cursor repair/seed, cursor painting, Enter-to-diff plumbing.
- [ ] **Step 4:** `cargo test -p alacritree` (full suite) + `cargo fmt`. Manual smoke steps for the report: build runs, Ctrl+Shift+G focuses the panel, arrows move the outline, Enter opens a diff, Esc returns to the terminal, Ctrl+Shift+G from focused state returns to terminal, auto-shown panel hides again.
- [ ] **Step 5: Commit.** `feat(sidebar): add keyboard focus mode to the git sidebar`

---

### Task 6: search + toggles in the left panel

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: Task 2's `PanelFilter`, Task 3's `filtered_rows`/`ensure_cursor`.
- Produces: field `project_filter: PanelFilter` (`PanelFilter::new(&['s', 'a'])`), helper `workspace_has_sessions(&self, key: &WorkspaceKey) -> bool`, header prompt/chips rendering shared with Task 7 (free function `panel_header_filter_ui(ui, title, filter, theme)` replacing both panels' title labels — build it here, adopt it for the git panel in Task 7).

**Implementation notes:**
- Event routing: in `handle_sidebar_nav`'s retain-loop, offer each event to the filter first — `egui::Event::Text(t)` → `project_filter.on_text(t)`, `egui::Event::Key { pressed: true, modifiers: none, .. }` → `project_filter.on_key(key)`; a `Some(outcome)` consumes the event and maps to: `FilterChanged`/`Consumed` → nothing more, `MoveCursor(d)` → step over the *current* row list, `Activate` → the existing Enter handling for the cursor row, `LeavePanel` → `focus_terminal()`. Events the filter declines fall through to the existing `is_sidebar_nav_key` handling (which keeps Browsing-mode arrows/Enter/Escape working; note the Browsing `Escape` now reaches the filter first and only falls through as `LeavePanel`). In Search mode the filter consumes Up/Down/Enter/Escape/Backspace via `on_key`, so the fall-through never sees them.
- Current row list: extract a helper `current_project_rows(&mut self) -> Vec<SidebarRow>` — when `project_filter.is_filtering()`, `sidebar_nav::filtered_rows` with predicates below; else `sidebar_nav::visible_rows`. `apply_sidebar_nav` switches from `visible_rows` to this helper.
- Predicates (borrow-splitting: snapshot session/attention data into maps *before* building the closures, the way `show_project_sidebar` already snapshots): `home` = `matches("Home")` and (if `s`) home has sessions and (if `a`) home needs attention; `worktree(p, wt)` = `matches(&wt.name)` and toggle predicates on `Some(wt.path)`; `project_self(p)` = no toggle active and `matches(p.display_name())`.
- `workspace_has_sessions`: `self.sessions.iter().any(|s| s.working_directory == *key)` — with `WorkspaceKey = Option<PathBuf>` and `working_directory: Option<PathBuf>` this is direct equality; confirm field types before writing.
- After any `FilterChanged`: recompute rows and `sidebar_nav::ensure_cursor`; set `sidebar_cursor_moved` when the cursor changed.
- Render: `show_project_sidebar` computes the same row list once per frame into a `HashSet`-style membership (`home_visible: bool`, `visible_projects: HashSet<PathBuf>`, `visible_worktrees: HashSet<PathBuf>`) when filtering; skips non-members; renders worktrees of visible projects regardless of `project.expanded` while filtering (display-only force-expand — never write the flag). Session rows render only under visible parents (they already only render under their parent).
- Header: `panel_header_filter_ui` renders after the title: active-toggle chips (small monospace `[s]` labels in `theme.accent`), and in Search mode (or with a non-empty query) a `/query▌` monospace label in `theme.text`; nothing when idle. Keep the `+` add-project button working (it lives in the same `horizontal`).
- Empty filtered list: render the existing empty-state hint only when there are no projects at all; when a filter empties the list, render `no matches` in `theme.text_dim` small text.

- [ ] **Step 1:** Field + helpers + event routing (`cargo check` loop).
- [ ] **Step 2:** Render-side membership filtering + force-expand + header UI + empty state.
- [ ] **Step 3:** Full suite + fmt. Manual smoke: `/` narrows as you type with cursor following, Enter jumps and clears, `s`/`a` chips toggle and stack with the query, Esc ladder works, mouse interactions unaffected while unfiltered.
- [ ] **Step 4: Commit.** `feat(sidebar): fuzzy search and filter toggles in the projects panel`

---

### Task 7: search + toggles in the git panel

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: everything from Tasks 2, 4, 5, 6 (`PanelFilter`, `git_nav`, `git_rows`/cursor fields, `panel_header_filter_ui`).
- Produces: field `git_filter: PanelFilter` (`PanelFilter::new(&['m', 'd', 'u'])`).

**Implementation notes:**
- Event routing in `handle_git_sidebar_nav`: identical shape to Task 6 — filter first (`on_text`/`on_key`), fall through to the Task 5 nav handling. `MoveCursor` steps `git_rows`; `Activate` = the Task 5 Enter path; `LeavePanel` → `focus_terminal()`.
- Kind predicate for `git_nav::visible_rows`: with no kind toggle active, `|_| true`; else Conflicted always passes, `m` admits Modified|Renamed, `d` admits Deleted, `u` admits Untracked|Added (union of active sets).
- Query predicate: `|path| self.git_filter.matches(path)` — mind the borrow: run `visible_rows` with the filter borrowed mutably before touching other `self` state, or snapshot the lists first (Task 5 already clones them into locals).
- Render: skip rows not in the visible set; section headers show `visible of total` when filtering (`section(ui, …, "Unstaged", …)` takes a count — extend the call sites to format `"2 of 14"`-style via the existing `SectionCount`; when not filtering keep the plain count). A section with zero visible rows renders header-only. Header chips/prompt via `panel_header_filter_ui`.
- After `FilterChanged`: recompute rows + `git_nav::ensure_cursor` (the render pass already repairs, but repair immediately so the next key event acts on fresh rows).
- Esc ladder comes free from `PanelFilter`.

- [ ] **Step 1:** Field + event routing + filtered `visible_rows` wiring.
- [ ] **Step 2:** Header UI + section counts + hidden-row skipping + empty-section rendering.
- [ ] **Step 3:** Full suite + fmt. Manual smoke: `/` narrows file rows, `m`/`d`/`u` chips filter kinds (branch section unaffected by kinds), Enter opens the cursor row's diff, counts read `x of y`, Esc ladder.
- [ ] **Step 4: Commit.** `feat(sidebar): fuzzy search and kind filters in the git panel`

---

## Completion

After all tasks: final whole-branch review (superpowers:requesting-code-review), then superpowers:finishing-a-development-branch. The user has already chosen the finishing option: **merge `feat/searchable-sidebars` into `integration/all-features`** (not master), verify the suite on the merged result, then clean up the feature worktree and branch.
