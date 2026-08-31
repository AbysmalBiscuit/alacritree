# Session-Display Visibility & Directional Focus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Config + runtime toggles to always show single-session sidebar rows and tab-strip segments, plus `FocusLeft`/`FocusRight` actions that move focus across `ProjectsSidebar ↔ Terminal ↔ GitSidebar` and defer to a tmux/nvim TUI until it hits its own edge.

**Architecture:** All changes live in the `alacritree/` crate. Feature A threads a new `[ui.session_display]` config table (`config.rs`) into two runtime bools on `AlacritreeApp` read by `sidebar_session_ids` and `show_tab_strip`, flipped by two new binding actions. Feature B adds a `GitSidebar` variant to `PaneFocus` and a pure decision function `focus_move` (title × direction × panel visibility → passthrough | focus | nothing); passthrough re-synthesizes Ctrl+Arrow bytes via `input::key_to_bytes` and writes them to the active PTY.

**Tech Stack:** Rust (workspace MSRV 1.85, edition 2024), egui/eframe, serde/toml, `alacritty_terminal`.

**Spec:** `docs/superpowers/specs/2026-07-15-session-display-and-focus-design.md`

## Global Constraints

- Only touch the `alacritree/` crate. `alacritty/`, `alacritty_terminal/`, `alacritty_config*/` are read-only vendored code. Exception: Task 5 makes one function `pub` in `alacritree/src/input.rs` (that file is part of alacritree, not vendored).
- `cargo fmt` is enforced (`rustfmt.toml`); run it before every commit.
- Test loop: `cargo test -p alacritree`. Fast type-check: `cargo check -p alacritree`.
- Comments explain the *why*, never restate the *what*; no change-relative phrasing ("now", "previously", "this PR"); never delete existing comments unless the change makes them wrong.
- Conventional Commits, imperative mood, subject ≤50 chars preferred / 72 hard, lowercase after the colon. Every commit ends with the trailer:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- `docs/superpowers/` is excluded via `.git/info/exclude` — NEVER `git add` the spec or this plan. Stage files explicitly by path; never `git add -A`.
- Config defaults preserve current behavior: both `session_display` flags default `false`; new actions default unbound.

---

### Task 1: `[ui.session_display]` config parsing

**Files:**
- Modify: `alacritree/src/config.rs` (struct `UiTheme` ~line 234, `RawUi` ~line 785, `RawConfig::into_config` ~line 915, tests module ~line 1136)

**Interfaces:**
- Produces: `pub struct SessionDisplay { pub sidebar_always: bool, pub tabs_always: bool }` in `config.rs`, reachable as `config.ui.session_display` on the app's `Config`. Task 2 reads it at app construction.

- [ ] **Step 1: Write the failing tests**

In `alacritree/src/config.rs`, inside the existing `#[cfg(test)] mod tests` (it already has the `ui_from_toml` helper), add:

```rust
    #[test]
    fn session_display_defaults_to_hidden() {
        let ui = ui_from_toml("");
        assert!(!ui.session_display.sidebar_always);
        assert!(!ui.session_display.tabs_always);
    }

    #[test]
    fn session_display_parses_both_flags() {
        let ui = ui_from_toml("[ui.session_display]\nsidebar_always = true\ntabs_always = true");
        assert!(ui.session_display.sidebar_always);
        assert!(ui.session_display.tabs_always);
    }

    #[test]
    fn session_display_partial_table_leaves_the_other_flag_off() {
        let ui = ui_from_toml("[ui.session_display]\nsidebar_always = true");
        assert!(ui.session_display.sidebar_always);
        assert!(!ui.session_display.tabs_always);
    }

    /// alacritree.toml merges over alacritty.toml key-by-key, so setting one
    /// flag per file must yield both.
    #[test]
    fn session_display_merges_key_by_key() {
        let base: toml::Value =
            toml::from_str("[ui.session_display]\nsidebar_always = true").unwrap();
        let over: toml::Value =
            toml::from_str("[ui.session_display]\ntabs_always = true").unwrap();
        let raw: RawConfig = merge(base, over).try_into().unwrap();
        let sd = raw.into_config().ui.session_display;
        assert!(sd.sidebar_always);
        assert!(sd.tabs_always);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree session_display`
Expected: COMPILE ERROR — `no field `session_display` on type `UiTheme``. (In Rust TDD, a compile failure on the new field is the RED state.)

- [ ] **Step 3: Implement**

In `config.rs`, after the `parse_confirm_session_close` function (~line 232), add:

```rust
/// Whether per-session UI (sidebar session rows, tab-strip segments) renders
/// for a single-session workspace instead of waiting for the two-session
/// threshold.  These are startup defaults only: the app copies them into
/// runtime state that key bindings can toggle, and nothing is persisted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionDisplay {
    pub sidebar_always: bool,
    pub tabs_always: bool,
}
```

Add the field to `UiTheme` (after `confirm_session_close`):

```rust
    /// Show single-session sidebar rows / tab segments ([`SessionDisplay`]).
    pub session_display: SessionDisplay,
```

And to `impl Default for UiTheme` (after `confirm_session_close: ConfirmSessionClose::Never,`):

```rust
            session_display: SessionDisplay::default(),
```

After `RawUiWsl` (~line 783), add the raw struct:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawSessionDisplay {
    /// Show a workspace's sidebar session row even with a single session.
    sidebar_always: Option<bool>,
    /// Draw a tab-strip segment even with a single session.
    tabs_always: Option<bool>,
}
```

Add the field to `RawUi` (after `confirm_session_close`):

```rust
    session_display: RawSessionDisplay,
```

In `RawConfig::into_config`, extend the `UiTheme` literal (~line 915, after the `confirm_session_close` entry):

```rust
            session_display: SessionDisplay {
                sidebar_always: self.ui.session_display.sidebar_always.unwrap_or(false),
                tabs_always: self.ui.session_display.tabs_always.unwrap_or(false),
            },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree session_display`
Expected: 4 passed. Then `cargo test -p alacritree` — all pass, nothing else broken.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): add [ui.session_display] visibility options" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: rendering honors `session_display`

**Files:**
- Modify: `alacritree/src/app.rs` — `AlacritreeApp` struct (~line 130), constructor struct literal (~line 329), `show_tab_strip` (~line 1092), `sidebar_session_ids` (~line 2441), `workspace_session_rows` (~line 2751), tests (~line 3667)

**Interfaces:**
- Consumes: `config.ui.session_display` (Task 1).
- Produces: fields `session_rows_always: bool` and `session_tabs_always: bool` on `AlacritreeApp`; `sidebar_session_ids(pairs, ws, always: bool)`. Task 3's toggle actions flip these fields.

- [ ] **Step 1: Update existing tests and add the new one**

In `app.rs` tests (~line 3667): the three existing `sidebar_session_ids` tests gain a third argument `false` — every call site in `session_ids_filter_by_workspace_and_keep_spawn_order`, `session_ids_empty_for_unknown_workspace`, and `session_ids_apply_two_session_threshold` becomes e.g. `sidebar_session_ids(&pairs, &None, false)`. Then add:

```rust
    #[test]
    fn session_ids_always_flag_lists_single_sessions() {
        let one_match = vec![(ws("/a"), 1), (ws("/other"), 2)];
        assert_eq!(sidebar_session_ids(&one_match, &ws("/a"), true), vec![1]);

        // Zero sessions stays empty even with the flag on.
        let no_match: Vec<(WorkspaceKey, SessionId)> = vec![(ws("/other"), 2)];
        assert!(sidebar_session_ids(&no_match, &ws("/a"), true).is_empty());
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p alacritree session_ids`
Expected: COMPILE ERROR — `this function takes 2 arguments but 3 arguments were supplied`.

- [ ] **Step 3: Implement**

Replace `sidebar_session_ids` (~line 2437) including its doc comment:

```rust
/// Spawn-ordered ids of the sessions in `ws`, or empty below the list
/// threshold.  The threshold is normally two — a single-session workspace row
/// keeps its compact form, mirroring the tab strip — and `always` lowers it
/// to one.  Pure over (workspace, id) pairs so the grouping rule is testable
/// without spawning PTYs.
fn sidebar_session_ids(
    pairs: &[(WorkspaceKey, SessionId)],
    ws: &WorkspaceKey,
    always: bool,
) -> Vec<SessionId> {
    let ids: Vec<SessionId> = pairs.iter().filter(|(w, _)| w == ws).map(|(_, id)| *id).collect();
    let threshold = if always { 1 } else { 2 };
    if ids.len() < threshold { Vec::new() } else { ids }
}
```

In `workspace_session_rows` (~line 2751), change the call and its doc comment's threshold reference:

```rust
    /// Session rows for `ws`'s sidebar list, per `sidebar_session_ids`'s
    /// list threshold.
```

```rust
        let ids = sidebar_session_ids(&pairs, ws, self.session_rows_always);
```

Add fields to `AlacritreeApp` (after `focus: PaneFocus,` ~line 133):

```rust
    /// Runtime copies of `[ui.session_display]`.  The config is only the
    /// startup default; toggles flip these and are never persisted.
    session_rows_always: bool,
    session_tabs_always: bool,
```

In the constructor struct literal (~line 329), add the two initializers **above** the `config,` line (the literal moves `config` into the struct; these must read it first):

```rust
            session_rows_always: config.ui.session_display.sidebar_always,
            session_tabs_always: config.ui.session_display.tabs_always,
```

In `show_tab_strip` (~line 1089), update the guard and its comment:

```rust
        // Session segments only when there is a choice to make (or the user
        // forces them via session_display), but the trailing + segment always
        // renders alongside them once the strip itself renders (i.e. at least
        // one session exists).
        if indices.len() >= 2 || self.session_tabs_always {
```

(With one session the existing segment-width math yields a single full-width segment; no other change needed.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all pass, including `session_ids_always_flag_lists_single_sessions`.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(ui): apply session_display to sidebar and tab strip" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `ToggleSessionRows` / `ToggleSessionTabs` actions

**Files:**
- Modify: `alacritree/src/bindings.rs` — `NamedAction` enum (~line 22), `parse_action` (~line 457), `new_action_names_parse` test (~line 696)
- Modify: `alacritree/src/app.rs` — `dispatch_action` (~line 996)

**Interfaces:**
- Consumes: `session_rows_always` / `session_tabs_always` fields (Task 2).
- Produces: `NamedAction::ToggleSessionRows`, `NamedAction::ToggleSessionTabs`, parseable from the strings `"ToggleSessionRows"` / `"ToggleSessionTabs"`.

- [ ] **Step 1: Write the failing test**

In `bindings.rs`, extend the array in `new_action_names_parse` (~line 697) with:

```rust
            ("ToggleSessionRows", NamedAction::ToggleSessionRows),
            ("ToggleSessionTabs", NamedAction::ToggleSessionTabs),
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alacritree new_action_names_parse`
Expected: COMPILE ERROR — `no variant named `ToggleSessionRows``.

- [ ] **Step 3: Implement**

In the `NamedAction` enum, after `FocusTerminal` (~line 55):

```rust
    /// Flip the runtime `session_display.sidebar_always` value.
    ToggleSessionRows,
    /// Flip the runtime `session_display.tabs_always` value.
    ToggleSessionTabs,
```

In `parse_action`, after the `"FocusTerminal"` arm (~line 501):

```rust
        "ToggleSessionRows" => BindingAction::Named(ToggleSessionRows),
        "ToggleSessionTabs" => BindingAction::Named(ToggleSessionTabs),
```

In `app.rs` `dispatch_action`, after the `ToggleRightSidebar` arm (~line 999):

```rust
            BindingAction::Named(NamedAction::ToggleSessionRows) => {
                self.session_rows_always = !self.session_rows_always;
            },
            BindingAction::Named(NamedAction::ToggleSessionTabs) => {
                self.session_tabs_always = !self.session_tabs_always;
            },
```

(Without these arms the actions would silently fall into `dispatch_scroll_or_other` and no-op — the dispatch arms are load-bearing even though only the parse is unit-tested.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all pass.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs alacritree/src/app.rs
git commit -m "feat(bindings): add session-display toggle actions" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: focus decision function + `GitSidebar` focus variant

**Files:**
- Modify: `alacritree/src/app.rs` — `PaneFocus` (~line 124), new items next to it, `dispatch_action`'s `ToggleRightSidebar` and `ToggleSidebarFocus` arms (~lines 996, 1007), tests module

**Interfaces:**
- Produces (Task 5 consumes exactly these):
  - `enum FocusDir { Left, Right }`
  - `enum FocusMove { Passthrough, Focus(PaneFocus), Nothing }`
  - `fn focus_move(focus: PaneFocus, dir: FocusDir, left_open: bool, right_open: bool, title: &str) -> FocusMove`
  - `PaneFocus::GitSidebar`

- [ ] **Step 1: Write the failing tests**

In `app.rs` tests module, after `session_ids_always_flag_lists_single_sessions`:

```rust
    /// `focus_move` with both panels open.
    fn mv(focus: PaneFocus, dir: FocusDir, title: &str) -> FocusMove {
        focus_move(focus, dir, true, true, title)
    }

    #[test]
    fn focus_moves_between_open_panels() {
        assert_eq!(
            mv(PaneFocus::Terminal, FocusDir::Left, ""),
            FocusMove::Focus(PaneFocus::ProjectsSidebar)
        );
        assert_eq!(
            mv(PaneFocus::Terminal, FocusDir::Right, ""),
            FocusMove::Focus(PaneFocus::GitSidebar)
        );
        assert_eq!(
            mv(PaneFocus::ProjectsSidebar, FocusDir::Right, ""),
            FocusMove::Focus(PaneFocus::Terminal)
        );
        assert_eq!(
            mv(PaneFocus::GitSidebar, FocusDir::Left, ""),
            FocusMove::Focus(PaneFocus::Terminal)
        );
    }

    #[test]
    fn focus_stops_at_the_outer_edges() {
        assert_eq!(mv(PaneFocus::ProjectsSidebar, FocusDir::Left, ""), FocusMove::Nothing);
        assert_eq!(mv(PaneFocus::GitSidebar, FocusDir::Right, ""), FocusMove::Nothing);
    }

    #[test]
    fn focus_never_moves_toward_a_closed_panel() {
        assert_eq!(
            focus_move(PaneFocus::Terminal, FocusDir::Left, false, true, ""),
            FocusMove::Nothing
        );
        assert_eq!(
            focus_move(PaneFocus::Terminal, FocusDir::Right, true, false, ""),
            FocusMove::Nothing
        );
    }

    #[test]
    fn tmux_edges_gate_passthrough() {
        // No letter for the direction: the inner stack can still move.
        assert_eq!(mv(PaneFocus::Terminal, FocusDir::Left, "tmux:R /home/lev"), FocusMove::Passthrough);
        // Against that wall: alacritree takes over.
        assert_eq!(
            mv(PaneFocus::Terminal, FocusDir::Left, "tmux:LU /home/lev"),
            FocusMove::Focus(PaneFocus::ProjectsSidebar)
        );
        // Bare prefix publishes no blocked edges at all.
        assert_eq!(mv(PaneFocus::Terminal, FocusDir::Right, "tmux:"), FocusMove::Passthrough);
    }

    #[test]
    fn tmux_at_edge_with_closed_panel_does_nothing() {
        assert_eq!(
            focus_move(PaneFocus::Terminal, FocusDir::Left, false, true, "tmux:L x"),
            FocusMove::Nothing
        );
    }

    #[test]
    fn nvim_titles_always_pass_through() {
        assert_eq!(mv(PaneFocus::Terminal, FocusDir::Left, "nvim ~/notes.md"), FocusMove::Passthrough);
        assert_eq!(mv(PaneFocus::Terminal, FocusDir::Right, "vim"), FocusMove::Passthrough);
        // A shell whose title merely mentions vim is not vim.
        assert_eq!(
            mv(PaneFocus::Terminal, FocusDir::Left, "pwsh — nvim docs"),
            FocusMove::Focus(PaneFocus::ProjectsSidebar)
        );
    }

    #[test]
    fn sidebars_never_pass_through() {
        assert_eq!(
            mv(PaneFocus::ProjectsSidebar, FocusDir::Right, "tmux: x"),
            FocusMove::Focus(PaneFocus::Terminal)
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree focus_`
Expected: COMPILE ERROR — `cannot find function `focus_move`` / `no variant named `GitSidebar``.

- [ ] **Step 3: Implement**

Replace the `PaneFocus` definition (~line 121) — the derive gains `Debug` (needed by the `FocusMove` assertions) and the enum gains the variant:

```rust
/// Which pane owns keyboard input.  The terminal re-requests egui focus
/// every frame while it owns this; anything else holding focus (modals
/// aside) must win here first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneFocus {
    Terminal,
    ProjectsSidebar,
    /// Visual focus only — the git sidebar has no keyboard interaction yet.
    GitSidebar,
}
```

Directly below `PaneFocus`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusDir {
    Left,
    Right,
}

/// What a FocusLeft/FocusRight press does, decided by [`focus_move`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusMove {
    /// The TUI inside the terminal can still move that way — forward the
    /// Ctrl+Arrow to the PTY instead of switching panels.
    Passthrough,
    Focus(PaneFocus),
    Nothing,
}

/// Whether the program inside the terminal should receive the directional key
/// instead of alacritree switching panels.  Cooperating setups publish edge
/// state through the terminal title: tmux as `tmux:<edges>`, where a letter
/// (L/R/U/D) marks a wall its active pane — with any nvim split edges folded
/// in — cannot move past.  An absent letter means the inner stack still has
/// somewhere to go.  A title that looks like bare (n)vim always wins: vim
/// publishes no edge state, so alacritree can never tell when it is done.
fn inner_handles(title: &str, dir: FocusDir) -> bool {
    if let Some(rest) = title.strip_prefix("tmux:") {
        let letter = match dir {
            FocusDir::Left => 'L',
            FocusDir::Right => 'R',
        };
        return !rest.chars().take_while(char::is_ascii_uppercase).any(|c| c == letter);
    }
    title.starts_with("nvim") || title.starts_with("vim")
}

/// Panel-focus decision for FocusLeft/FocusRight.  Panels sit in a fixed
/// `ProjectsSidebar ↔ Terminal ↔ GitSidebar` row; movement toward a hidden
/// panel is dropped (focus never opens a panel), and from the terminal the
/// inner TUI gets first refusal via [`inner_handles`].
fn focus_move(
    focus: PaneFocus,
    dir: FocusDir,
    left_open: bool,
    right_open: bool,
    title: &str,
) -> FocusMove {
    if focus == PaneFocus::Terminal && inner_handles(title, dir) {
        return FocusMove::Passthrough;
    }
    let target = match (focus, dir) {
        (PaneFocus::Terminal, FocusDir::Left) => left_open.then_some(PaneFocus::ProjectsSidebar),
        (PaneFocus::Terminal, FocusDir::Right) => right_open.then_some(PaneFocus::GitSidebar),
        (PaneFocus::ProjectsSidebar, FocusDir::Right) => Some(PaneFocus::Terminal),
        (PaneFocus::GitSidebar, FocusDir::Left) => Some(PaneFocus::Terminal),
        _ => None,
    };
    match target {
        Some(t) => FocusMove::Focus(t),
        None => FocusMove::Nothing,
    }
}
```

The new variant breaks two exhaustive spots in `dispatch_action`; fix both:

`ToggleSidebarFocus` (~line 1007):

```rust
            BindingAction::Named(NamedAction::ToggleSidebarFocus) => match self.focus {
                PaneFocus::Terminal => self.focus_sidebar(),
                PaneFocus::ProjectsSidebar | PaneFocus::GitSidebar => self.focus_terminal(),
            },
```

`ToggleRightSidebar` (~line 996) — this is also the spec's "hiding the focused git panel returns focus to the terminal":

```rust
            BindingAction::Named(NamedAction::ToggleRightSidebar) => {
                self.show_right_sidebar = !self.show_right_sidebar;
                // A hidden panel cannot keep keyboard focus.
                if !self.show_right_sidebar && self.focus == PaneFocus::GitSidebar {
                    self.focus = PaneFocus::Terminal;
                }
                self.persist_sidebars();
            },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all pass, including the 7 new `focus`/`tmux`/`nvim`/`sidebars` tests.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): add git-sidebar focus and focus-move decision" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: `FocusLeft` / `FocusRight` actions with PTY passthrough

**Files:**
- Modify: `alacritree/src/bindings.rs` — `NamedAction` enum, `parse_action`, `new_action_names_parse` test
- Modify: `alacritree/src/app.rs` — `dispatch_action`, new `move_focus` method next to `focus_terminal` (~line 788), git-sidebar header in `show_git_sidebar` (~line 1643)
- Modify: `alacritree/src/input.rs` — `key_to_bytes` visibility (line 27)

**Interfaces:**
- Consumes: `focus_move`, `FocusDir`, `FocusMove`, `PaneFocus::GitSidebar` (Task 4); `Session::write(Vec<u8>)`, `session.term.lock().mode()`, `session.title` (existing).
- Produces: `NamedAction::FocusLeft` / `NamedAction::FocusRight` parseable from `"FocusLeft"` / `"FocusRight"`; `pub fn key_to_bytes(key: Key, mods: Modifiers, mode: TermMode) -> Option<Vec<u8>>` in `input.rs`.

- [ ] **Step 1: Write the failing test**

Extend the array in `new_action_names_parse` (`bindings.rs`) with:

```rust
            ("FocusLeft", NamedAction::FocusLeft),
            ("FocusRight", NamedAction::FocusRight),
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alacritree new_action_names_parse`
Expected: COMPILE ERROR — `no variant named `FocusLeft``.

- [ ] **Step 3: Implement**

`bindings.rs` — enum, after the Task 3 variants:

```rust
    /// Directional panel focus with TUI passthrough (see `focus_move` in
    /// `app.rs`).
    FocusLeft,
    FocusRight,
```

`parse_action`, after the Task 3 arms:

```rust
        "FocusLeft" => BindingAction::Named(FocusLeft),
        "FocusRight" => BindingAction::Named(FocusRight),
```

`input.rs` line 27 — export the encoder (the terminal view goes through `event_to_bytes`; `move_focus` needs the key-level entry point):

```rust
pub fn key_to_bytes(key: Key, mods: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
```

`app.rs` — dispatch arms, after the `FocusTerminal` arm (~line 1016):

```rust
            BindingAction::Named(NamedAction::FocusLeft) => self.move_focus(FocusDir::Left),
            BindingAction::Named(NamedAction::FocusRight) => self.move_focus(FocusDir::Right),
```

`app.rs` — new method directly after `focus_terminal` (~line 788):

```rust
    fn move_focus(&mut self, dir: FocusDir) {
        let idx = self.active_session_index();
        let title = idx.map(|i| self.sessions[i].title.as_str()).unwrap_or("");
        let decision =
            focus_move(self.focus, dir, self.show_left_sidebar, self.show_right_sidebar, title);
        match decision {
            FocusMove::Passthrough => {
                let Some(i) = idx else { return };
                let key = match dir {
                    FocusDir::Left => egui::Key::ArrowLeft,
                    FocusDir::Right => egui::Key::ArrowRight,
                };
                let mode = *self.sessions[i].term.lock().mode();
                // The binding consumed the key press before the terminal view
                // saw it, so the Ctrl+Arrow the inner TUI listens for is
                // re-synthesized with the terminal's own encoding.
                if let Some(bytes) = crate::input::key_to_bytes(key, egui::Modifiers::CTRL, mode) {
                    self.sessions[i].write(bytes);
                }
            },
            FocusMove::Focus(PaneFocus::ProjectsSidebar) => self.focus_sidebar(),
            FocusMove::Focus(PaneFocus::Terminal) => self.focus_terminal(),
            FocusMove::Focus(PaneFocus::GitSidebar) => self.focus = PaneFocus::GitSidebar,
            FocusMove::Nothing => {},
        }
    }
```

(`focus_sidebar()` auto-shows a hidden sidebar, but `focus_move` only yields `Focus(ProjectsSidebar)` when `left_open` is true, so the never-auto-open rule holds.)

`app.rs` — visual focus for the git panel: in `show_git_sidebar` (~line 1643), replace the header row:

```rust
                ui.horizontal(|ui| {
                    let title_color =
                        if self.focus == PaneFocus::GitSidebar { theme.accent } else { theme.text };
                    ui.label(RichText::new("Git").color(title_color).strong());
                });
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: all pass (input.rs's own tests still pass — its test module calls `super::key_to_bytes`, unaffected by the visibility change).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs alacritree/src/app.rs alacritree/src/input.rs
git commit -m "feat(bindings): add FocusLeft/FocusRight with TUI passthrough" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: whole-feature verification

**Files:** none created; runs the app.

- [ ] **Step 1: Full suite and formatting check**

Run: `cargo fmt --check && cargo test -p alacritree`
Expected: no formatting diffs, all tests pass.

- [ ] **Step 2: Manual GUI verification**

Temporarily add to the user's `%APPDATA%\alacritty\alacritree.toml` (or a scratch config) — remove after checking if not wanted:

```toml
[ui.session_display]
sidebar_always = true
tabs_always = true

[[keyboard.bindings]]
key = "F6"
action = "ToggleSessionRows"

[[keyboard.bindings]]
key = "F6"
action = "ToggleSessionTabs"
```

Run: `cargo run -p alacritree` and confirm:
- A workspace with one session shows its indented sidebar row and a full-width tab segment.
- F6 (both actions stacked on one key) hides both at once; F6 again restores them — verifying same-key stacking.
- With a fresh config (flags off), behavior matches master: no row/segment below two sessions.
- Bind `Ctrl+Left`/`Ctrl+Right` to `FocusLeft`/`FocusRight` (see Task 7 TOML) and confirm: focus cycles ProjectsSidebar ← Terminal → GitSidebar ("Git" header turns accent-colored); at the outer edges nothing happens; with a sidebar hidden, movement toward it does nothing; `Ctrl+G` while the git panel is focused returns focus to the terminal.
- In a WSL session running tmux (title `tmux:<edges> …`): Ctrl+Left moves tmux panes until the leftmost pane, then alacritree's sidebar takes focus. In bare nvim: Ctrl+Arrow always reaches nvim.

**Report the outcome honestly** — if any check fails, stop and fix before Task 7; do not paper over.

---

### Task 7: user config update

**Files:**
- Modify: `C:\Users\Lev\AppData\Roaming\alacritty\alacritree.toml` (outside the repo — never committed). Reading this path is currently denied by permission settings; the Read/Edit attempt will prompt the user to allow it.

- [ ] **Step 1: Read the existing file**

Read `C:\Users\Lev\AppData\Roaming\alacritty\alacritree.toml` in full. Check for existing `[[keyboard.bindings]]` entries with the same key+mods triggers as below (`T` Ctrl+Shift, `PageUp`/`PageDown` Ctrl+Shift, `Left`/`Right` Ctrl). If a trigger already exists, surface it to the user instead of silently double-binding (user bindings on the same trigger both run).

- [ ] **Step 2: Append the bindings**

```toml
# session management
[[keyboard.bindings]]
key = "T"
mods = "Control|Shift"
action = "SpawnNewInstance"

[[keyboard.bindings]]
key = "PageUp"
mods = "Control|Shift"
action = "SelectPreviousTab"

[[keyboard.bindings]]
key = "PageDown"
mods = "Control|Shift"
action = "SelectNextTab"

# directional panel focus (TUI-aware; tmux/nvim keep the key till their edge)
[[keyboard.bindings]]
key = "Left"
mods = "Control"
action = "FocusLeft"

[[keyboard.bindings]]
key = "Right"
mods = "Control"
action = "FocusRight"
```

- [ ] **Step 3: Verify the config loads**

Run: `cargo run -p alacritree` — confirm no config warnings at startup and each binding fires (Ctrl+Shift+T opens a shell; Ctrl+Shift+PgUp/PgDn cycles a 2-session workspace; Ctrl+Left/Right moves focus). No commit — this file is not in the repository.

---

## Self-review notes

- Spec coverage: config table (T1), sidebar threshold + fallbacks (T2), tab strip (T2), toggles + stacking (T3, stacking verified manually in T6), GitSidebar focus + visual highlight + hide-guard (T4/T5), FocusLeft/Right + passthrough + nvim fallback + plain shell (T4/T5), user config edits (T7), all listed tests present.
- Deliberate scope holds: no persistence, no per-workspace overrides, no git-panel keyboard nav, no Up/Down, no focus-gating of bindings.
- Type consistency: `focus_move` signature identical in T4 definition, T4 tests, and T5 caller; `key_to_bytes(key, mods, mode)` matches `input.rs:27`.
