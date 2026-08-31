# Close Last Session Without Respawn — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Closing a workspace's last session (× button, new Ctrl+Shift+W keybinding, MCP/CLI, or shell exit) no longer auto-respawns a shell; the view falls back to the project's main checkout when it has a live session, otherwise to home.

**Architecture:** All removal paths already funnel through `AlacritreeApp::close_session` (`reap_exited_sessions` calls it too), so a single post-removal hook there covers every close path. A pure, PTY-free decision helper (`close_fallback` + `project_main_for`) picks Stay/Activate/Home; the per-frame session recovery in the central panel becomes adopt-only (never spawns). The `CloseSession` binding action is adapted from commit `d708fc2f` on `feat/keyboard-actions` (verbatim cherry-pick conflicts on master and references branch-only `SidebarRow::Session`; the adaptation targets the active session only).

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, cargo workspace. Only the `alacritree/` crate changes.

**Spec:** `docs/superpowers/specs/2026-07-16-close-last-session-design.md`

## Global Constraints

- Only touch `alacritree/src/**` and `docs/keyboard-shortcuts.md`. Vendored crates (`alacritty*`) are read-only.
- Do **not** commit anything under `docs/superpowers/` — specs and plans stay untracked and must never reach an upstream PR.
- Conventional Commits, imperative subject, ≤50 chars incl. type prefix, lowercase after colon.
- `cargo fmt` before every commit (rustfmt is enforced).
- Comments explain *why*, never *what*; no task/PR references in comments.
- All work happens on branch `feat/close-last-session` cut from `master`.
- Windows host: the Bash tool runs Git Bash; repo path is `/c/Users/Lev/Git/github/alacritree`.

## Verified codebase facts (read before assuming otherwise)

- `WorkspaceKey = Option<PathBuf>` (`app.rs:30`); `None` = home. `SessionId = u64` (`session.rs:67`).
- `reap_exited_sessions` (`app.rs:2910`) already delegates to `close_session`, and runs at the **end** of `update` (`app.rs:4047`).
- `close_session` callers: sidebar × (`app.rs:1754` → `request_close_session`), confirm dialog (`app.rs:3173`), IPC `Req::CloseSession` (`app.rs:3843`), reap. Every call site has `ctx: &Context` in scope.
- The initial shell is spawned in `AlacritreeApp::new` (`app.rs:429`), **not** by the per-frame recovery — making the frame path adopt-only cannot break startup.
- `Project { root, name, label, default_branch, worktrees, expanded, shell_override }`, `Worktree { name, path, branch, is_main, prunable }` (`projects.rs:11-35`). Non-git roots get a single pseudo-worktree pointing at themselves with `is_main: true`, so `project_main_for` correctly returns `None` for them (falls back to home).
- Diff panes (`SessionKind::Diff`) are ordinary entries in `self.sessions` and count as live sessions everywhere; this plan keeps that uniform (a lone diff pane at the project main counts as a session to fall back to).
- `app.rs` tests mod starts at `app.rs:4113` with helper `fn ws(p: &str) -> WorkspaceKey`. `bindings.rs` tests mod has `parse_bindings`, `named_matches`, `parse_action` helpers.

---

### Task 1: `CloseSession` binding action (adapted from d708fc2f)

**Files:**
- Modify: `alacritree/src/bindings.rs` (enum ~line 53, `default_bindings()` ~line 195, `parse_action()` ~line 499, tests mod end ~line 789)
- Modify: `alacritree/src/app.rs:1087-1096` (dispatch match)
- Modify: `docs/keyboard-shortcuts.md` (defaults table ~line 59, action list ~line 121)

**Interfaces:**
- Produces: `NamedAction::CloseSession` variant; default `Ctrl+Shift+W` binding; TOML action name `"CloseSession"`; dispatch arm calling `self.request_close_session(ctx, id)` — note Task 2 changes `request_close_session` to take `ctx`; if Task 1 runs first, call `self.request_close_session(id)` (current signature) and Task 2's threading sweep updates this arm.
- Consumes: existing `request_close_session`, `active_session_index`.

Reference commit (context only — do NOT cherry-pick, it conflicts and references `SidebarRow::Session` which doesn't exist on master): `git show d708fc2f`.

- [ ] **Step 1: Create the branch**

```bash
cd /c/Users/Lev/Git/github/alacritree
git checkout master && git pull && git checkout -b feat/close-last-session
```

- [ ] **Step 2: Write the failing tests**

At the end of the `mod tests` block in `alacritree/src/bindings.rs` (before its closing `}`), add:

```rust
    #[test]
    fn close_session_is_a_default_ctrl_shift_w_binding() {
        let b = parse_bindings(vec![]);
        assert_eq!(
            named_matches(&b, Key::W, Modifiers::CTRL | Modifiers::SHIFT),
            vec![NamedAction::CloseSession]
        );
    }

    #[test]
    fn close_session_parses_from_config_name() {
        assert!(matches!(
            parse_action("CloseSession"),
            BindingAction::Named(NamedAction::CloseSession)
        ));
    }
```

- [ ] **Step 3: Run tests to verify they fail for the right reason**

Run: `cargo test -p alacritree close_session`
Expected: compile error `no variant or associated item named 'CloseSession' found for enum 'NamedAction'` — RED because the variant doesn't exist, not a typo.

- [ ] **Step 4: Implement the action**

In `alacritree/src/bindings.rs`:

(a) Enum — after the `ToggleSidebarFocus,` variant (~line 53), add:

```rust
    CloseSession,
```

(b) `default_bindings()` — after the `ToggleSidebarFocus` KeyBinding block (the one with `key: Key::B, mods: ctrl_shift`, ~line 191-195) and before the `Key::T`/`SpawnNewInstance` line, add:

```rust
        KeyBinding { key: Key::W, mods: ctrl_shift, action: BindingAction::Named(CloseSession) },
```

(c) `parse_action()` — after the `"ToggleSidebarFocus"` arm (~line 499), add:

```rust
        "CloseSession" => BindingAction::Named(CloseSession),
```

In `alacritree/src/app.rs`, dispatch match — after the `ToggleSidebarFocus` arm (ends `app.rs:1090`) and before the `FocusProjectsSidebar` arm, add:

```rust
            BindingAction::Named(NamedAction::CloseSession) => {
                if let Some(idx) = self.active_session_index() {
                    let id = self.sessions[idx].id;
                    self.request_close_session(id);
                }
            },
```

(The `feat/keyboard-actions` version also targets the sidebar-cursored session; master has no session cursor rows, so this targets only the active session. Keep the arm position identical to the branch so the eventual merge conflicts minimally.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alacritree close_session`
Expected: 2 passed. Then `cargo test -p alacritree` — full suite green (a `session.rs` pager-probe test may be flaky on CI until the upstream TERM fix merges; locally it should pass — if it fails, confirm it also fails on untouched `master` before treating it as ours).

- [ ] **Step 6: Document the shortcut**

In `docs/keyboard-shortcuts.md`:

(a) Defaults table — after the `Ctrl+Shift+B` row (~line 59), add:

```markdown
| `Ctrl+Shift+W`       | Close the active session in the current workspace     |
```

(b) Action-name list — after the `SelectLastTab` bullet (~line 121), add:

```markdown
- `CloseSession` — close the active session in the current workspace.
  Honors the `confirm_session_close` policy (may open a confirmation
  dialog).
```

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs alacritree/src/app.rs docs/keyboard-shortcuts.md
git diff --staged   # review: exactly the changes above, nothing else
git commit -m "feat(bindings): add rebindable CloseSession (ctrl+shift+w)" -m "Closing a session had no keyboard path; the sidebar's mouse-only
close button was the sole way to end one. Add CloseSession as a
NamedAction following the existing rebindable-shortcut pattern, bound
to Ctrl+Shift+W by default. Dispatch closes the on-screen shell and
goes through request_close_session so confirm_session_close is
honored.

Adapted from d708fc2f on feat/keyboard-actions, minus the
sidebar-cursor targeting that depends on that branch's session rows.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Fallback navigation instead of respawn

**Files:**
- Modify: `alacritree/src/app.rs`:
  - `ensure_active_session` (line 660) — split out adopt-only recovery
  - `close_session` (line 675) — thread `ctx`, apply fallback
  - `request_close_session` (line 696) — thread `ctx`
  - `reap_exited_sessions` (line 2910) — thread `ctx`
  - central-panel frame recovery (line 3991-3993) — adopt-only
  - call sites: `app.rs:1755`, `app.rs:3173`, `app.rs:3843`, `app.rs:4047`, and the Task 1 dispatch arm
  - free functions near `sidebar_session_ids` (line 2663): new `CloseFallback`, `close_fallback`, `project_main_for`
  - tests mod (line 4113+)

**Interfaces:**
- Consumes: `Project`/`Worktree` from `crate::projects`; existing `activate_home(ctx)`, `spawn_session`, `workspace_session_indices`, `active_session_index`.
- Produces:
  - `enum CloseFallback { Stay, Activate(PathBuf), Home }` (derives `Debug, PartialEq`)
  - `fn close_fallback(removed_ws: &WorkspaceKey, current_ws: &WorkspaceKey, remaining: &[(WorkspaceKey, SessionId)], main_checkout: Option<PathBuf>) -> CloseFallback`
  - `fn project_main_for(projects: &[Project], ws: &Path) -> Option<PathBuf>`
  - `fn close_session(&mut self, ctx: &Context, id: SessionId)`, `fn request_close_session(&mut self, ctx: &Context, id: SessionId)`, `fn reap_exited_sessions(&mut self, ctx: &Context)`
  - `fn adopt_active_session(&mut self)`

The helpers and their wiring land in one commit: helpers without a consumer
would trip `dead_code` on a non-test build.

- [ ] **Step 1: Write the failing helper tests**

In the `mod tests` block of `alacritree/src/app.rs` (after the `session_ids_apply_two_session_threshold` test, ~line 4188), add:

```rust
    use crate::projects::{Project, Worktree};

    /// A project whose main checkout is `root`, plus secondary worktrees.
    fn project_with(root: &str, extra: &[&str]) -> Project {
        let wt = |path: &str, is_main: bool| Worktree {
            name: path.to_string(),
            path: PathBuf::from(path),
            branch: None,
            is_main,
            prunable: false,
        };
        Project {
            root: PathBuf::from(root),
            name: "p".to_string(),
            label: None,
            default_branch: None,
            worktrees: std::iter::once(wt(root, true))
                .chain(extra.iter().map(|p| wt(p, false)))
                .collect(),
            expanded: true,
            shell_override: None,
        }
    }

    #[test]
    fn fallback_prefers_project_main_with_live_session() {
        let remaining = vec![(ws("/repo"), 1)];
        assert_eq!(
            close_fallback(
                &ws("/repo/wt"),
                &ws("/repo/wt"),
                &remaining,
                Some(PathBuf::from("/repo"))
            ),
            CloseFallback::Activate(PathBuf::from("/repo"))
        );
    }

    #[test]
    fn fallback_goes_home_when_project_main_has_no_session() {
        let remaining = vec![(ws("/other"), 1)];
        assert_eq!(
            close_fallback(
                &ws("/repo/wt"),
                &ws("/repo/wt"),
                &remaining,
                Some(PathBuf::from("/repo"))
            ),
            CloseFallback::Home
        );
    }

    #[test]
    fn fallback_goes_home_from_the_project_main_itself() {
        // project_main_for returns None when ws is the main checkout, so the
        // decision sees no main to activate.
        assert_eq!(close_fallback(&ws("/repo"), &ws("/repo"), &[], None), CloseFallback::Home);
    }

    #[test]
    fn fallback_goes_home_from_home() {
        assert_eq!(close_fallback(&None, &None, &[], None), CloseFallback::Home);
    }

    #[test]
    fn fallback_stays_on_background_workspace_close() {
        assert_eq!(
            close_fallback(&ws("/repo/wt"), &None, &[], Some(PathBuf::from("/repo"))),
            CloseFallback::Stay
        );
    }

    #[test]
    fn fallback_stays_when_siblings_survive() {
        let remaining = vec![(ws("/repo/wt"), 2)];
        assert_eq!(
            close_fallback(
                &ws("/repo/wt"),
                &ws("/repo/wt"),
                &remaining,
                Some(PathBuf::from("/repo"))
            ),
            CloseFallback::Stay
        );
    }

    #[test]
    fn project_main_resolves_for_secondary_worktrees_only() {
        let projects = vec![project_with("/repo", &["/repo-wt/feat"])];
        assert_eq!(
            project_main_for(&projects, Path::new("/repo-wt/feat")),
            Some(PathBuf::from("/repo"))
        );
        // The main itself and unknown paths have no fallback target.
        assert_eq!(project_main_for(&projects, Path::new("/repo")), None);
        assert_eq!(project_main_for(&projects, Path::new("/elsewhere")), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail for the right reason**

Run: `cargo test -p alacritree fallback`
Expected: compile error — `cannot find function 'close_fallback'` / `'project_main_for'` / `cannot find type 'CloseFallback'`.

- [ ] **Step 3: Implement the pure helpers**

In `alacritree/src/app.rs`, directly after `sidebar_session_ids` (ends line 2666), add:

```rust
/// Where the view goes after a session's removal.
#[derive(Debug, PartialEq)]
enum CloseFallback {
    /// Removal didn't empty the on-screen workspace — no navigation.
    Stay,
    /// Switch to the project's main checkout, which still has a session.
    Activate(PathBuf),
    /// Switch to home; `activate_home` spawns a shell there if none exists.
    Home,
}

/// Post-close navigation for the workspace that just lost a session.
/// `remaining` is the session list after removal; `main_checkout` is the
/// removed workspace's project main (None when the workspace *is* the main,
/// is home, or belongs to no known project). Pure over (workspace, id)
/// pairs for the same reason as `sidebar_session_ids`.
fn close_fallback(
    removed_ws: &WorkspaceKey,
    current_ws: &WorkspaceKey,
    remaining: &[(WorkspaceKey, SessionId)],
    main_checkout: Option<PathBuf>,
) -> CloseFallback {
    if removed_ws != current_ws || remaining.iter().any(|(w, _)| w == removed_ws) {
        return CloseFallback::Stay;
    }
    match main_checkout {
        Some(main) if remaining.iter().any(|(w, _)| w.as_deref() == Some(main.as_path())) => {
            CloseFallback::Activate(main)
        },
        _ => CloseFallback::Home,
    }
}

/// The owning project's main checkout for `ws`, or None when `ws` already
/// is the main (including non-git roots, whose single pseudo-worktree is
/// its own main) or belongs to no known project.
fn project_main_for(projects: &[Project], ws: &Path) -> Option<PathBuf> {
    let project = projects.iter().find(|p| p.worktrees.iter().any(|w| w.path == ws))?;
    let main = project.worktrees.iter().find(|w| w.is_main)?;
    if main.path == ws { None } else { Some(main.path.clone()) }
}
```

If `app.rs` doesn't already import `Worktree`, the tests-mod `use crate::projects::{Project, Worktree};` from Step 1 covers the tests; `project_main_for` itself only needs `Project` (already imported — `self.projects` is `Vec<Project>`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree fallback && cargo test -p alacritree project_main`
Expected: all 7 new tests pass. `cargo build -p alacritree` will warn about the unused helpers at this point — expected; Step 5 consumes them.

- [ ] **Step 5: Wire the fallback into `close_session` and make frame recovery adopt-only**

(a) Replace `ensure_active_session` (`app.rs:660-673`) with:

```rust
    fn ensure_active_session(&mut self, ctx: &Context) {
        if self.active_session_index().is_some() {
            return;
        }
        self.adopt_active_session();
        if self.active_session_index().is_some() {
            return;
        }
        if let Err(e) = self.spawn_session(ctx, self.current_workspace.clone()) {
            self.last_error = Some(format!("failed to spawn shell: {e}"));
        }
    }

    /// Re-attach to an existing session when the active id went stale
    /// (closed or reaped this frame). Never spawns: an emptied on-screen
    /// workspace either navigated away in `close_session` or shows the
    /// "no session" placeholder.
    fn adopt_active_session(&mut self) {
        let ws_idx = self.workspace_session_indices(&self.current_workspace);
        if let Some(&idx) = ws_idx.first() {
            let id = self.sessions[idx].id;
            self.active_session.insert(self.current_workspace.clone(), id);
        }
    }
```

(b) Replace `close_session` (`app.rs:675-694`) with:

```rust
    fn close_session(&mut self, ctx: &Context, id: SessionId) {
        let Some(idx) = self.sessions.iter().position(|s| s.id == id) else {
            return;
        };
        let workspace = self.sessions[idx].working_directory.clone();
        self.sessions.remove(idx);

        if self.active_session.get(&workspace).copied() == Some(id) {
            let fallback =
                self.sessions.iter().find(|s| s.working_directory == workspace).map(|s| s.id);
            match fallback {
                Some(new_id) => {
                    self.active_session.insert(workspace.clone(), new_id);
                },
                None => {
                    self.active_session.remove(&workspace);
                },
            }
        }

        // Closing the on-screen workspace's last session must not strand the
        // view on an empty pane, and respawning in place would make the last
        // session unclosable — fall back to the project main, then home.
        let remaining: Vec<(WorkspaceKey, SessionId)> =
            self.sessions.iter().map(|s| (s.working_directory.clone(), s.id)).collect();
        let main = workspace.as_deref().and_then(|p| project_main_for(&self.projects, p));
        match close_fallback(&workspace, &self.current_workspace, &remaining, main) {
            CloseFallback::Stay => {},
            CloseFallback::Activate(main) => {
                // The fallback verified a session exists there, so this
                // adopts rather than spawns.
                self.current_workspace = Some(main);
                self.ensure_active_session(ctx);
            },
            CloseFallback::Home => self.activate_home(ctx),
        }
    }
```

(c) Thread `ctx` through the callers:

- `request_close_session` (`app.rs:696`): signature becomes `fn request_close_session(&mut self, ctx: &Context, id: SessionId)`; its `self.close_session(id)` becomes `self.close_session(ctx, id)`.
- `reap_exited_sessions` (`app.rs:2910`): signature becomes `fn reap_exited_sessions(&mut self, ctx: &Context)`; loop body `self.close_session(ctx, id)`.
- Call sites: `app.rs:1755` → `self.request_close_session(ctx, id);` (verify `ctx` is the `&Context` param of that method — it is; `add_project_via_dialog(ctx)` at line 1704 uses it); `app.rs:3173` (in `show_close_session_dialog(&mut self, ctx: &Context)`) → `self.close_session(ctx, id);`; `app.rs:3843` (in `handle_ipc_request(&mut self, ctx, …)`) → `self.close_session(ctx, session_id);`; `app.rs:4047` → `self.reap_exited_sessions(ctx);`; Task 1's dispatch arm → `self.request_close_session(ctx, id);`.

(d) Central-panel frame recovery (`app.rs:3991-3993`): replace

```rust
                if self.active_session_index().is_none() {
                    self.ensure_active_session(ctx);
                }
```

with

```rust
                if self.active_session_index().is_none() {
                    self.adopt_active_session();
                }
```

(e) Update the stale-id comment at `app.rs:1748-1750` to match the new mechanism:

```rust
            // A stale id (session reaped this frame) self-heals next frame:
            // active_session_index() misses and adopt_active_session picks
            // an existing shell, or the empty-workspace placeholder shows.
```

- [ ] **Step 6: Build and run the full suite**

Run: `cargo check -p alacritree` — expect no warnings (the helpers now have consumers), then `cargo test -p alacritree`.
Expected: everything green, including Task 1's binding tests and the 7 fallback tests.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git diff --staged   # review: helpers, close_session wiring, ctx threading, adopt-only recovery — nothing else
git commit -m "feat(app): fall back instead of respawning closed sessions" -m "Closing (or exiting) the last session in the on-screen workspace
respawned a shell in place, which made the last session impossible to
actually close. Stop spawning from the per-frame recovery path and
navigate at close time instead: to the project's main checkout when it
still has a live session, otherwise to home, which spawns a shell only
as the last resort. Background closes and workspaces with surviving
sessions keep the old sibling-promotion behavior.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Verification

**Files:** none created; read-only checks plus a live GUI smoke test.

- [ ] **Step 1: Lints and full suite**

```bash
cargo fmt --check
cargo clippy -p alacritree --no-deps -- -D warnings
cargo test -p alacritree
```

Expected: all clean. (Clippy scoped with `--no-deps` matches CI — vendored alacritty crates are not ours to lint.)

- [ ] **Step 2: GUI smoke test**

Use the isolated GUI verification lab (isolated `APPDATA` + scratch repo under `target/`; see memory `gui-verification-lab.md`; never blanket-kill `alacritree.exe`). Build with `cargo build -p alacritree` and verify, in a scratch project with a main checkout and one worktree:

1. Home with one shell: `Ctrl+Shift+W` → confirm-or-close per config; a fresh shell appears (home is the last-resort spawn).
2. Worktree with one shell, project main also has a shell: type `exit` in the worktree shell → view lands on the project main's existing session, no new shell spawned.
3. Worktree with one shell, project main has none: `Ctrl+Shift+W` → view lands home.
4. Worktree with two shells: close one → stays in the worktree, sibling becomes active (unchanged behavior).
5. CLI path: `alacritree session close <id>` against a *background* workspace's session → no navigation in the GUI.
6. Startup unchanged: fresh launch opens home with one shell.

Record what was actually observed for each item.

- [ ] **Step 3: Report**

Summarize results to the user; do not open a PR (user asks explicitly per their global preferences).

---

## Self-review notes

- Spec coverage: no-respawn (Task 2d), fallback order (Task 2 helpers + wiring), any-exit-status closes (reap → close_session, unchanged reap predicate), keybinding (Task 1), confirm policy (dispatch goes through `request_close_session`), MCP/CLI inherits (IPC handler calls `close_session`), sidebar threshold untouched, home-spawns-as-last-resort (`activate_home` → `ensure_active_session`), explicit activations still spawn (`activate_home`/`activate_worktree` still call `ensure_active_session`). Startup verified safe (`new()` spawns at `app.rs:429`).
- macOS `Cmd+W` from the spec is deliberately **not** added: the reference commit d708fc2f omits it, and diverging would guarantee a conflict when `feat/keyboard-actions` merges. Flagged to the user as an open question.
- Type consistency: `close_fallback` takes `Option<PathBuf>` and returns `Activate(PathBuf)`; `project_main_for` returns `Option<PathBuf>`; wiring passes one into the other unchanged.
