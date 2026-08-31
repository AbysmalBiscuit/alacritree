# Sidebar Path Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every user-visible path in alacritree read the same on Windows, Linux and macOS, and add opt-in abbreviated path rendering for the diff-pane title, the git file rows, and the git panel header.

**Architecture:** Two bug fixes change default output — a diff pane's title stops accepting ConPTY's startup title, and WSL workspace paths render in the distro's own spelling through a new `wsl::display_path`. On top of that sits a new pure `path_style` module (`Full` / `Fish` / `Zed`) driven by a per-site `[ui.path_style]` table that defaults to `Full`, i.e. inert. Only the `Zed` style at the two egui sites builds a `LayoutJob` so the filename can carry its own color/weight.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe 0.31.1, epaint 0.31.1, `alacritty_terminal`, `git2`, `toml` + `serde`, `home`.

## Global Constraints

- Only the `alacritree/` crate is edited. `alacritty/`, `alacritty_terminal/`, `alacritty_config/`, `alacritty_config_derive/` are vendored upstream and read-only.
- New UX/UI features must be config-gated and default to today's behavior. `path_style` and the `Zed` emphasis are inert with an unmodified config: `PathStyle::Full` at every site, one plain truncating label per path, no home collapsing.
- Tasks 1, 2 and 3 deliberately change default output with no config key. They are the reported bugs; gating them would leave the bugs in place.
- Formatting is display-only. `diff_key`, the git filter, `git_nav` cursors and `paint_git_row_cursor` keep operating on the raw path. Typing `src/git` in the filter still matches a row displayed as `s/g/status.rs`.
- Style strings that are unrecognized warn once and fall back to `"full"`, mirroring `parse_scrollbar` (`alacritree/src/config.rs:260`).
- Emphasis colors go through `RgbStr`, which rejects a blank string (`alacritree/src/config.rs:1120`), and any raw-schema error discards the entire merged config (`alacritree/src/config.rs:657`). `color = ""` is therefore a hard config error, not "same as absent".
- Comments explain *why*, never restate *what*. No PR/issue/task references, no change-relative phrasing ("now we", "previously", "this PR"), no RED/GREEN narration outside tests.
- Conventional Commits: `type(scope): description`, imperative, lowercase after the colon, no trailing period, subject ≤72 chars.
- `cargo fmt` is enforced. Run `cargo fmt` before each commit and stage **only** the files the task names — `cargo fmt` sometimes reformats unrelated pre-existing drift (it has done so in `alacritree/src/builtin_font.rs`); revert any such file with `git checkout -- <path>` before committing.
- Verification commands: `cargo test -p alacritree`, `cargo fmt --check`, `cargo clippy -p alacritree`. `cargo test` accepts **one** positional filter — `cargo test -p alacritree a b` errors with "unexpected argument"; run each filter separately.
- **Line numbers are as of this plan's baseline commit.** Earlier tasks insert code and shift later ones — Task 8's anchors in `app.rs`, in particular, sit below Task 7's insertions. Where a line number and a named function disagree, the **function name is authoritative**; re-locate with `rg -n "fn <name>" alacritree/src/<file>.rs`.
- Known pre-existing flake: `session::tests::a_pane_runs_its_child_without_a_console_host_handshake` fails on a slow machine on the unmodified base. If it is the only failure, it is not yours.

---

### Task 1: Pin a diff pane's title

ConPTY publishes the child's command line as an OSC-0 title at startup, so a diff pane on Windows reads `C:\Program Files\Git\cmd\git …` instead of the `diff: <path>` name alacritree gave it. On Linux nothing emits a title, so the spawn-time value survives — which is why this is Windows-only. The title also feeds the tab tooltip (`app.rs:2283`), the sidebar session row (`app.rs:4932`) and command-palette session entries (`app.rs:5494-5498`), so one fix covers four surfaces.

Derive the flag from `Session.kind` rather than storing a second `title_pinned` field: two sources of truth can drift, and the wiring through `spawn_command` / `spawn_with` would not be covered by the test below.

Knock-on effects on diff sessions, all downstream of the title never changing and all acceptable for a pane with no agent in it: the spinner-transition attention trigger (`session.rs:1115-1121`), `agent_glyph` (`session.rs:985`), `is_busy` (`session.rs:999`), and the BEL attention debounce (`app.rs:5027`).

**Files:**
- Modify: `alacritree/src/session.rs:920-957` (`drain_events`)
- Modify: `alacritree/src/session.rs:1104-1132` (`apply_term_event`)
- Test: `alacritree/src/session.rs` (the `mod tests` block starting at `:1162`)

**Interfaces:**
- Consumes: `SessionKind` (`session.rs:77`), `Session.kind` (`session.rs:88`), `Session::drain_events(&mut self, &Palette) -> DrainOutcome` (`session.rs:920`).
- Produces: `fn apply_term_event(event: TermEvent, title: &mut String, pinned: bool, exited: &mut bool, outcome: &mut DrainOutcome) -> Option<Vec<u8>>` — one new `bool` parameter in third position. The existing OSC-52 test at `session.rs:1265` calls this function and must be updated to pass `false`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `alacritree/src/session.rs`, after `osc52_copy_is_carried_out_to_the_clipboard` (which ends at `:1281`):

```rust
/// A session whose child has already exited, so nothing more arrives from
/// the PTY and an injected sequence is the only event left to drain.  On
/// Windows that also consumes ConPTY's own startup title, so the assertions
/// below are about *our* sequence rather than racing that one.
fn spawn_exited_probe(kind: SessionKind, title: &str) -> Session {
    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/c", "exit"]);
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-c", "true"]);

    let mut session = Session::spawn_command(
        egui::Context::default(),
        &Config::default(),
        std::env::current_dir().ok(),
        TermSize::new(80, 24),
        (8.0, 16.0),
        program.to_string(),
        args.into_iter().map(str::to_string).collect(),
        title.to_string(),
        kind,
    )
    .unwrap();

    // Draining until `ChildExit` is seen consumes everything the child sent:
    // the loop emits `ChildExit` and then stops reading the PTY, because
    // `spawn_with` passes `drain_on_exit: false` (`session.rs:866`,
    // `event_loop.rs:263`).  Only a `Wakeup` can follow, and nothing maps it
    // to a title.
    let palette = Palette::default();
    let start = Instant::now();
    while !session.is_exited() {
        assert!(start.elapsed() < Duration::from_secs(10), "child never exited");
        session.drain_events(&palette);
        std::thread::sleep(Duration::from_millis(1));
    }
    session
}

/// Drive a real OSC 0 through the real VT parser into the real drain, the
/// way ConPTY delivers its startup title.
fn title_after_osc(mut session: Session, osc_title: &str) -> String {
    let sequence = format!("\x1b]0;{osc_title}\x07");
    {
        let mut term = session.term.lock();
        Processor::<StdSyncHandler>::new().advance(&mut *term, sequence.as_bytes());
    }
    session.drain_events(&Palette::default());
    session.title.clone()
}

/// ConPTY defaults a child's console title to its command line and publishes
/// it as OSC 0, so a diff pane on Windows renamed itself after git's exe
/// path.  A diff pane's title is set by alacritree and never means to change.
#[test]
fn a_diff_panes_title_survives_a_title_sequence() {
    let session = spawn_exited_probe(SessionKind::Diff { key: "probe".to_string() }, "diff: src/app.rs");
    assert_eq!(
        title_after_osc(session, r"C:\Program Files\Git\cmd\git"),
        "diff: src/app.rs",
        "a diff pane must keep the name alacritree gave it"
    );
}

/// The pin is scoped to diff panes: a shell's title is the child's to set,
/// and that is how editors and agents label their tab.
#[test]
fn a_shell_still_follows_its_childs_title() {
    let session = spawn_exited_probe(SessionKind::Shell, "shell");
    assert_eq!(title_after_osc(session, "nvim src/app.rs"), "nvim src/app.rs");
}

/// Pinning suppresses one event arm, not the drain.  A bell in a diff pane
/// still raises attention and a child exit still reaps the pane.
#[test]
fn a_pinned_session_still_reports_bells_and_exit() {
    let mut outcome = DrainOutcome::default();
    let mut title = "diff: src/app.rs".to_string();
    let mut exited = false;

    apply_term_event(TermEvent::Bell, &mut title, true, &mut exited, &mut outcome);
    apply_term_event(TermEvent::ChildExit(0), &mut title, true, &mut exited, &mut outcome);

    assert!(outcome.attention);
    assert!(exited);
    assert_eq!(title, "diff: src/app.rs");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree session::tests::a_diff_panes_title -- --nocapture`

Expected: compile error — `apply_term_event` takes 4 arguments, not 5. That is the signature change the fix introduces, and it is the right failure for `a_pinned_session_still_reports_bells_and_exit`.

To prove RED for the behavioral test specifically, temporarily comment out `a_pinned_session_still_reports_bells_and_exit`, re-run `cargo test -p alacritree session::tests::a_diff_panes_title`, and confirm it fails with `assertion \`left == right\` failed: left: "C:\\Program Files\\Git\\cmd\\git", right: "diff: src/app.rs"`. This fails on **every** platform, not just Windows: `title_after_osc` injects OSC 0 through the parser itself rather than relying on the child to emit one. (What is Windows-only is the *bug* — ConPTY is what emits an unwanted title in the field.) Restore the commented-out test before Step 3.

- [ ] **Step 3: Add the `pinned` parameter and skip the title arm**

In `alacritree/src/session.rs`, replace the signature and title arm of `apply_term_event` (`:1106-1122`):

```rust
fn apply_term_event(
    event: TermEvent,
    title: &mut String,
    pinned: bool,
    exited: &mut bool,
    outcome: &mut DrainOutcome,
) -> Option<Vec<u8>> {
    match event {
        TermEvent::PtyWrite(s) => return Some(s.into_bytes()),
        TermEvent::Title(t) if !pinned => {
            // A spinner-shaped title transitioning to a non-spinner one
            // is how Claude Code (and similar tools that don't ring
            // BEL) signal "done — your turn".  Treat it like a bell.
            if is_spinner_title(title) && !is_spinner_title(&t) {
                outcome.attention = true;
            }
            *title = t;
        },
```

Leave the rest of the `match` unchanged. `TermEvent::Title` with `pinned == true` falls through to the existing `_ => {}` arm.

- [ ] **Step 4: Derive the flag in `drain_events`**

In `alacritree/src/session.rs`, at the top of `drain_events` (`:921`), insert the derivation, and pass it at the call site (`:948-950`):

```rust
    pub fn drain_events(&mut self, palette: &Palette) -> DrainOutcome {
        let mut outcome = DrainOutcome::default();
        // Derived rather than stored: a `title_pinned` field set at spawn is a
        // second source of truth that can drift from `kind`.
        let title_pinned = matches!(self.kind, SessionKind::Diff { .. });
        while let Ok(event) = self.events.try_recv() {
```

and:

```rust
                event => {
                    if let Some(bytes) = apply_term_event(
                        event,
                        &mut self.title,
                        title_pinned,
                        &mut self.exited,
                        &mut outcome,
                    ) {
                        self.write(bytes);
                    }
                },
```

- [ ] **Step 5: Update the existing OSC-52 test call**

In `alacritree/src/session.rs:1278`, change:

```rust
        apply_term_event(event, &mut title, false, &mut exited, &mut outcome);
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree session::`

Expected: PASS, including `a_diff_panes_title_survives_a_title_sequence`, `a_shell_still_follows_its_childs_title`, `a_pinned_session_still_reports_bells_and_exit` and `osc52_copy_is_carried_out_to_the_clipboard`.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/session.rs
git commit -m "fix(session): keep a diff pane's title off conpty's"
```

---

### Task 2: `wsl::display_path`

The git panel prints its workspace path raw, producing `\\wsl.localhost\kali-linux\home\lev\Git\adaptyv\monorepo` where the user expects `/home/lev/Git/adaptyv/monorepo`. This is not a discovery defect: `Project::discover_wsl` deliberately re-emits every git-reported path through `wsl::linux_to_windows` (`projects.rs:103`) so path equality survives refreshes, and `state.toml` persists the UNC form. Only the rendering is missing.

Known limitation, inherited not introduced: `classify` discards every non-`Component::Normal` component (`wsl.rs:78-83`), so a UNC path containing `..` renders as though the parent component were absent. Not fixed here.

**Files:**
- Modify: `alacritree/src/wsl.rs` (add after `normalize_root`, which ends at `:131`)
- Test: `alacritree/src/wsl.rs` (the `mod tests` block starting at `:378`)

**Interfaces:**
- Consumes: `classify(&Path) -> Location` (`wsl.rs:64`), `Location::{Wsl { distro, linux_path }, Windows}` (`wsl.rs:45`).
- Produces: `pub fn display_path(path: &Path) -> String` in module `crate::wsl`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `alacritree/src/wsl.rs`, after `classifies_drive_and_non_wsl_unc_as_windows` (`:421`):

```rust
/// `classify` is documented to accept the verbatim forms, but only the plain
/// prefixes were ever exercised.  `display_path` makes that reachable from
/// the UI, so pin it.
#[cfg(windows)]
#[test]
fn classifies_verbatim_unc() {
    let loc = classify(Path::new(r"\\?\UNC\wsl.localhost\kali-linux\home\lev"));
    assert_eq!(
        loc,
        Location::Wsl { distro: "kali-linux".to_string(), linux_path: "/home/lev".to_string() }
    );
}

#[cfg(windows)]
#[test]
fn display_path_shows_wsl_paths_in_the_distros_spelling() {
    assert_eq!(
        display_path(Path::new(r"\\wsl.localhost\kali-linux\home\lev\Git\monorepo")),
        "/home/lev/Git/monorepo"
    );
    assert_eq!(display_path(Path::new(r"\\wsl$\Ubuntu\srv")), "/srv");
    assert_eq!(
        display_path(Path::new(r"\\?\UNC\wsl.localhost\kali-linux\home\lev")),
        "/home/lev"
    );
    // A distro root has no segments of its own.
    assert_eq!(display_path(Path::new(r"\\wsl.localhost\kali-linux")), "/");
}

/// Native paths are the user's own spelling and must survive untouched —
/// this is not `windows_to_linux`, which would rewrite `C:\` into `/mnt/c`.
#[cfg(windows)]
#[test]
fn display_path_leaves_windows_paths_alone() {
    assert_eq!(display_path(Path::new(r"C:\Users\Lev\Git")), r"C:\Users\Lev\Git");
    assert_eq!(display_path(Path::new(r"\\server\share\x")), r"\\server\share\x");
}

#[cfg(not(windows))]
#[test]
fn display_path_leaves_native_paths_alone() {
    assert_eq!(display_path(Path::new("/home/lev/Git")), "/home/lev/Git");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree wsl::tests::display_path`

Expected: FAIL to compile with `cannot find function \`display_path\` in this scope`.

- [ ] **Step 3: Add `display_path`**

In `alacritree/src/wsl.rs`, immediately after `normalize_root` (which ends at `:131`):

```rust
/// How a workspace path should read to the user: WSL workspaces in the
/// distro's own spelling, native paths untouched.  Not `windows_to_linux`,
/// which also rewrites `C:\…` into `/mnt/c/…` — correct for handing a path to
/// git inside a distro, wrong for showing a Windows user their own path.
pub fn display_path(path: &Path) -> String {
    match classify(path) {
        Location::Wsl { linux_path, .. } => linux_path,
        Location::Windows(_) => path.display().to_string(),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree wsl::`

Expected: PASS, including `classifies_verbatim_unc`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/wsl.rs
git commit -m "feat(wsl): add display_path for user-facing paths"
```

---

### Task 3: Render every user-visible path through `display_path`

One worktree must not read as `/home/lev/…` in the git panel and `\\wsl.localhost\…` in the command palette. Every site below renders a workspace or project path to the user.

`notify_attention` (`app.rs:6495-6501`) is **not** changed: it takes `file_name()` and falls back to `session.title`, never to a path, so no UNC spelling can reach it. Verify this by reading `app.rs:6496-6501` before concluding the task.

**Files:**
- Modify: `alacritree/src/app.rs:3180` (git panel header), `:5509` (palette project detail), `:5528-5530` (`workspace_label` fallback), `:5537` (`workspace_entry_label`), `:5759-5763` (base-branch picker title)
- Modify: `alacritree/src/row_label.rs:58` and `:77` (`$path` template variable)
- Modify: `alacritree/src/projects.rs:273-277` (`display_name` fallback)
- Test: `alacritree/src/row_label.rs` (the `#[cfg(test)] mod tests` block at `:102`), `alacritree/src/projects.rs` (its block at `:364`)

**Interfaces:**
- Consumes: `wsl::display_path(&Path) -> String` from Task 2.
- Produces: no new API. Behavior only.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `alacritree/src/row_label.rs`:

```rust
/// `$path` is the template's window onto the filesystem, so a WSL worktree
/// must substitute the path the user would type inside the distro.
#[cfg(windows)]
#[test]
fn the_path_variable_uses_the_distros_spelling() {
    let wt = Worktree {
        name: "monorepo".to_string(),
        path: PathBuf::from(r"\\wsl.localhost\kali-linux\home\lev\Git\monorepo"),
        branch: Some("main".to_string()),
        is_main: true,
        prunable: false,
    };
    let mut templates = LabelTemplates::new(Some("$path".to_string()), None);
    assert_eq!(templates.worktree_label(&wt, None), "/home/lev/Git/monorepo");
}
```

`Worktree` and `PathBuf` need to be in scope — add `use std::path::PathBuf;` and extend the existing `use` of `crate::projects::…` to include `Worktree` if the test module does not already import them.

Add to the `mod tests` block in `alacritree/src/projects.rs`:

```rust
/// A distro root has no `file_name()`, so the name falls back to the whole
/// path — which must not be the UNC spelling.
#[cfg(windows)]
#[test]
fn a_rootless_path_names_itself_in_the_distros_spelling() {
    assert_eq!(display_name(std::path::Path::new(r"\\wsl.localhost\kali-linux")), "/");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run (one filter per invocation — `cargo test` takes a single positional filter):

```bash
cargo test -p alacritree the_path_variable_uses_the_distros_spelling
cargo test -p alacritree a_rootless_path_names_itself
```


Expected (Windows): FAIL — `left: "\\\\wsl.localhost\\kali-linux\\home\\lev\\Git\\monorepo"`, `right: "/home/lev/Git/monorepo"`, and `left: "\\\\wsl.localhost\\kali-linux"`, `right: "/"`. Both tests are `#[cfg(windows)]`; on Linux/macOS they do not build and this step is a no-op — record that and rely on the Windows run.

- [ ] **Step 3: Apply `display_path` at every site**

`alacritree/src/row_label.rs:58`:

```rust
        vars.insert("path".to_string(), crate::wsl::display_path(&wt.path));
```

`alacritree/src/row_label.rs:77`:

```rust
        vars.insert("path".to_string(), crate::wsl::display_path(&project.root));
```

`alacritree/src/projects.rs:273-277`:

```rust
fn display_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| wsl::display_path(root))
}
```

`alacritree/src/app.rs:3178-3185`:

```rust
                    ui.add(
                        egui::Label::new(
                            RichText::new(wsl::display_path(&path))
                                .color(theme.text_muted)
                                .small(),
                        )
                        .truncate(),
                    );
```

`alacritree/src/app.rs:5506-5510`:

```rust
            items.push(PaletteItem::create_worktree(
                project.root.clone(),
                format!("{}: new worktree", project.display_name()),
                format!("project · {}", wsl::display_path(&project.root)),
            ));
```

`alacritree/src/app.rs:5528-5530`:

```rust
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| wsl::display_path(path))
```

`alacritree/src/app.rs:5535-5538`:

```rust
        let secondary = match ws {
            None => "workspace · home".to_string(),
            Some(path) => format!("workspace · {}", wsl::display_path(path)),
        };
```

`alacritree/src/app.rs:5759-5763`:

```rust
                let name = picker
                    .worktree
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| wsl::display_path(&picker.worktree));
```

`app.rs` already has `use crate::wsl;` — confirm with `rg -n "^use crate::wsl" alacritree/src/app.rs` and add it if absent. `projects.rs` already has `use crate::wsl;` at `:8`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree`

Expected: PASS (modulo the known `a_pane_runs_its_child_without_a_console_host_handshake` flake).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs alacritree/src/row_label.rs alacritree/src/projects.rs
git commit -m "fix(sidebar): show wsl workspaces in linux spelling"
```

---

### Task 4: The `path_style` module

Pure and free of egui so it unit-tests without a `Ui`. The root is recognized **first**, by prefix, and it decides the separator — that is what makes this safe, because backslash and `:` are both legal inside a Unix filename, so neither `dir/name\part.txt` nor `dir/name:\part` may be treated as Windows-spelled. Splitting the root off before abbreviation is also what keeps `C:\Program Files\Git` from fish-abbreviating into `C\P\G`.

`\\wsl.localhost\distro\` and `\\wsl$\distro\` are ordinary UNC here: the distro is the *share*, so it belongs to the root and is never abbreviated.

**Files:**
- Create: `alacritree/src/path_style.rs`
- Modify: `alacritree/src/main.rs:26` (module list, alphabetical — insert between `mod panel_filter;` at `:25` and `mod paste;` at `:26`)
- Test: `alacritree/src/path_style.rs` (its own `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub enum PathStyle { Full, Fish, Zed }` — `Copy`, `Default` is `Full`.
  - `pub struct Parts { pub root: String, pub parent: String, pub name: String }`
  - `pub fn split(path: &str, style: PathStyle, home: Option<&str>) -> Parts`
  - `pub fn render(path: &str, style: PathStyle, home: Option<&str>) -> String`

- [ ] **Step 1: Register the module**

In `alacritree/src/main.rs`, add between `mod panel_filter;` (`:25`) and `mod paste;` (`:26`):

```rust
mod path_style;
```

- [ ] **Step 2: Write the failing tests**

Create `alacritree/src/path_style.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The identity guarantee: an unmodified config renders every path
    /// byte-for-byte, home directory included.
    #[test]
    fn full_is_the_identity() {
        let home = Some("/home/lev");
        for path in [
            "",
            "/",
            "src/app.rs",
            "/home/lev/Git/x/y.rs",
            r"C:\Program Files\Git",
            r"C:",
            r"C:foo\bar",
            r"\\server\share\x",
            r"\\wsl.localhost\kali-linux\home\lev",
            r"\\?\UNC\wsl.localhost\kali-linux\home\lev",
            r"\\?\C:\Users\Lev",
            r"dir/name\part.txt",
            "a/b/",
        ] {
            assert_eq!(render(path, PathStyle::Full, home), path, "Full changed {path:?}");
            assert_eq!(render(path, PathStyle::Full, None), path, "Full changed {path:?}");
        }
    }

    #[test]
    fn split_recognizes_roots_before_separators() {
        let cases: &[(&str, (&str, &str, &str))] = &[
            ("", ("", "", "")),
            ("/", ("/", "", "")),
            (r"C:\", (r"C:\", "", "")),
            ("C:", ("C:", "", "")),
            ("C:foo", ("C:", "", "foo")),
            (r"C:foo\bar", ("C:", r"f\", "bar")),
            ("f.txt", ("", "", "f.txt")),
            ("/f.txt", ("/", "", "f.txt")),
            ("a/b/", ("", "a/", "b")),
            (r"C:\Program Files\Git", (r"C:\", r"P\", "Git")),
            (r"\\server\share\a\b.txt", (r"\\server\share\", r"a\", "b.txt")),
            (
                r"\\wsl.localhost\kali-linux\home\lev\x.rs",
                (r"\\wsl.localhost\kali-linux\", r"h\l\", "x.rs"),
            ),
            (r"\\?\C:\Users\Lev\x", (r"\\?\C:\", r"U\L\", "x")),
            (
                r"\\?\UNC\server\share\a\b",
                (r"\\?\UNC\server\share\", r"a\", "b"),
            ),
        ];
        for (input, (root, parent, name)) in cases {
            let parts = split(input, PathStyle::Fish, None);
            assert_eq!(
                (parts.root.as_str(), parts.parent.as_str(), parts.name.as_str()),
                (*root, *parent, *name),
                "split({input:?})"
            );
        }
    }

    /// Backslash and `:` are legal in a Unix filename, so a path that does
    /// not match a Windows root prefix must split only on `/`.
    #[test]
    fn a_unix_filename_may_contain_a_backslash_or_colon() {
        let parts = split(r"dir/name\part.txt", PathStyle::Fish, None);
        assert_eq!(parts.root, "");
        assert_eq!(parts.parent, "d/");
        assert_eq!(parts.name, r"name\part.txt");

        let parts = split(r"dir/name:\part", PathStyle::Fish, None);
        assert_eq!(parts.name, r"name:\part");
    }

    /// Windows paths spelled with forward slashes split on either separator
    /// and re-join with a backslash.
    #[test]
    fn a_drive_path_accepts_forward_slashes() {
        assert_eq!(render("C:/Users/Lev/x.rs", PathStyle::Fish, None), r"C:\U\L\x.rs");
    }

    #[test]
    fn fish_abbreviates_parents_and_keeps_a_leading_dot() {
        assert_eq!(render("path/to/file.txt", PathStyle::Fish, None), "p/t/file.txt");
        assert_eq!(render("/a/.config/nvim/init.lua", PathStyle::Fish, None), "/a/.c/n/init.lua");
        assert_eq!(render("f.txt", PathStyle::Fish, None), "f.txt");
        assert_eq!(render("/", PathStyle::Fish, None), "/");
        assert_eq!(render("", PathStyle::Fish, None), "");
    }

    /// Zed gets the same input classes as Fish — drive-relative, UNC, dotted,
    /// trailing separator, empty, root-only — because the reorder is where a
    /// root can most easily be dropped or duplicated.
    #[test]
    fn zed_swaps_the_name_ahead_of_the_parent_and_keeps_the_root() {
        assert_eq!(render("path/to/file.txt", PathStyle::Zed, None), "file.txt path/to/");
        assert_eq!(render("/a/b/c.txt", PathStyle::Zed, None), "c.txt /a/b/");
        // No parent: the bare name, no trailing space — but the root stays.
        assert_eq!(render("f.txt", PathStyle::Zed, None), "f.txt");
        assert_eq!(render("/f.txt", PathStyle::Zed, None), "/f.txt");
        assert_eq!(render(r"C:\f.txt", PathStyle::Zed, None), r"C:\f.txt");
        assert_eq!(render("", PathStyle::Zed, None), "");
        assert_eq!(render("/", PathStyle::Zed, None), "/");
        assert_eq!(render("C:", PathStyle::Zed, None), "C:");
        // Drive-relative: no separator may appear after the root.
        assert_eq!(render(r"C:foo\bar", PathStyle::Zed, None), r"bar C:foo\");
        assert_eq!(render("a/b/", PathStyle::Zed, None), "b a/");
        assert_eq!(render("/a/.config/init.lua", PathStyle::Zed, None), "init.lua /a/.config/");
        assert_eq!(
            render(r"\\wsl.localhost\kali-linux\home\lev\x.rs", PathStyle::Zed, None),
            r"x.rs \\wsl.localhost\kali-linux\home\lev\"
        );
    }

    #[test]
    fn home_collapses_for_fish_and_zed_only() {
        let home = Some("/home/lev");
        assert_eq!(render("/home/lev/Git/x/y.rs", PathStyle::Fish, home), "~/G/x/y.rs");
        assert_eq!(render("/home/lev/Git/x/y.rs", PathStyle::Zed, home), "y.rs ~/Git/x/");
        assert_eq!(render("/home/lev", PathStyle::Fish, home), "~");
        assert_eq!(render("/home/lev/Git/x/y.rs", PathStyle::Full, home), "/home/lev/Git/x/y.rs");
        // A sibling directory whose name merely starts with the home prefix
        // is not inside it.
        assert_eq!(render("/home/levi/x.rs", PathStyle::Fish, home), "/h/l/x.rs");
        // No home, no guess.
        assert_eq!(render("/home/lev/Git/y.rs", PathStyle::Fish, None), "/h/l/G/y.rs");
    }

    #[test]
    fn home_matching_is_case_and_separator_insensitive_on_windows_paths() {
        let home = Some(r"C:\Users\Lev");
        assert_eq!(render(r"c:\users\lev\Git\y.rs", PathStyle::Fish, home), r"~\G\y.rs");
        assert_eq!(render("C:/Users/Lev/Git/y.rs", PathStyle::Fish, home), r"~\G\y.rs");
        // NTFS folds case past ASCII, so `to_ascii_lowercase` is not enough.
        assert_eq!(
            render(r"c:\üsers\lev\Git\y.rs", PathStyle::Fish, Some(r"C:\Üsers\Lev")),
            r"~\G\y.rs"
        );
        // POSIX paths compare exactly — case matters on a Unix filesystem.
        assert_eq!(render("/HOME/LEV/y.rs", PathStyle::Fish, Some("/home/lev")), "/H/L/y.rs");
        // A home of nothing but separators would collapse every absolute path.
        assert_eq!(render("/a/b.rs", PathStyle::Fish, Some("/")), "/a/b.rs");
    }

    /// The distro is the UNC *share*, so it is part of the root and never
    /// abbreviates away.
    #[test]
    fn a_wsl_unc_root_is_never_abbreviated() {
        assert_eq!(
            render(r"\\wsl.localhost\kali-linux\home\lev\x.rs", PathStyle::Fish, None),
            r"\\wsl.localhost\kali-linux\h\l\x.rs"
        );
    }

    /// `split_root` and `strip_home` index by byte, and a slice landing mid
    /// character panics rather than degrading — so a non-ASCII path is not an
    /// edge case, it is a crash waiting for a user with an accent in a
    /// directory name.
    #[test]
    fn multibyte_paths_do_not_panic() {
        for path in ["Ä/Ö/ü.rs", "/日本/語/x.rs", "Ä:foo", "日本語", "/Ä", "C:Ä\\ö"] {
            let _ = render(path, PathStyle::Fish, Some("/日本"));
            let _ = render(path, PathStyle::Zed, Some("Ä"));
            let _ = render(path, PathStyle::Fish, None);
        }
    }
}
```

The whole module plus these tests has been compiled and run standalone with `rustc --edition 2024 --test`. Every table row and every assertion above is a verified expectation, not a predicted one — including the multibyte, Unicode-case-fold and separator-only-home cases.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p alacritree path_style::`

Expected: FAIL to compile — `cannot find function \`render\` in this scope`, `cannot find type \`PathStyle\``, `cannot find function \`split\``.

- [ ] **Step 4: Write the implementation**

Prepend to `alacritree/src/path_style.rs`, above the `mod tests` block:

```rust
//! Abbreviated path rendering for sidebar rows and pane titles.
//!
//! Pure and egui-free so the table below can be unit-tested without a `Ui`,
//! and so the caller decides what counts as `home` — a WSL path's home lives
//! inside the distro and cannot be inferred from the path.

/// How a path is spelled to the user.  `Full` is the identity and the
/// default, so an unmodified config renders exactly what it renders today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathStyle {
    #[default]
    Full,
    /// Every parent segment collapses to its first character, fish-style.
    Fish,
    /// The filename leads, the parent trails it.
    Zed,
}

/// A path cut where it may be abbreviated.  `root` is never abbreviated and
/// never reordered; `parent` keeps its trailing separator, and is empty for a
/// bare name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parts {
    pub root: String,
    pub parent: String,
    pub name: String,
}

pub fn split(path: &str, style: PathStyle, home: Option<&str>) -> Parts {
    let (root, rest, sep) = split_root(path);
    let collapsed = match style {
        PathStyle::Full => None,
        _ => home.and_then(|home| strip_home(path, home, sep)),
    };
    let (root, segments) = match collapsed {
        // `~` replaces the root as well as the leading segments, so a
        // collapsed path carries no root of its own.
        Some(tail) => {
            let mut segments = vec!["~".to_string()];
            segments.extend(segments_of(tail, sep));
            (String::new(), segments)
        },
        None => (root, segments_of(rest, sep)),
    };

    let name = segments.last().cloned().unwrap_or_default();
    let mut parent = String::new();
    for segment in &segments[..segments.len().saturating_sub(1)] {
        parent.push_str(&abbreviate(segment, style));
        parent.push(sep);
    }
    Parts { root, parent, name }
}

pub fn render(path: &str, style: PathStyle, home: Option<&str>) -> String {
    if style == PathStyle::Full {
        return path.to_string();
    }
    let parts = split(path, style, home);
    match style {
        PathStyle::Full => unreachable!("returned above"),
        PathStyle::Fish => format!("{}{}{}", parts.root, parts.parent, parts.name),
        PathStyle::Zed if parts.parent.is_empty() => format!("{}{}", parts.root, parts.name),
        PathStyle::Zed => format!("{} {}{}", parts.name, parts.root, parts.parent),
    }
}

/// The root token, the remainder, and the separator the root implies.
///
/// Matched by prefix and in this order, because `\` and `:` are legal inside
/// a Unix filename: scanning for separator characters would misread
/// `dir/name\part.txt` as a Windows path.
fn split_root(path: &str) -> (String, &str, char) {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return match segments_len(rest, 2) {
            Some(len) => (path[..r"\\?\UNC\".len() + len].to_string(), &rest[len..], '\\'),
            None => (path.to_string(), "", '\\'),
        };
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        if let Some(len) = drive_root_len(rest) {
            return (path[..r"\\?\".len() + len].to_string(), &rest[len..], '\\');
        }
    }
    if let Some(rest) = path.strip_prefix(r"\\") {
        return match segments_len(rest, 2) {
            Some(len) => (path[..2 + len].to_string(), &rest[len..], '\\'),
            None => (path.to_string(), "", '\\'),
        };
    }
    if let Some(len) = drive_root_len(path) {
        // Normalize `C:/` to `C:\`: a drive path is re-joined with backslashes.
        return (format!("{}:\\", &path[..1]), &path[len..], '\\');
    }
    if is_drive_relative(path) {
        // `C:foo` is relative to the drive's current directory, so no
        // separator may be inserted after the root.
        return (path[..2].to_string(), &path[2..], '\\');
    }
    if let Some(rest) = path.strip_prefix('/') {
        return ("/".to_string(), rest, '/');
    }
    (String::new(), path, '/')
}

/// Byte length of `<letter>:<sep>`, or `None` when `s` does not start with one.
fn drive_root_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let is_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    is_drive.then_some(3)
}

fn is_drive_relative(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Byte offset past `n` `\`-separated segments *and* the separator closing the
/// last one.  `None` when the string runs out first, which means the whole
/// input is root and there is nothing beneath it.
fn segments_len(s: &str, n: usize) -> Option<usize> {
    let mut idx = 0;
    for _ in 0..n {
        idx += s[idx..].find('\\')? + 1;
    }
    Some(idx)
}

/// Windows paths are split on either separator; POSIX paths only on `/`, so a
/// Unix filename containing `\` survives intact.
fn segments_of(rest: &str, sep: char) -> Vec<String> {
    let split_on = |c: char| c == sep || (sep == '\\' && c == '/');
    rest.split(split_on).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The part of `path` beneath `home`, or `None` when it is not beneath it.
/// An exact match yields `""` — the path *is* home.
fn strip_home<'a>(path: &'a str, home: &str, sep: char) -> Option<&'a str> {
    // A home that is nothing but separators would collapse every absolute
    // path to `~`, which reads as a bug rather than as a shortening.  `/` is
    // no one's home directory; root's is `/root`.
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return None;
    }
    // A Windows filesystem is case-insensitive and accepts both separators;
    // a Unix one is neither, and `/Home` is a different directory.
    let same = |a: &str, b: &str| {
        if sep == '\\' { normalize_windows(a) == normalize_windows(b) } else { a == b }
    };
    if same(path, home) {
        return Some("");
    }
    let (head, tail) = path.split_at_checked(home.len())?;
    if !same(head, home) {
        return None;
    }
    let first = tail.chars().next()?;
    (first == sep || (sep == '\\' && first == '/')).then(|| &tail[first.len_utf8()..])
}

/// Full Unicode lowering, not `to_ascii_lowercase`: NTFS folds case beyond
/// ASCII, so a home under `C:\Üsers` must still match `c:\üsers`.
fn normalize_windows(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

/// Fish keeps enough of a segment to still recognize it: the first character,
/// plus one more when that character is a dot, so `.config` reads `.c`.
fn abbreviate(segment: &str, style: PathStyle) -> String {
    if style != PathStyle::Fish || segment == "~" {
        return segment.to_string();
    }
    let mut chars = segment.chars();
    let mut out = String::new();
    if let Some(first) = chars.next() {
        out.push(first);
        if first == '.' {
            out.extend(chars.next());
        }
    }
    out
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree path_style::`

Expected: PASS — 9 tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/path_style.rs alacritree/src/main.rs
git commit -m "feat(path-style): add fish and zed path rendering"
```

---

### Task 5: `[ui.path_style]` config

Per-site table only. There is deliberately **no scalar shorthand**: TOML cannot have `path_style = "zed"` and `[ui.path_style.filename]` in one document, because that is the same key used two ways, so a shorthand would be unusable alongside the emphasis in Task 8.

Each key is parsed once in `RawConfig::into_config`; three misspelled keys warn three times, which is correct — they are three separate mistakes.

There is deliberately no `size` field: rows are laid out at `interact_size.y`, so a larger span in one row would make section heights jitter.

Target config:

```toml
[ui.path_style]              # any omitted key is "full"
diff_title = "zed"           # the `diff: <path>` pane title
git_rows   = "fish"          # Staged / Unstaged / Changes-vs file rows
git_header = "full"          # the workspace path atop the git panel

[ui.path_style.filename]     # zed style only
color  = "#e6e6e6"
bold   = true

[ui.path_style.parent]
color  = "#6b6b6b"
```

**Files:**
- Modify: `alacritree/src/config.rs` — add `PathStyleConfig`/`TextEmphasis` near `UiTheme` (`:380-442`), a field on `UiTheme`, `parse_path_style` beside `parse_scrollbar` (`:260`), `RawPathStyle`/`RawTextEmphasis` beside `RawUi` (`:1061`), a field on `RawUi`, and the mapping in `into_config` (`:1207-1234`)
- Test: `alacritree/src/config.rs` (the `mod tests` block starting at `:1462`)

**Interfaces:**
- Consumes: `crate::path_style::PathStyle` (Task 4), `RgbStr` (`config.rs:1118`), `rgb_to_color32`, the `ui_from_toml` test helper (`config.rs:1466`).
- Produces:
  - `pub struct TextEmphasis { pub color: Option<Color32>, pub bold: bool, pub italic: bool }` — `Copy`, `Default`, `PartialEq`.
  - `pub struct PathStyleConfig { pub diff_title: PathStyle, pub git_rows: PathStyle, pub git_header: PathStyle, pub filename: TextEmphasis, pub parent: TextEmphasis }` — `Copy`, `Default`.
  - `UiTheme.path_style: PathStyleConfig`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `alacritree/src/config.rs`, after `delta_path_parses_and_blank_is_none`:

```rust
#[test]
fn path_style_defaults_to_full_everywhere() {
    let ui = ui_from_toml("");
    assert_eq!(ui.path_style.diff_title, PathStyle::Full);
    assert_eq!(ui.path_style.git_rows, PathStyle::Full);
    assert_eq!(ui.path_style.git_header, PathStyle::Full);
    assert_eq!(ui.path_style.filename, TextEmphasis::default());
    assert_eq!(ui.path_style.parent, TextEmphasis::default());
}

#[test]
fn path_style_parses_per_site_and_falls_back_on_nonsense() {
    let ui = ui_from_toml("[ui.path_style]\ndiff_title = \"zed\"\ngit_rows = \"fish\"");
    assert_eq!(ui.path_style.diff_title, PathStyle::Zed);
    assert_eq!(ui.path_style.git_rows, PathStyle::Fish);
    // An omitted key is not an error, it is "full".
    assert_eq!(ui.path_style.git_header, PathStyle::Full);

    let ui = ui_from_toml("[ui.path_style]\ngit_header = \"zeb\"");
    assert_eq!(ui.path_style.git_header, PathStyle::Full);
}

#[test]
fn path_style_emphasis_parses_and_a_blank_color_is_an_error() {
    let ui = ui_from_toml(
        "[ui.path_style.filename]\ncolor = \"#e6e6e6\"\nbold = true\n\
         [ui.path_style.parent]\nitalic = true\n",
    );
    assert_eq!(ui.path_style.filename.color, Some(Color32::from_rgb(0xe6, 0xe6, 0xe6)));
    assert!(ui.path_style.filename.bold);
    assert!(!ui.path_style.filename.italic);
    assert_eq!(ui.path_style.parent.color, None);
    assert!(ui.path_style.parent.italic);

    // `RgbStr` rejects a blank string and a raw-schema error discards the
    // whole merged config, so an empty color is a mistake to fix, not a way
    // to say "absent" — omit the key instead.
    let value: toml::Value =
        toml::from_str("[ui.path_style.filename]\ncolor = \"\"").expect("valid toml");
    let raw: Result<RawConfig, _> = value.try_into();
    assert!(raw.is_err(), "a blank color must not parse as absent");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree config::tests::path_style`

Expected: FAIL to compile — `no field \`path_style\` on type \`UiTheme\``, `cannot find type \`TextEmphasis\``, `cannot find type \`PathStyle\` in this scope`.

- [ ] **Step 3: Add the resolved types**

In `alacritree/src/config.rs`, immediately above `pub struct UiTheme` (`:379`):

```rust
/// How one text span is emphasized.  `color: None` inherits whatever color the
/// site normally paints, so an emphasis that sets only `bold` still tracks the
/// theme.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextEmphasis {
    pub color: Option<Color32>,
    pub bold: bool,
    pub italic: bool,
}

/// `[ui.path_style]`: how each site spells a path, plus the two emphases the
/// `Zed` style paints with.  Every field defaults to today's rendering.
#[derive(Debug, Clone, Copy, Default)]
pub struct PathStyleConfig {
    /// The `diff: <path>` pane title.
    pub diff_title: PathStyle,
    /// Staged / Unstaged / Changes-vs file rows in the git panel.
    pub git_rows: PathStyle,
    /// The workspace path atop the git panel.
    pub git_header: PathStyle,
    /// `Zed` style only, and only at the two egui sites.
    pub filename: TextEmphasis,
    pub parent: TextEmphasis,
}
```

Add the field to `UiTheme` after `project_name` (`:418`):

```rust
    /// `[ui.path_style]`: per-site path abbreviation.  All `Full` by default,
    /// which renders every path byte-for-byte as it does today.
    pub path_style: PathStyleConfig,
```

Add to `impl Default for UiTheme` after `project_name: None,` (`:439`):

```rust
            path_style: PathStyleConfig::default(),
```

Add the import at the top of `config.rs`, alongside the other `use crate::…` lines:

```rust
use crate::path_style::PathStyle;
```

- [ ] **Step 4: Add the parser**

In `alacritree/src/config.rs`, immediately after `parse_scrollbar` (which ends at `:270`):

```rust
fn parse_path_style(raw: Option<&str>) -> PathStyle {
    match raw {
        None => PathStyle::default(),
        Some("full") => PathStyle::Full,
        Some("fish") => PathStyle::Fish,
        Some("zed") => PathStyle::Zed,
        Some(other) => {
            log::warn!("unknown ui.path_style value {other:?}, using \"full\"");
            PathStyle::default()
        },
    }
}
```

- [ ] **Step 5: Add the raw schema**

In `alacritree/src/config.rs`, immediately after `struct RawUi` (which ends at `:1091`):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPathStyle {
    /// "full" (default) | "fish" | "zed", per site.
    diff_title: Option<String>,
    git_rows: Option<String>,
    git_header: Option<String>,
    filename: RawTextEmphasis,
    parent: RawTextEmphasis,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTextEmphasis {
    color: Option<RgbStr>,
    bold: Option<bool>,
    italic: Option<bool>,
}
```

Add the field to `RawUi` after `sidebar_click_focus` (`:1090`):

```rust
    path_style: RawPathStyle,
```

- [ ] **Step 6: Map raw to resolved**

In `alacritree/src/config.rs`, add to the `UiTheme { … }` literal in `into_config`, after `project_name:` (`:1233`):

```rust
            path_style: PathStyleConfig {
                diff_title: parse_path_style(self.ui.path_style.diff_title.as_deref()),
                git_rows: parse_path_style(self.ui.path_style.git_rows.as_deref()),
                git_header: parse_path_style(self.ui.path_style.git_header.as_deref()),
                filename: text_emphasis(&self.ui.path_style.filename),
                parent: text_emphasis(&self.ui.path_style.parent),
            },
```

And add the helper next to `parse_path_style`:

```rust
fn text_emphasis(raw: &RawTextEmphasis) -> TextEmphasis {
    TextEmphasis {
        color: raw.color.map(|v| rgb_to_color32(v.0)),
        bold: raw.bold.unwrap_or(false),
        italic: raw.italic.unwrap_or(false),
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p alacritree config::`

Expected: PASS — the three new tests plus every existing config test.

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): add per-site ui.path_style"
```

---

### Task 6: Carry the distro `$HOME` on `Project`

Home cannot be inferred from a path: taking the first two segments of `/home/…` is a guess that is wrong for `/home/shared/repo` and misses `/root`. And it cannot be queried at paint time — `wsl_helper::try_run` (`wsl_helper.rs:534`) blocks, and `wsl.rs:305` explicitly warns against calling it on the UI thread.

`DISCOVER_SCRIPT` already runs one `sep()`-delimited batch per project inside the distro, on the background discovery thread. One more section costs no extra round trip and needs no new cache.

**Discovering `home` is not enough — it must survive a refresh.** A WSL project added through the folder picker starts life as a `placeholder` (`app.rs:573`) and is filled in later by background discovery, and *both* refresh paths copy a hand-written list of fields:

```rust
                    project.worktrees = fresh.worktrees;
                    project.default_branch = fresh.default_branch;
```

— `poll_project_refreshes` (`app.rs:770-773`) and `Project::refresh` (`projects.rs:181-184`). A `home` set only in `discover_wsl` would be discarded by the dominant startup path and `~` would never appear. Two independent field lists are what makes that failure possible, so this task replaces both with one `adopt_discovered` method rather than adding a third line to each.

**Files:**
- Modify: `alacritree/src/projects.rs:11-23` (`Project`), `:56-73` (`placeholder`), `:119-128` (`discover_wsl`), `:160-170` (`from_repo`), `:181-184` (`refresh`), `:279-295` (`DISCOVER_SCRIPT` and its section comment)
- Modify: `alacritree/src/app.rs:768-774` (`poll_project_refreshes`)
- Modify: `alacritree/src/app.rs:6866` (test fixture `project_with`), `alacritree/src/sidebar_nav.rs:196` (test fixture), `alacritree/src/row_label.rs:122` (test fixture)
- Test: `alacritree/src/projects.rs` (the `#[cfg(test)] mod tests` block at `:364`)

**Interfaces:**
- Consumes: `wsl::run_batch` / `wsl::split_sections` (`wsl.rs:306`, `:367`), `Project::placeholder` (`projects.rs:56`).
- Produces: `Project.home: Option<String>` — the distro's `$HOME` for a WSL project, `None` for a native project (whose home comes from `home::home_dir()`) and `None` for any project that degraded to `placeholder`. Also `pub fn adopt_discovered(&mut self, fresh: Project)` on `Project`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `alacritree/src/projects.rs` (starts at `:364`):

```rust
/// `$HOME` rides the existing discovery batch, so its section index must not
/// drift from the script.  A blank section means the distro reported nothing
/// and there is no home to collapse to.
#[test]
fn the_discover_script_ends_with_the_home_section() {
    assert!(
        DISCOVER_SCRIPT.trim_end().ends_with(r#"printf '%s' "$HOME""#),
        "the $HOME section must be last so its index stays 5"
    );
    assert_eq!(DISCOVER_SCRIPT.matches("\nsep\n").count(), 5, "one sep per section boundary");
}

/// Discovery is adopted through one method by both refresh paths.  Two paths
/// each listing fields by hand is what lets a newly discovered field land in
/// one and be dropped by the other — and the folder-picker path, which starts
/// from a placeholder, is the one that matters most.
#[test]
fn adopting_a_discovery_keeps_user_state_and_takes_the_rest() {
    let mut existing = Project::placeholder(PathBuf::from("/repo"));
    existing.label = Some("Work".to_string());
    existing.expanded = false;

    let mut fresh = Project::placeholder(PathBuf::from("/repo"));
    fresh.default_branch = Some("main".to_string());
    fresh.home = Some("/home/lev".to_string());

    existing.adopt_discovered(fresh);

    assert_eq!(existing.home.as_deref(), Some("/home/lev"));
    assert_eq!(existing.default_branch.as_deref(), Some("main"));
    assert_eq!(existing.label.as_deref(), Some("Work"), "a rename is user state");
    assert!(!existing.expanded, "the expand toggle is user state");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alacritree projects::tests::the_discover_script_ends_with_the_home_section`

Expected: FAIL with `the $HOME section must be last so its index stays 5`.

Run: `cargo test -p alacritree projects::tests::adopting_a_discovery`

Expected: FAIL to compile — `no method named \`adopt_discovered\``, `no field \`home\``.

- [ ] **Step 3: Add the script section**

In `alacritree/src/projects.rs`, replace the section comment and script (`:279-295`):

```rust
/// Sections: 0 repo-or-not, 1 `worktree list --porcelain -z`,
/// 2 origin/HEAD symref, 3 which common default-branch names exist,
/// 4 `init.defaultBranch` only if it names an existing branch,
/// 5 the distro's `$HOME`.
const DISCOVER_SCRIPT: &str = r#"
p="$1"
sep() { printf '\n@@ALACRITREE@@\n'; }
git -C "$p" rev-parse --is-inside-work-tree >/dev/null 2>&1 && printf yes || printf no
sep
git -C "$p" worktree list --porcelain -z 2>/dev/null
sep
git -C "$p" symbolic-ref refs/remotes/origin/HEAD 2>/dev/null
sep
git -C "$p" for-each-ref --format='%(refname:short)' refs/heads/main refs/heads/master refs/heads/trunk refs/heads/develop 2>/dev/null
sep
cfg=$(git -C "$p" config init.defaultBranch 2>/dev/null)
if [ -n "$cfg" ] && git -C "$p" rev-parse --verify --quiet "refs/heads/$cfg" >/dev/null 2>&1; then printf '%s' "$cfg"; fi
sep
printf '%s' "$HOME"
"#;
```

- [ ] **Step 4: Add the field and populate it**

In `alacritree/src/projects.rs`, add to `struct Project` after `shell_override` (`:22`):

```rust
    /// The distro's own `$HOME` for a WSL project, so a path can collapse to
    /// `~` without guessing the prefix from the path itself.  `None` for a
    /// native project, whose home comes from `home::home_dir()`.
    pub home: Option<String>,
```

In `placeholder` (`:58-72`), add to the literal after `shell_override: None,`:

```rust
            home: None,
```

In `discover_wsl` (`:119-127`), add to the literal after `shell_override: None,`:

```rust
            home: Some(text(5)).filter(|h| !h.is_empty()),
```

In `from_repo` (its `Project { … }` literal near `:164`), add after `shell_override: None,`:

```rust
            home: None,
```

Add `home: None,` to the three test fixtures: `alacritree/src/app.rs:6866`, `alacritree/src/sidebar_nav.rs:196`, `alacritree/src/row_label.rs:122`.

- [ ] **Step 5: Make both refresh paths adopt through one method**

In `alacritree/src/projects.rs`, replace `refresh` (`:181-184`):

```rust
    pub fn refresh(&mut self) {
        self.adopt_discovered(Project::discover(self.root.clone()));
    }

    /// Take everything discovery owns from a freshly discovered copy, leaving
    /// user state — `label`, `expanded`, `shell_override` — in place.  One
    /// list, so a field cannot be adopted by the synchronous refresh and
    /// dropped by the background one.
    pub fn adopt_discovered(&mut self, fresh: Project) {
        self.worktrees = fresh.worktrees;
        self.default_branch = fresh.default_branch;
        self.home = fresh.home;
    }
```

In `alacritree/src/app.rs`, replace the body of the `Ok(fresh)` arm in `poll_project_refreshes` (`:770-773`):

```rust
            Ok(fresh) => {
                if let Some(project) = projects.iter_mut().find(|p| p.root == *root) {
                    project.adopt_discovered(fresh);
                }
                false
            },
```

The doc comment above `poll_project_refreshes` (`app.rs:774`) says user state survives refreshes "mirrors `Project::refresh`" — it no longer mirrors it, it calls it. Update that comment accordingly.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree`

Expected: PASS (modulo the known session-timing flake).

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/projects.rs alacritree/src/app.rs alacritree/src/sidebar_nav.rs alacritree/src/row_label.rs
git commit -m "feat(projects): discover the distro's home directory"
```

---

### Task 7: Wire `path_style` into the three sites

`git_rows` and `diff_title` operate on repo-relative paths (`FileChange.path`, `DiffStat.path`, `DiffRequest.file` are all `git status --porcelain`-shaped), so they get no `home`. Only `git_header` renders an absolute path, and its ordering is fixed: `wsl::display_path` first, then `path_style`, so the style operates on `/home/lev/…` rather than on the UNC form.

`Theme` is `Copy` and already reaches `file_row` and `branch_diff_row`; carrying `PathStyleConfig` on it avoids threading `&Config` through free functions.

`worktree_name` / `project_name` templates deliberately get Task 3's conversion but not `path_style` — they already have their own substitution language.

**Files:**
- Modify: `alacritree/src/app.rs:54-85` (`Theme`), `:113-137` (`Theme::from_config`), `:3096` (compute the header's home), `:3178-3185` (git header), `:3426` (`open_diff` title), `:3959-3965` (`file_row`), `:4027-4033` (`branch_diff_row`), plus a new `workspace_home` method
- Test: `alacritree/src/app.rs` (its `mod tests` block)

**Interfaces:**
- Consumes: `path_style::render` (Task 4), `PathStyleConfig` (Task 5), `Project.home` (Task 6), `wsl::display_path` (Task 2).
- Produces: `Theme.path_style: PathStyleConfig`; `fn workspace_home(&self, path: &Path) -> Option<String>` on `AlacritreeApp`.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `alacritree/src/app.rs`:

```rust
/// The row painters are free functions that only ever see a `Theme`, so the
/// configured style has to survive the trip through it.
#[test]
fn the_theme_carries_the_configured_path_style() {
    let mut config = Config::default();
    config.ui.path_style.git_rows = PathStyle::Fish;
    config.ui.path_style.filename.bold = true;

    let theme = Theme::from_config(&config);
    assert_eq!(theme.path_style.git_rows, PathStyle::Fish);
    assert_eq!(theme.path_style.git_header, PathStyle::Full);
    assert!(theme.path_style.filename.bold);
}

/// The header is the one site whose path is absolute, so it is the one that
/// must convert before it abbreviates: fish-abbreviating the UNC spelling
/// would produce `\\w\k\h\l\monorepo` instead of `~/G/monorepo`.
#[cfg(windows)]
#[test]
fn the_git_header_converts_before_it_abbreviates() {
    let unc = std::path::Path::new(r"\\wsl.localhost\kali-linux\home\lev\Git\monorepo");
    let shown = crate::path_style::render(
        &crate::wsl::display_path(unc),
        crate::path_style::PathStyle::Fish,
        Some("/home/lev"),
    );
    assert_eq!(shown, "~/G/monorepo");
}
```

- [ ] **Step 2: Run the tests to verify the first fails**

Run:

```bash
cargo test -p alacritree the_theme_carries_the_configured_path_style
cargo test -p alacritree the_git_header_converts_before_it_abbreviates
```


Expected: `the_theme_carries_the_configured_path_style` FAILs to compile with `no field \`path_style\` on type \`Theme\`` — that is this task's RED.

`the_git_header_converts_before_it_abbreviates` passes immediately: it exercises Tasks 2 and 4, which already landed, and exists to pin the *ordering contract* the wiring must honor. Record it as such rather than manufacturing a false RED; the behavioral proof for the wiring itself is the GUI check in Step 8.

- [ ] **Step 3: Carry `PathStyleConfig` on `Theme`**

In `alacritree/src/app.rs`, add to `struct Theme` after `focus_outline` (`:84`):

```rust
    /// Per-site path abbreviation, so free-standing row painters can spell a
    /// path without taking a `&Config`.
    path_style: PathStyleConfig,
```

In `Theme::from_config`, add to the literal after the `focus_outline` block (`:136`):

```rust
            path_style: config.ui.path_style,
```

Extend the `use crate::config::…` line in `app.rs` to import `PathStyleConfig`, and add:

```rust
use crate::path_style::{self, PathStyle};
```

`TextEmphasis` is *not* imported here — nothing in this task names it, and an unused import is a warning. Task 8 adds it.

- [ ] **Step 4: Add `workspace_home`**

In `alacritree/src/app.rs`, immediately after `active_session_path` (`:3007-3010`):

```rust
    /// The home directory a workspace path should collapse to.  A WSL path's
    /// home lives inside the distro and is only known through discovery, so a
    /// project that has not finished discovering yet simply gets no `~`.
    fn workspace_home(&self, path: &Path) -> Option<String> {
        match wsl::classify(path) {
            wsl::Location::Wsl { .. } => self
                .projects
                .iter()
                .find(|p| p.worktrees.iter().any(|w| w.path == path))
                .and_then(|p| p.home.clone()),
            wsl::Location::Windows(_) => home::home_dir().map(|h| h.display().to_string()),
        }
    }
```

- [ ] **Step 5: Apply the style at the git header**

In `alacritree/src/app.rs`, after the `let path = match self.active_session_path() { … };` block ends (`:3096`):

```rust
                let workspace_home = self.workspace_home(&path);
```

Then replace the header label (`:3178-3185`, as left by Task 3):

```rust
                    ui.add(
                        egui::Label::new(
                            RichText::new(path_style::render(
                                &wsl::display_path(&path),
                                theme.path_style.git_header,
                                workspace_home.as_deref(),
                            ))
                            .color(theme.text_muted)
                            .small(),
                        )
                        .truncate(),
                    );
```

- [ ] **Step 6: Apply the style at the diff title and the two row painters**

`alacritree/src/app.rs:3426`:

```rust
        let title = format!(
            "diff: {}",
            path_style::render(&req.file, self.config.ui.path_style.diff_title, None)
        );
```

The `.monospace()` on these two labels is inert — `.small()` overwrites the same `text_style` field, and `app.rs:514` resolves `TextStyle::Small` to a proportional font. Keep it verbatim anyway: this task preserves today's rendering exactly, and Task 8 replaces both labels. Do not "fix" it here.

`alacritree/src/app.rs:3959-3965` (inside `file_row`):

```rust
                ui.add(
                    egui::Label::new(
                        RichText::new(path_style::render(
                            &change.path,
                            theme.path_style.git_rows,
                            None,
                        ))
                        .color(path_color)
                        .monospace()
                        .small(),
                    )
                    .truncate()
                    .selectable(false),
                );
```

`alacritree/src/app.rs:4027-4033` (inside `branch_diff_row`):

```rust
                            ui.add(
                                egui::Label::new(
                                    RichText::new(path_style::render(
                                        &stat.path,
                                        theme.path_style.git_rows,
                                        None,
                                    ))
                                    .color(path_color)
                                    .monospace()
                                    .small(),
                                )
                                .truncate()
                                .selectable(false),
                            );
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p alacritree && cargo clippy -p alacritree`

Expected: PASS, no new clippy warnings.

- [ ] **Step 8: Verify in the GUI lab**

Build and run against the isolated lab config:

```bash
cargo build -p alacritree
```

With no `[ui.path_style]` in the config, confirm the git panel header, git file rows and a diff pane title read exactly as they did before this task — that is the inert-by-default guarantee. Then add:

```toml
[ui.path_style]
git_rows = "fish"
git_header = "fish"
```

and confirm the rows abbreviate (`s/g/status.rs`), the header collapses to `~/…`, and typing `src/git` in the git filter still matches the abbreviated row (formatting is display-only).

- [ ] **Step 9: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): render paths through the configured style"
```

---

### Task 8: Zed filename emphasis

The row renders as a single `Label` built from a `LayoutJob` with two differently-formatted sections in `filename parent/` order. Two labels would introduce `item_spacing.x` between them (so the gap would not be the single space the style specifies), let an untruncated filename overflow `row_with_trailing`, which deliberately manages remaining width (`app.rs:4187`), and split one response into two, complicating click fall-through and tooltips.

The filename is *prioritized*, not guaranteed: epaint lays out one linear glyph stream and truncates the suffix (`epaint-0.31.1/src/text/text_layout.rs:197-338`), so putting the filename first means the parent is eaten before it — but a row narrower than the filename plus the overflow marker still truncates the filename. Two labels would be strictly worse; neither arrangement makes it untouchable.

`Full` and `Fish` stay exactly as they are — one plain truncating label, no emphasis, no `LayoutJob`.

Two details the implementation must not miss:
- `WidgetText::into_galley_impl` overwrites `job.wrap` from the requested wrap mode (`egui-0.31.1/src/widget_text.rs:703-706`), so `.truncate()` works on a `LayoutJob` and elision — and egui's built-in elision tooltip — still applies.
- A hand-built `LayoutJob` does **not** inherit `ui.text_valign()` the way `RichText` does (`widget_text.rs:699`), so `TextFormat::valign` must be set explicitly or the path text will sit off-center against the `A`/`M` glyph beside it.

Real bold and italic faces are registered by `fonts.rs` (`BOLD_FAMILY`, `ITALIC_FAMILY`, `BOLD_ITALIC_FAMILY` at `fonts.rs:30-32`), so `bold = true` renders a genuine bold face rather than a color swap. Only the *terminal* font registers those variants; `FontFamily::Proportional` is headed by the UI font when `[ui.font]` names one (`fonts.rs:646-650`) and by the terminal font otherwise (`fonts.rs:730`), and neither has a bold sibling. An emphasized span at a proportional site therefore renders in the terminal's bold face — the weight is kept, the family shifts. This only reaches anyone who opts into `git_header = "zed"` *and* an emphasis, since the header defaults to `Full`.

**The git rows are not monospace today, despite appearances.** `RichText::monospace()` and `RichText::small()` both write the same `text_style` field (`widget_text.rs:196`, `:248`), so in `.monospace().small()` the second call wins, and `app.rs:514` maps `TextStyle::Small` to `FontId::proportional(normal_px)`. The `.monospace()` on the path labels at `app.rs:3961` and `:4029` is inert. Passing `FontFamily::Monospace` here would therefore *change* default output — `RichText::family` is an independent override applied after style resolution (`widget_text.rs:159`), so it would take effect even under `PathStyle::Full`, altering the face, the advance width, and where truncation lands.

So every call passes `FontFamily::Proportional`, which is what all three sites render in now. Whether the rows *should* be monospace is a real question, but it is a separate default-output change and not this plan's to make silently. Note it for later; do not fix it here.

`Session.title` is a plain `String` painted as one label, so `diff_title = "zed"` yields plain `diff: file.txt path/to/` with no emphasis. That is by design and needs no code.

**Files:**
- Modify: `alacritree/src/app.rs` — new `path_label` and `emphasis_family` helpers next to `fill_row` (`:4048`), and three call sites: `:3178-3185` (header), `file_row`, `branch_diff_row`
- Test: `alacritree/src/app.rs` (its `mod tests` block)

**Interfaces:**
- Consumes: `path_style::{split, render, PathStyle, Parts}` (Task 4), `PathStyleConfig` (Task 5), `Theme.path_style` (Task 7), `fonts::{BOLD_FAMILY, ITALIC_FAMILY, BOLD_ITALIC_FAMILY}`. Add `TextEmphasis` to the `use crate::config::…` line in this task — Task 7 deliberately left it out.
- Produces: `fn path_label(ui: &mut egui::Ui, path: &str, base: Color32, theme: &Theme, style: PathStyle, family: egui::FontFamily, home: Option<&str>)` and `fn emphasis_family(e: &TextEmphasis, base: &egui::FontFamily) -> egui::FontFamily`, both private to `app.rs`.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `alacritree/src/app.rs`:

```rust
/// Every emphasis combination must resolve to a registered face; falling back
/// to the base family for, say, bold-italic would silently drop the weight.
/// An unemphasized span keeps whatever family the site already paints in.
#[test]
fn emphasis_resolves_to_the_registered_faces() {
    let plain = TextEmphasis::default();
    let bold = TextEmphasis { bold: true, ..Default::default() };
    let italic = TextEmphasis { italic: true, ..Default::default() };
    let both = TextEmphasis { bold: true, italic: true, ..Default::default() };

    for base in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
        assert_eq!(emphasis_family(&plain, &base), base);
        assert_eq!(
            emphasis_family(&bold, &base),
            egui::FontFamily::Name(crate::fonts::BOLD_FAMILY.into())
        );
        assert_eq!(
            emphasis_family(&italic, &base),
            egui::FontFamily::Name(crate::fonts::ITALIC_FAMILY.into())
        );
        assert_eq!(
            emphasis_family(&both, &base),
            egui::FontFamily::Name(crate::fonts::BOLD_ITALIC_FAMILY.into())
        );
    }
}

/// The job's two spans must reassemble into exactly what `render` produces,
/// so the emphasis only changes how the text looks, never what it says.
#[test]
fn the_zed_job_spells_the_same_text_as_render() {
    for (path, home) in [
        ("path/to/file.txt", None),
        ("/a/b/c.txt", None),
        ("f.txt", None),
        ("/f.txt", None),
        ("/home/lev/Git/x/y.rs", Some("/home/lev")),
    ] {
        let parts = crate::path_style::split(path, PathStyle::Zed, home);
        let spans = if parts.parent.is_empty() {
            format!("{}{}", parts.root, parts.name)
        } else {
            format!("{} {}{}", parts.name, parts.root, parts.parent)
        };
        assert_eq!(spans, crate::path_style::render(path, PathStyle::Zed, home), "{path:?}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p alacritree emphasis_resolves_to_the_registered_faces
cargo test -p alacritree the_zed_job_spells_the_same_text_as_render
```


Expected: FAIL to compile — `cannot find function \`emphasis_family\` in this scope`. `the_zed_job_spells_the_same_text_as_render` exercises Task 4 and will pass once the module compiles; it exists to pin the span decomposition the painter relies on.

- [ ] **Step 3: Add the helpers**

In `alacritree/src/app.rs`, immediately above `fn fill_row` (`:4048`):

```rust
/// Bold and italic are real faces rather than a colour swap, but only the
/// terminal font registers them — an emphasized span at a proportional site
/// keeps the weight and shifts family rather than losing the weight.
fn emphasis_family(e: &TextEmphasis, base: &egui::FontFamily) -> egui::FontFamily {
    match (e.bold, e.italic) {
        (true, true) => egui::FontFamily::Name(crate::fonts::BOLD_ITALIC_FAMILY.into()),
        (true, false) => egui::FontFamily::Name(crate::fonts::BOLD_FAMILY.into()),
        (false, true) => egui::FontFamily::Name(crate::fonts::ITALIC_FAMILY.into()),
        (false, false) => base.clone(),
    }
}

/// Paint a path as one truncating label.
///
/// `Zed` needs two differently-formatted spans, and one `LayoutJob` is the
/// only way to get them without an `item_spacing` gap between two labels, a
/// second response competing for the row's click, and a filename that can
/// overflow the width `row_with_trailing` is managing.  Putting the filename
/// first only *prioritizes* it: epaint truncates the tail of one linear glyph
/// stream, so a row narrower than the filename still elides it.
fn path_label(
    ui: &mut egui::Ui,
    path: &str,
    base: Color32,
    theme: &Theme,
    style: PathStyle,
    family: egui::FontFamily,
    home: Option<&str>,
) {
    if style != PathStyle::Zed {
        ui.add(
            egui::Label::new(
                RichText::new(path_style::render(path, style, home))
                    .color(base)
                    .family(family)
                    .small(),
            )
            .truncate()
            .selectable(false),
        );
        return;
    }

    let size = egui::TextStyle::Small.resolve(ui.style()).size;
    // A hand-built job does not inherit the ui's text valign the way RichText
    // does, so it must be carried across or the path sits off-centre against
    // the change glyph beside it.
    let valign = ui.text_valign();
    let parts = path_style::split(path, style, home);
    let mut job = egui::text::LayoutJob::default();
    let mut push = |text: String, e: &TextEmphasis| {
        if text.is_empty() {
            return;
        }
        job.append(&text, 0.0, egui::TextFormat {
            font_id: egui::FontId::new(size, emphasis_family(e, &family)),
            color: e.color.unwrap_or(base),
            valign,
            ..Default::default()
        });
    };
    if parts.parent.is_empty() {
        push(format!("{}{}", parts.root, parts.name), &theme.path_style.filename);
    } else {
        push(parts.name.clone(), &theme.path_style.filename);
        push(format!(" {}{}", parts.root, parts.parent), &theme.path_style.parent);
    }
    ui.add(egui::Label::new(job).truncate().selectable(false));
}
```

- [ ] **Step 4: Route the three sites through it**

Every call passes `Proportional`, which is what all three sites resolve to today (see the note above — the rows' `.monospace()` is overwritten by `.small()`). No site changes face.

Replace the `file_row` path label added in Task 7:

```rust
                path_label(
                    ui,
                    &change.path,
                    path_color,
                    theme,
                    theme.path_style.git_rows,
                    egui::FontFamily::Proportional,
                    None,
                );
```

Replace the `branch_diff_row` path label added in Task 7:

```rust
                            path_label(
                                ui,
                                &stat.path,
                                path_color,
                                theme,
                                theme.path_style.git_rows,
                                egui::FontFamily::Proportional,
                                None,
                            );
```

Replace the git header label added in Task 7:

```rust
                    path_label(
                        ui,
                        &wsl::display_path(&path),
                        theme.text_muted,
                        &theme,
                        theme.path_style.git_header,
                        egui::FontFamily::Proportional,
                        workspace_home.as_deref(),
                    );
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alacritree && cargo clippy -p alacritree`

Expected: PASS.

- [ ] **Step 6: Verify in the GUI lab**

With no `[ui.path_style]`, confirm the git rows and header are pixel-identical to Task 7's baseline — each site keeps its own font family, so nothing should move. Then add:

```toml
[ui.path_style]
git_rows = "zed"

[ui.path_style.filename]
color = "#e6e6e6"
bold = true

[ui.path_style.parent]
color = "#6b6b6b"
```

Confirm each git row reads `status.rs src/git/` with the filename bold and light and the parent dim, that exactly one space separates them, that clicking anywhere on the row still opens the diff, and that narrowing the panel elides the parent before the filename.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): emphasize the filename in zed style"
```

---

### Task 9: Diagnose the missing hover tooltips

**This task writes no feature code until a measurement is in hand.** egui 0.31 already adds the full text as a tooltip whenever a galley elided (`egui-0.31.1/src/widgets/label.rs:245-259`):

```rust
if galley.elided {
    // Show the full (non-elided) text on hover:
    response = response.on_hover_text(galley.text());
}
```

So every `.truncate()` label in the sidebar is *supposed* to reveal its full text on hover, gated on actual elision, and rows carry their own tooltips already (prunable worktrees at `app.rs:4842`, PR badges at `app.rs:4812-4835`). The reported problem is that on Windows these do not reliably appear.

Two designs are already ruled out, so do not propose them:
- **Re-deriving `galley.elided` and passing the galley to `Label::new(galley)` does nothing.** `Label::ui` computes the same flag from the same field and installs the tooltip itself (`label.rs:250-259`), and a supplied galley takes the fast path at `label.rs:154-162`, leaving the hover response — the suspect — exactly as it was.
- **Attaching our own tooltip to the row does not replace the label's.** egui exposes no way to suppress the built-in one, so this stacks a second tooltip wherever the label's hover already works.

The leading hypothesis: the path label is `.selectable(false)`, which egui senses as `Sense::hover()` (`label.rs:123-152`), and the row then calls `.interact(Sense::click())` on the enclosing rect *after* the label is registered (`app.rs:3970`). Whether the label's tooltip fires therefore depends on how egui resolves hover between a hover-sense label and a later-registered click-sense row covering the same pixels — the ordering hazard already documented at `app.rs:4848-4851`. Credible, unproven.

**Files:**
- Read only until the measurement lands: `alacritree/src/app.rs:3942-3973` (`file_row`), `:4836-4853` (the prunable-worktree tooltip and z-order comment), `egui-0.31.1/src/widgets/label.rs:112-266`
- Create: no file yet. The fix's files are decided by the measurement.

**Interfaces:**
- Consumes: nothing from earlier tasks. This task is independent and may be done first if the lab is available.
- Produces: a written measurement, then a scoped fix.

- [ ] **Step 1: Invoke the debugging skill**

REQUIRED SUB-SKILL: `superpowers:systematic-debugging`. Do not skip to a fix.

- [ ] **Step 2: Instrument the hover resolution**

In `alacritree/src/app.rs`, temporarily add inside `file_row`, capturing the label's own response instead of discarding it:

```rust
                let label = ui.add(
                    egui::Label::new(
                        RichText::new(&change.path).color(path_color).monospace().small(),
                    )
                    .truncate()
                    .selectable(false),
                );
                if label.hovered() || label.rect.contains(ui.ctx().pointer_hover_pos().unwrap_or_default()) {
                    log::info!(
                        "path label: hovered={} rect={:?} pointer={:?}",
                        label.hovered(),
                        label.rect,
                        ui.ctx().pointer_hover_pos()
                    );
                }
```

and after `.interact(egui::Sense::click())`:

```rust
    if resp.hovered() {
        log::info!("row: hovered=true rect={:?}", resp.rect);
    }
```

This instrumentation is scaffolding and must not be committed.

- [ ] **Step 3: Measure on both platforms**

In the GUI verification lab, with the panel narrowed enough that a path elides, on **Windows** and on the **WSL/kali** build:

1. Hover an elided git file row. Record which response reports `hovered()` — the label's, the row's, or both.
2. Hover an elided worktree row in the left sidebar. Record the same.
3. Record whether `galley.elided` is true at that width (add a `log::info!` on `galley.elided` via `Label::layout_in_ui` if the tooltip's absence makes it ambiguous).

Write the results into `docs/superpowers/specs/2026-07-23-sidebar-path-display-design.md` under §5 before writing any fix.

- [ ] **Step 4: Branch on the answer**

- **The row shadows the label** (row hovered, label not): fix the shadowing — ordering or sense — so the label's own tooltip fires. No custom layout, no second tooltip, no config flag: the behavior already exists and merely becomes reliable, which makes it a bug fix and Linux the target. This is where the evidence currently points.
- **The label is hovered on both platforms but nothing paints on Windows**: the cause is below the widget layer (tooltip placement, viewport, or a `Context` difference). Scope the fix once identified; report the finding before implementing.
- **Anything else**: report the measurement and stop. Do not improvise a fix that the measurement does not support.

A config flag is needed only if the outcome deliberately moves the hover target from the label to the whole row, which is new UX and would default to off.

- [ ] **Step 5: Remove the instrumentation**

```bash
git diff alacritree/src/app.rs
```

Confirm no `log::info!` scaffolding from Step 2 remains before committing anything from this task.

- [ ] **Step 6: Amend this plan with the chosen fix**

The measurement is not the deliverable — the spec requires identical tooltip behavior on Windows, Linux and macOS. Once Step 3's results are in, write the diagnosis and the selected fix into §5 of the spec, then add the fix to this plan as **Task 10** in the normal shape: files, interfaces, failing test, RED, implementation, GREEN, commit. If the fix turns out to be large enough to need its own decomposition, say so and write a separate plan rather than inflating this task.

- [ ] **Step 7: Implement and verify on both platforms**

Implement Task 10. Then re-run the Step 3 measurement on Windows *and* on the WSL/kali build and confirm both now behave identically. The plan is not complete until this passes on both.

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "fix(sidebar): <what the measurement showed>"
```

---

## Manual verification that CI cannot cover

The crate has no PTY harness, so a real ConPTY spawn cannot be asserted in CI. Before calling the plan done, in the GUI verification lab on Windows:

- Open a diff from the git panel and confirm the pane header, tab tooltip, sidebar session row and command-palette session entry all read `diff: <path>` and never `C:\Program Files\Git\cmd\git …`.
- Open a WSL worktree and confirm the git panel header reads `/home/…`, and that the command palette and the base-branch picker agree with it.
- Confirm an unmodified config renders every path exactly as it did before this plan.

## Decisions taken

- **`display_name` on a bare distro root yields `/`.** A project rooted at `\\wsl.localhost\kali-linux` is labelled `/`. Accepted as-is.
- **Every site keeps its current font family.** `path_label` takes the family from the caller: monospace for the git rows, proportional for the header. The single space between the filename and its parent is a space glyph in whatever font the site uses, so it needs no monospace grid; what one `LayoutJob` buys is the absence of `item_spacing.x` between two separate widgets.

## Unresolved questions

1. **Task 9 has no designed fix**, by construction. It will need a decision once the measurement lands, and it may turn out to need its own small plan.
