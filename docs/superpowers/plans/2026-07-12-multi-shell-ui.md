# Multi-Shell Sidebar UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show each workspace's sessions as an auto-expanding list under its sidebar row (worktrees + Home), with per-row spawn (`+`) and close (`×`) affordances and a configurable close-confirmation policy.

**Architecture:** Pure sidebar UI on top of the existing session model (no session-model changes). New session rows reuse the sidebar's row idioms in `app.rs` (`row_with_trailing`, `icon_button`, `paint_row_status_icon`, `Cell`-based click requests). One new config option (`ui.confirm_session_close`) and one new modal. Spec: `docs/superpowers/specs/2026-07-12-multi-shell-ui-design.md` (in the main checkout; copied into the worktree by Task 1).

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, serde/toml. Windows host; commands below are Git Bash/PowerShell-agnostic unless noted.

## Global Constraints

- Only the `alacritree/` crate is touched; `alacritty*` crates are read-only vendored deps.
- `cargo fmt` is enforced (root `rustfmt.toml`) — run before every commit.
- Conventional Commits, imperative, subject ≤ ~72 chars (e.g. `feat(sidebar): …`).
- Comments explain *why*, never *what*; no task/PR references in code comments.
- Sessions must survive workspace switches — never drop a `Session` except on explicit close/delete/quit.
- `docs/specs/`, `docs/plans/`, `docs/superpowers/` are git-excluded (`.git/info/exclude`) — never commit spec/plan files.
- Session list renders only when a workspace has 2+ sessions ("auto, no chevron"); tab strip stays unchanged.
- New `[ui]` option default: `confirm_session_close = "never"`; unknown values fall back to `never` with a `log::warn!`.
- Rust TDD note: a test that fails to *compile* because the item under test doesn't exist yet counts as RED.
- Baseline: `cargo test -p alacritree` passes 2 tests on master (both in `pr_status.rs`).

---

### Task 1: Worktree and branch setup

**Files:**
- No source changes. Creates the feature worktree and copies the (git-excluded) spec/plan in.

**Interfaces:**
- Produces: worktree at `../alacritree-worktrees/feat/multi-shell-ui` on branch `feat/multi-shell-ui` (base `master`); all later tasks run inside it.

- [ ] **Step 1: Create the worktree**

From the main checkout `C:\Users\Lev\Git\github\alacritree`:

```bash
git worktree add ../alacritree-worktrees/feat/multi-shell-ui -b feat/multi-shell-ui master
```

Expected: `Preparing worktree (new branch 'feat/multi-shell-ui')`.

- [ ] **Step 2: Copy spec and plan into the worktree**

```bash
mkdir -p ../alacritree-worktrees/feat/multi-shell-ui/docs/superpowers/specs ../alacritree-worktrees/feat/multi-shell-ui/docs/superpowers/plans
cp docs/superpowers/specs/2026-07-12-multi-shell-ui-design.md ../alacritree-worktrees/feat/multi-shell-ui/docs/superpowers/specs/
cp docs/superpowers/plans/2026-07-12-multi-shell-ui.md ../alacritree-worktrees/feat/multi-shell-ui/docs/superpowers/plans/
```

These paths are covered by `.git/info/exclude` (shared across worktrees), so `git status` stays clean.

- [ ] **Step 3: Baseline check**

In the worktree:

```bash
cargo check -p alacritree && cargo test -p alacritree
```

Expected: check passes; `test result: ok. 2 passed` (from `pr_status.rs`). No commit for this task.

---

### Task 2: `ui.confirm_session_close` config option

**Files:**
- Modify: `alacritree/src/config.rs` (`UiTheme` ~line 160, its `Default` ~line 169, `RawUi` ~line 594, `into_config`'s `ui` construction ~line 674, new `#[cfg(test)] mod tests` at end of file)

**Interfaces:**
- Produces: `pub enum ConfirmSessionClose { Never, Busy, Always }` in `crate::config`, `Copy + PartialEq + Default(=Never)`; method `pub fn requires_prompt(self, busy: bool) -> bool`; field `pub confirm_session_close: ConfirmSessionClose` on `UiTheme` (so `config.ui.confirm_session_close` in `app.rs`).

- [ ] **Step 1: Write the failing tests**

At the very end of `alacritree/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ui_from_toml(input: &str) -> UiTheme {
        let value: toml::Value = toml::from_str(input).expect("valid toml");
        let raw: RawConfig = value.try_into().expect("valid config");
        raw.into_config().ui
    }

    #[test]
    fn confirm_session_close_defaults_to_never() {
        let ui = ui_from_toml("");
        assert_eq!(ui.confirm_session_close, ConfirmSessionClose::Never);
    }

    #[test]
    fn confirm_session_close_parses_all_values() {
        for (raw, expected) in [
            ("never", ConfirmSessionClose::Never),
            ("busy", ConfirmSessionClose::Busy),
            ("always", ConfirmSessionClose::Always),
        ] {
            let ui = ui_from_toml(&format!("[ui]\nconfirm_session_close = \"{raw}\""));
            assert_eq!(ui.confirm_session_close, expected, "value {raw:?}");
        }
    }

    #[test]
    fn confirm_session_close_invalid_falls_back_to_never() {
        let ui = ui_from_toml("[ui]\nconfirm_session_close = \"sometimes\"");
        assert_eq!(ui.confirm_session_close, ConfirmSessionClose::Never);
    }

    #[test]
    fn requires_prompt_covers_policy_matrix() {
        use ConfirmSessionClose::*;
        for (policy, busy, expected) in [
            (Never, false, false),
            (Never, true, false),
            (Busy, false, false),
            (Busy, true, true),
            (Always, false, true),
            (Always, true, true),
        ] {
            assert_eq!(policy.requires_prompt(busy), expected, "{policy:?} busy={busy}");
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree confirm
```

Expected: FAIL to compile — `cannot find type ConfirmSessionClose` / no field `confirm_session_close`. (Compile failure = RED.)

- [ ] **Step 3: Implement the option**

In `alacritree/src/config.rs`, add near `UiTheme` (~line 158):

```rust
/// When the sidebar's per-session `×` asks before killing the PTY.
/// Confirmations otherwise exist only at worktree/app level, so the
/// default keeps session close immediate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmSessionClose {
    #[default]
    Never,
    /// Prompt only when the session looks busy (agent glyph or spinner title).
    Busy,
    Always,
}

impl ConfirmSessionClose {
    pub fn requires_prompt(self, busy: bool) -> bool {
        match self {
            Self::Never => false,
            Self::Busy => busy,
            Self::Always => true,
        }
    }
}

fn parse_confirm_session_close(raw: Option<&str>) -> ConfirmSessionClose {
    match raw {
        None => ConfirmSessionClose::default(),
        Some("never") => ConfirmSessionClose::Never,
        Some("busy") => ConfirmSessionClose::Busy,
        Some("always") => ConfirmSessionClose::Always,
        Some(other) => {
            log::warn!("unknown ui.confirm_session_close value {other:?}, using \"never\"");
            ConfirmSessionClose::default()
        },
    }
}
```

Add to `UiTheme` (after `notifications`):

```rust
    /// Ask before the sidebar's per-session `×` kills the PTY.
    pub confirm_session_close: ConfirmSessionClose,
```

Add to `impl Default for UiTheme`:

```rust
            confirm_session_close: ConfirmSessionClose::Never,
```

Add to `RawUi` (after `notifications`):

```rust
    confirm_session_close: Option<String>,
```

In `into_config`'s `let ui = UiTheme { ... }` (~line 674), add:

```rust
            confirm_session_close: parse_confirm_session_close(
                self.ui.confirm_session_close.as_deref(),
            ),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree
```

Expected: `6 passed` (2 baseline + 4 new).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): add ui.confirm_session_close option"
```

---

### Task 3: Session busy detection

**Files:**
- Modify: `alacritree/src/session.rs` (new methods near `agent_glyph` ~line 401, new `#[cfg(test)] mod tests` at end of file)

**Interfaces:**
- Consumes: existing `Session::agent_glyph() -> Option<char>`, private `is_spinner_title(&str) -> bool` (session.rs:137).
- Produces: `pub fn is_busy(&self) -> bool` on `Session`; private pure `fn looks_busy(agent_glyph: Option<char>, title: &str) -> bool`.

- [ ] **Step 1: Write the failing tests**

At the very end of `alacritree/src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_when_agent_glyph_present() {
        assert!(looks_busy(Some('✳'), "plain title"));
    }

    #[test]
    fn busy_when_title_is_spinner() {
        assert!(looks_busy(None, "⠋ Thinking…"));
    }

    #[test]
    fn idle_when_no_glyph_and_plain_title() {
        assert!(!looks_busy(None, "~/projects/alacritree"));
        assert!(!looks_busy(None, ""));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree looks_busy -- --list
```

Expected: FAIL to compile — `cannot find function looks_busy`. (Compile failure = RED; `cargo test -p alacritree busy` also works.)

- [ ] **Step 3: Implement**

In `alacritree/src/session.rs`, below `agent_glyph` (~line 408):

```rust
    /// A session "looks busy" when its foreground process is a recognized
    /// agent or its title is in a spinner state — the signal the sidebar's
    /// close-confirmation policy keys on.
    pub fn is_busy(&self) -> bool {
        looks_busy(self.agent_glyph(), &self.title)
    }
```

And as a free function (near `is_spinner_title`):

```rust
fn looks_busy(agent_glyph: Option<char>, title: &str) -> bool {
    agent_glyph.is_some() || is_spinner_title(title)
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree
```

Expected: `9 passed`. (`is_busy` itself is only referenced from Task 8 — a `#[allow(dead_code)]`-free build still compiles because `pub` items on a binary crate's modules don't trigger dead-code warnings only if used; if `cargo check` warns about unused `is_busy`, ignore the warning for now — Task 8 consumes it. Do NOT add `#[allow(dead_code)]`.)

- [ ] **Step 5: Format and commit**

```bash
cargo fmt
git add alacritree/src/session.rs
git commit -m "feat(session): expose busy detection for close confirmation"
```

---

### Task 4: Session lists under worktree rows

**Files:**
- Modify: `alacritree/src/app.rs`:
  - new `SessionRowData`, `sidebar_session_ids`, `workspace_session_rows` near the other helpers
  - new `session_row` widget fn near `worktree_row` (~line 1555)
  - `show_project_sidebar` (~line 698): snapshot, render loop, request cells, apply block
  - new `#[cfg(test)] mod tests` at end of file

**Interfaces:**
- Consumes: `Session { id, title, working_directory, needs_attention }`, `Session::agent_glyph()`, `close_session(SessionId)`, `Theme`, `row_with_trailing`, `icon_button`, `paint_row_status_icon`, `WorkspaceKey = Option<PathBuf>`.
- Produces (used by Tasks 5–8):
  - `struct SessionRowData { id: SessionId, title: String, needs_attention: bool, agent_glyph: Option<char>, is_active: bool, is_displayed: bool }`
  - `fn sidebar_session_ids(pairs: &[(WorkspaceKey, SessionId)], ws: &WorkspaceKey) -> Vec<SessionId>`
  - `impl AlacritreeApp { fn workspace_session_rows(&self, ws: &WorkspaceKey) -> Vec<SessionRowData> }` — returns `Vec::new()` when the workspace has fewer than 2 sessions (this IS the visibility rule)
  - `struct SessionRowAction { activate: bool, close: bool }`
  - `fn session_row(ui: &mut egui::Ui, row: &SessionRowData, theme: &Theme) -> SessionRowAction`
  - request cells inside `show_project_sidebar`: `spawn_shell_request: Cell<Option<WorkspaceKey>>` (set by Task 6), `activate_session_request: Cell<Option<(WorkspaceKey, SessionId)>>`, `close_session_request: Cell<Option<SessionId>>`

- [ ] **Step 1: Write the failing tests**

At the very end of `alacritree/src/app.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ws(p: &str) -> WorkspaceKey {
        Some(PathBuf::from(p))
    }

    #[test]
    fn session_ids_filter_by_workspace_and_keep_spawn_order() {
        let pairs = vec![(None, 1), (ws("/a"), 2), (None, 3), (ws("/b"), 4), (ws("/a"), 5)];
        assert_eq!(sidebar_session_ids(&pairs, &None), vec![1, 3]);
        assert_eq!(sidebar_session_ids(&pairs, &ws("/a")), vec![2, 5]);
        assert_eq!(sidebar_session_ids(&pairs, &ws("/b")), vec![4]);
    }

    #[test]
    fn session_ids_empty_for_unknown_workspace() {
        let pairs = vec![(None, 1)];
        assert!(sidebar_session_ids(&pairs, &ws("/missing")).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alacritree session_ids
```

Expected: FAIL to compile — `cannot find function sidebar_session_ids`.

- [ ] **Step 3: Implement the data layer**

In `alacritree/src/app.rs`, near `WorktreeAction` (~line 1550), add:

```rust
/// Everything a sidebar session row needs, snapshotted before the panel
/// closure so rendering doesn't borrow `self.sessions`.
struct SessionRowData {
    id: SessionId,
    title: String,
    needs_attention: bool,
    agent_glyph: Option<char>,
    /// This workspace's remembered active session (accent icon).
    is_active: bool,
    /// Active *and* the workspace is current — the session on screen
    /// (row background highlight).
    is_displayed: bool,
}

/// Spawn-ordered ids of the sessions in `ws`.  Pure over (workspace, id)
/// pairs so the grouping rule is testable without spawning PTYs.
fn sidebar_session_ids(pairs: &[(WorkspaceKey, SessionId)], ws: &WorkspaceKey) -> Vec<SessionId> {
    pairs.iter().filter(|(w, _)| w == ws).map(|(_, id)| *id).collect()
}
```

In `impl AlacritreeApp` (near `workspace_agent_glyph`, ~line 1705), add:

```rust
    /// Session rows for `ws`'s sidebar list.  Empty below two sessions — a
    /// single-session workspace row keeps its compact form, mirroring the
    /// tab strip's threshold.
    fn workspace_session_rows(&self, ws: &WorkspaceKey) -> Vec<SessionRowData> {
        let pairs: Vec<(WorkspaceKey, SessionId)> =
            self.sessions.iter().map(|s| (s.working_directory.clone(), s.id)).collect();
        let ids = sidebar_session_ids(&pairs, ws);
        if ids.len() < 2 {
            return Vec::new();
        }
        let active = self.active_session.get(ws).copied();
        let is_current = self.current_workspace == *ws;
        ids.iter()
            .filter_map(|id| self.sessions.iter().find(|s| s.id == *id))
            .map(|s| SessionRowData {
                id: s.id,
                title: s.title.clone(),
                needs_attention: s.needs_attention,
                agent_glyph: s.agent_glyph(),
                is_active: active == Some(s.id),
                is_displayed: is_current && active == Some(s.id),
            })
            .collect()
    }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alacritree
```

Expected: `11 passed`.

- [ ] **Step 5: Implement the session row widget**

In `alacritree/src/app.rs`, after `worktree_row` (~line 1631), add:

```rust
struct SessionRowAction {
    activate: bool,
    close: bool,
}

fn session_row(ui: &mut egui::Ui, row: &SessionRowData, theme: &Theme) -> SessionRowAction {
    // Reserve a slot *before* the labels so the hover bg paints beneath them.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();

    let mut close_clicked = false;
    let mut close_rect: Option<egui::Rect> = None;
    // One indent level deeper than worktree rows (16); right: 0 keeps the ×
    // at the same x as the other rows' trailing icons.
    let frame = Frame::default().inner_margin(Margin { left: 28, right: 0, top: 3, bottom: 3 });
    let resp = frame
        .show(ui, |ui| {
            let title_color = if row.is_active { theme.text } else { theme.text_dim };
            row_with_trailing(
                ui,
                |ui| {
                    paint_row_status_icon(
                        ui,
                        theme,
                        row.needs_attention,
                        row.agent_glyph,
                        "▪",
                        row.is_active,
                    );
                    ui.add(
                        egui::Label::new(RichText::new(&row.title).color(title_color).small())
                            .truncate(),
                    );
                },
                |ui| {
                    let btn = icon_button(ui, "×", theme.text_muted, theme)
                        .on_hover_text("close session");
                    close_rect = Some(btn.rect);
                    if btn.clicked() {
                        close_clicked = true;
                    }
                },
            );
        })
        .response
        .interact(egui::Sense::click());

    // Frame allocates its space at end-of-show, so its retroactive `interact`
    // registers *after* the inner button in egui's z-order — meaning clicks on
    // the × land on this row response, not the button.  Recover by routing
    // clicks whose position falls inside the button rect to close.
    if resp.clicked() && !close_clicked {
        if let (Some(rect), Some(pos)) = (close_rect, resp.interact_pointer_pos()) {
            if rect.contains(pos) {
                close_clicked = true;
            }
        }
    }

    let bg = if row.is_displayed {
        theme.row_active_bg
    } else if resp.hovered() {
        theme.row_hover_bg
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        let rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
        ui.painter().set(bg_idx, egui::Shape::rect_filled(rect, 0.0, bg));
    }
    SessionRowAction { activate: resp.clicked() && !close_clicked, close: close_clicked }
}
```

- [ ] **Step 6: Wire it into `show_project_sidebar`**

In `show_project_sidebar` (~line 698):

a) With the other request cells at the top of the function, add:

```rust
        let spawn_shell_request: std::cell::Cell<Option<WorkspaceKey>> =
            std::cell::Cell::new(None);
        let activate_session_request: std::cell::Cell<Option<(WorkspaceKey, SessionId)>> =
            std::cell::Cell::new(None);
        let close_session_request: std::cell::Cell<Option<SessionId>> =
            std::cell::Cell::new(None);
```

(`spawn_shell_request` stays unread until Task 6 — that's expected; if the compiler warns about an unused variable in the interim, leave it, Task 6 consumes it within the same PR.)

b) In the snapshot section (after the `worktree_agent` vec, ~line 735), add:

```rust
        let home_session_rows = self.workspace_session_rows(&None);
        let worktree_session_rows: Vec<Vec<Vec<SessionRowData>>> = self
            .projects
            .iter()
            .map(|p| {
                p.worktrees
                    .iter()
                    .map(|wt| self.workspace_session_rows(&Some(wt.path.clone())))
                    .collect()
            })
            .collect();
```

(`home_session_rows` is consumed by Task 5; until then prefix nothing and accept the unused warning only if it appears — it won't, because Task 5 lands in the same PR before review. If executing tasks strictly one-commit-at-a-time and the warning bothers `cargo check`, it is a warning, not an error.)

c) In the worktree loop, directly after the `if action.delete { ... }` block (~line 858) and still inside `for (wt_idx, wt) in project.worktrees.iter().enumerate()`, add:

```rust
                                let session_rows = worktree_session_rows
                                    .get(idx)
                                    .and_then(|v| v.get(wt_idx))
                                    .map(Vec::as_slice)
                                    .unwrap_or(&[]);
                                for row in session_rows {
                                    let act = session_row(ui, row, &theme);
                                    if act.activate {
                                        activate_session_request
                                            .set(Some((Some(wt.path.clone()), row.id)));
                                    }
                                    if act.close {
                                        close_session_request.set(Some(row.id));
                                    }
                                }
```

d) In the apply block after the panel (next to `if let Some(path) = activate_request.take()`, ~line 881), add:

```rust
        if let Some((ws, id)) = activate_session_request.take() {
            // A stale id (session reaped this frame) self-heals next frame:
            // active_session_index() misses and ensure_active_session picks
            // an existing shell or spawns one.
            self.current_workspace = ws.clone();
            self.active_session.insert(ws, id);
        }
        if let Some(id) = close_session_request.take() {
            self.close_session(id);
        }
```

- [ ] **Step 7: Check, format, commit**

```bash
cargo check -p alacritree && cargo test -p alacritree && cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): list sessions under worktree rows"
```

Expected: check clean (unused-variable warnings for `spawn_shell_request`/`home_session_rows` are acceptable until Tasks 5–6), 11 tests pass.

---

### Task 5: Session list under the Home row

**Files:**
- Modify: `alacritree/src/app.rs` — `show_project_sidebar`, the `home_row(...)` call site (~line 758)

**Interfaces:**
- Consumes: `home_session_rows`, `session_row`, `activate_session_request`, `close_session_request` (all from Task 4).
- Produces: nothing new — Home behaves like a worktree row.

- [ ] **Step 1: Render Home's session rows**

In `show_project_sidebar`, directly after the `home_row(...)` if-block and *before* `ui.add_space(2.0);` (~line 768), add:

```rust
                    for row in &home_session_rows {
                        let act = session_row(ui, row, &theme);
                        if act.activate {
                            activate_session_request.set(Some((None, row.id)));
                        }
                        if act.close {
                            close_session_request.set(Some(row.id));
                        }
                    }
```

- [ ] **Step 2: Check, format, commit**

```bash
cargo check -p alacritree && cargo test -p alacritree && cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): list sessions under the home row"
```

Expected: check clean, 11 tests pass.

---

### Task 6: Spawn (`+`) affordances on workspace rows

**Files:**
- Modify: `alacritree/src/app.rs` — `worktree_row` (~line 1555), `WorktreeAction` (~line 1550), `home_row` (~line 1511), their call sites in `show_project_sidebar`, the apply block

**Interfaces:**
- Consumes: `icon_button`, `row_with_trailing`, `spawn_shell_request` cell (declared in Task 4), `spawn_session(ctx, ws)` (app.rs:290 — inserts the new session as the workspace's active session).
- Produces: `WorktreeAction { activate, delete, spawn: bool }`; `struct HomeAction { activate: bool, spawn: bool }`; `home_row` now returns `HomeAction` instead of `egui::Response`.

- [ ] **Step 1: Add `spawn` to `WorktreeAction` and the `+` button to `worktree_row`**

Change `WorktreeAction` to:

```rust
struct WorktreeAction {
    activate: bool,
    delete: bool,
    spawn: bool,
}
```

In `worktree_row`, add alongside the delete locals:

```rust
    let mut spawn_clicked = false;
    let mut spawn_rect: Option<egui::Rect> = None;
```

Replace the trailing closure body with (× first so it stays rightmost; `+` sits left of it, matching the project row's icon order):

```rust
                |ui| {
                    if !wt.is_main {
                        let btn = icon_button(ui, "×", theme.text_muted, theme)
                            .on_hover_text("delete worktree and branch");
                        delete_rect = Some(btn.rect);
                        if btn.clicked() {
                            delete_clicked = true;
                        }
                    }
                    let btn = icon_button(ui, "+", theme.text_muted, theme)
                        .on_hover_text("new shell");
                    spawn_rect = Some(btn.rect);
                    if btn.clicked() {
                        spawn_clicked = true;
                    }
                },
```

Extend the click-recovery block to cover both rects:

```rust
    if resp.clicked() && !delete_clicked && !spawn_clicked {
        if let Some(pos) = resp.interact_pointer_pos() {
            if delete_rect.is_some_and(|r| r.contains(pos)) {
                delete_clicked = true;
            } else if spawn_rect.is_some_and(|r| r.contains(pos)) {
                spawn_clicked = true;
            }
        }
    }
```

And return:

```rust
    WorktreeAction {
        activate: resp.clicked() && !delete_clicked && !spawn_clicked,
        delete: delete_clicked,
        spawn: spawn_clicked,
    }
```

- [ ] **Step 2: Rework `home_row` to carry a `+`**

Replace `home_row` entirely with (right margin drops 6→0 so the `+` aligns with the other rows' trailing icons; `row_with_trailing` replaces the plain `horizontal` to pin it right):

```rust
struct HomeAction {
    activate: bool,
    spawn: bool,
}

fn home_row(
    ui: &mut egui::Ui,
    is_active: bool,
    attention: bool,
    agent_glyph: Option<char>,
    theme: &Theme,
) -> HomeAction {
    // Reserve a slot *before* the labels so the hover bg paints beneath them.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();

    let mut spawn_clicked = false;
    let mut spawn_rect: Option<egui::Rect> = None;
    let frame = Frame::default().inner_margin(Margin { left: 6, right: 0, top: 3, bottom: 3 });
    let resp = frame
        .show(ui, |ui| {
            row_with_trailing(
                ui,
                |ui| {
                    paint_row_status_icon(ui, theme, attention, agent_glyph, "⌂", is_active);
                    ui.label(
                        RichText::new("Home")
                            .color(if is_active { theme.text } else { theme.text_dim })
                            .strong()
                            .small(),
                    );
                },
                |ui| {
                    let btn = icon_button(ui, "+", theme.text_muted, theme)
                        .on_hover_text("new shell");
                    spawn_rect = Some(btn.rect);
                    if btn.clicked() {
                        spawn_clicked = true;
                    }
                },
            );
        })
        .response
        .interact(egui::Sense::click());

    // Same z-order recovery as worktree_row: the retroactive frame interact
    // shadows the inner button, so route clicks inside its rect to spawn.
    if resp.clicked() && !spawn_clicked {
        if let (Some(rect), Some(pos)) = (spawn_rect, resp.interact_pointer_pos()) {
            if rect.contains(pos) {
                spawn_clicked = true;
            }
        }
    }

    let bg = if is_active {
        theme.row_active_bg
    } else if resp.hovered() {
        theme.row_hover_bg
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        let rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
        ui.painter().set(bg_idx, egui::Shape::rect_filled(rect, 0.0, bg));
    }
    HomeAction { activate: resp.clicked() && !spawn_clicked, spawn: spawn_clicked }
}
```

- [ ] **Step 3: Update the call sites**

Home call site (~line 758) — replace the `if home_row(...).clicked()` block with:

```rust
                    let home_action = home_row(
                        ui,
                        self.current_workspace.is_none(),
                        home_attention,
                        home_agent_glyph,
                        &theme,
                    );
                    if home_action.activate {
                        home_clicked = true;
                    }
                    if home_action.spawn {
                        spawn_shell_request.set(Some(None));
                    }
```

Worktree call site — after the existing `if action.delete { ... }` block, add:

```rust
                                if action.spawn {
                                    spawn_shell_request.set(Some(Some(wt.path.clone())));
                                }
```

Apply block — next to the other new handlers from Task 4, add:

```rust
        if let Some(ws) = spawn_shell_request.take() {
            // Spawning activates the workspace and the new session, matching
            // Ctrl+T and worktree-creation's open-on-done.
            self.current_workspace = ws.clone();
            if let Err(e) = self.spawn_session(ctx, ws) {
                self.last_error = Some(format!("failed to spawn shell: {e}"));
            }
        }
```

- [ ] **Step 4: Check, format, commit**

```bash
cargo check -p alacritree && cargo test -p alacritree && cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): spawn shells from workspace rows"
```

Expected: check clean — the unused-variable warnings from Task 4 are gone; 11 tests pass.

---

### Task 7: Suppress duplicate signals on parent rows

**Files:**
- Modify: `alacritree/src/app.rs` — the snapshot section of `show_project_sidebar` (~line 712)

**Interfaces:**
- Consumes: `home_session_rows` / `worktree_session_rows` (Task 4), `workspace_needs_attention`, `workspace_agent_glyph`.
- Produces: nothing new — changes what the existing `home_attention`, `home_agent_glyph`, `worktree_attention`, `worktree_agent` snapshots contain.

- [ ] **Step 1: Gate the aggregates on the session list being hidden**

The session-rows snapshot must be computed *first* — move the Task 4 block (`home_session_rows` + `worktree_session_rows`) to the top of the snapshot section, then replace the four aggregate snapshots with:

```rust
        // A rendered session list carries its own per-session dots and
        // glyphs; repeating them on the parent row reads as noise — the same
        // rule the project row applies when expanded.  Aggregates therefore
        // apply only while the list is hidden (fewer than two sessions).
        let home_attention =
            home_session_rows.is_empty() && self.workspace_needs_attention(&None);
        let home_agent_glyph = if home_session_rows.is_empty() {
            self.workspace_agent_glyph(&None)
        } else {
            None
        };
        let project_attention: Vec<bool> =
            self.projects.iter().map(|p| self.project_needs_attention(p)).collect();
        let worktree_attention: Vec<Vec<bool>> = self
            .projects
            .iter()
            .enumerate()
            .map(|(p_idx, p)| {
                p.worktrees
                    .iter()
                    .enumerate()
                    .map(|(w_idx, wt)| {
                        let listed = worktree_session_rows
                            .get(p_idx)
                            .and_then(|v| v.get(w_idx))
                            .is_some_and(|rows| !rows.is_empty());
                        !listed && self.workspace_needs_attention(&Some(wt.path.clone()))
                    })
                    .collect()
            })
            .collect();
        let worktree_agent: Vec<Vec<Option<char>>> = self
            .projects
            .iter()
            .enumerate()
            .map(|(p_idx, p)| {
                p.worktrees
                    .iter()
                    .enumerate()
                    .map(|(w_idx, wt)| {
                        let listed = worktree_session_rows
                            .get(p_idx)
                            .and_then(|v| v.get(w_idx))
                            .is_some_and(|rows| !rows.is_empty());
                        if listed {
                            None
                        } else {
                            self.workspace_agent_glyph(&Some(wt.path.clone()))
                        }
                    })
                    .collect()
            })
            .collect();
```

Note `project_attention` stays fully aggregated — a collapsed project shows no worktree rows at all, so its bubble-up dot must keep firing regardless of session lists.

- [ ] **Step 2: Check, format, commit**

```bash
cargo check -p alacritree && cargo test -p alacritree && cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): suppress parent signals when session list is shown"
```

Expected: check clean, 11 tests pass.

---

### Task 8: Close-confirmation policy and modal

**Files:**
- Modify: `alacritree/src/app.rs` — `AlacritreeApp` struct (~line 114), `::new` initializer (~line 251), `is_modal_open` (~line 425), the Task 4 close handler, new `request_close_session` + `show_close_session_dialog` methods (near `show_delete_dialog`, ~line 1725), `update` (~line 2182)

**Interfaces:**
- Consumes: `ConfirmSessionClose::requires_prompt` (Task 2), `Session::is_busy()` (Task 3), `close_session_request` (Task 4), `modal_frame`, `consume_modal_keys`, `focus_default`, `close_session`.
- Produces: field `pending_session_close: Option<SessionId>`; `fn request_close_session(&mut self, id: SessionId)`.

- [ ] **Step 1: Add the pending state**

Add to `AlacritreeApp` after `pending_create`:

```rust
    pending_session_close: Option<SessionId>,
```

Initialize in `::new` after `pending_create: None`:

```rust
            pending_session_close: None,
```

Extend `is_modal_open`:

```rust
    fn is_modal_open(&self) -> bool {
        self.quit_dialog_open
            || self.pending_delete.is_some()
            || self.pending_create.is_some()
            || self.pending_session_close.is_some()
    }
```

- [ ] **Step 2: Route close requests through the policy**

Add near `close_session` (~line 333):

```rust
    fn request_close_session(&mut self, id: SessionId) {
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else {
            return;
        };
        if self.config.ui.confirm_session_close.requires_prompt(session.is_busy()) {
            self.pending_session_close = Some(id);
        } else {
            self.close_session(id);
        }
    }
```

In the Task 4 apply block, replace `self.close_session(id);` with:

```rust
            self.request_close_session(id);
```

- [ ] **Step 3: Add the modal**

Add near `show_delete_dialog`:

```rust
    fn show_close_session_dialog(&mut self, ctx: &Context) {
        let theme = self.theme;
        let danger = rgb_to_color32(self.config.palette.normal[1]);
        let Some(id) = self.pending_session_close else {
            return;
        };
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else {
            // Exited between the click and this frame — nothing left to close.
            self.pending_session_close = None;
            return;
        };
        let title = format!("Close session `{}`?", session.title);
        let busy = session.is_busy();

        let (cancel_via_key, confirm_via_key) = consume_modal_keys(ctx);
        let frame = modal_frame(&theme);
        let mut confirmed = false;
        let mut cancelled = false;

        let s = theme.ui_scale;
        let modal = egui::Modal::new(egui::Id::new("alacritree_close_session_dialog"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(320.0 * s);
                ui.spacing_mut().item_spacing.y = 6.0 * s;
                ui.label(RichText::new(title).color(theme.text).strong());
                if busy {
                    ui.label(
                        RichText::new("A process appears to be running.").color(danger).small(),
                    );
                }
                ui.add_space(4.0 * s);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Enter to close · Esc to cancel")
                            .color(theme.text_muted)
                            .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = ui.add(
                            egui::Button::new(RichText::new("Close").color(danger)).frame(false),
                        );
                        if close_btn.clicked() {
                            confirmed = true;
                        }
                        let cancel = ui.add(
                            egui::Button::new(RichText::new("Cancel").color(theme.text_dim))
                                .frame(false),
                        );
                        if cancel.clicked() {
                            cancelled = true;
                        }
                        focus_default(ui.ctx(), close_btn.id);
                    });
                });
            });

        if confirm_via_key || confirmed {
            self.pending_session_close = None;
            self.close_session(id);
            return;
        }
        if cancel_via_key || cancelled || modal.should_close() {
            self.pending_session_close = None;
        }
    }
```

In `update`, next to the other dialogs (~line 2182), add:

```rust
        if self.pending_session_close.is_some() {
            self.show_close_session_dialog(ctx);
        }
```

- [ ] **Step 4: Check, format, commit**

```bash
cargo check -p alacritree && cargo test -p alacritree && cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): confirm session close per config policy"
```

Expected: check clean (no remaining unused warnings — `is_busy` is now consumed), 11 tests pass.

---

### Task 9: Final verification and handoff

**Files:**
- Modify: `docs/specs/planned_features.md` (main checkout — git-excluded, no commit)
- Build: release binary for the user's manual pass

**Interfaces:**
- Consumes: everything above.
- Produces: green build/tests, release binary in the worktree, status note for the feature ledger.

- [ ] **Step 1: Full check**

```bash
cargo fmt && git diff --exit-code && cargo test -p alacritree && cargo check -p alacritree
```

Expected: fmt produces no diff; 11 tests pass; check clean with zero warnings.

- [ ] **Step 2: Release build for manual acceptance**

```bash
cargo build -p alacritree --release
```

Expected: builds clean. Note the binary path (`target/release/alacritree.exe`) for the user.

- [ ] **Step 3: Update the feature ledger**

In the **main checkout**'s `docs/specs/planned_features.md`: re-read the file first (other sessions edit it concurrently — append, never rewrite others' entries), then add under feature 6 a paragraph following the established shape:

```
Status 2026-07-12: feat/multi-shell-ui implemented (worktree at
../alacritree-worktrees/feat/multi-shell-ui, N commits, 9 new tests).
Session lists under worktree/Home rows at 2+ sessions, + spawn / × close
affordances, parent-signal de-dup, [ui] confirm_session_close
(never|busy|always, default never). Pending: code review, user GUI
acceptance check (manual checklist in the plan Task 9), push/PR decision.
Spec/plan: docs/superpowers/{specs,plans}/2026-07-12-multi-shell-ui*.
```

(Replace `N` with the actual commit count. Do not commit this file.)

- [ ] **Step 4: Report the manual GUI checklist to the user**

Present this checklist (do not attempt it yourself — it needs interactive GUI use):

1. `+` on a worktree row spawns a shell there and switches to it; same for Home.
2. A row's list appears at 2 sessions and disappears back at 1.
3. Clicking a session row of a *different* workspace switches workspace and session.
4. Per-session attention dot (run `sleep 2 && printf '\a'` in a non-visible session); no duplicate dot on the parent row while its list is visible; single-session rows still aggregate.
5. Agent glyph (run `claude`) shows on the session row; parent de-dups the same way.
6. `×` closes: immediately with `confirm_session_close` unset; modal with `"always"`; modal only while busy with `"busy"` (set in `%APPDATA%\alacritty\alacritree.toml` under `[ui]`).
7. A git-sidebar diff shows up as a `diff: <file>` row in the current workspace's list; `q` in delta removes it.
8. Tab strip, Ctrl+T, Ctrl+Tab unchanged; active session-row highlight follows tab-strip clicks and Ctrl+Tab.
9. Sidebar hidden (Ctrl+B) → everything still works via tab strip.

No commit in this task (ledger and plan files are git-excluded).
