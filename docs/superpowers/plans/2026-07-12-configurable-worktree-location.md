# Configurable Worktree Location Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users configure the base directory new git worktrees are created under — globally and per project — via a `[workspace]` table in `alacritree.toml`.

**Architecture:** `config.rs` parses `[workspace]` into `WorkspaceConfig` (tilde-expanded, absolute-only paths; bad entries warn and drop). `app.rs` resolves the effective base dir (override → global → `None`) when the create modal confirms and puts it on `CreateRequest.base_dir`. `worktree.rs` uses that base, keeping `~/.alacritree/worktrees` as the `None` fallback and the `<project>-<hash>/<branch>` layout under every base.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), serde + toml for config, `home` crate for `~` expansion. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-12-configurable-worktree-location-design.md`

## Global Constraints

- Only the `alacritree/` crate changes (plus `README.md`, `docs/alacritree.md`, `CLAUDE.md`). Vendored `alacritty*` crates are read-only.
- Work on a dedicated branch `feat/configurable-worktree-location` off `master`, in its own git worktree (repo workflow: one feature = one worktree = one upstream PR).
- `cargo fmt` is enforced (`rustfmt.toml`: comment wrap at 100, `use_small_heuristics = "Max"`). Run it before every commit.
- Conventional Commits, imperative subject, ≤50 chars including `type:` prefix, lowercase after the colon.
- Never commit anything under `docs/superpowers/` — spec and plan stay local (`.git/info/exclude`).
- Comments explain *why*, in the present tense; no change-narration, no task references.
- Type-check loop: `cargo check -p alacritree`. Tests: `cargo test -p alacritree` (this feature adds the crate's first tests).

---

### Task 1: `worktree.rs` — accept a configurable base directory

**Files:**
- Modify: `alacritree/src/worktree.rs` (struct `CreateRequest` at :17, `run_create` at :94, `pick_worktree_path` at :266-282, `project_worktree_dir` at :284-299; tests appended at end of file)
- Modify: `alacritree/src/app.rs:1943` (add `base_dir: None` so the crate keeps compiling; real value wired in Task 3)

**Interfaces:**
- Consumes: nothing new.
- Produces: `CreateRequest { project_root: PathBuf, default_branch: Option<String>, branch: String, base_dir: Option<PathBuf> }`; internal `project_worktree_dir(repo: &Path, base: Option<&Path>) -> Result<PathBuf, String>` and `pick_worktree_path(repo: &Path, branch: &str, base: Option<&Path>) -> Result<PathBuf, String>`. Task 3 relies on the `base_dir` field name and `Option<PathBuf>` type.

- [ ] **Step 1: Write the failing tests**

Append at the end of `alacritree/src/worktree.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn abs(tail: &str) -> PathBuf {
        if cfg!(windows) { PathBuf::from(format!("C:\\{tail}")) } else { PathBuf::from(format!("/{tail}")) }
    }

    #[test]
    fn base_dir_replaces_default_worktree_parent() {
        let base = abs("wt-base");
        let dir = project_worktree_dir(Path::new("repo"), Some(&base)).unwrap();
        assert!(dir.starts_with(&base), "{} not under {}", dir.display(), base.display());
        let leaf = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(leaf.starts_with("repo-"), "leaf {leaf:?} should keep <project>-<hash> layout");
    }

    #[test]
    fn no_base_dir_falls_back_to_home_default() {
        let dir = project_worktree_dir(Path::new("repo"), None).unwrap();
        let expected = home::home_dir().unwrap().join(".alacritree").join("worktrees");
        assert!(dir.starts_with(&expected), "{} not under {}", dir.display(), expected.display());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail for the right reason**

Run: `cargo test -p alacritree worktree::tests`
Expected: compile error `error[E0061]: this function takes 1 argument but 2 arguments were supplied` on the `project_worktree_dir(Path::new("repo"), Some(&base))` call — the new parameter doesn't exist yet.

- [ ] **Step 3: Implement the base-dir parameter**

In `alacritree/src/worktree.rs`, change `CreateRequest` (currently :17-21) to:

```rust
pub struct CreateRequest {
    pub project_root: PathBuf,
    pub default_branch: Option<String>,
    pub branch: String,
    /// Base directory to create the worktree under; `None` uses the built-in
    /// `~/.alacritree/worktrees` default.
    pub base_dir: Option<PathBuf>,
}
```

Change the call in `run_create` (currently :94) to:

```rust
    let target = pick_worktree_path(&req.project_root, &req.branch, req.base_dir.as_deref())?;
```

Replace `pick_worktree_path` and `project_worktree_dir` (currently :266-299) with:

```rust
/// Worktrees live under `<base>/<project>-<hash>/<branch>`.  `base` defaults
/// to `~/.alacritree/worktrees` so worktrees don't clutter the repo's parent
/// directory and stay grouped per app; a configured `workspace.worktree_dir`
/// relocates them.  The path hash disambiguates same-named repos in different
/// locations.
fn pick_worktree_path(repo: &Path, branch: &str, base: Option<&Path>) -> Result<PathBuf, String> {
    let parent = project_worktree_dir(repo, base)?;
    std::fs::create_dir_all(&parent)
        .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    let safe_branch: String =
        branch.chars().map(|c| if c == '/' || c.is_whitespace() { '-' } else { c }).collect();
    let mut candidate = parent.join(&safe_branch);
    let mut suffix = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{safe_branch}-{suffix}"));
        suffix += 1;
    }
    Ok(candidate)
}

fn project_worktree_dir(repo: &Path, base: Option<&Path>) -> Result<PathBuf, String> {
    let base = match base {
        Some(dir) => dir.to_path_buf(),
        None => home::home_dir()
            .ok_or_else(|| "could not locate home directory".to_string())?
            .join(".alacritree")
            .join("worktrees"),
    };
    let canonical = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let project_name = canonical
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "project".to_string());

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    let hash = hasher.finish() as u32;

    Ok(base.join(format!("{project_name}-{hash:08x}")))
}
```

In `alacritree/src/app.rs:1943`, add the new field so the crate compiles (Task 3 replaces the `None`):

```rust
            let req =
                CreateRequest { project_root, default_branch, branch: canonical.clone(), base_dir: None };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree worktree::tests`
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: Format, check, commit**

```bash
cargo fmt
cargo check -p alacritree
git add alacritree/src/worktree.rs alacritree/src/app.rs
git commit -m "feat(worktree): accept configurable base dir"
```

---

### Task 2: `config.rs` — parse `[workspace]` and resolve the effective base dir

**Files:**
- Modify: `alacritree/src/config.rs` (imports at :10, `Config` struct at :19-30, `Default for Config` at :181-196, `RawConfig` at :422-436, `into_config` at :632-775; new types + tests)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub struct WorkspaceConfig { pub worktree_dir: Option<PathBuf>, pub overrides: Vec<WorktreeOverride> }` with `pub fn base_dir_for(&self, project_root: &Path) -> Option<PathBuf>`; `pub struct WorktreeOverride { pub project: PathBuf, pub worktree_dir: PathBuf }`; `Config` gains `pub workspace: WorkspaceConfig`. Task 3 calls `self.config.workspace.base_dir_for(&project_root)`.

- [ ] **Step 1: Write the failing tests**

Append at the end of `alacritree/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn abs(tail: &str) -> String {
        if cfg!(windows) { format!("C:\\{tail}") } else { format!("/{tail}") }
    }

    #[test]
    fn tilde_expands_to_home() {
        let home = home::home_dir().unwrap();
        assert_eq!(parse_config_path("~/wt", "test"), Some(home.join("wt")));
        assert_eq!(parse_config_path("~", "test"), Some(home));
    }

    #[test]
    fn absolute_path_passes_through() {
        let raw = abs("wt");
        assert_eq!(parse_config_path(&raw, "test"), Some(PathBuf::from(raw)));
    }

    #[test]
    fn relative_and_user_tilde_paths_are_rejected() {
        assert_eq!(parse_config_path("relative/dir", "test"), None);
        assert_eq!(parse_config_path("~user/dir", "test"), None);
    }

    #[test]
    fn workspace_table_parses_into_config() {
        let toml_src = format!(
            r#"
            [workspace]
            worktree_dir = "{global}"

            [[workspace.overrides]]
            project = "{proj}"
            worktree_dir = "{over}"
            "#,
            global = abs("global-wt").replace('\\', "\\\\"),
            proj = abs("proj").replace('\\', "\\\\"),
            over = abs("proj-wt").replace('\\', "\\\\"),
        );
        let raw: RawConfig = toml::from_str(&toml_src).unwrap();
        let config = raw.into_config();
        assert_eq!(config.workspace.worktree_dir, Some(PathBuf::from(abs("global-wt"))));
        assert_eq!(config.workspace.overrides.len(), 1);
        assert_eq!(config.workspace.overrides[0].project, PathBuf::from(abs("proj")));
        assert_eq!(config.workspace.overrides[0].worktree_dir, PathBuf::from(abs("proj-wt")));
    }

    #[test]
    fn base_dir_for_prefers_override_then_global_then_none() {
        let ws = WorkspaceConfig {
            worktree_dir: Some(PathBuf::from(abs("global-wt"))),
            overrides: vec![WorktreeOverride {
                project: PathBuf::from(abs("proj")),
                worktree_dir: PathBuf::from(abs("proj-wt")),
            }],
        };
        assert_eq!(
            ws.base_dir_for(Path::new(&abs("proj"))),
            Some(PathBuf::from(abs("proj-wt")))
        );
        assert_eq!(
            ws.base_dir_for(Path::new(&abs("other"))),
            Some(PathBuf::from(abs("global-wt")))
        );
        let empty = WorkspaceConfig::default();
        assert_eq!(empty.base_dir_for(Path::new(&abs("proj"))), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail for the right reason**

Run: `cargo test -p alacritree config::tests`
Expected: compile errors `error[E0425]: cannot find function `parse_config_path`` and `error[E0422]: cannot find struct ... `WorkspaceConfig``.

- [ ] **Step 3: Implement parsing and resolution**

In `alacritree/src/config.rs`:

Change the import at :10 to `use std::path::{Path, PathBuf};`.

Add `pub workspace: WorkspaceConfig,` to the `Config` struct (:19-30) and `workspace: WorkspaceConfig::default(),` to `Default for Config` (:181-196).

Add after the `UiTheme` definitions (around :179):

```rust
/// Where new git worktrees are created.  alacritree-only, lives under
/// `[workspace]` in `alacritree.toml`.  Every base directory — default,
/// global, or override — gets the `<project>-<hash>/<branch>` layout beneath
/// it; changing these options never moves existing worktrees because
/// discovery goes through `git worktree list`.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceConfig {
    /// Global base directory for new worktrees; `None` means the built-in
    /// `~/.alacritree/worktrees`.
    pub worktree_dir: Option<PathBuf>,
    pub overrides: Vec<WorktreeOverride>,
}

/// Per-project base-directory override, matched against the project root.
#[derive(Debug, Clone)]
pub struct WorktreeOverride {
    pub project: PathBuf,
    pub worktree_dir: PathBuf,
}

impl WorkspaceConfig {
    /// Base directory for a project's new worktrees: first matching override,
    /// then the global `worktree_dir`, then `None` (the caller falls back to
    /// the built-in default).  Paths compare canonicalized so a symlinked
    /// spelling of the same root still matches; canonicalization failure
    /// (path doesn't exist) falls back to the literal path.
    pub fn base_dir_for(&self, project_root: &Path) -> Option<PathBuf> {
        let canonical =
            |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let root = canonical(project_root);
        self.overrides
            .iter()
            .find(|o| canonical(&o.project) == root)
            .map(|o| o.worktree_dir.clone())
            .or_else(|| self.worktree_dir.clone())
    }
}
```

Add the raw structs next to `RawUi` (around :594):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWorkspace {
    worktree_dir: Option<String>,
    overrides: Vec<RawWorktreeOverride>,
}

#[derive(Debug, Deserialize)]
struct RawWorktreeOverride {
    project: String,
    worktree_dir: String,
}
```

Add `workspace: RawWorkspace,` to `RawConfig` (:422-436).

Add the path parser near `parse_hex_rgb` (around :617):

```rust
/// Expand a leading `~` to the home directory and require the result to be
/// absolute.  Relative paths are rejected rather than resolved against the
/// process CWD, which is meaningless for a GUI app; `~user` expansion is not
/// supported.  Returns `None` (after logging) for anything unusable.
fn parse_config_path(raw: &str, key: &str) -> Option<PathBuf> {
    let path = if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        let Some(home) = home::home_dir() else {
            log::warn!("{key}: cannot expand `~` in {raw:?}: no home directory");
            return None;
        };
        home.join(raw[1..].trim_start_matches(['/', '\\']))
    } else {
        PathBuf::from(raw)
    };
    if !path.is_absolute() {
        log::warn!("{key}: ignoring non-absolute path {raw:?}");
        return None;
    }
    Some(path)
}
```

In `into_config` (:632), before the final `Config { ... }` expression, add:

```rust
        // ---- Workspace ----
        let workspace = WorkspaceConfig {
            worktree_dir: self
                .workspace
                .worktree_dir
                .as_deref()
                .and_then(|raw| parse_config_path(raw, "workspace.worktree_dir")),
            overrides: self
                .workspace
                .overrides
                .iter()
                .filter_map(|o| {
                    let project = parse_config_path(&o.project, "workspace.overrides.project")?;
                    let worktree_dir =
                        parse_config_path(&o.worktree_dir, "workspace.overrides.worktree_dir")?;
                    Some(WorktreeOverride { project, worktree_dir })
                })
                .collect(),
        };
```

and add `workspace,` to the `Config { ... }` constructor at the end of `into_config`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree config::tests`
Expected: `test result: ok. 5 passed`
Also run the full suite: `cargo test -p alacritree` — expected: all pass (includes Task 1's 2 tests).

- [ ] **Step 5: Format, check, commit**

```bash
cargo fmt
cargo check -p alacritree
git add alacritree/src/config.rs
git commit -m "feat(config): parse [workspace] worktree dirs"
```

---

### Task 3: `app.rs` — wire resolution into worktree creation, verify end-to-end

**Files:**
- Modify: `alacritree/src/app.rs:1943` (the `CreateRequest` construction inside `show_create_prompt`)

**Interfaces:**
- Consumes: `self.config.workspace.base_dir_for(&project_root) -> Option<PathBuf>` (Task 2); `CreateRequest.base_dir: Option<PathBuf>` (Task 1).
- Produces: the user-visible feature; nothing further consumes it.

- [ ] **Step 1: Replace the placeholder `None` with real resolution**

In `alacritree/src/app.rs`, `show_create_prompt` — `project_root` was cloned at :1863 and is moved into the request, so resolve before constructing:

```rust
            let base_dir = self.config.workspace.base_dir_for(&project_root);
            let req = CreateRequest { project_root, default_branch, branch: canonical.clone(), base_dir };
```

- [ ] **Step 2: Type-check**

Run: `cargo check -p alacritree`
Expected: clean.

- [ ] **Step 3: Verify end-to-end in the running app**

1. Add to the alacritree config (Windows: `%APPDATA%\alacritty\alacritree.toml`; else `~/.config/alacritty/alacritree.toml`):
   ```toml
   [workspace]
   worktree_dir = "~/alacritree-wt-test"
   ```
2. `cargo run -p alacritree`, pick a project in the left sidebar, create a worktree named `wt-config-test`.
3. Confirm the new worktree lands under `~/alacritree-wt-test/<project>-<hash>/wt-config-test` (not `~/.alacritree/worktrees/...`) and the session opens in it.
4. Add an override for that project pointing somewhere else, restart, create `wt-config-test-2`, confirm it lands under the override dir with the same `<project>-<hash>` layout.
5. Clean up: delete both test worktrees from the sidebar, remove the `[workspace]` block (or keep it if you want the feature live), delete the test dirs.

Expected: both worktrees created in the configured locations; deletion works from the sidebar as before.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat: make worktree location configurable"
```

---

### Task 4: Documentation

**Files:**
- Modify: `README.md:123-125` (Configuration section)
- Modify: `docs/alacritree.md:72-76` (worktree location paragraph) and `docs/alacritree.md:202-214` (config example)
- Modify: `CLAUDE.md` (two `[ui]`-only claims)

**Interfaces:** none — prose only.

- [ ] **Step 1: README**

Replace (at `README.md:123-125`):

```markdown
Alacritree-only options live under `[ui]` in `alacritree.toml` — sidebar
colours, panel visibility, etc. See `alacritree/src/config.rs` for the
current schema.
```

with:

```markdown
Alacritree-only options live in `alacritree.toml`: `[ui]` for sidebar
colours, panel visibility, etc., and `[workspace]` for where new git
worktrees are created (`worktree_dir`, plus per-project
`[[workspace.overrides]]`). See `alacritree/src/config.rs` for the current
schema.
```

- [ ] **Step 2: docs/alacritree.md**

Replace the paragraph at :72-76:

```markdown
Worktrees are created under
`~/.alacritree/worktrees/<project>-<hash>/<branch>` so they never clutter the
repo's parent directory and stay grouped per app. The `<hash>` disambiguates
same-named repos in different locations. `/` in branch names is rewritten to
`-`, and a numeric suffix is appended if the target already exists.
```

with:

```markdown
Worktrees are created under
`<base>/<project>-<hash>/<branch>`, where `<base>` defaults to
`~/.alacritree/worktrees` so they never clutter the repo's parent directory
and stay grouped per app. The base is configurable per `[workspace]` in
`alacritree.toml` (see Configuration below); changing it never moves existing
worktrees — discovery goes through `git worktree list`. The `<hash>`
disambiguates same-named repos in different locations. `/` in branch names is
rewritten to `-`, and a numeric suffix is appended if the target already
exists.
```

In the Configuration section, extend the `alacritree.toml` example (after the `[ui]` block at :205-210, before `[window]`):

```toml
[workspace]
worktree_dir = "~/dev/worktrees"   # base dir for new worktrees (default ~/.alacritree/worktrees)

[[workspace.overrides]]            # optional per-project override
project = "~/Git/github/alacritree"
worktree_dir = "D:/wt"
```

Also change the sentence at :201-202 from "while Alacritree-specific UI options live in `alacritree.toml` under `[ui]`:" to "while Alacritree-specific options live in `alacritree.toml` under `[ui]` and `[workspace]`:".

- [ ] **Step 3: CLAUDE.md**

In the `config.rs` bullet, change "alacritree-only options (sidebar colors, etc.) live under `[ui]`." to "alacritree-only options live under `[ui]` (sidebar colors, etc.) and `[workspace]` (worktree location)."

In the conventions section, change "`alacritree.toml` (sidebar/UI overrides under `[ui]`)" to "`alacritree.toml` (alacritree-only options under `[ui]` and `[workspace]`)".

- [ ] **Step 4: Commit**

```bash
git add README.md docs/alacritree.md CLAUDE.md
git commit -m "docs: document [workspace] worktree location"
```

---

## Completion

All four tasks done → run `cargo fmt && cargo test -p alacritree && cargo check -p alacritree` once more, then use superpowers:finishing-a-development-branch (this repo's convention: PR upstream to mathix420/alacritree; PR description carries the spec context since specs are never committed).
