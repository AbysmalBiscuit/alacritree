# Templated Worktree/Project Names Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Template-driven display names for sidebar worktree and project rows via shell-style `$variable` substitution (`subst` crate), e.g. `worktree_name = "${branch:$name}"`.

**Architecture:** A new `row_label.rs` module owns template rendering, per-row variable maps, warn-once error handling, and fallback; `config.rs` parses two optional `[ui]` template strings; `app.rs` precomputes display strings per frame (before the panel closure, following the existing precompute pattern) and threads them into the row painters. Defaults (`None` templates) skip templating entirely.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), `subst = "0.3"` (its deps `memchr` + `unicode-width` are already in the workspace lockfile).

**Spec:** `docs/superpowers/specs/2026-07-16-templated-names-design.md` (untracked — never commit it).

## Global Constraints

- Branch: `feat/templated-names`, created from `master` — independent of `feat/sidebar-appearance`. (Both plans touch `worktree_row`'s signature; the merge conflict in `integration/all-features` is expected and resolved there, not here.)
- Only touch files under `alacritree/` plus the one dependency line in `alacritree/Cargo.toml` (and `Cargo.lock`). Vendored `alacritty*/` crates are read-only.
- **Never commit anything under `docs/superpowers/`.** Stage files explicitly; never `git add -A` or `git add .`.
- Defaults keep today's behavior exactly: absent template keys → plain names, zero extra work per frame.
- A bad template must degrade to the plain name with one `warn!` per template string — never a blank or missing row.
- Manual `Project.label` (the rename feature) always wins over the project template.
- Commit messages: Conventional Commits, imperative, subject ≤ 72 chars, lowercase after colon; end with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- `cargo fmt` before every commit. Test command: `cargo test -p alacritree`.
- Do NOT run the built GUI, and never kill any running `alacritree.exe`.
- Comments explain *why*, never *what*.
- Search with `rg` / `fd` only (never `grep`/`find`; never `rg -r`).

## File Structure

- `alacritree/Cargo.toml` — add `subst = "0.3"`.
- `alacritree/src/row_label.rs` (new) — `render_label` (pure substitution wrapper) + `LabelTemplates` (templates + warn-once state + variable maps + fallback). Everything unit-tested here.
- `alacritree/src/config.rs` — `UiTheme.worktree_name` / `UiTheme.project_name` (both `Option<String>`).
- `alacritree/src/app.rs` — `row_labels: LabelTemplates` field, per-frame precompute, `worktree_row` display-name parameter, project row label swap.
- `alacritree/src/main.rs` (or wherever modules are declared — check `rg -n "^mod " alacritree/src/main.rs`) — `mod row_label;`.

---

### Task 1: `subst` dependency + `render_label`

**Files:**
- Modify: `alacritree/Cargo.toml`
- Create: `alacritree/src/row_label.rs`
- Modify: `alacritree/src/main.rs` (module declaration)

**Interfaces:**
- Produces: `row_label::render_label(template: &str, vars: &HashMap<String, String>) -> Option<String>` — `None` on any substitution error or when the trimmed result is empty. Task 3 consumes it.

- [ ] **Step 1: Create the branch**

```bash
git checkout -b feat/templated-names master
```

- [ ] **Step 2: Add the dependency**

In `alacritree/Cargo.toml` `[dependencies]`, after `serde_json = "1"`:

```toml
# Shell-style $var / ${var} / ${var:fallback} substitution for the sidebar's
# templated row names.  Chosen over a template engine: the fallback syntax is
# the whole feature, and its deps are already in the workspace lockfile.
subst = { version = "0.3", default-features = false }
```

Run: `cargo check -p alacritree`
Expected: builds; `cargo tree -p alacritree -i subst` shows only `memchr`/`unicode-width` as its deps. If `default-features = false` breaks the build (the crate's features gate optional serde/toml integrations, not core substitution), drop the option and use `subst = "0.3"`.

- [ ] **Step 3: Create the module with failing tests**

Create `alacritree/src/row_label.rs`:

```rust
//! Render templated sidebar row names.
//!
//! Templates come from `[ui] worktree_name` / `[ui] project_name` and use
//! subst's shell-style syntax: `$var`, `${var}`, and `${var:fallback}` (the
//! fallback may itself contain variables, so `${branch:$name}` reads "the
//! branch, or the worktree name when detached").  Any error — parse failure,
//! unknown variable — falls back to the plain name with one warning per
//! template string, so a typo'd config degrades to today's sidebar rather
//! than blank rows.

use std::collections::HashMap;

/// Substitute `vars` into `template`.  `None` on any subst error or when the
/// trimmed result is empty — the caller falls back to the plain name either
/// way, because a blank row label is as useless as a failed one.
pub fn render_label(template: &str, vars: &HashMap<String, String>) -> Option<String> {
    let rendered = subst::substitute(template, vars).ok()?;
    let trimmed = rendered.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn plain_variable_substitutes() {
        assert_eq!(
            render_label("$name", &vars(&[("name", "feature-x")])).as_deref(),
            Some("feature-x")
        );
    }

    #[test]
    fn literal_text_passes_through() {
        assert_eq!(
            render_label("wt: $name", &vars(&[("name", "a")])).as_deref(),
            Some("wt: a")
        );
    }

    #[test]
    fn fallback_used_when_variable_missing() {
        let v = vars(&[("name", "main-wt")]);
        assert_eq!(render_label("${branch:$name}", &v).as_deref(), Some("main-wt"));
    }

    #[test]
    fn fallback_ignored_when_variable_present() {
        let v = vars(&[("name", "main-wt"), ("branch", "feat/x")]);
        assert_eq!(render_label("${branch:$name}", &v).as_deref(), Some("feat/x"));
    }

    #[test]
    fn unknown_variable_is_an_error() {
        assert_eq!(render_label("$nope", &vars(&[("name", "a")])), None);
    }

    #[test]
    fn empty_render_is_an_error() {
        assert_eq!(render_label("  ", &vars(&[])), None);
        assert_eq!(render_label("$name", &vars(&[("name", " ")])), None);
    }
}
```

Declare the module: add `mod row_label;` to `alacritree/src/main.rs` alongside the other `mod` lines (alphabetical order if the list is sorted — check with `rg -n "^mod " alacritree/src/main.rs`).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p alacritree row_label`
Expected: 6 passed. (If `subst::substitute` rejects `HashMap<String, String>` directly, the fix is `subst::substitute(template, &subst::Env)`-style adapter — but `HashMap<String, String>` implements subst's `VariableMap`, so this should compile as written; consult `docs.rs/subst/0.3` only if it doesn't.)

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/Cargo.toml Cargo.lock alacritree/src/row_label.rs alacritree/src/main.rs
git commit -m "feat(sidebar): add subst-backed label template rendering

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `[ui] worktree_name` / `project_name` config keys

**Files:**
- Modify: `alacritree/src/config.rs` (`UiTheme` at ~234, its `Default`, `RawUi` at ~787, `into_config` `UiTheme` literal at ~915, tests)

**Interfaces:**
- Produces: `UiTheme.worktree_name: Option<String>`, `UiTheme.project_name: Option<String>` — `None` (default) means "no templating". Task 3 consumes both.

- [ ] **Step 1: Write the failing tests**

Append to config.rs's `mod tests` (the `ui_from_toml` helper exists):

```rust
#[test]
fn name_templates_default_to_none() {
    let ui = ui_from_toml("");
    assert_eq!(ui.worktree_name, None);
    assert_eq!(ui.project_name, None);
}

#[test]
fn name_templates_parse() {
    let ui = ui_from_toml("[ui]\nworktree_name = \"${branch:$name}\"\nproject_name = \"[$name]\"");
    assert_eq!(ui.worktree_name.as_deref(), Some("${branch:$name}"));
    assert_eq!(ui.project_name.as_deref(), Some("[$name]"));
}

#[test]
fn blank_name_templates_are_dropped() {
    let ui = ui_from_toml("[ui]\nworktree_name = \"  \"");
    assert_eq!(ui.worktree_name, None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree name_templates`
Expected: compile error — fields missing.

- [ ] **Step 3: Implement**

`UiTheme` gains:

```rust
/// `[ui] worktree_name`: template for worktree row labels (subst syntax:
/// `$name`, `$branch`, `$path`, `${var:fallback}`).  `None` keeps the plain
/// worktree name.
pub worktree_name: Option<String>,
/// `[ui] project_name`: template for project row labels (`$name`, `$path`).
/// A manual rename (`Project.label`) always wins over the template.
pub project_name: Option<String>,
```

with `worktree_name: None, project_name: None,` in its `Default`.

`RawUi` gains `worktree_name: Option<String>,` and `project_name: Option<String>,`.

The `UiTheme { ... }` literal in `into_config` gains:

```rust
worktree_name: self.ui.worktree_name.clone().filter(|t| !t.trim().is_empty()),
project_name: self.ui.project_name.clone().filter(|t| !t.trim().is_empty()),
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alacritree name_templates`
Expected: 3 passed. Then the full config suite: `cargo test -p alacritree config` — the literal change must not break existing tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): parse [ui] worktree_name and project_name templates

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `LabelTemplates` — variable maps, precedence, warn-once fallback

**Files:**
- Modify: `alacritree/src/row_label.rs`

**Interfaces:**
- Consumes: `render_label` (Task 1); `projects::{Project, Worktree}` (existing pub structs with pub fields — `Worktree { name, path, branch: Option<String>, is_main, prunable }`, `Project { root, name, label: Option<String>, ... }`).
- Produces:
  - `row_label::LabelTemplates::new(worktree: Option<String>, project: Option<String>) -> LabelTemplates`
  - `LabelTemplates::worktree_label(&mut self, wt: &Worktree) -> String`
  - `LabelTemplates::project_label(&mut self, project: &Project) -> String`

  Task 4 consumes all three. (`&mut self` because a failed template records itself in the warn-once set.)

- [ ] **Step 1: Write the failing tests**

Append to `row_label.rs` tests:

```rust
use crate::projects::{Project, Worktree};
use std::path::PathBuf;

fn wt(name: &str, branch: Option<&str>) -> Worktree {
    Worktree {
        name: name.to_string(),
        path: PathBuf::from("/tmp/wt").join(name),
        branch: branch.map(str::to_string),
        is_main: false,
        prunable: false,
    }
}

fn project(name: &str, label: Option<&str>) -> Project {
    Project {
        root: PathBuf::from("/tmp/projects").join(name),
        name: name.to_string(),
        label: label.map(str::to_string),
        default_branch: None,
        worktrees: Vec::new(),
        expanded: false,
        shell_override: None,
    }
}

#[test]
fn no_template_returns_plain_names() {
    let mut t = LabelTemplates::new(None, None);
    assert_eq!(t.worktree_label(&wt("alpha", Some("feat/a"))), "alpha");
    assert_eq!(t.project_label(&project("proj", None)), "proj");
}

#[test]
fn worktree_template_renders_branch_with_name_fallback() {
    let mut t = LabelTemplates::new(Some("${branch:$name}".into()), None);
    assert_eq!(t.worktree_label(&wt("alpha", Some("feat/a"))), "feat/a");
    assert_eq!(t.worktree_label(&wt("detached", None)), "detached");
}

#[test]
fn project_template_renders_but_manual_label_wins() {
    let mut t = LabelTemplates::new(None, Some("[$name]".into()));
    assert_eq!(t.project_label(&project("proj", None)), "[proj]");
    assert_eq!(t.project_label(&project("proj", Some("Renamed"))), "Renamed");
}

#[test]
fn bad_template_falls_back_to_plain_name() {
    let mut t = LabelTemplates::new(Some("$typo".into()), Some("$typo".into()));
    assert_eq!(t.worktree_label(&wt("alpha", None)), "alpha");
    assert_eq!(t.project_label(&project("proj", None)), "proj");
}

#[test]
fn path_variable_is_available() {
    let mut t = LabelTemplates::new(Some("$path".into()), Some("$path".into()));
    let w = wt("alpha", None);
    assert_eq!(t.worktree_label(&w), w.path.display().to_string());
    let p = project("proj", None);
    assert_eq!(t.project_label(&p), p.root.display().to_string());
}
```

(Adjust the `Project` literal if its field list differs — check with `rg -n "pub struct Project" -A 12 alacritree/src/projects.rs`. As of master it is exactly the seven fields above.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree row_label`
Expected: compile error — `LabelTemplates` not defined.

- [ ] **Step 3: Implement**

Add to `row_label.rs`:

```rust
use std::collections::HashSet;

use crate::projects::{Project, Worktree};

/// The configured templates plus warn-once bookkeeping.  Config strings are
/// static per run, so one warning per template string covers every row that
/// hits the same mistake without flooding the log every frame.
pub struct LabelTemplates {
    worktree: Option<String>,
    project: Option<String>,
    warned: HashSet<String>,
}

impl LabelTemplates {
    pub fn new(worktree: Option<String>, project: Option<String>) -> Self {
        Self { worktree, project, warned: HashSet::new() }
    }

    /// Display name for a worktree row.  Variables: `$name` (worktree name),
    /// `$branch` (absent when detached, so `${branch:...}` falls back),
    /// `$path` (full worktree path).
    pub fn worktree_label(&mut self, wt: &Worktree) -> String {
        let Some(template) = self.worktree.clone() else {
            return wt.name.clone();
        };
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), wt.name.clone());
        if let Some(branch) = &wt.branch {
            vars.insert("branch".to_string(), branch.clone());
        }
        vars.insert("path".to_string(), wt.path.display().to_string());
        self.render_or_fallback(&template, &vars, &wt.name)
    }

    /// Display name for a project row.  A manual rename always wins — the
    /// template only shapes the *default* name.  Variables: `$name`
    /// (directory name), `$path` (full project root).
    pub fn project_label(&mut self, project: &Project) -> String {
        if let Some(label) = &project.label {
            return label.clone();
        }
        let Some(template) = self.project.clone() else {
            return project.name.clone();
        };
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), project.name.clone());
        vars.insert("path".to_string(), project.root.display().to_string());
        self.render_or_fallback(&template, &vars, &project.name)
    }

    fn render_or_fallback(
        &mut self,
        template: &str,
        vars: &HashMap<String, String>,
        fallback: &str,
    ) -> String {
        match render_label(template, vars) {
            Some(rendered) => rendered,
            None => {
                if self.warned.insert(template.to_string()) {
                    log::warn!("label template {template:?} failed to render; using plain name");
                }
                fallback.to_string()
            },
        }
    }
}
```

(`template` is cloned before building `vars` to satisfy the borrow checker — `self.worktree` is borrowed while `self.render_or_fallback` needs `&mut self`. Template strings are short; this is per-row-per-frame string work either way.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p alacritree row_label`
Expected: all 11 pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/row_label.rs
git commit -m "feat(sidebar): resolve row labels through name templates

Worktree rows get $name/$branch/$path, project rows $name/$path;
$branch is absent when detached so ${branch:$name} falls back.  A
manual project rename always beats the template, and any subst error
degrades to the plain name with one warning per template string.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Wire templated labels into the sidebar

**Files:**
- Modify: `alacritree/src/app.rs` (`AlacritreeApp` struct ~130, `new()` ~280, `show_project_sidebar` precompute ~1360, project label ~1461-1472, `worktree_row` ~2709 + call site ~1639)

**Interfaces:**
- Consumes: `LabelTemplates` (Task 3), `UiTheme.worktree_name`/`project_name` (Task 2).
- Produces: `worktree_row` gains a `display_name: &str` parameter (inserted immediately after `wt: &Worktree`); painted labels use the templated strings. Rename seeding, remove-confirmation prompts, and modals keep using raw `display_name()`/`name` — templates are cosmetic paint-time strings only.

- [ ] **Step 1: Add the field and construct it**

In `AlacritreeApp` (struct at ~130), add:

```rust
/// Renders `[ui] worktree_name` / `project_name` templates at paint time.
row_labels: crate::row_label::LabelTemplates,
```

In `AlacritreeApp::new` (find the struct literal near the end of `new`, ~line 392 area with `pr_cache: PrCache::new(),`), add:

```rust
row_labels: crate::row_label::LabelTemplates::new(
    config.ui.worktree_name.clone(),
    config.ui.project_name.clone(),
),
```

(Place it before `config` is moved into the struct — the literal's field order makes this natural since `config` is a later field; if the compiler complains about a partial move, clone from a local taken earlier.)

- [ ] **Step 2: Precompute display strings per frame**

In `show_project_sidebar`, next to the other pre-closure locals (~1360, near `let distros = wsl::distros();`) — plain `for` loops, because the panel closure borrows `self.projects` mutably and `self.row_labels` is a disjoint field only outside it:

```rust
// Rendered up front: the panel closure borrows `projects` mutably, and
// substitution over short strings is microseconds, so no cache is kept.
let mut project_labels: Vec<String> = Vec::with_capacity(self.projects.len());
let mut worktree_labels: Vec<Vec<String>> = Vec::with_capacity(self.projects.len());
for project in &self.projects {
    project_labels.push(self.row_labels.project_label(project));
    let mut rows = Vec::with_capacity(project.worktrees.len());
    for wt in &project.worktrees {
        rows.push(self.row_labels.worktree_label(wt));
    }
    worktree_labels.push(rows);
}
```

- [ ] **Step 3: Use the project label at the painted row**

Only the *painted* project label changes. At ~1461-1472 replace the label text:

```rust
name_resp = Some(
    ui.add(
        egui::Label::new(
            RichText::new(project_labels.get(idx).map(String::as_str).unwrap_or(project.display_name()))
                .color(theme.text)
                .strong()
                .small(),
        )
        .truncate()
        .sense(egui::Sense::click()),
    ),
);
```

Leave untouched: `project_name` for the remove-confirmation prompt (~1446), the rename dialog's seed label (~1546 — seeding the edit box with a *rendered template* would corrupt a manual rename), and every modal that calls `display_name()`.

- [ ] **Step 4: Thread the worktree label through `worktree_row`**

Change `worktree_row`'s signature — add `display_name: &str,` immediately after `wt: &Worktree,`. Inside, replace

```rust
RichText::new(&wt.name)
```

with

```rust
RichText::new(display_name)
```

At the call site (~1639), pass after `wt`:

```rust
worktree_labels
    .get(idx)
    .and_then(|v| v.get(wt_idx))
    .map(String::as_str)
    .unwrap_or(&wt.name),
```

Everything else keeps `wt.name`: the delete dialog (`worktree_name: wt.name.clone()` at ~1662) names the actual git worktree being deleted, which must not be masked by a template.

- [ ] **Step 5: Check and test**

Run: `cargo check -p alacritree`, then `cargo test -p alacritree`.
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): paint worktree and project rows through name templates

Templates are cosmetic paint-time strings only: rename seeding, delete
prompts, and modals keep the raw names so destructive dialogs always
name the real git object.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Final verification

- [ ] **Step 1: Full suite and format check**

Run: `cargo fmt --check && cargo test -p alacritree`
Expected: no fmt diffs, all tests pass.

- [ ] **Step 2: Default-behavior audit**

- `rg -n "row_labels" alacritree/src/app.rs` — used only in `new()` and the sidebar precompute.
- Confirm `LabelTemplates::new(None, None)` short-circuits (no `subst` call) by reading `worktree_label`/`project_label` — the `else` return precedes map construction.

- [ ] **Step 3: Report**

Report completion; manual GUI verification (isolated lab only) is a follow-up: default config unchanged; `${branch:$name}` shows branches on worktree rows and falls back on detached; a typo'd template degrades to plain names with exactly one log warning.

## Self-review notes

- Rename-dialog seeding deliberately keeps `display_name()` (raw label/dir-name): seeding the edit box with a rendered template would turn a cosmetic template into a stored manual label on the first rename.
- Delete-confirmation and prune flows keep `wt.name` so destructive prompts always name the real worktree.
- `subst` API risk is contained in Task 1 Step 4 with an explicit verification point (`HashMap<String, String>` implements subst's `VariableMap`).
