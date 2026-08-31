# Shell Launch Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Named shell profiles (`[[ui.profiles]]` in alacritree.toml) launchable from a tab-strip + affordance, bindable via `SpawnProfile1..9` actions, selectable as a per-project override, with a `default_profile` that plain new-session uses.

**Architecture:** Config parses profiles into `Config { profiles, default_profile }`. A pure `shell_decision()` function in app.rs owns the precedence chain (project override → WSL location auto → default profile → config shell); `resolve_shell` feeds it live data. Explicit spawns (menu / `SpawnProfileN`) bypass the chain via a new `spawn_profile_session`. `ShellChoice` (wsl.rs) gains a `Profile(String)` variant persisted as `"profile:<name>"`.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe, alacritty_terminal, serde/toml. Windows host.

**Spec:** `docs/superpowers/specs/2026-07-12-shell-profiles-design.md`

## Global Constraints

- Branch `feat/shell-profiles` stacked on `feat/wsl-support` (NOT master). Worktree at `../alacritree-worktrees/feat/shell-profiles`.
- All changes in `alacritree/` crate only; vendored `alacritty*` crates are read-only.
- Bad config/state never panics: warn via `log::warn!` and degrade.
- Comments explain *why*, not *what*; no change-narration or task references.
- Conventional Commits, imperative subject, ≤72 chars, lowercase after colon.
- `cargo fmt` before every commit; `cargo test -p alacritree` must pass.
- Do NOT commit anything under `docs/superpowers/` or `docs/specs/` (git-excluded).
- Profile-built `Shell`s go through `Session::spawn`'s `shell_override` parameter, which already sets `escape_args = true` on Windows. Deliberate: profiles have no upstream alacritty contract; TOML array args must survive quoting. Do not change session.rs.

---

### Task 0: Worktree setup

**Files:** none (git only)

- [ ] **Step 1: Create the worktree branched off feat/wsl-support**

```bash
cd /c/Users/Lev/Git/github/alacritree
git worktree add ../alacritree-worktrees/feat/shell-profiles -b feat/shell-profiles feat/wsl-support
cd ../alacritree-worktrees/feat/shell-profiles
```

- [ ] **Step 2: Verify baseline builds and tests pass**

Run: `cargo test -p alacritree`
Expected: all existing tests pass (wsl::, config::, state:: suites from the wsl-support branch).

All file paths below are relative to `C:\Users\Lev\Git\github\alacritree-worktrees\feat\shell-profiles\`.

---

### Task 1: Config — parse `[[ui.profiles]]` and `default_profile`

**Files:**
- Modify: `alacritree/src/config.rs`

**Interfaces:**
- Produces: `pub struct Profile { pub name: String, pub program: String, pub args: Vec<String> }` (derives `Debug, Clone, PartialEq, Eq`); `Config.profiles: Vec<Profile>`; `Config.default_profile: Option<String>` (validated: names an existing profile or is `None`); `Config::profile(&self, name: &str) -> Option<&Profile>`.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` at the bottom of `config.rs` (starts line ~838):

```rust
    #[test]
    fn profiles_parse_and_validate() {
        let toml_src = r#"
[ui]
default_profile = "pwsh"

[[ui.profiles]]
name = "pwsh"
program = "pwsh"
args = ["-NoLogo"]

[[ui.profiles]]
name = "ubuntu"
program = "wsl.exe"
args = ["-d", "ubuntu"]
"#;
        let raw: RawConfig = toml::from_str(toml_src).unwrap();
        let config = raw.into_config();
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.profiles[0], Profile {
            name: "pwsh".into(),
            program: "pwsh".into(),
            args: vec!["-NoLogo".into()],
        });
        assert_eq!(config.default_profile.as_deref(), Some("pwsh"));
        assert_eq!(config.profile("ubuntu").unwrap().program, "wsl.exe");
        assert!(config.profile("nope").is_none());
    }

    #[test]
    fn invalid_profiles_are_dropped() {
        let toml_src = r#"
[ui]
default_profile = "ghost"

[[ui.profiles]]
name = ""
program = "pwsh"

[[ui.profiles]]
name = "noprog"

[[ui.profiles]]
name = "dup"
program = "first"

[[ui.profiles]]
name = "dup"
program = "second"
"#;
        let raw: RawConfig = toml::from_str(toml_src).unwrap();
        let config = raw.into_config();
        assert_eq!(config.profiles.len(), 1, "empty name, missing program, and dup dropped");
        assert_eq!(config.profiles[0].program, "first");
        assert_eq!(config.default_profile, None, "dangling default_profile is ignored");
    }

    #[test]
    fn no_profiles_by_default() {
        let raw: RawConfig = toml::from_str("").unwrap();
        let config = raw.into_config();
        assert!(config.profiles.is_empty());
        assert_eq!(config.default_profile, None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree config::tests::profiles -- --nocapture`
Expected: COMPILE ERROR — `Profile` not found, no field `profiles` on `Config`. (A compile failure of the test code is the RED state here.)

- [ ] **Step 3: Implement**

3a. Add the public struct after `ShellConfig` (line ~126):

```rust
/// A named shell launch profile from `[[ui.profiles]]`.  Program + args
/// only; cwd and env come from the session as usual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}
```

3b. Add fields to `Config` (line ~19, after `wsl_automount_root`):

```rust
    pub profiles: Vec<Profile>,
    /// Validated at load: always names an entry in `profiles` when `Some`.
    pub default_profile: Option<String>,
```

3c. Extend `impl Default for Config` (line ~182) with:

```rust
            profiles: Vec::new(),
            default_profile: None,
```

3d. Add the lookup helper. `Config` has no inherent impl yet — create one near the struct:

```rust
impl Config {
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}
```

3e. Extend `RawUi` (line ~608):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawUi {
    sidebar_background: Option<RgbStr>,
    sidebar_foreground: Option<RgbStr>,
    sidebar_border: Option<RgbStr>,
    sidebar_accent: Option<RgbStr>,
    notifications: Option<bool>,
    wsl: RawUiWsl,
    profiles: Vec<RawProfile>,
    default_profile: Option<String>,
}

/// One `[[ui.profiles]]` entry.  Fields are optional so a malformed entry
/// degrades to a warning instead of failing the whole config parse.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawProfile {
    name: Option<String>,
    program: Option<String>,
    args: Vec<String>,
}
```

3f. Build + validate in `into_config()`, next to the existing `// ---- WSL ----` block (line ~775):

```rust
        // ---- Profiles ----
        let profiles = build_profiles(self.ui.profiles);
        let default_profile = self.ui.default_profile.filter(|n| {
            let known = profiles.iter().any(|p| &p.name == n);
            if !known {
                log::warn!("default_profile `{n}` names no [[ui.profiles]] entry; ignoring");
            }
            known
        });
```

and add both to the `Config { ... }` constructor at the end of `into_config()`:

```rust
            profiles,
            default_profile,
```

3g. Add the free function near `apply_cursor_style` (line ~800):

```rust
/// Drop unusable `[[ui.profiles]]` entries instead of failing the parse:
/// bad config degrades with a warning, matching the rest of this module.
fn build_profiles(raw: Vec<RawProfile>) -> Vec<Profile> {
    let mut out: Vec<Profile> = Vec::with_capacity(raw.len());
    for p in raw {
        let name = p.name.filter(|n| !n.is_empty());
        let program = p.program.filter(|x| !x.is_empty());
        let (Some(name), Some(program)) = (name, program) else {
            log::warn!("[[ui.profiles]] entry needs non-empty `name` and `program`; dropping");
            continue;
        };
        if out.iter().any(|e| e.name == name) {
            log::warn!("duplicate profile name `{name}`; keeping the first");
            continue;
        }
        out.push(Profile { name, program, args: p.args });
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree config::tests`
Expected: PASS, including the three new tests and all pre-existing config tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs
git commit -m "feat(config): parse [[ui.profiles]] and default_profile"
```

---

### Task 2: Bindings — `SpawnProfile1..9` actions

**Files:**
- Modify: `alacritree/src/bindings.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `NamedAction::SpawnProfile(u8)` (1-indexed; parser only emits 1..=9), parsed from action strings `"SpawnProfile1"`…`"SpawnProfile9"`.

Note: this branch has master's bindings.rs (first-match-wins, no default-replacement filter, **no tests module yet**) — not the rebindable-app-shortcuts version. The `parse_action` addition below is the only overlap with that branch; keep it a single block so the eventual merge conflict stays trivial.

- [ ] **Step 1: Write the failing tests**

bindings.rs has no `#[cfg(test)]` module. Append one at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(action: &str) -> BindingAction {
        let raw = RawBinding {
            key: "F1".into(),
            mods: None,
            mode: None,
            chars: None,
            action: Some(action.into()),
            command: None,
        };
        // User bindings are parsed before the appended defaults, so the
        // first entry is ours.
        parse_bindings(vec![raw]).remove(0).action
    }

    #[test]
    fn spawn_profile_actions_parse() {
        for n in 1..=9u8 {
            let action = parse_one(&format!("SpawnProfile{n}"));
            assert!(
                matches!(action, BindingAction::Named(NamedAction::SpawnProfile(m)) if m == n),
                "SpawnProfile{n} parsed to {action:?}"
            );
        }
    }

    #[test]
    fn out_of_range_spawn_profile_is_unsupported() {
        for name in ["SpawnProfile0", "SpawnProfile10", "SpawnProfile"] {
            let action = parse_one(name);
            assert!(
                matches!(&action, BindingAction::Unsupported(s) if s == name),
                "{name} parsed to {action:?}"
            );
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree bindings::tests`
Expected: COMPILE ERROR — no variant `SpawnProfile` on `NamedAction`.

- [ ] **Step 3: Implement**

3a. Add the variant to `NamedAction` (line ~22), directly after `SelectLastTab`:

```rust
    /// 1-indexed into the `[[ui.profiles]]` order.
    SpawnProfile(u8),
```

3b. Add parse arms in `parse_action` (line ~386), directly after the `"SelectLastTab"` arm, mirroring the `SelectTab1..9` style:

```rust
        "SpawnProfile1" => BindingAction::Named(SpawnProfile(1)),
        "SpawnProfile2" => BindingAction::Named(SpawnProfile(2)),
        "SpawnProfile3" => BindingAction::Named(SpawnProfile(3)),
        "SpawnProfile4" => BindingAction::Named(SpawnProfile(4)),
        "SpawnProfile5" => BindingAction::Named(SpawnProfile(5)),
        "SpawnProfile6" => BindingAction::Named(SpawnProfile(6)),
        "SpawnProfile7" => BindingAction::Named(SpawnProfile(7)),
        "SpawnProfile8" => BindingAction::Named(SpawnProfile(8)),
        "SpawnProfile9" => BindingAction::Named(SpawnProfile(9)),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree bindings::tests`
Expected: PASS (2 tests). Then `cargo check -p alacritree` — expect a non-exhaustive-match error in `app.rs::dispatch_action`? No: `dispatch_action` ends with `BindingAction::Named(other) => self.dispatch_scroll_or_other(other)`, which swallows the new variant, so it compiles. The real dispatch arm lands in Task 5.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/bindings.rs
git commit -m "feat(bindings): add SpawnProfile1..9 actions"
```

---

### Task 3: `ShellChoice::Profile` persistence

**Files:**
- Modify: `alacritree/src/wsl.rs`
- Modify: `alacritree/src/state.rs` (doc comment only)

**Interfaces:**
- Produces: `ShellChoice::Profile(String)`; `ShellChoice::parse("profile:<name>")`; `to_state_string()` → `"profile:<name>"`.

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` in `wsl.rs` (next to `shell_choice_round_trips`, line ~497):

```rust
    #[test]
    fn profile_choice_round_trips() {
        assert_eq!(
            ShellChoice::parse("profile:pwsh"),
            Some(ShellChoice::Profile("pwsh".to_string()))
        );
        assert_eq!(ShellChoice::parse("profile:"), None);
        assert_eq!(
            ShellChoice::Profile("pwsh".to_string()).to_state_string(),
            "profile:pwsh"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alacritree wsl::tests::profile_choice_round_trips`
Expected: COMPILE ERROR — no variant `Profile` on `ShellChoice`.

- [ ] **Step 3: Implement**

3a. Extend the enum and its doc comment (wsl.rs line ~13):

```rust
/// Per-project shell override, persisted in state.toml as `"windows"`,
/// `"wsl:<distro>"`, or `"profile:<name>"`.  Absent means auto-by-location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellChoice {
    Windows,
    Wsl(String),
    Profile(String),
}
```

3b. Extend `parse` and `to_state_string`:

```rust
impl ShellChoice {
    pub fn parse(s: &str) -> Option<Self> {
        if s == "windows" {
            return Some(Self::Windows);
        }
        if let Some(d) = s.strip_prefix("wsl:").filter(|d| !d.is_empty()) {
            return Some(Self::Wsl(d.to_string()));
        }
        s.strip_prefix("profile:").filter(|n| !n.is_empty()).map(|n| Self::Profile(n.to_string()))
    }

    pub fn to_state_string(&self) -> String {
        match self {
            Self::Windows => "windows".to_string(),
            Self::Wsl(distro) => format!("wsl:{distro}"),
            Self::Profile(name) => format!("profile:{name}"),
        }
    }
}
```

3c. Update the `shell` field doc in `state.rs` (line ~22):

```rust
    /// Shell override: `"windows"`, `"wsl:<distro>"`, or `"profile:<name>"`.
    /// Absent = auto by project location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree wsl::tests`
Expected: PASS, including the pre-existing `shell_choice_round_trips`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/wsl.rs alacritree/src/state.rs
git commit -m "feat(wsl): add profile variant to the shell override choice"
```

---

### Task 4: Profile-aware shell resolution

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `Config.profiles` / `Config.default_profile` / `Config::profile` (Task 1), `ShellChoice::Profile` (Task 3).
- Produces: `enum ShellDecision { ConfigShell, WslDistro(String), Profile(String) }` (private to app.rs, derives `Debug, PartialEq, Eq`); `fn shell_decision(override_choice: Option<&ShellChoice>, location_distro: Option<&str>, known_distros: &[String], profiles: &[Profile], default_profile: Option<&str>) -> ShellDecision`; `fn profile_shell(profile: &Profile) -> Shell`. `resolve_shell` keeps its signature (`&self, &WorkspaceKey) -> Option<Shell>`).

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` in `app.rs` (line ~2477). The precedence chain is the heart of the feature — pin every rung:

```rust
    fn test_profiles() -> Vec<crate::config::Profile> {
        vec![
            crate::config::Profile {
                name: "pwsh".into(),
                program: "pwsh".into(),
                args: vec!["-NoLogo".into()],
            },
            crate::config::Profile {
                name: "ubuntu".into(),
                program: "wsl.exe".into(),
                args: vec!["-d".into(), "ubuntu".into()],
            },
        ]
    }

    #[test]
    fn override_profile_wins_over_location_and_default() {
        let d = shell_decision(
            Some(&ShellChoice::Profile("pwsh".into())),
            Some("ubuntu"),
            &["ubuntu".into()],
            &test_profiles(),
            Some("ubuntu"),
        );
        assert_eq!(d, ShellDecision::Profile("pwsh".into()));
    }

    #[test]
    fn override_windows_skips_default_profile() {
        let d = shell_decision(
            Some(&ShellChoice::Windows),
            Some("ubuntu"),
            &["ubuntu".into()],
            &test_profiles(),
            Some("pwsh"),
        );
        assert_eq!(d, ShellDecision::ConfigShell);
    }

    #[test]
    fn stale_profile_override_falls_back_to_auto() {
        // Unknown profile behaves like the unknown-distro case: warn, then
        // continue down the auto chain (location, then default profile).
        let d = shell_decision(
            Some(&ShellChoice::Profile("gone".into())),
            Some("ubuntu"),
            &["ubuntu".into()],
            &test_profiles(),
            None,
        );
        assert_eq!(d, ShellDecision::WslDistro("ubuntu".into()));

        let d = shell_decision(
            Some(&ShellChoice::Profile("gone".into())),
            None,
            &[],
            &test_profiles(),
            Some("pwsh"),
        );
        assert_eq!(d, ShellDecision::Profile("pwsh".into()));
    }

    #[test]
    fn wsl_location_beats_default_profile() {
        let d = shell_decision(None, Some("ubuntu"), &["ubuntu".into()], &test_profiles(), Some("pwsh"));
        assert_eq!(d, ShellDecision::WslDistro("ubuntu".into()));
    }

    #[test]
    fn default_profile_applies_without_override_or_location() {
        // This is also the home-tab case: no project, no WSL location.
        let d = shell_decision(None, None, &[], &test_profiles(), Some("pwsh"));
        assert_eq!(d, ShellDecision::Profile("pwsh".into()));
    }

    #[test]
    fn no_config_means_config_shell() {
        let d = shell_decision(None, None, &[], &[], None);
        assert_eq!(d, ShellDecision::ConfigShell);
    }

    #[test]
    fn stale_wsl_override_falls_through_to_default_profile() {
        let d = shell_decision(
            Some(&ShellChoice::Wsl("gone".into())),
            None,
            &["ubuntu".into()],
            &test_profiles(),
            Some("pwsh"),
        );
        assert_eq!(d, ShellDecision::Profile("pwsh".into()));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree app::tests`
Expected: COMPILE ERROR — `shell_decision` / `ShellDecision` not found.

- [ ] **Step 3: Implement**

3a. Replace the current `resolve_shell` (app.rs line ~379, including its doc comment) with:

```rust
    /// Shell for a workspace; `None` means "no override" — `Session::spawn`
    /// falls through to alacritty's config-driven shell with its
    /// OS-guaranteed fallback.  The home tab (`None` workspace) has no
    /// project or location, so only the default profile can apply there.
    fn resolve_shell(&self, workspace: &WorkspaceKey) -> Option<Shell> {
        let path = workspace.as_deref();
        let choice = path.and_then(|p| {
            self.projects
                .iter()
                .find(|proj| proj.worktrees.iter().any(|wt| wt.path.as_path() == p))
                .and_then(|proj| proj.shell_override.clone())
        });
        let location_distro = path.and_then(|p| match wsl::classify(p) {
            wsl::Location::Wsl { distro, .. } => Some(distro),
            wsl::Location::Windows(_) => None,
        });
        let known: Vec<String> = wsl::distros().into_iter().map(|d| d.name).collect();
        match shell_decision(
            choice.as_ref(),
            location_distro.as_deref(),
            &known,
            &self.config.profiles,
            self.config.default_profile.as_deref(),
        ) {
            ShellDecision::ConfigShell => None,
            // A WSL decision only arises from a workspace path (override or
            // location), never from the home tab.
            ShellDecision::WslDistro(distro) => path.map(|p| wsl_shell(&distro, p)),
            ShellDecision::Profile(name) => self.config.profile(&name).map(profile_shell),
        }
    }
```

3b. Add the decision enum + function as free items next to the existing `wsl_shell` helper (line ~1290), and **delete `auto_wsl_shell`** (its callers are gone — `resolve_shell` was the only one):

```rust
/// What shell a new session should run, decided from plain data so the
/// precedence chain stays testable off the GUI.
#[derive(Debug, PartialEq, Eq)]
enum ShellDecision {
    /// Fall through to `[terminal.shell]` / the OS default.
    ConfigShell,
    /// A shell inside this WSL distro (`wsl_shell` builds the argv).
    WslDistro(String),
    /// A named `[[ui.profiles]]` entry, verified to exist.
    Profile(String),
}

/// Precedence: project override, then WSL location, then the default
/// profile, then the config shell.  A stale override (distro unregistered,
/// profile removed from config) warns and continues down the chain rather
/// than failing the spawn.
fn shell_decision(
    override_choice: Option<&ShellChoice>,
    location_distro: Option<&str>,
    known_distros: &[String],
    profiles: &[crate::config::Profile],
    default_profile: Option<&str>,
) -> ShellDecision {
    match override_choice {
        Some(ShellChoice::Windows) => return ShellDecision::ConfigShell,
        Some(ShellChoice::Wsl(d)) => {
            if known_distros.iter().any(|k| k == d) {
                return ShellDecision::WslDistro(d.clone());
            }
            log::warn!("shell override names unknown WSL distro `{d}`; using auto");
        },
        Some(ShellChoice::Profile(n)) => {
            if profiles.iter().any(|p| &p.name == n) {
                return ShellDecision::Profile(n.clone());
            }
            log::warn!("shell override names unknown profile `{n}`; using auto");
        },
        None => {},
    }
    if let Some(d) = location_distro {
        return ShellDecision::WslDistro(d.to_string());
    }
    if let Some(n) = default_profile {
        return ShellDecision::Profile(n.to_string());
    }
    ShellDecision::ConfigShell
}

fn profile_shell(profile: &crate::config::Profile) -> Shell {
    Shell::new(profile.program.clone(), profile.args.clone())
}
```

Note the doc comment currently on `auto_wsl_shell` ("WSL-resident paths get their distro's shell…") is superseded by `shell_decision`'s comment — delete it with the function. The existing `wsl_shell(distro, workdir)` helper stays untouched.

Behavior change to be aware of (spec-intended): the old `resolve_shell` returned early for the home tab and stale WSL overrides fell back to `auto_wsl_shell` only. The chain now continues into `default_profile` — that is the feature ("plain new session uses the default profile"), not a regression.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: PASS — the 7 new precedence tests plus all existing suites.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): route shell resolution through a profile-aware chain"
```

---

### Task 5: Explicit profile spawns (`SpawnProfileN` dispatch + spawn helper)

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `NamedAction::SpawnProfile(u8)` (Task 2), `profile_shell` (Task 4).
- Produces: `fn spawn_profile_session(&mut self, ctx: &Context, name: &str)` (spawns into the current workspace, sets `last_error` on failure); `fn spawn_session_with_shell(&mut self, ctx: &Context, working_directory: WorkspaceKey, shell: Option<Shell>) -> std::io::Result<SessionId>`. Task 7's + menu calls `spawn_profile_session`.

No new unit tests: both functions are thin glue over `Session::spawn` (PTY) and egui state; the resolution logic they depend on is pinned by Task 4's tests. Verified manually in Task 8.

- [ ] **Step 1: Split `spawn_session` so a caller can force a shell**

Replace the current `spawn_session` (line ~362):

```rust
    fn spawn_session(
        &mut self,
        ctx: &Context,
        working_directory: WorkspaceKey,
    ) -> std::io::Result<SessionId> {
        let shell = self.resolve_shell(&working_directory);
        self.spawn_session_with_shell(ctx, working_directory, shell)
    }

    fn spawn_session_with_shell(
        &mut self,
        ctx: &Context,
        working_directory: WorkspaceKey,
        shell: Option<Shell>,
    ) -> std::io::Result<SessionId> {
        let session = Session::spawn(
            ctx.clone(),
            &self.config,
            working_directory.clone(),
            TermSize::new(80, 24),
            (8.0, 16.0),
            shell,
        )?;
        let id = session.id;
        self.sessions.push(session);
        self.active_session.insert(working_directory, id);
        Ok(id)
    }
```

- [ ] **Step 2: Add the by-name spawn used by both the menu and the shortcut**

Add after `spawn_session_with_shell`:

```rust
    /// Spawn a named profile into the current workspace, bypassing the
    /// override/auto resolution chain — the user asked for this profile
    /// explicitly.
    fn spawn_profile_session(&mut self, ctx: &Context, name: &str) {
        let Some(profile) = self.config.profile(name) else {
            self.last_error = Some(format!("no shell profile named `{name}`"));
            return;
        };
        let shell = Some(profile_shell(profile));
        let ws = self.current_workspace.clone();
        if let Err(e) = self.spawn_session_with_shell(ctx, ws, shell) {
            self.last_error = Some(format!("failed to spawn profile `{name}`: {e}"));
        }
    }
```

- [ ] **Step 3: Dispatch the binding action**

In `dispatch_action` (line ~633), add an arm directly after the `SelectLastTab` arm (before the `NoOp` arm):

```rust
            BindingAction::Named(NamedAction::SpawnProfile(n)) => {
                match self.config.profiles.get((n - 1) as usize).map(|p| p.name.clone()) {
                    Some(name) => self.spawn_profile_session(ctx, &name),
                    None => {
                        log::warn!(
                            "SpawnProfile{n}: only {} profiles configured",
                            self.config.profiles.len()
                        );
                        self.last_error = Some(format!("SpawnProfile{n}: no such profile"));
                    },
                }
            },
```

(`n` is always ≥ 1 — the parser only emits `SpawnProfile1..9` — so `(n - 1)` cannot underflow.)

- [ ] **Step 4: Verify it compiles and existing tests pass**

Run: `cargo test -p alacritree`
Expected: PASS. Also `cargo check -p alacritree` clean — the catch-all `BindingAction::Named(other)` arm still exists below the new arm and now never sees `SpawnProfile`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): spawn sessions from named profiles"
```

---

### Task 6: Profiles in the per-project "Open in…" menu

**Files:**
- Modify: `alacritree/src/app.rs` (`show_project_sidebar`)

**Interfaces:**
- Consumes: `ShellChoice::Profile` (Task 3), `Config.profiles` (Task 1).
- Produces: menu entries persisting `project.shell_override = Some(ShellChoice::Profile(name))` through the existing `shell_override_changed` → `self.persist()` path. No new API.

- [ ] **Step 1: Snapshot profile names before the project loop**

In `show_project_sidebar`, next to the existing snapshot (line ~844):

```rust
        let distros = wsl::distros();
        let profile_names: Vec<String> =
            self.config.profiles.iter().map(|p| p.name.clone()).collect();
        let mut shell_override_changed = false;
```

- [ ] **Step 2: Widen the menu gate and add profile entries**

The menu is currently gated on distros only (line ~945):

```rust
                        // Right-click: choose which shell this project's
                        // sessions use. Hidden entirely when no distros are
                        // registered so non-WSL setups see zero new UI.
                        if !distros.is_empty() {
```

Change the gate and comment to:

```rust
                        // Right-click: choose which shell this project's
                        // sessions use. Hidden entirely when there is nothing
                        // to choose (no distros, no profiles) so minimal
                        // setups see zero new UI.
                        if !distros.is_empty() || !profile_names.is_empty() {
```

Then, inside the `resp.context_menu(|ui| { ... })` closure, after the `for distro in &distros { ... }` loop, add:

```rust
                                    for name in &profile_names {
                                        let selected = matches!(
                                            &project.shell_override,
                                            Some(ShellChoice::Profile(n)) if n == name
                                        );
                                        if ui
                                            .button(format!("{}Profile: {}", mark(selected), name))
                                            .clicked()
                                        {
                                            project.shell_override =
                                                Some(ShellChoice::Profile(name.clone()));
                                            shell_override_changed = true;
                                            ui.close_menu();
                                        }
                                    }
```

- [ ] **Step 3: Verify it compiles and tests pass**

Run: `cargo test -p alacritree && cargo check -p alacritree`
Expected: PASS / clean.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): offer profiles in the open-in menu"
```

---

### Task 7: Tab-strip + affordance

**Files:**
- Modify: `alacritree/src/app.rs` (`show_tab_strip`, line ~751)

**Interfaces:**
- Consumes: `spawn_profile_session` (Task 5), `spawn_session` (existing), `Config.profiles` (Task 1).
- Produces: UI only; no new API.

Context: the strip today is a row of 2px-tall segments and **early-returns when fewer than 2 sessions exist**. The + affordance must exist even with one session, so the strip row now renders whenever the workspace has ≥ 1 session — session segments still only appear at ≥ 2 (a lone session needs no selector). Net visual change for single-session workspaces: the reserved row grows from 2px (`add_space`) to 4px and gains a small muted + segment at the right edge. Deliberate, user-approved placement.

- [ ] **Step 1: Rewrite `show_tab_strip`**

Replace the whole function with:

```rust
    fn show_tab_strip(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        let indices = self.current_session_indices();
        if indices.is_empty() {
            ui.add_space(2.0);
            return;
        }
        let active_idx = self.active_session_index();

        // Reserve a 2px-tall strip across the full width of the terminal pane.
        let strip_height = 2.0;
        let gap = 4.0;
        let plus_width = 12.0;
        let avail = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(avail, strip_height + 2.0), egui::Sense::hover());

        let mut activate: Option<SessionId> = None;
        // Session segments only when there is a choice to make; the trailing
        // + segment renders regardless so new-session stays reachable.
        if indices.len() >= 2 {
            let seg_avail = avail - plus_width - gap;
            let segment_width =
                ((seg_avail - gap * (indices.len() as f32 - 1.0)) / indices.len() as f32).max(1.0);
            for (i, &session_idx) in indices.iter().enumerate() {
                let x0 = rect.min.x + i as f32 * (segment_width + gap);
                let seg_rect = egui::Rect::from_min_size(
                    egui::pos2(x0, rect.min.y + 1.0),
                    egui::vec2(segment_width, strip_height),
                );
                let is_active = active_idx == Some(session_idx);
                // 2px is too small to reliably click — expand the hit zone vertically.
                let click_rect = seg_rect.expand2(egui::vec2(0.0, 4.0));
                let id = ui.id().with(("tab_strip", self.sessions[session_idx].id));
                let resp = ui.interact(click_rect, id, egui::Sense::click());
                // Attention wins over the active/inactive shading so a bell from a
                // non-active tab pulls the eye even when another tab is selected.
                let color = if self.sessions[session_idx].needs_attention {
                    theme.attention
                } else if is_active {
                    theme.text
                } else if resp.hovered() {
                    theme.text_dim
                } else {
                    theme.text_muted
                };
                ui.painter().rect_filled(seg_rect, 0.0, color);
                if resp.clicked() {
                    activate = Some(self.sessions[session_idx].id);
                }
                if resp.hovered() {
                    resp.on_hover_text(&self.sessions[session_idx].title);
                }
            }
        }

        let profile_names: Vec<String> =
            self.config.profiles.iter().map(|p| p.name.clone()).collect();
        let mut spawn_default = false;
        let mut spawn_profile: Option<String> = None;

        let plus_rect = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - plus_width, rect.min.y + 1.0),
            egui::vec2(plus_width, strip_height),
        );
        let click_rect = plus_rect.expand2(egui::vec2(0.0, 4.0));
        let resp = ui.interact(click_rect, ui.id().with("tab_strip_plus"), egui::Sense::click());
        let color = if resp.hovered() { theme.text_dim } else { theme.text_muted };
        ui.painter().rect_filled(plus_rect, 0.0, color);
        if resp.clicked() {
            spawn_default = true;
        }
        if !profile_names.is_empty() {
            resp.context_menu(|ui| {
                ui.label(RichText::new("New session with…").color(theme.text_muted).small());
                for name in &profile_names {
                    if ui.button(name).clicked() {
                        spawn_profile = Some(name.clone());
                        ui.close_menu();
                    }
                }
            });
        }
        resp.on_hover_text("New session (right-click: profiles)");

        if let Some(id) = activate {
            self.set_active_in_current_workspace(id);
        }
        if spawn_default {
            let ctx = ui.ctx().clone();
            let ws = self.current_workspace.clone();
            if let Err(e) = self.spawn_session(&ctx, ws) {
                self.last_error = Some(format!("failed to spawn shell: {e}"));
            }
        }
        if let Some(name) = spawn_profile {
            let ctx = ui.ctx().clone();
            self.spawn_profile_session(&ctx, &name);
        }
    }
```

Borrow notes for the implementer:
- `resp.on_hover_text(...)` consumes the response; call it **after** `context_menu` (which takes `&resp` semantics via `self`), exactly in the order shown, or bind `let resp = resp.on_hover_text(...)` before `clicked()` checks if the borrow checker complains. Adjust order to compile; behavior is identical.
- Spawning happens after all `ui` painting to avoid `&mut self` conflicts inside closures — same pattern as `activate`.

- [ ] **Step 2: Verify compile + tests**

Run: `cargo test -p alacritree && cargo check -p alacritree`
Expected: PASS / clean.

- [ ] **Step 3: Quick visual smoke test**

Run: `cargo run -p alacritree`
Expected: with one session, a small muted segment sits at the right edge of the strip row; clicking it opens a second session; with ≥ 2 sessions the session segments no longer overlap the + zone.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(tabs): add new-session affordance with profile menu"
```

---

### Task 8: Docs, full verification, manual checklist

**Files:**
- Modify: `docs/keyboard-shortcuts.md`
- Modify: `docs/alacritree.md`

- [ ] **Step 1: Document the config surface in `docs/alacritree.md`**

Add a subsection under the configuration/two-file-config part of the doc (find the section describing `[ui]` options):

```markdown
### Shell launch profiles

Named launch profiles live in `alacritree.toml`:

```toml
[ui]
default_profile = "ubuntu"       # what plain new-session (Ctrl+T) uses

[[ui.profiles]]
name = "ubuntu"
program = "wsl.exe"
args = ["-d", "ubuntu"]

[[ui.profiles]]
name = "pwsh"
program = "pwsh"
args = ["-NoLogo"]
```

Launch a profile from the small **+** segment at the right end of the
session tab strip (left-click: default new session; right-click: pick a
profile), bind one to a key with the `SpawnProfile1`…`SpawnProfile9`
actions (1-indexed into the `[[ui.profiles]]` order), or right-click a
project row and pin a profile as that project's shell override.

Shell selection precedence for a plain new session: per-project override →
WSL auto-selection by project location → `default_profile` →
`[terminal.shell]` / OS default.
```

- [ ] **Step 2: Document the actions in `docs/keyboard-shortcuts.md`**

In the "Configurable terminal bindings" section, alongside the supported `action = "…"` list, add `SpawnProfile1` … `SpawnProfile9` with a one-line description ("spawn the Nth `[[ui.profiles]]` entry in the current workspace") and an example binding:

```toml
[[keyboard.bindings]]
key = "2"
mods = "Control|Shift"
action = "SpawnProfile2"
```

- [ ] **Step 3: Full verification**

```bash
cargo fmt
cargo test -p alacritree
cargo check -p alacritree
cargo build -p alacritree --release
```

Expected: all pass; release build clean.

- [ ] **Step 4: Commit docs**

```bash
git add docs/keyboard-shortcuts.md docs/alacritree.md
git commit -m "docs: document shell launch profiles"
```

- [ ] **Step 5: Manual GUI acceptance checklist (user-run, release build)**

With a test `alacritree.toml` containing the two profiles above plus `default_profile = "pwsh"` and a `SpawnProfile2` binding on Ctrl+Shift+2:

1. Launch → first session runs pwsh (default profile applies to the home tab).
2. Ctrl+T → new session runs pwsh.
3. + segment left-click → new session runs pwsh.
4. + segment right-click → menu lists `ubuntu`, `pwsh`; clicking `ubuntu` spawns a WSL session in the current workspace directory.
5. Ctrl+Shift+2 → spawns the second profile (`pwsh` if ordered as above — adjust to your ordering).
6. Bind `SpawnProfile9` with only 2 profiles configured → pressing it shows the "no such profile" toast, no crash.
7. Right-click a project row → "Profile: ubuntu" / "Profile: pwsh" entries appear with selection marks; pick one, restart the app → override persisted (check `state.toml` has `shell = "profile:…"`), new sessions in that project use it.
8. Remove that profile from config, restart → warning logged, project falls back to auto; no crash.
9. On a WSL-resident project with no override: sessions still auto-select the owning distro (default profile does NOT hijack it).
10. Set a project override to "Windows shell" with `default_profile` set → that project's sessions use the config shell, not the default profile.
11. Profile with an arg containing a space (e.g. `args = ["-NoExit", "-Command", "echo hi there"]`) → argument survives as one word (escape_args quoting).

---

## Execution notes

- Tasks 1–3 are independent of each other; 4 needs 1+3; 5 needs 2+4; 6 needs 3; 7 needs 5. Execute in order.
- After completion: `superpowers:finishing-a-development-branch`. This branch targets `feat/wsl-support`, so any PR should be marked as stacked on the wsl-support PR (or merged into that branch) — do not target master directly.
- Known future conflict: `parse_action`/`NamedAction` also change on `feat/rebindable-app-shortcuts`; both additions are append-style and merge trivially.
